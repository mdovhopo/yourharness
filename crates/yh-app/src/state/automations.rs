//! Automations runtime (`yh-core`'s `automations.rs`):
//! - Schedule timer waking when the next run is due (at least every 20s), catching up once after
//!   sleep/close.
//! - Event-triggered runs (`event_triggers.rs`: Mac wake/unlock/app/power/network, FSEvents
//!   folder watch, git branch/commit, PR diffs) and manual/CLI runs (`yh trigger`).
//! - Guardrails (global Pause All, active hours, AC-power-only, cooldown, overlap policy,
//!   optional shell pre-check command).
//! - Thread policies: New thread, New thread & settle previous, and Same thread.
//! - Live run lifecycle updates when automation threads finish or pause waiting on you.

use crate::state::workspace::Workspace;
use gpui_kit::Context;
use serde_json::Value;
use std::time::Duration;
use yh_core::automations::{self, Automation, NotifyPolicy, OverlapPolicy, RunStatus, Scope, ThreadPolicy, TriggerEvent};

/// How often the timer looks.
const TICK: Duration = Duration::from_secs(20);
/// The first look after launch: once the threads are up.
const FIRST: Duration = Duration::from_secs(8);
/// At most this many threads working in a project before a run waits for the next one.
const MAX_RUNNING: usize = 8;

fn logs() -> bool {
    std::env::var("YH_AUTOMATION_LOG").is_ok_and(|v| v == "1")
}

/// Starts the schedule timer and mounts any active Mac/file/git event triggers.
/// Does not run in perf runs or with `YH_AUTOMATIONS=0`.
pub fn start(cx: &mut Context<Workspace>) {
    if std::env::var("YH_PERF").is_ok() || std::env::var("YH_AUTOMATIONS").is_ok_and(|v| v == "0") {
        return;
    }
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(FIRST).await;
        // Reconcile event watchers & fire one-shot AppLaunch automations once startup has settled.
        if this
            .update(cx, |ws, cx| {
                ws.reconcile_event_triggers(cx);
                ws.fire_app_launch_automations(cx);
            })
            .is_err()
        {
            return;
        }
        // Optional synthetic Mac event for deterministic testing (`YH_AUTOMATION_TEST_EVENT=wake|unlock|...`).
        if let Ok(test_ev) = std::env::var("YH_AUTOMATION_TEST_EVENT") {
            let ev = match test_ev.trim().to_lowercase().as_str() {
                "wake" => Some(crate::platform::MacSystemEvent::Wake),
                "unlock" => Some(crate::platform::MacSystemEvent::ScreenUnlock),
                "power:ac" => Some(crate::platform::MacSystemEvent::PowerChanged { on_ac: true }),
                "power:battery" => Some(crate::platform::MacSystemEvent::PowerChanged { on_ac: false }),
                s if s.starts_with("app_launched:") => Some(crate::platform::MacSystemEvent::AppLaunched {
                    name: test_ev.split_once(':').map(|(_, v)| v.to_string()).unwrap_or_default(),
                    bundle_id: String::new(),
                }),
                s if s.starts_with("app_quit:") => Some(crate::platform::MacSystemEvent::AppQuit {
                    name: test_ev.split_once(':').map(|(_, v)| v.to_string()).unwrap_or_default(),
                    bundle_id: String::new(),
                }),
                _ => None,
            };
            if let Some(ev) = ev {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let _ = this.update(cx, |ws, cx| ws.handle_mac_event(ev, cx));
            }
        }
        loop {
            let Ok(next) = this.update(cx, |ws, cx| {
                ws.run_due_automations(cx);
                if ws.automations_paused() {
                    return f64::INFINITY;
                }
                ws.automations().iter().filter(|a| a.enabled).filter_map(|a| a.next_run).fold(f64::INFINITY, f64::min)
            }) else {
                break;
            };
            let wait = (next - automations::now() + 0.2).clamp(0.5, TICK.as_secs_f64());
            cx.background_executor().timer(Duration::from_secs_f64(wait)).await;
        }
    })
    .detach();
}

impl Workspace {
    /// Every automation, as saved.
    pub fn automations(&self) -> Vec<Automation> {
        automations::all(self.library.as_ref())
    }

    /// Global Pause All kill switch.
    pub fn automations_paused(&self) -> bool {
        self.library.setting(automations::PAUSED_SETTING).and_then(|v| v.as_bool()).unwrap_or(false)
    }

    /// Whether automation-created threads are hidden from the sidebar (off by default).
    pub fn hide_automation_threads(&self) -> bool {
        self.library.setting(automations::HIDE_THREADS_SETTING).and_then(|v| v.as_bool()).unwrap_or(false)
    }

    pub fn set_automations_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        self.library.set_setting(automations::PAUSED_SETTING, &Value::Bool(paused));
        self.automations_changed(cx);
    }

    pub fn set_hide_automation_threads(&mut self, hide: bool, cx: &mut Context<Self>) {
        self.library.set_setting(automations::HIDE_THREADS_SETTING, &Value::Bool(hide));
        self.automations_changed(cx);
    }

    /// Creates or updates an automation from a remote client (`yh_server::wire::AutomationDraft`).
    pub fn save_automation_draft(&mut self, draft: yh_server::wire::AutomationDraft, cx: &mut Context<Self>) -> Result<String, String> {
        use yh_core::automations::{FileWatchTrigger, RepoEventKind, RepoTrigger, SystemEventKind, SystemTrigger, Trigger};
        let now = automations::now();
        let name = draft.name.trim().to_string();
        if name.is_empty() {
            return Err("Give the automation a name.".into());
        }
        let prompt = draft.prompt.trim().to_string();
        if prompt.is_empty() {
            return Err("Write a prompt for the automation.".into());
        }
        let scope = if draft.is_global {
            Scope::Global
        } else {
            let project_id = draft.project_id.as_deref().map(str::trim).filter(|s| !s.is_empty()).ok_or("Pick a workspace or choose Global.")?;
            if self.project(project_id).is_none() {
                return Err("No such project.".into());
            }
            Scope::Project { project_id: project_id.to_string() }
        };
        let trigger = match draft.trigger_kind.as_str() {
            "schedule" => {
                let text = draft.schedule_text.as_deref().map(str::trim).unwrap_or("");
                if text.is_empty() {
                    return Err("Write a schedule (e.g. daily 9:00 or every 30m).".into());
                }
                Trigger::Schedule { schedule: automations::parse(text, now)? }
            }
            "system" => {
                let event = match draft.system_event.as_deref().unwrap_or("macWake") {
                    "macWake" => SystemEventKind::MacWake,
                    "screenUnlock" => SystemEventKind::ScreenUnlock,
                    "appLaunched" => SystemEventKind::AppLaunched,
                    "appQuit" => SystemEventKind::AppQuit,
                    "powerChanged" => SystemEventKind::PowerChanged,
                    "networkOnline" => SystemEventKind::NetworkOnline,
                    "appLaunch" => SystemEventKind::AppLaunch,
                    other => return Err(format!("Unknown Mac event: {other}")),
                };
                let app = draft.system_app.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
                let power = draft.system_power.as_deref().map(str::trim).filter(|s| *s == "ac" || *s == "battery").map(str::to_string);
                Trigger::System(SystemTrigger { event, app, power })
            }
            "file_watch" => Trigger::FileWatch(FileWatchTrigger {
                path: draft.watch_path.as_deref().map(str::trim).unwrap_or("").to_string(),
                glob: draft.watch_glob.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string),
                events: vec![],
            }),
            "repo" => {
                let event = match draft.repo_event.as_deref().unwrap_or("commitMade") {
                    "branchChanged" => RepoEventKind::BranchChanged,
                    "commitMade" => RepoEventKind::CommitMade,
                    "prReviewRequested" => RepoEventKind::PrReviewRequested,
                    "prChecksFailed" => RepoEventKind::PrChecksFailed,
                    "prMerged" => RepoEventKind::PrMerged,
                    other => return Err(format!("Unknown repo event: {other}")),
                };
                let branch = draft.repo_branch.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
                Trigger::Repo(RepoTrigger { event, branch })
            }
            "manual" => Trigger::Manual,
            other => return Err(format!("Unknown trigger kind: {other}")),
        };
        let policy = match draft.thread_policy.as_str() {
            "new_settle_previous" => ThreadPolicy::NewSettlePrevious,
            "same" => ThreadPolicy::Same,
            _ => ThreadPolicy::New,
        };
        let notify = match draft.notify.as_deref() {
            Some("needs_attention") => NotifyPolicy::NeedsAttention,
            Some("silent") => NotifyPolicy::Silent,
            _ => NotifyPolicy::Always,
        };
        let overlap = match draft.overlap.as_deref() {
            Some("queue") => OverlapPolicy::Queue,
            Some("allow") => OverlapPolicy::Allow,
            _ => OverlapPolicy::Skip,
        };
        let clean = |s: Option<String>| s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty() && v != "default");
        let agent = clean(draft.agent);
        let model = clean(draft.model);
        let effort = clean(draft.effort);
        let mode = clean(draft.mode);
        let working_dir = clean(draft.working_dir);
        let cooldown_secs = draft.cooldown_secs;
        let ac_power_only = draft.ac_power_only;
        let enabled_opt = draft.enabled;

        if let Some(edit_id) = draft.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let edit_id = edit_id.to_string();
            let mut res: Result<(), String> = Err("No such automation.".into());
            self.edit_automations(cx, |list| {
                if let Some(a) = list.iter_mut().find(|a| a.id == edit_id) {
                    let same_schedule = matches!(
                        (&a.effective_trigger(), &trigger),
                        (Trigger::Schedule { schedule: s1 }, Trigger::Schedule { schedule: s2 }) if s1.to_input_string() == s2.to_input_string()
                    );
                    if !same_schedule {
                        if let Err(e) = a.set_trigger(trigger, now) {
                            res = Err(e);
                            return;
                        }
                    }
                    a.name = name;
                    a.prompt = prompt;
                    a.set_scope(scope);
                    a.set_thread_policy(policy);
                    a.agent = agent;
                    a.model = model;
                    a.effort = effort;
                    a.mode = mode;
                    a.working_dir = working_dir;
                    a.notify = notify;
                    a.guardrails.cooldown_secs = cooldown_secs;
                    a.guardrails.ac_power_only = ac_power_only;
                    a.guardrails.overlap = overlap;
                    if let Some(on) = enabled_opt {
                        a.set_enabled(on, now);
                    }
                    res = Ok(());
                }
            });
            res.map(|()| edit_id)
        } else {
            let mut auto = Automation::new_with_trigger(scope, &name, &prompt, trigger, now)?;
            auto.set_thread_policy(policy);
            auto.agent = agent;
            auto.model = model;
            auto.effort = effort;
            auto.mode = mode;
            auto.working_dir = working_dir;
            auto.notify = notify;
            auto.guardrails.cooldown_secs = cooldown_secs;
            auto.guardrails.ac_power_only = ac_power_only;
            auto.guardrails.overlap = overlap;
            if let Some(on) = enabled_opt {
                auto.set_enabled(on, now);
            }
            let new_id = auto.id.clone();
            self.edit_automations(cx, |list| list.push(auto));
            Ok(new_id)
        }
    }

    /// They changed (an agent's tools, the Automations page): pages re-read and watchers reconcile.
    pub fn automations_changed(&mut self, cx: &mut Context<Self>) {
        self.automations_revision += 1;
        self.reconcile_event_triggers(cx);
        cx.notify();
    }

    /// Changes them, saves, and tells pages + watchers.
    pub fn edit_automations<R>(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Vec<Automation>) -> R) -> R {
        let result = automations::edit(self.library.as_ref(), change);
        self.automations_changed(cx);
        result
    }

    /// Resolves the target project ID for an automation (Home for Global).
    pub fn automation_target_project(&self, auto: &Automation) -> Option<String> {
        match auto.scope() {
            Scope::Project { project_id } => self.project(&project_id).map(|_| project_id),
            Scope::Global => self.home_project().or_else(|| self.projects.first().map(|p| p.id.clone())),
        }
    }

    /// Looks up the automation that owns a thread (if any).
    pub fn automation_for_thread(&self, thread_id: &str, cx: &gpui_kit::App) -> Option<Automation> {
        let auto_id = self.thread(thread_id)?.read(cx).meta.automation_id.clone()?;
        self.automations().into_iter().find(|a| a.id == auto_id)
    }

    /// Notification policy for a thread (Always for regular threads, or the automation's policy).
    pub fn notify_policy_for_thread(&self, thread_id: &str, cx: &gpui_kit::App) -> NotifyPolicy {
        self.automation_for_thread(thread_id, cx).map(|a| a.notify).unwrap_or(NotifyPolicy::Always)
    }

    pub fn run_due_automations(&mut self, cx: &mut Context<Self>) {
        if self.automations_paused() {
            return;
        }
        let now = automations::now();
        let due: Vec<(String, bool)> = self
            .automations()
            .into_iter()
            .filter(|a| a.due(now) && self.automation_target_project(a).is_some())
            .map(|a| {
                let late = a.next_run.is_some_and(|t| now - t > 120.0);
                (a.id, late)
            })
            .collect();
        for (id, late) in due {
            let _ = self.trigger_automation(&id, TriggerEvent::scheduled(late), false, cx);
        }
    }

    /// Runs one now (manual Run Now or agent's `run_automation`): bypasses schedule/guardrails.
    pub fn run_automation(&mut self, id: &str, manual: bool, cx: &mut Context<Self>) -> Result<String, String> {
        let ev = if manual { TriggerEvent::manual() } else { TriggerEvent::scheduled(false) };
        self.trigger_automation(id, ev, manual, cx)
    }

    /// Triggers an automation by ID or name/slug (for `yh trigger`), with optional payload & files.
    pub fn trigger_automation_by_key(&mut self, key: &str, data: Option<String>, files: Vec<String>, cx: &mut Context<Self>) -> Result<String, String> {
        let list = self.automations();
        let auto = automations::find_any(&list, key)?.clone();
        let mut fields = vec![];
        if let Some(d) = data.filter(|s| !s.trim().is_empty()) {
            fields.push(("payload".into(), d));
        }
        let ev = TriggerEvent {
            reason: "CLI trigger (yh)".into(),
            kind: "cli".into(),
            fields,
            files,
        };
        self.trigger_automation(&auto.id, ev, true, cx)
    }

    /// Evaluates guardrails (when `!manual`), runs optional shell pre-check, and fires the automation.
    pub fn trigger_automation(&mut self, id: &str, event: TriggerEvent, manual: bool, cx: &mut Context<Self>) -> Result<String, String> {
        let Some(auto) = self.automations().into_iter().find(|a| a.id == id) else {
            return Err("That automation is gone.".into());
        };
        let now = automations::now();

        // Guardrails for non-manual triggers.
        if !manual {
            if self.automations_paused() {
                return Err("All automations are paused.".into());
            }
            if !auto.enabled {
                return Err("Automation is paused.".into());
            }
            if let Some(hours) = &auto.guardrails.active_hours {
                if !hours.contains(now) {
                    return self.record_skip(&auto, now, &event.reason, "Outside active hours", cx);
                }
            }
            if auto.guardrails.ac_power_only && !crate::platform::is_on_ac_power() {
                return self.record_skip(&auto, now, &event.reason, "Mac is on battery power", cx);
            }
            if let Some(rem) = auto.cooldown_remaining(now) {
                return self.record_skip(&auto, now, &event.reason, &format!("Cooldown ({:.0}s left)", rem), cx);
            }
            if auto.guardrails.overlap == OverlapPolicy::Skip && self.is_automation_running(&auto, cx) {
                return self.record_skip(&auto, now, &event.reason, "Previous run is still working", cx);
            }
            // Optional shell pre-check command: run off the main thread so UI never stalls.
            if let Some(cmd) = auto.guardrails.condition_cmd.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                let cmd = cmd.to_string();
                let cwd = self.base_dir_for(&auto);
                let id_owned = auto.id.clone();
                let ev_owned = event.clone();
                let path_env = std::env::var("YH_PATH").ok();
                cx.spawn(async move |this, cx| {
                    let ok = cx
                        .background_executor()
                        .spawn(async move {
                            let mut c = std::process::Command::new("/bin/sh");
                            c.args(["-c", &cmd]);
                            if let Some(dir) = cwd {
                                c.current_dir(dir);
                            }
                            if let Some(p) = path_env {
                                c.env("PATH", p);
                            }
                            c.output().map(|o| o.status.success()).unwrap_or(false)
                        })
                        .await;
                    let _ = this.update(cx, |ws, cx| {
                        let Some(fresh) = ws.automations().into_iter().find(|a| a.id == id_owned) else { return };
                        let now = automations::now();
                        if ok {
                            let _ = ws.execute_and_record(&fresh, &ev_owned, false, now, cx);
                        } else {
                            let _ = ws.record_skip(&fresh, now, &ev_owned.reason, "Pre-check command exited non-zero", cx);
                        }
                    });
                })
                .detach();
                return Ok("pending-condition".into());
            }
        }

        self.execute_and_record(&auto, &event, manual, now, cx)
    }

    fn is_automation_running(&self, auto: &Automation, cx: &gpui_kit::App) -> bool {
        let check = |tid: &str| self.thread(tid).is_some_and(|t| t.read(cx).run.running);
        auto.thread_id.as_deref().is_some_and(check) || auto.last_thread_id.as_deref().is_some_and(check)
    }

    fn record_skip(&mut self, auto: &Automation, now: f64, reason: &str, why: &str, cx: &mut Context<Self>) -> Result<String, String> {
        if logs() {
            eprintln!("AUTOMATION skipped “{}” ({reason}): {why}", auto.name);
        }
        let id = auto.id.clone();
        self.edit_automations(cx, |list| {
            if let Some(a) = list.iter_mut().find(|a| a.id == id) {
                a.record_run(now, None, Some(why.to_string()), false, reason.to_string(), Some(RunStatus::Skipped), None);
            }
        });
        Err(why.to_string())
    }

    fn execute_and_record(&mut self, auto: &Automation, event: &TriggerEvent, manual: bool, now: f64, cx: &mut Context<Self>) -> Result<String, String> {
        let result = self.fire(auto, event, now, cx);
        if logs() {
            match &result {
                Ok((thread, settled)) => eprintln!(
                    "AUTOMATION ran “{}” ({} · {}) in {thread}{}{}",
                    auto.name,
                    auto.effective_trigger().describe(now),
                    event.reason,
                    if manual { " by hand" } else { "" },
                    settled.as_deref().map(|s| format!(" (settled prev {s})")).unwrap_or_default()
                ),
                Err(e) => eprintln!("AUTOMATION couldn't run “{}”: {e}", auto.name),
            }
        }
        let (thread, settled_prev, error, status) = match &result {
            Ok((t, s)) => (Some(t.clone()), s.clone(), None, RunStatus::Running),
            Err(e) => (None, None, Some(e.clone()), RunStatus::Error),
        };
        let id = auto.id.clone();
        self.edit_automations(cx, |list| {
            if let Some(a) = list.iter_mut().find(|a| a.id == id) {
                a.record_run(now, thread, error, manual, event.reason.clone(), Some(status), settled_prev);
            }
        });
        result.map(|(t, _)| t)
    }

    fn fire(&mut self, auto: &Automation, event: &TriggerEvent, now: f64, cx: &mut Context<Self>) -> Result<(String, Option<String>), String> {
        let Some(target_project) = self.automation_target_project(auto) else {
            return Err("Its project is gone.".into());
        };
        let rendered = automations::render_prompt(&auto.prompt, Some(event), now);
        let policy = auto.thread_policy();

        // Same thread policy: reuse existing thread if still present (un-settle if settled, queue behind active turn).
        if policy == ThreadPolicy::Same {
            if let Some(thread) = auto.thread_id.as_deref().and_then(|t| self.thread(t)) {
                let id = thread.read(cx).id().to_string();
                if thread.read(cx).meta.settled_at.is_some() {
                    self.set_settled(&id, false, cx);
                }
                self.send_message(&thread, &rendered, vec![], vec![], yh_core::claude::Timing::Queue, cx);
                return Ok((id, None));
            }
        }

        let running = self.threads.iter().filter(|t| {
            let t = t.read(cx);
            t.run.running && t.meta.project_id == target_project
        });
        if running.count() >= MAX_RUNNING {
            return Err(format!("{MAX_RUNNING} threads were already working in the project."));
        }

        // NewSettlePrevious: settle the previous run's thread if idle & unpinned & not waiting for user;
        // if still running, mark it to auto-settle when idle.
        let mut settled_previous = None;
        if policy == ThreadPolicy::NewSettlePrevious {
            if let Some(prev_id) = auto.last_thread_id.clone() {
                if let Some(prev_thread) = self.thread(&prev_id) {
                    let (already_settled, pinned, waiting, running_now) = {
                        let t = prev_thread.read(cx);
                        (t.meta.settled_at.is_some(), t.meta.pinned, t.waiting_for_you(), t.run.running)
                    };
                    if !already_settled && !pinned && !waiting {
                        if running_now {
                            self.set_auto_settle(&prev_id, true, cx);
                        } else {
                            self.set_settled(&prev_id, true, cx);
                            settled_previous = Some(prev_id);
                        }
                    }
                }
            }
        }

        // Create thread without sending yet so metadata (automationID, per-thread permissionMode,
        // workingDirectory, title) is in place before the agent process starts.
        let id = self.start_thread(
            &target_project,
            None,
            auto.model.clone(),
            auto.effort.clone(),
            None, // Never mutate the project's permission mode; apply per-thread below!
            auto.agent.as_deref(),
            None,
            cx,
        )?;
        self.rename(&id, &auto.name, cx);
        let automation_id = auto.id.clone();
        let thread_mode = auto.mode.clone().filter(|m| !m.is_empty() && m != "default");
        let work_dir = auto
            .working_dir
            .as_deref()
            .and_then(|w| crate::state::event_triggers::resolve_watch_path(w, None))
            .filter(|p| p.is_dir())
            .map(|p| p.to_string_lossy().to_string());
        self.update_meta(&id, cx, |m, record| {
            m.automation_id = Some(automation_id.clone());
            record.insert("automationID".into(), automation_id.into());
            if let Some(mode) = thread_mode.clone() {
                m.permission_mode = Some(mode.clone());
                record.insert("permissionMode".into(), mode.into());
            }
            if let Some(wd) = work_dir.clone() {
                m.working_directory = Some(wd.clone());
                record.insert("workingDirectory".into(), wd.into());
            }
        });
        if let Some(thread) = self.thread(&id) {
            self.send(&thread, &rendered, false, cx);
        }
        Ok((id, settled_previous))
    }

    /// Updates the matching automation run's status/duration when a turn ends or waits for user.
    pub fn note_automation_run_state(&mut self, thread_id: &str, status: RunStatus, cx: &mut Context<Self>) {
        let Some(auto_id) = self.thread(thread_id).and_then(|t| t.read(cx).meta.automation_id.clone()) else {
            return;
        };
        let now = automations::now();
        let tid = thread_id.to_string();
        let changed = automations::edit(self.library.as_ref(), |list| {
            if let Some(a) = list.iter_mut().find(|a| a.id == auto_id) {
                return a.finish_run(&tid, status, now);
            }
            false
        });
        if changed {
            self.automations_revision += 1;
            cx.notify();
        }
    }
}
