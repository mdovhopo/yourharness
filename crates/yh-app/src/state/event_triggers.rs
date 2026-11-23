//! Native Mac, file-watch, git, and PR event triggers for Automations v2.
//!
//! Zero-cost when unused: `MacEventMonitor` and `FolderWatch` instances are only started when at
//! least one enabled automation needs them (and the global pause switch is off), and torn down
//! automatically when automations are paused or deleted.

use crate::platform::{MacEventMonitor, MacSystemEvent};
use crate::state::file_watch::FolderWatch;
use crate::state::workspace::Workspace;
use gpui_kit::{Context, Task};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use yh_core::automations::{self, Automation, RepoEventKind, Scope, SystemEventKind, Trigger, TriggerEvent};

fn logs() -> bool {
    std::env::var("YH_AUTOMATION_LOG").is_ok_and(|v| v == "1")
}

/// Snapshot of a single PR's state for diffing PR events between scans.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PrState {
    review_requested: bool,
    checks_failed: bool,
    state: String,
    title: String,
    url: String,
    branch: String,
    number: u64,
}

/// Per-automation pending file burst (for per-automation debounce).
struct PendingBurst {
    files: HashSet<String>,
    _task: Task<()>,
}

/// Owns all live Mac/file/git/PR trigger watchers on `Workspace`.
#[derive(Default)]
pub struct EventTriggers {
    /// Mounted only while at least one enabled System trigger exists.
    mac_monitor: Option<(MacEventMonitor, Task<()>)>,
    /// Folder watchers keyed by canonical watched directory.
    folder_watches: HashMap<PathBuf, (FolderWatch, Task<()>)>,
    /// Git `.git` watchers keyed by repo root path, plus last observed HEAD & log line.
    git_watches: HashMap<PathBuf, (FolderWatch, Task<()>)>,
    git_heads: HashMap<PathBuf, (String, String)>,
    /// Debounced file-watch bursts keyed by automation ID.
    pending_files: HashMap<String, PendingBurst>,
    /// Baseline PR state per project (`project_id -> (pr_id -> PrState)`).
    pr_baselines: HashMap<String, HashMap<String, PrState>>,
    /// Whether `AppLaunch` triggers have already fired this session.
    launched_fired: bool,
    /// Last power state seen, to ignore duplicate adapter notifications.
    last_ac: Option<bool>,
    /// Last network check state (used for NetworkOnline transitions).
    net_task: Option<Task<()>>,
    /// When each watched folder was reconciled, to avoid churn.
    reconciled_at: Option<Instant>,
}

/// Expands leading `~/` to `$HOME` or resolves a relative path against `base`.
pub fn resolve_watch_path(raw: &str, base: Option<&Path>) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return base.map(Path::to_path_buf);
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        let home = std::env::var("HOME").ok()?;
        return Some(PathBuf::from(home).join(rest));
    }
    if trimmed == "~" {
        return std::env::var("HOME").ok().map(PathBuf::from);
    }
    let p = PathBuf::from(trimmed);
    if p.is_absolute() {
        Some(p)
    } else {
        base.map(|b| b.join(p))
    }
}

/// Reads current branch name (or detached SHA) and last line of `.git/logs/HEAD`.
fn read_git_head(git_dir: &Path) -> (String, String) {
    let head_raw = std::fs::read_to_string(git_dir.join("HEAD")).unwrap_or_default();
    let head_trimmed = head_raw.trim();
    let branch = head_trimmed
        .strip_prefix("ref: refs/heads/")
        .map(str::to_string)
        .unwrap_or_else(|| head_trimmed.chars().take(12).collect());
    let log_last = std::fs::read_to_string(git_dir.join("logs/HEAD"))
        .ok()
        .and_then(|s| s.lines().rev().find(|l| !l.trim().is_empty()).map(str::to_string))
        .unwrap_or_default();
    (branch, log_last)
}

/// Resolves the `.git` directory for a repo folder (supporting worktree `.git` files too).
fn git_dir_of(repo: &Path) -> Option<PathBuf> {
    let dot = repo.join(".git");
    if dot.is_dir() {
        return Some(dot);
    }
    if dot.is_file() {
        let text = std::fs::read_to_string(&dot).ok()?;
        let rel = text.trim().strip_prefix("gitdir:")?.trim();
        let p = PathBuf::from(rel);
        return Some(if p.is_absolute() { p } else { repo.join(p) });
    }
    None
}

fn app_matches(filter: Option<&str>, name: &str, bundle_id: &str) -> bool {
    let Some(f) = filter.map(str::trim).filter(|s| !s.is_empty()) else { return true };
    let fl = f.to_lowercase();
    name.to_lowercase().contains(&fl) || bundle_id.to_lowercase().contains(&fl)
}

fn branch_matches(filter: Option<&str>, branch: &str) -> bool {
    let Some(f) = filter.map(str::trim).filter(|s| !s.is_empty()) else { return true };
    automations::glob_matches(f, branch)
}

fn is_network_reachable() -> bool {
    // Fast non-blocking check: does the Mac have a non-loopback UP interface with an address?
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 || ifap.is_null() {
            return true;
        }
        let mut up = false;
        let mut cur = ifap;
        while !cur.is_null() {
            let ifa = &*cur;
            let flags = ifa.ifa_flags as i32;
            let is_up = (flags & libc::IFF_UP) != 0 && (flags & libc::IFF_RUNNING) != 0 && (flags & libc::IFF_LOOPBACK) == 0;
            if is_up && !ifa.ifa_addr.is_null() {
                let family = (*ifa.ifa_addr).sa_family as i32;
                if family == libc::AF_INET || family == libc::AF_INET6 {
                    up = true;
                    break;
                }
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
        up
    }
}

impl Workspace {
    /// Reconciles active Mac/file/git/network watchers against current enabled automations.
    pub fn reconcile_event_triggers(&mut self, cx: &mut Context<Self>) {
        if std::env::var("YH_PERF").is_ok() || std::env::var("YH_AUTOMATIONS").is_ok_and(|v| v == "0") {
            return;
        }
        if self.automations_paused() {
            self.event_triggers.mac_monitor = None;
            self.event_triggers.folder_watches.clear();
            self.event_triggers.git_watches.clear();
            self.event_triggers.net_task = None;
            return;
        }

        let enabled: Vec<Automation> = self.automations().into_iter().filter(|a| a.enabled).collect();
        let need_mac = enabled.iter().any(|a| {
            matches!(
                a.effective_trigger(),
                Trigger::System(s) if matches!(
                    s.event,
                    SystemEventKind::MacWake
                        | SystemEventKind::ScreenUnlock
                        | SystemEventKind::AppLaunched
                        | SystemEventKind::AppQuit
                        | SystemEventKind::PowerChanged
                )
            )
        });
        let need_net = enabled.iter().any(|a| matches!(a.effective_trigger(), Trigger::System(s) if s.event == SystemEventKind::NetworkOnline));

        // 1. Mac system monitor
        if need_mac && self.event_triggers.mac_monitor.is_none() {
            let (monitor, rx) = MacEventMonitor::start();
            self.event_triggers.last_ac = Some(crate::platform::is_on_ac_power());
            let task = cx.spawn(async move |this, cx| {
                while let Ok(ev) = rx.recv_async().await {
                    if this.update(cx, |ws, cx| ws.handle_mac_event(ev, cx)).is_err() {
                        break;
                    }
                }
            });
            self.event_triggers.mac_monitor = Some((monitor, task));
            if logs() {
                eprintln!("AUTOMATION mac event monitor started");
            }
        } else if !need_mac && self.event_triggers.mac_monitor.is_some() {
            self.event_triggers.mac_monitor = None;
            if logs() {
                eprintln!("AUTOMATION mac event monitor stopped");
            }
        }

        // 2. NetworkOnline monitor (lightweight 5s interface poll only when used)
        if need_net && self.event_triggers.net_task.is_none() {
            let task = cx.spawn(async move |this, cx| {
                let mut was_online = is_network_reachable();
                loop {
                    cx.background_executor().timer(Duration::from_secs(5)).await;
                    let online = is_network_reachable();
                    if !was_online && online {
                        let ev = TriggerEvent {
                            reason: "Network came online".into(),
                            kind: "networkOnline".into(),
                            fields: vec![],
                            files: vec![],
                        };
                        if this.update(cx, |ws, cx| ws.dispatch_system_trigger(SystemEventKind::NetworkOnline, None, None, ev, cx)).is_err() {
                            break;
                        }
                    }
                    was_online = online;
                }
            });
            self.event_triggers.net_task = Some(task);
        } else if !need_net {
            self.event_triggers.net_task = None;
        }

        // 3. Folder watchers (deduplicated canonical paths)
        let mut wanted_folders: HashSet<PathBuf> = HashSet::new();
        for a in &enabled {
            if let Trigger::FileWatch(fw) = a.effective_trigger() {
                let base = self.base_dir_for(a);
                if let Some(path) = resolve_watch_path(&fw.path, base.as_deref()) {
                    if let Ok(canon) = std::fs::canonicalize(&path) {
                        if canon.is_dir() {
                            wanted_folders.insert(canon);
                        }
                    }
                }
            }
        }
        self.event_triggers.folder_watches.retain(|p, _| wanted_folders.contains(p));
        for dir in wanted_folders {
            if self.event_triggers.folder_watches.contains_key(&dir) {
                continue;
            }
            if let Some((watch, rx)) = FolderWatch::start(&dir) {
                let watched = dir.clone();
                let task = cx.spawn(async move |this, cx| {
                    while let Ok(paths) = rx.recv_async().await {
                        if this.update(cx, |ws, cx| ws.handle_folder_events(&watched, paths, cx)).is_err() {
                            break;
                        }
                    }
                });
                self.event_triggers.folder_watches.insert(dir.clone(), (watch, task));
                if logs() {
                    eprintln!("AUTOMATION watching folder {}", dir.display());
                }
            }
        }

        // 4. Git HEAD / commit watchers for Repo(BranchChanged | CommitMade)
        let mut wanted_git: HashSet<PathBuf> = HashSet::new();
        for a in &enabled {
            if let Trigger::Repo(r) = a.effective_trigger() {
                if matches!(r.event, RepoEventKind::BranchChanged | RepoEventKind::CommitMade) {
                    if let Some(root) = self.base_dir_for(a) {
                        if let Some(gd) = git_dir_of(&root) {
                            if let Ok(canon) = std::fs::canonicalize(&gd) {
                                wanted_git.insert(canon);
                            }
                        }
                    }
                }
            }
        }
        self.event_triggers.git_watches.retain(|p, _| wanted_git.contains(p));
        self.event_triggers.git_heads.retain(|p, _| wanted_git.contains(p));
        for git_dir in wanted_git {
            if self.event_triggers.git_watches.contains_key(&git_dir) {
                continue;
            }
            let initial = read_git_head(&git_dir);
            self.event_triggers.git_heads.insert(git_dir.clone(), initial);
            if let Some((watch, rx)) = FolderWatch::start(&git_dir) {
                let gd = git_dir.clone();
                let task = cx.spawn(async move |this, cx| {
                    while let Ok(_paths) = rx.recv_async().await {
                        if this.update(cx, |ws, cx| ws.handle_git_events(&gd, cx)).is_err() {
                            break;
                        }
                    }
                });
                self.event_triggers.git_watches.insert(git_dir.clone(), (watch, task));
                if logs() {
                    eprintln!("AUTOMATION watching git dir {}", git_dir.display());
                }
            }
        }

        self.event_triggers.reconciled_at = Some(Instant::now());
    }

    /// Fires `SystemEventKind::AppLaunch` automations once after launch settles.
    pub fn fire_app_launch_automations(&mut self, cx: &mut Context<Self>) {
        if self.event_triggers.launched_fired || self.automations_paused() {
            return;
        }
        self.event_triggers.launched_fired = true;
        let ev = TriggerEvent {
            reason: "YourHarness opened".into(),
            kind: "appLaunch".into(),
            fields: vec![],
            files: vec![],
        };
        self.dispatch_system_trigger(SystemEventKind::AppLaunch, None, None, ev, cx);
    }

    /// Resolves the base directory for an automation (its project folder, or Home for Global).
    pub(crate) fn base_dir_for(&self, auto: &Automation) -> Option<PathBuf> {
        if let Some(wd) = auto.working_dir.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            return resolve_watch_path(wd, None);
        }
        let pid = match auto.scope() {
            Scope::Project { project_id } => Some(project_id),
            Scope::Global => self.home_project(),
        }?;
        self.project(&pid).map(|p| PathBuf::from(&p.path))
    }

    /// Handles a native macOS system event (or synthetic `YH_AUTOMATION_TEST_EVENT` event).
    pub fn handle_mac_event(&mut self, ev: MacSystemEvent, cx: &mut Context<Self>) {
        if self.automations_paused() {
            return;
        }
        if logs() {
            eprintln!("AUTOMATION mac event: {ev:?}");
        }
        match ev {
            MacSystemEvent::Wake => {
                let trigger_ev = TriggerEvent { reason: "Mac woke from sleep".into(), kind: "macWake".into(), fields: vec![], files: vec![] };
                self.dispatch_system_trigger(SystemEventKind::MacWake, None, None, trigger_ev, cx);
            }
            MacSystemEvent::ScreenUnlock => {
                let trigger_ev = TriggerEvent { reason: "Screen unlocked".into(), kind: "screenUnlock".into(), fields: vec![], files: vec![] };
                self.dispatch_system_trigger(SystemEventKind::ScreenUnlock, None, None, trigger_ev, cx);
            }
            MacSystemEvent::AppLaunched { name, bundle_id } => {
                let trigger_ev = TriggerEvent {
                    reason: format!("{name} launched"),
                    kind: "appLaunched".into(),
                    fields: vec![("app".into(), name.clone()), ("bundleId".into(), bundle_id.clone())],
                    files: vec![],
                };
                self.dispatch_system_trigger(SystemEventKind::AppLaunched, Some((&name, &bundle_id)), None, trigger_ev, cx);
            }
            MacSystemEvent::AppQuit { name, bundle_id } => {
                let trigger_ev = TriggerEvent {
                    reason: format!("{name} quit"),
                    kind: "appQuit".into(),
                    fields: vec![("app".into(), name.clone()), ("bundleId".into(), bundle_id.clone())],
                    files: vec![],
                };
                self.dispatch_system_trigger(SystemEventKind::AppQuit, Some((&name, &bundle_id)), None, trigger_ev, cx);
            }
            MacSystemEvent::PowerChanged { on_ac } => {
                if self.event_triggers.last_ac == Some(on_ac) {
                    return;
                }
                self.event_triggers.last_ac = Some(on_ac);
                let state = if on_ac { "ac" } else { "battery" };
                let trigger_ev = TriggerEvent {
                    reason: if on_ac { "Mac plugged in (AC)".into() } else { "Mac switched to battery".into() },
                    kind: "powerChanged".into(),
                    fields: vec![("power".into(), state.into())],
                    files: vec![],
                };
                self.dispatch_system_trigger(SystemEventKind::PowerChanged, None, Some(state), trigger_ev, cx);
            }
        }
    }

    fn dispatch_system_trigger(
        &mut self,
        kind: SystemEventKind,
        app: Option<(&str, &str)>,
        power: Option<&str>,
        ev: TriggerEvent,
        cx: &mut Context<Self>,
    ) {
        let matching: Vec<String> = self
            .automations()
            .into_iter()
            .filter(|a| a.enabled)
            .filter(|a| match a.effective_trigger() {
                Trigger::System(s) if s.event == kind => {
                    let app_ok = match app {
                        Some((name, bid)) => app_matches(s.app.as_deref(), name, bid),
                        None => true,
                    };
                    let power_ok = match (power, s.power.as_deref()) {
                        (Some(cur), Some(want)) => cur.eq_ignore_ascii_case(want),
                        _ => true,
                    };
                    app_ok && power_ok
                }
                _ => false,
            })
            .map(|a| a.id)
            .collect();
        for id in matching {
            let _ = self.trigger_automation(&id, ev.clone(), false, cx);
        }
    }

    fn handle_folder_events(&mut self, watched: &Path, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.automations_paused() {
            return;
        }
        let autos: Vec<Automation> = self.automations().into_iter().filter(|a| a.enabled).collect();
        for a in autos {
            let Trigger::FileWatch(fw) = a.effective_trigger() else { continue };
            let base = self.base_dir_for(&a);
            let Some(target) = resolve_watch_path(&fw.path, base.as_deref()).and_then(|p| std::fs::canonicalize(p).ok()) else { continue };
            if target != watched {
                continue;
            }
            let mut matched: Vec<String> = vec![];
            for p in &paths {
                let rel = p.strip_prefix(&target).map(|r| r.to_string_lossy().to_string()).unwrap_or_else(|_| p.to_string_lossy().to_string());
                if rel.is_empty() || automations::is_ignored_path(&rel, fw.glob.as_deref()) {
                    continue;
                }
                if let Some(glob) = fw.glob.as_deref().filter(|g| !g.trim().is_empty()) {
                    if !automations::glob_matches(glob, &rel) {
                        continue;
                    }
                }
                matched.push(p.to_string_lossy().to_string());
            }
            if matched.is_empty() {
                continue;
            }
            let id = a.id.clone();
            let folder_display = if fw.path.trim().is_empty() { target.to_string_lossy().to_string() } else { fw.path.clone() };
            let debounce = Duration::from_secs(a.guardrails.effective_debounce_secs() as u64);
            let mut files: HashSet<String> = self.event_triggers.pending_files.remove(&id).map(|b| b.files).unwrap_or_default();
            files.extend(matched);
            let id_fire = id.clone();
            let task = cx.spawn(async move |this, cx| {
                cx.background_executor().timer(debounce).await;
                let _ = this.update(cx, |ws, cx| {
                    let Some(burst) = ws.event_triggers.pending_files.remove(&id_fire) else { return };
                    let mut list: Vec<String> = burst.files.into_iter().collect();
                    list.sort();
                    let sample = list.first().and_then(|p| Path::new(p).file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    let reason = if list.len() == 1 {
                        format!("File changed: {sample}")
                    } else {
                        format!("{} files changed ({sample}, …)", list.len())
                    };
                    let ev = TriggerEvent {
                        reason,
                        kind: "fileWatch".into(),
                        fields: vec![("folder".into(), folder_display)],
                        files: list,
                    };
                    let _ = ws.trigger_automation(&id_fire, ev, false, cx);
                });
            });
            self.event_triggers.pending_files.insert(id, PendingBurst { files, _task: task });
        }
    }

    fn handle_git_events(&mut self, git_dir: &Path, cx: &mut Context<Self>) {
        if self.automations_paused() {
            return;
        }
        let (branch, log_last) = read_git_head(git_dir);
        let prev = self.event_triggers.git_heads.insert(git_dir.to_path_buf(), (branch.clone(), log_last.clone()));
        let Some((prev_branch, prev_log)) = prev else { return };
        let branch_changed = !branch.is_empty() && branch != prev_branch;
        let commit_made = !log_last.is_empty() && log_last != prev_log && log_last.contains("commit");
        if !branch_changed && !commit_made {
            return;
        }
        let autos: Vec<Automation> = self.automations().into_iter().filter(|a| a.enabled).collect();
        for a in autos {
            let Trigger::Repo(r) = a.effective_trigger() else { continue };
            let Some(root) = self.base_dir_for(&a) else { continue };
            let Some(gd) = git_dir_of(&root).and_then(|p| std::fs::canonicalize(p).ok()) else { continue };
            if gd != git_dir || !branch_matches(r.branch.as_deref(), &branch) {
                continue;
            }
            if r.event == RepoEventKind::BranchChanged && branch_changed {
                let ev = TriggerEvent {
                    reason: format!("Branch switched to {branch}"),
                    kind: "branchChanged".into(),
                    fields: vec![("branch".into(), branch.clone()), ("previousBranch".into(), prev_branch.clone())],
                    files: vec![],
                };
                let _ = self.trigger_automation(&a.id, ev, false, cx);
            } else if r.event == RepoEventKind::CommitMade && commit_made {
                let summary = log_last.split('\t').nth(1).unwrap_or("").trim().to_string();
                let ev = TriggerEvent {
                    reason: if summary.is_empty() { format!("New commit on {branch}") } else { format!("Commit on {branch}: {summary}") },
                    kind: "commitMade".into(),
                    fields: vec![("branch".into(), branch.clone()), ("commit".into(), summary)],
                    files: vec![],
                };
                let _ = self.trigger_automation(&a.id, ev, false, cx);
            }
        }
    }

    /// Called after a project's `WorkspaceScan` finishes: diffs PR states and triggers PR automations.
    pub fn event_triggers_on_scan(&mut self, project_id: &str, scan: &yh_core::workspace::Workspace, cx: &mut Context<Self>) {
        let mut current: HashMap<String, PrState> = HashMap::new();
        for pr in scan.pull_requests() {
            current.insert(
                pr.id(),
                PrState {
                    review_requested: pr.review_requested_from_me,
                    checks_failed: pr.checks == "failing",
                    state: pr.state.clone(),
                    title: pr.title.clone(),
                    url: pr.link.url.clone(),
                    branch: pr.head.clone(),
                    number: pr.link.number,
                },
            );
        }
        let prev = self.event_triggers.pr_baselines.insert(project_id.to_string(), current.clone());
        let Some(prev) = prev else {
            // Baseline seeded on first scan: don't fire for pre-existing PRs on launch.
            return;
        };
        if self.automations_paused() {
            return;
        }
        let autos: Vec<Automation> = self
            .automations()
            .into_iter()
            .filter(|a| a.enabled && (a.is_global || a.project_id == project_id))
            .filter(|a| matches!(a.effective_trigger(), Trigger::Repo(r) if matches!(r.event, RepoEventKind::PrReviewRequested | RepoEventKind::PrChecksFailed | RepoEventKind::PrMerged)))
            .collect();
        if autos.is_empty() {
            return;
        }

        let mut events: Vec<(RepoEventKind, PrState)> = vec![];
        for (id, cur) in &current {
            let old = prev.get(id);
            if cur.review_requested && !old.is_some_and(|o| o.review_requested) {
                events.push((RepoEventKind::PrReviewRequested, cur.clone()));
            }
            if cur.checks_failed && !old.is_some_and(|o| o.checks_failed) {
                events.push((RepoEventKind::PrChecksFailed, cur.clone()));
            }
        }
        for (id, old) in &prev {
            if !current.contains_key(id) && old.state.eq_ignore_ascii_case("open") {
                events.push((RepoEventKind::PrMerged, old.clone()));
            }
        }

        for (kind, pr) in events {
            for a in &autos {
                let Trigger::Repo(r) = a.effective_trigger() else { continue };
                if r.event != kind || !branch_matches(r.branch.as_deref(), &pr.branch) {
                    continue;
                }
                let reason = match kind {
                    RepoEventKind::PrReviewRequested => format!("Review requested on PR #{}", pr.number),
                    RepoEventKind::PrChecksFailed => format!("Checks failed on PR #{}", pr.number),
                    RepoEventKind::PrMerged => format!("PR #{} closed/merged", pr.number),
                    _ => continue,
                };
                let ev = TriggerEvent {
                    reason,
                    kind: format!("{kind:?}"),
                    fields: vec![
                        ("prNumber".into(), pr.number.to_string()),
                        ("prTitle".into(), pr.title.clone()),
                        ("prUrl".into(), pr.url.clone()),
                        ("branch".into(), pr.branch.clone()),
                    ],
                    files: vec![],
                };
                let _ = self.trigger_automation(&a.id, ev, false, cx);
            }
        }
    }
}
