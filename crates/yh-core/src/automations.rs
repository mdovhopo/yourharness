//! Automations: prompts YourHarness runs by itself — per workspace or globally — on a
//! schedule (once, every N minutes, daily/weekdays, or 5-field cron), in reaction to Mac system
//! events (wake, screen unlock, app launch/quit, power, network, app start), file/folder or
//! git/PR events, or manually / via `yh trigger`.
//!
//! Each run can start a new thread, start a new thread and settle the previous run's thread,
//! or continue in the same thread every time. Guardrails (debounce, cooldown, active hours,
//! overlap policy, AC-power-only, optional shell pre-check) prevent event storms and wasted runs.
//!
//! Times are the Mac's local time. Records are kept in one setting, `automations` (a JSON array),
//! preserving full compatibility with v1 (0.4.0) and 0.0.x databases; unknown entries are kept
//! untouched on save.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Mutex;

/// The setting the automations are kept in.
pub const SETTING: &str = "automations";
/// Global kill-switch setting: when true, no scheduled or event automations fire automatically.
pub const PAUSED_SETTING: &str = "automationsPaused";
/// When true, threads created by automations are hidden from the sidebar (default false).
pub const HIDE_THREADS_SETTING: &str = "hideAutomationThreads";
/// Runs remembered per automation, newest last.
const MAX_RUNS: usize = 50;
/// A run later than this after it was due counts as caught up (the app was closed, or asleep).
const LATE: f64 = 120.0;
/// The shortest repeat: a minute.
const MIN_EVERY: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Scope {
    Project { project_id: String },
    Global,
}

impl Scope {
    pub fn is_global(&self) -> bool {
        matches!(self, Scope::Global)
    }

    pub fn project_id(&self) -> Option<&str> {
        match self {
            Scope::Project { project_id } => Some(project_id.as_str()),
            Scope::Global => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThreadPolicy {
    /// Start a new thread every run; previous threads stay live.
    #[default]
    New,
    /// Start a new thread every run and automatically settle the previous run's thread.
    NewSettlePrevious,
    /// Reuse one thread every run (made on the first run, or pinned to an existing thread).
    Same,
}

impl ThreadPolicy {
    pub fn label(self) -> &'static str {
        match self {
            ThreadPolicy::New => "New thread each run",
            ThreadPolicy::NewSettlePrevious => "New thread, settle previous",
            ThreadPolicy::Same => "Same thread each run",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            ThreadPolicy::New => "New thread",
            ThreadPolicy::NewSettlePrevious => "New & settle prev",
            ThreadPolicy::Same => "Same thread",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OverlapPolicy {
    /// Skip this trigger if the previous run of this automation is still working.
    #[default]
    Skip,
    /// Queue into the thread (for SameThread) or run once the active run finishes.
    Queue,
    /// Allow concurrent runs (still capped by the per-project running limit).
    Allow,
}

impl OverlapPolicy {
    pub fn label(self) -> &'static str {
        match self {
            OverlapPolicy::Skip => "Skip if still running",
            OverlapPolicy::Queue => "Queue behind active run",
            OverlapPolicy::Allow => "Allow parallel runs",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NotifyPolicy {
    #[default]
    Always,
    NeedsAttention,
    Silent,
}

impl NotifyPolicy {
    pub fn label(self) -> &'static str {
        match self {
            NotifyPolicy::Always => "When finished or waiting",
            NotifyPolicy::NeedsAttention => "Only when waiting on you",
            NotifyPolicy::Silent => "Silent",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Schedule {
    /// Once, at this moment (seconds since 1970).
    Once { at: f64 },
    /// Every so many minutes, counted from `from`.
    Every { minutes: u32, #[serde(default)] from: f64 },
    /// At a time of day on the weekdays given (0 Sunday … 6 Saturday; none: every day).
    Daily { hour: u32, minute: u32, #[serde(default)] weekdays: Vec<u8> },
    /// A cron line: minute, hour, day of month, month, day of week.
    Cron { expr: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SystemEventKind {
    MacWake,
    ScreenUnlock,
    AppLaunched,
    AppQuit,
    PowerChanged,
    NetworkOnline,
    AppLaunch,
}

impl SystemEventKind {
    pub const ALL: [SystemEventKind; 7] = [
        SystemEventKind::MacWake,
        SystemEventKind::ScreenUnlock,
        SystemEventKind::AppLaunched,
        SystemEventKind::AppQuit,
        SystemEventKind::PowerChanged,
        SystemEventKind::NetworkOnline,
        SystemEventKind::AppLaunch,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SystemEventKind::MacWake => "Mac wakes from sleep",
            SystemEventKind::ScreenUnlock => "Screen unlocks",
            SystemEventKind::AppLaunched => "App launches",
            SystemEventKind::AppQuit => "App quits",
            SystemEventKind::PowerChanged => "Power source changes",
            SystemEventKind::NetworkOnline => "Network comes online",
            SystemEventKind::AppLaunch => "YourHarness opens",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            SystemEventKind::MacWake => "On wake",
            SystemEventKind::ScreenUnlock => "On unlock",
            SystemEventKind::AppLaunched => "App launch",
            SystemEventKind::AppQuit => "App quit",
            SystemEventKind::PowerChanged => "Power change",
            SystemEventKind::NetworkOnline => "Back online",
            SystemEventKind::AppLaunch => "On startup",
        }
    }

    pub fn needs_app_filter(self) -> bool {
        matches!(self, SystemEventKind::AppLaunched | SystemEventKind::AppQuit)
    }

    pub fn needs_power_filter(self) -> bool {
        matches!(self, SystemEventKind::PowerChanged)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemTrigger {
    pub event: SystemEventKind,
    /// Optional app name or bundle ID substring for AppLaunched / AppQuit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// Optional power filter for PowerChanged: "ac" or "battery".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<String>,
}

impl SystemTrigger {
    pub fn describe(&self) -> String {
        match self.event {
            SystemEventKind::AppLaunched => match self.app.as_deref().filter(|s| !s.trim().is_empty()) {
                Some(app) => format!("When {app} launches"),
                None => "When any app launches".into(),
            },
            SystemEventKind::AppQuit => match self.app.as_deref().filter(|s| !s.trim().is_empty()) {
                Some(app) => format!("When {app} quits"),
                None => "When any app quits".into(),
            },
            SystemEventKind::PowerChanged => match self.power.as_deref() {
                Some("ac") => "When plugged in".into(),
                Some("battery") => "When on battery".into(),
                _ => "When power changes".into(),
            },
            other => other.label().into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileWatchTrigger {
    /// Relative to project folder ("" for project root), or absolute / `~`-prefixed path.
    #[serde(default)]
    pub path: String,
    /// Optional comma-separated glob patterns (e.g. "*.pdf", "docs/**/*.md").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
    /// Optional event kinds: "create", "modify", "delete" (empty: any change).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<String>,
}

impl FileWatchTrigger {
    pub fn describe(&self) -> String {
        let target = if self.path.trim().is_empty() { "workspace" } else { self.path.trim() };
        match self.glob.as_deref().map(str::trim).filter(|g| !g.is_empty()) {
            Some(g) => format!("Watch {target} ({g})"),
            None => format!("Watch {target}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RepoEventKind {
    BranchChanged,
    CommitMade,
    PrReviewRequested,
    PrChecksFailed,
    PrMerged,
}

impl RepoEventKind {
    pub const ALL: [RepoEventKind; 5] = [
        RepoEventKind::BranchChanged,
        RepoEventKind::CommitMade,
        RepoEventKind::PrReviewRequested,
        RepoEventKind::PrChecksFailed,
        RepoEventKind::PrMerged,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RepoEventKind::BranchChanged => "Git branch changes",
            RepoEventKind::CommitMade => "New git commit",
            RepoEventKind::PrReviewRequested => "PR review requested",
            RepoEventKind::PrChecksFailed => "PR checks fail",
            RepoEventKind::PrMerged => "PR merged",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoTrigger {
    pub event: RepoEventKind,
    /// Optional branch filter (e.g. "main", "release/*").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

impl RepoTrigger {
    pub fn describe(&self) -> String {
        match self.branch.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
            Some(b) => format!("{} ({b})", self.event.label()),
            None => self.event.label().into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Trigger {
    Schedule { schedule: Schedule },
    System(SystemTrigger),
    FileWatch(FileWatchTrigger),
    Repo(RepoTrigger),
    Manual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerCategory {
    Schedule,
    System,
    FileRepo,
    Manual,
}

impl Trigger {
    pub fn category(&self) -> TriggerCategory {
        match self {
            Trigger::Schedule { .. } => TriggerCategory::Schedule,
            Trigger::System(_) => TriggerCategory::System,
            Trigger::FileWatch(_) | Trigger::Repo(_) => TriggerCategory::FileRepo,
            Trigger::Manual => TriggerCategory::Manual,
        }
    }

    pub fn schedule(&self) -> Option<&Schedule> {
        match self {
            Trigger::Schedule { schedule } => Some(schedule),
            _ => None,
        }
    }

    pub fn is_event(&self) -> bool {
        matches!(self, Trigger::System(_) | Trigger::FileWatch(_) | Trigger::Repo(_))
    }

    pub fn describe(&self, now: f64) -> String {
        match self {
            Trigger::Schedule { schedule } => schedule.describe(now),
            Trigger::System(s) => s.describe(),
            Trigger::FileWatch(f) => f.describe(),
            Trigger::Repo(r) => r.describe(),
            Trigger::Manual => "Manual / CLI".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveHours {
    pub start_hour: u32,
    #[serde(default)]
    pub start_minute: u32,
    pub end_hour: u32,
    #[serde(default)]
    pub end_minute: u32,
    /// 0 Sunday … 6 Saturday; empty means every day.
    #[serde(default)]
    pub weekdays: Vec<u8>,
}

impl ActiveHours {
    pub fn contains(&self, now: f64) -> bool {
        let (_, _, _, hh, mm, wd) = local(now);
        if !self.weekdays.is_empty() && !self.weekdays.contains(&(wd as u8)) {
            return false;
        }
        let cur = hh * 60 + mm;
        let start = self.start_hour * 60 + self.start_minute;
        let end = self.end_hour * 60 + self.end_minute;
        if start <= end {
            cur >= start && cur <= end
        } else {
            // Overnight window, e.g. 22:00 - 06:00.
            cur >= start || cur <= end
        }
    }

    pub fn describe(&self) -> String {
        let range = format!("{}:{:02}–{}:{:02}", self.start_hour, self.start_minute, self.end_hour, self.end_minute);
        if self.weekdays.is_empty() {
            range
        } else {
            let mut days = self.weekdays.clone();
            days.sort_unstable();
            days.dedup();
            match days.as_slice() {
                [1, 2, 3, 4, 5] => format!("Weekdays {range}"),
                [0, 6] => format!("Weekends {range}"),
                many => format!("{} {range}", many.iter().map(|d| DAYS[*d as usize % 7]).collect::<Vec<_>>().join(", ")),
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Guardrails {
    /// Burst debounce for file/repo watchers, in seconds (0 uses default 2s).
    #[serde(default)]
    pub debounce_secs: u32,
    /// Minimum seconds between non-manual runs (0 = no cooldown).
    #[serde(default)]
    pub cooldown_secs: u32,
    /// Optional local-time window during which non-manual runs may fire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_hours: Option<ActiveHours>,
    #[serde(default)]
    pub overlap: OverlapPolicy,
    /// Skip automatic runs when the Mac is on battery power.
    #[serde(default)]
    pub ac_power_only: bool,
    /// Optional fast shell pre-check command; non-zero exit skips invoking the AI agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition_cmd: Option<String>,
}

impl Guardrails {
    pub fn effective_debounce_secs(&self) -> u32 {
        if self.debounce_secs == 0 { 2 } else { self.debounce_secs }
    }

    pub fn is_default(&self) -> bool {
        *self == Guardrails::default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunStatus {
    #[default]
    Done,
    Running,
    Waiting,
    Skipped,
    Error,
}

impl RunStatus {
    pub fn label(self) -> &'static str {
        match self {
            RunStatus::Done => "Done",
            RunStatus::Running => "Running",
            RunStatus::Waiting => "Waiting",
            RunStatus::Skipped => "Skipped",
            RunStatus::Error => "Failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    /// When it ran.
    pub at: f64,
    /// The thread it started or sent to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// Why it couldn't run (or why it was skipped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// It ran late: it came due while the app was closed or the Mac slept.
    #[serde(default)]
    pub caught_up: bool,
    /// Run Now, not the schedule/event.
    #[serde(default)]
    pub manual: bool,
    /// Live or final status (None in v1 records; inferred via `effective_status`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<RunStatus>,
    /// Short human description of what triggered this run ("Scheduled", "Mac woke", "File changed", "CLI").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_reason: Option<String>,
    /// How long the turn took in seconds, recorded when it finishes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<f64>,
    /// The previous thread that was settled by this run (`NewSettlePrevious`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_previous: Option<String>,
}

impl Run {
    pub fn effective_status(&self) -> RunStatus {
        if let Some(s) = self.status {
            return s;
        }
        if self.error.is_some() { RunStatus::Error } else { RunStatus::Done }
    }
}

/// What an event (or CLI trigger) hands to the prompt renderer and run history.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TriggerEvent {
    /// Short summary for run history, e.g. "Mac woke", "File changed: report.pdf", "CLI".
    pub reason: String,
    /// Machine category, e.g. "schedule", "macWake", "fileWatch", "repo", "cli", "manual".
    pub kind: String,
    /// Key-value details for `{{event.<key>}}` interpolation and `<automation_event>` context.
    pub fields: Vec<(String, String)>,
    /// Changed or attached file paths.
    pub files: Vec<String>,
}

impl TriggerEvent {
    pub fn manual() -> Self {
        TriggerEvent { reason: "Run manually".into(), kind: "manual".into(), fields: vec![], files: vec![] }
    }

    pub fn scheduled(caught_up: bool) -> Self {
        TriggerEvent {
            reason: if caught_up { "Scheduled (caught up)".into() } else { "Scheduled".into() },
            kind: "schedule".into(),
            fields: vec![],
            files: vec![],
        }
    }

    pub fn has_context(&self) -> bool {
        !self.fields.is_empty() || !self.files.is_empty()
    }
}

/// Renders an automation prompt with optional `{{time}}` and `{{event.*}}` placeholders.
/// When an event carries files or fields that were not explicitly interpolated, appends a
/// structured `<automation_event>` block so the agent always sees what happened.
pub fn render_prompt(template: &str, event: Option<&TriggerEvent>, now: f64) -> String {
    let mut out = template.replace("{{time}}", &when(now, now));
    let Some(ev) = event else { return out };
    let used_event_placeholder = out.contains("{{event.");
    out = out.replace("{{event.reason}}", &ev.reason);
    out = out.replace("{{event.kind}}", &ev.kind);
    out = out.replace("{{event.files}}", &ev.files.join("\n"));
    for (k, v) in &ev.fields {
        out = out.replace(&format!("{{{{event.{k}}}}}"), v);
    }
    if ev.has_context() && !used_event_placeholder {
        let mut block = format!("\n\n<automation_event kind=\"{}\">\nTrigger: {}", ev.kind, ev.reason);
        for (k, v) in &ev.fields {
            block.push_str(&format!("\n{k}: {v}"));
        }
        if !ev.files.is_empty() {
            block.push_str("\nFiles:\n");
            for f in ev.files.iter().take(50) {
                block.push_str(&format!("- {f}\n"));
            }
            if ev.files.len() > 50 {
                block.push_str(&format!("…and {} more\n", ev.files.len() - 50));
            }
        }
        block.push_str("\n</automation_event>");
        out.push_str(&block);
    }
    out
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Automation {
    pub id: String,
    /// Workspace project ID when scoped to a project; empty (or Home's ID) when `is_global`.
    #[serde(default)]
    pub project_id: String,
    /// Global automations run in Home and are not bound to a single workspace.
    #[serde(default)]
    pub is_global: bool,
    pub name: String,
    /// What's sent to the agent each run.
    pub prompt: String,
    /// Legacy v1 schedule field; still kept in sync for Schedule triggers for downgrade safety.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<Schedule>,
    /// Full v2 trigger (Schedule, System, FileWatch, Repo, Manual). None on v1 records -> uses `schedule`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<Trigger>,
    /// Every run goes to one thread (`thread_id`, made by the first run when there's none)
    /// rather than a new thread each time.
    #[serde(default)]
    pub same_thread: bool,
    /// When starting a new thread (`!same_thread`), automatically settle the previous run's thread.
    #[serde(default)]
    pub settle_previous: bool,
    /// Persistent thread ID when `same_thread` is true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// The thread started by the most recent run (used by `NewSettlePrevious` and overlap checks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_thread_id: Option<String>,
    /// Agent override ("claude" | "antigravity"), None for app default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Per-automation permission mode override ("ask", "acceptEdits", "plan", "bypass"); None = project default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Optional working directory override (especially useful for Global automations).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub notify: NotifyPolicy,
    #[serde(default, skip_serializing_if = "Guardrails::is_default")]
    pub guardrails: Guardrails,
    pub enabled: bool,
    pub created_at: f64,
    /// The thread whose agent made it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    /// When it runs next (for Schedule triggers); None when paused, done, or event-triggered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run: Option<f64>,
    #[serde(default)]
    pub runs: Vec<Run>,
}

impl Automation {
    /// A new scheduled project automation (v1-compatible constructor).
    pub fn new(project_id: &str, name: &str, prompt: &str, schedule: Schedule, now: f64) -> Result<Automation, String> {
        Self::new_with_trigger(
            Scope::Project { project_id: project_id.into() },
            name,
            prompt,
            Trigger::Schedule { schedule },
            now,
        )
    }

    /// A new automation with any scope and trigger. Refuses a one-time schedule in the past.
    pub fn new_with_trigger(scope: Scope, name: &str, prompt: &str, trigger: Trigger, now: f64) -> Result<Automation, String> {
        let (schedule, next_run) = match &trigger {
            Trigger::Schedule { schedule } => {
                let next = schedule.next_after(now);
                if next.is_none() {
                    return Err("That time has already passed.".into());
                }
                (Some(schedule.clone()), next)
            }
            _ => (None, None),
        };
        let (is_global, project_id) = match scope {
            Scope::Global => (true, String::new()),
            Scope::Project { project_id } => (false, project_id),
        };
        Ok(Automation {
            id: crate::library::new_uuid(),
            project_id,
            is_global,
            name: name.trim().into(),
            prompt: prompt.trim().into(),
            schedule,
            trigger: Some(trigger),
            same_thread: false,
            settle_previous: false,
            thread_id: None,
            last_thread_id: None,
            agent: None,
            model: None,
            effort: None,
            mode: None,
            working_dir: None,
            notify: NotifyPolicy::Always,
            guardrails: Guardrails::default(),
            enabled: true,
            created_at: now,
            created_by: None,
            next_run,
            runs: vec![],
        })
    }

    pub fn scope(&self) -> Scope {
        if self.is_global || self.project_id.is_empty() {
            Scope::Global
        } else {
            Scope::Project { project_id: self.project_id.clone() }
        }
    }

    pub fn set_scope(&mut self, scope: Scope) {
        match scope {
            Scope::Global => {
                self.is_global = true;
                self.project_id.clear();
            }
            Scope::Project { project_id } => {
                self.is_global = false;
                self.project_id = project_id;
            }
        }
    }

    pub fn effective_trigger(&self) -> Trigger {
        if let Some(t) = &self.trigger {
            return t.clone();
        }
        match &self.schedule {
            Some(s) => Trigger::Schedule { schedule: s.clone() },
            None => Trigger::Manual,
        }
    }

    pub fn thread_policy(&self) -> ThreadPolicy {
        if self.same_thread {
            ThreadPolicy::Same
        } else if self.settle_previous {
            ThreadPolicy::NewSettlePrevious
        } else {
            ThreadPolicy::New
        }
    }

    pub fn set_thread_policy(&mut self, policy: ThreadPolicy) {
        let was_same = self.same_thread;
        match policy {
            ThreadPolicy::New => {
                self.same_thread = false;
                self.settle_previous = false;
                if was_same {
                    self.thread_id = None;
                }
            }
            ThreadPolicy::NewSettlePrevious => {
                self.same_thread = false;
                self.settle_previous = true;
                if was_same {
                    self.thread_id = None;
                }
            }
            ThreadPolicy::Same => {
                if !was_same {
                    self.thread_id = None;
                }
                self.same_thread = true;
                self.settle_previous = false;
            }
        }
    }

    /// Whether the schedule timer should run it now.
    pub fn due(&self, now: f64) -> bool {
        self.enabled && self.effective_trigger().schedule().is_some() && self.next_run.is_some_and(|t| t <= now)
    }

    /// Whether a one-time scheduled automation has finished its single run.
    pub fn is_done(&self) -> bool {
        !self.enabled
            && self.next_run.is_none()
            && self.effective_trigger().schedule().is_some_and(|s| !s.repeats())
            && !self.runs.is_empty()
    }

    /// Seconds remaining in cooldown before another automatic run is allowed, or None if clear.
    pub fn cooldown_remaining(&self, now: f64) -> Option<f64> {
        if self.guardrails.cooldown_secs == 0 {
            return None;
        }
        let last_real = self.runs.iter().rev().find(|r| !r.manual && r.effective_status() != RunStatus::Skipped)?;
        let elapsed = now - last_real.at;
        let rem = self.guardrails.cooldown_secs as f64 - elapsed;
        (rem > 0.0).then_some(rem)
    }

    /// Records a run and advances a scheduled automation's next run.
    pub fn ran(&mut self, now: f64, thread_id: Option<String>, error: Option<String>, manual: bool) {
        let reason = if manual { "Run manually".to_string() } else { "Scheduled".to_string() };
        self.record_run(now, thread_id, error, manual, reason, None, None);
    }

    /// Records a run with full v2 metadata (trigger reason, live status, settled previous thread).
    #[allow(clippy::too_many_arguments)]
    pub fn record_run(
        &mut self,
        now: f64,
        thread_id: Option<String>,
        error: Option<String>,
        manual: bool,
        trigger_reason: String,
        status: Option<RunStatus>,
        settled_previous: Option<String>,
    ) {
        let is_schedule = self.effective_trigger().schedule().is_some();
        let caught_up = !manual && is_schedule && self.next_run.is_some_and(|t| now - t > LATE);
        let status = status.unwrap_or_else(|| if error.is_some() { RunStatus::Error } else { RunStatus::Running });
        if error.is_none() && status != RunStatus::Skipped {
            if let Some(tid) = &thread_id {
                self.last_thread_id = Some(tid.clone());
                if self.same_thread {
                    self.thread_id = Some(tid.clone());
                }
            }
        }
        self.runs.push(Run {
            at: now,
            thread_id,
            error,
            caught_up,
            manual,
            status: Some(status),
            trigger_reason: Some(if caught_up && trigger_reason == "Scheduled" {
                "Scheduled (caught up)".into()
            } else {
                trigger_reason
            }),
            duration_secs: None,
            settled_previous,
        });
        if self.runs.len() > MAX_RUNS {
            self.runs.drain(..self.runs.len() - MAX_RUNS);
        }
        if !manual {
            if let Some(schedule) = self.effective_trigger().schedule() {
                self.next_run = schedule.next_after(now);
                if self.next_run.is_none() {
                    self.enabled = false;
                }
            }
        }
    }

    /// Marks the most recent run for `thread_id` as finished/waiting with its elapsed duration.
    pub fn finish_run(&mut self, thread_id: &str, status: RunStatus, now: f64) -> bool {
        if let Some(run) = self.runs.iter_mut().rev().find(|r| r.thread_id.as_deref() == Some(thread_id)) {
            if matches!(run.effective_status(), RunStatus::Running | RunStatus::Waiting) {
                run.status = Some(status);
                if matches!(status, RunStatus::Done | RunStatus::Error) {
                    run.duration_secs = Some((now - run.at).max(0.0));
                }
                return true;
            }
        }
        false
    }

    /// Paused or resumed; on resume, recalculates `next_run` for Schedule triggers.
    pub fn set_enabled(&mut self, on: bool, now: f64) {
        if let Some(schedule) = self.effective_trigger().schedule() {
            self.next_run = if on { schedule.next_after(now) } else { None };
            self.enabled = on && self.next_run.is_some();
        } else {
            self.next_run = None;
            self.enabled = on;
        }
    }

    /// Replaces the schedule (for Schedule triggers).
    pub fn set_schedule(&mut self, schedule: Schedule, now: f64) -> Result<(), String> {
        self.set_trigger(Trigger::Schedule { schedule }, now)
    }

    /// Replaces the trigger and updates `schedule` + `next_run` accordingly.
    pub fn set_trigger(&mut self, trigger: Trigger, now: f64) -> Result<(), String> {
        match &trigger {
            Trigger::Schedule { schedule } => {
                let next = schedule.next_after(now);
                if next.is_none() {
                    return Err("That time has already passed.".into());
                }
                self.schedule = Some(schedule.clone());
                self.trigger = Some(trigger);
                if self.enabled {
                    self.next_run = next;
                }
            }
            _ => {
                self.schedule = None;
                self.trigger = Some(trigger);
                self.next_run = None;
            }
        }
        Ok(())
    }

    pub fn last_run(&self) -> Option<&Run> {
        self.runs.last()
    }

    /// The first 8 characters of its id, as the tools show it.
    pub fn short_id(&self) -> &str {
        &self.id[..self.id.len().min(8)]
    }

    /// Slug usable with `yh trigger <slug>`.
    pub fn slug(&self) -> String {
        let mut out = String::new();
        let mut dash = false;
        for c in self.name.chars() {
            if c.is_ascii_alphanumeric() {
                out.push(c.to_ascii_lowercase());
                dash = false;
            } else if !out.is_empty() && !dash {
                out.push('-');
                dash = true;
            }
        }
        let s = out.trim_end_matches('-').to_string();
        if s.is_empty() { self.short_id().to_string() } else { s }
    }
}

// ---------------------------------------------------------------------------------------------
// Starter templates

#[derive(Clone, Debug)]
pub struct Template {
    pub id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    pub is_global: bool,
    pub prompt: &'static str,
    pub trigger: Trigger,
    pub thread_policy: ThreadPolicy,
    pub guardrails: Guardrails,
}

pub fn templates() -> Vec<Template> {
    vec![
        Template {
            id: "morning-briefing",
            name: "Morning Briefing",
            summary: "Global · When Mac wakes (once/day) · New & settle previous",
            is_global: true,
            prompt: "Review my open tasks and active threads across projects. Summarize what needs my attention today, any blocked items, and suggested next steps in a concise bullet list.",
            trigger: Trigger::System(SystemTrigger { event: SystemEventKind::MacWake, app: None, power: None }),
            thread_policy: ThreadPolicy::NewSettlePrevious,
            guardrails: Guardrails {
                cooldown_secs: 18 * 3600,
                active_hours: Some(ActiveHours { start_hour: 6, start_minute: 0, end_hour: 12, end_minute: 0, weekdays: vec![1, 2, 3, 4, 5] }),
                ..Default::default()
            },
        },
        Template {
            id: "weekday-pr-sweep",
            name: "Weekday PR Sweep",
            summary: "Workspace · Weekdays at 9:00 · New & settle previous",
            is_global: false,
            prompt: "Check open pull requests in this workspace (using gh/glab). Highlight PRs waiting on my review, failing CI checks, or ready to merge, and suggest the highest-priority action.",
            trigger: Trigger::Schedule { schedule: Schedule::Daily { hour: 9, minute: 0, weekdays: vec![1, 2, 3, 4, 5] } },
            thread_policy: ThreadPolicy::NewSettlePrevious,
            guardrails: Guardrails::default(),
        },
        Template {
            id: "ci-failure-triage",
            name: "CI Failure Triage",
            summary: "Workspace · When PR checks fail · New thread",
            is_global: false,
            prompt: "CI checks failed on a pull request. Inspect the failing check logs, identify the root cause, and propose a minimal fix (do not push without asking).",
            trigger: Trigger::Repo(RepoTrigger { event: RepoEventKind::PrChecksFailed, branch: None }),
            thread_policy: ThreadPolicy::New,
            guardrails: Guardrails { cooldown_secs: 600, ..Default::default() },
        },
        Template {
            id: "downloads-inbox",
            name: "Downloads Inbox",
            summary: "Global · Watch ~/Downloads (*.pdf) · New thread",
            is_global: true,
            prompt: "A new PDF landed in Downloads. Read its title/first page, summarize what it is in 2 lines, and suggest where it should be filed.",
            trigger: Trigger::FileWatch(FileWatchTrigger { path: "~/Downloads".into(), glob: Some("*.pdf".into()), events: vec!["create".into()] }),
            thread_policy: ThreadPolicy::New,
            guardrails: Guardrails { debounce_secs: 3, cooldown_secs: 30, ..Default::default() },
        },
        Template {
            id: "eod-memory-digest",
            name: "End-of-Day Digest",
            summary: "Workspace · Weekdays at 18:00 · Same thread",
            is_global: false,
            prompt: "Look over today's git commits and recent threads in this workspace. Summarize what shipped today and note any open loose ends for tomorrow.",
            trigger: Trigger::Schedule { schedule: Schedule::Daily { hour: 18, minute: 0, weekdays: vec![1, 2, 3, 4, 5] } },
            thread_policy: ThreadPolicy::Same,
            guardrails: Guardrails::default(),
        },
    ]
}

// ---------------------------------------------------------------------------------------------
// Glob & path matching for file/repo triggers

/// Default noisy paths ignored by folder watchers unless explicitly matched by pattern.
pub fn is_ignored_path(rel_path: &str, pattern: Option<&str>) -> bool {
    let norm = rel_path.replace('\\', "/");
    let pat = pattern.unwrap_or("");
    for segment in norm.split('/') {
        if segment == ".DS_Store" && !pat.contains(".DS_Store") {
            return true;
        }
        if matches!(segment, ".git" | "node_modules" | "target" | ".next" | "dist") && !pat.contains(segment) {
            return true;
        }
    }
    false
}

/// Comma-separated glob matching (`*` single segment, `**` any depth, `?` single char; case-insensitive).
pub fn glob_matches(patterns: &str, path: &str) -> bool {
    let trimmed = patterns.trim();
    if trimmed.is_empty() {
        return true;
    }
    let path_norm = path.trim_start_matches('/').to_lowercase();
    let filename = path_norm.rsplit('/').next().unwrap_or(&path_norm);
    for raw in trimmed.split(',') {
        let pat = raw.trim().trim_start_matches('/').to_lowercase();
        if pat.is_empty() {
            continue;
        }
        // A pattern without `/` matches either the full path or just the filename (like `*.pdf`).
        if !pat.contains('/') && glob_match_one(&pat, filename) {
            return true;
        }
        if glob_match_one(&pat, &path_norm) {
            return true;
        }
    }
    false
}

fn glob_match_one(pat: &str, text: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_dp(&p, &t)
}

fn glob_dp(p: &[char], t: &[char]) -> bool {
    let (m, n) = (p.len(), t.len());
    let mut dp = vec![vec![false; n + 1]; m + 1];
    dp[0][0] = true;
    for i in 1..=m {
        if p[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        } else if i >= 3 && p[i - 1] == '/' && p[i - 2] == '*' && p[i - 3] == '*' {
            // `**/` matches empty prefix.
            dp[i][0] = dp[i - 3][0];
        }
    }
    for i in 1..=m {
        let is_doublestar = i >= 2 && p[i - 1] == '*' && p[i - 2] == '*';
        for j in 1..=n {
            if is_doublestar {
                // `**` matches anything including `/`.
                dp[i][j] = dp[i - 1][j] || dp[i][j - 1] || dp[i - 2][j];
            } else if i >= 3 && p[i - 1] == '/' && p[i - 2] == '*' && p[i - 3] == '*' {
                // `**/` matches zero segments OR consumes up to `/`.
                dp[i][j] = dp[i - 3][j] || (t[j - 1] == '/' && dp[i - 1][j - 1]);
            } else if p[i - 1] == '*' {
                // Single `*` matches any char except `/`.
                dp[i][j] = dp[i - 1][j] || (t[j - 1] != '/' && dp[i][j - 1]);
            } else if p[i - 1] == '?' {
                dp[i][j] = t[j - 1] != '/' && dp[i - 1][j - 1];
            } else {
                dp[i][j] = p[i - 1] == t[j - 1] && dp[i - 1][j - 1];
            }
        }
    }
    dp[m][n]
}

// ---------------------------------------------------------------------------------------------
// Shorthand trigger parser (for MCP tools and CLI)

/// Parses a shorthand trigger string such as `"wake"`, `"unlock"`, `"app_launched:Xcode"`,
/// `"app_quit:Zoom"`, `"power:ac"`, `"network"`, `"app_launch"`, `"watch:docs/**/*.md"`,
/// `"branch"`, `"commit"`, `"pr_review"`, `"pr_failed"`, `"pr_merged"`, `"manual"`,
/// or falls back to `parse(schedule)`.
pub fn parse_trigger(text: &str, now: f64) -> Result<Trigger, String> {
    let raw = text.trim();
    let lower = raw.to_lowercase();
    if lower == "manual" || lower == "cli" {
        return Ok(Trigger::Manual);
    }
    if lower == "wake" || lower == "mac_wake" || lower == "macwake" {
        return Ok(Trigger::System(SystemTrigger { event: SystemEventKind::MacWake, app: None, power: None }));
    }
    if lower == "unlock" || lower == "screen_unlock" || lower == "screenunlock" {
        return Ok(Trigger::System(SystemTrigger { event: SystemEventKind::ScreenUnlock, app: None, power: None }));
    }
    if lower == "network" || lower == "network_online" || lower == "online" {
        return Ok(Trigger::System(SystemTrigger { event: SystemEventKind::NetworkOnline, app: None, power: None }));
    }
    if lower == "startup" || lower == "app_launch" || lower == "applaunch" {
        return Ok(Trigger::System(SystemTrigger { event: SystemEventKind::AppLaunch, app: None, power: None }));
    }
    if let Some(rest) = raw.strip_prefix("app_launched:").or_else(|| raw.strip_prefix("appLaunched:")) {
        let app = rest.trim();
        return Ok(Trigger::System(SystemTrigger { event: SystemEventKind::AppLaunched, app: (!app.is_empty()).then(|| app.to_string()), power: None }));
    }
    if let Some(rest) = raw.strip_prefix("app_quit:").or_else(|| raw.strip_prefix("appQuit:")) {
        let app = rest.trim();
        return Ok(Trigger::System(SystemTrigger { event: SystemEventKind::AppQuit, app: (!app.is_empty()).then(|| app.to_string()), power: None }));
    }
    if lower == "power" || lower.starts_with("power:") {
        let p = lower.strip_prefix("power:").map(str::trim).filter(|s| matches!(*s, "ac" | "battery")).map(str::to_string);
        return Ok(Trigger::System(SystemTrigger { event: SystemEventKind::PowerChanged, app: None, power: p }));
    }
    if let Some(rest) = raw.strip_prefix("watch:") {
        let rest = rest.trim();
        let (path, glob) = match rest.split_once('|') {
            Some((p, g)) => (p.trim().to_string(), Some(g.trim().to_string()).filter(|s| !s.is_empty())),
            None if rest.contains('*') && !rest.starts_with('/') && !rest.starts_with('~') => (String::new(), Some(rest.to_string())),
            None => (rest.to_string(), None),
        };
        return Ok(Trigger::FileWatch(FileWatchTrigger { path, glob, events: vec![] }));
    }
    if lower == "branch" || lower.starts_with("branch:") {
        let branch = raw.split_once(':').map(|(_, b)| b.trim().to_string()).filter(|s| !s.is_empty());
        return Ok(Trigger::Repo(RepoTrigger { event: RepoEventKind::BranchChanged, branch }));
    }
    if lower == "commit" || lower.starts_with("commit:") {
        let branch = raw.split_once(':').map(|(_, b)| b.trim().to_string()).filter(|s| !s.is_empty());
        return Ok(Trigger::Repo(RepoTrigger { event: RepoEventKind::CommitMade, branch }));
    }
    if lower == "pr_review" || lower == "pr_review_requested" {
        return Ok(Trigger::Repo(RepoTrigger { event: RepoEventKind::PrReviewRequested, branch: None }));
    }
    if lower == "pr_failed" || lower == "pr_checks_failed" {
        return Ok(Trigger::Repo(RepoTrigger { event: RepoEventKind::PrChecksFailed, branch: None }));
    }
    if lower == "pr_merged" {
        return Ok(Trigger::Repo(RepoTrigger { event: RepoEventKind::PrMerged, branch: None }));
    }
    parse(raw, now).map(|schedule| Trigger::Schedule { schedule })
}

/// Parses a duration string like `"30s"`, `"15m"`, `"2h"`, `"1d"` into seconds.
pub fn parse_duration_secs(text: &str) -> Option<u32> {
    let t = text.trim().to_lowercase();
    if t.is_empty() || t == "0" || t == "none" || t == "off" {
        return Some(0);
    }
    if let Ok(secs) = t.parse::<u32>() {
        return Some(secs);
    }
    if let Some(s) = t.strip_suffix('s').and_then(|n| n.trim().parse::<u32>().ok()) {
        return Some(s);
    }
    span(&t).map(|m| m.saturating_mul(60))
}

/// Formats seconds as a compact human duration ("30s", "15m", "2h", "1d").
pub fn format_duration(secs: u32) -> String {
    match secs {
        0 => "Off".into(),
        s if s % 86_400 == 0 => format!("{}d", s / 86_400),
        s if s % 3600 == 0 => format!("{}h", s / 3600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

// ---------------------------------------------------------------------------------------------
// Local time

/// A moment in local time: (year, month 1–12, day, hour, minute, weekday 0 Sunday).
fn local(seconds: f64) -> (i32, u32, u32, u32, u32, u32) {
    let t = seconds.floor() as libc::time_t;
    // SAFETY: localtime_r writes into the `tm` given and nothing else.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    (tm.tm_year + 1900, tm.tm_mon as u32 + 1, tm.tm_mday as u32, tm.tm_hour as u32, tm.tm_min as u32, tm.tm_wday as u32)
}

/// Seconds since 1970 of a local date and time; out-of-range days roll over (the 32nd is the
/// next month's 1st). A time skipped by daylight saving lands an hour on.
fn moment(year: i32, month: u32, day: i32, hour: u32, minute: u32) -> f64 {
    // SAFETY: mktime reads and normalizes the `tm` given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year - 1900;
    tm.tm_mon = month as i32 - 1;
    tm.tm_mday = day;
    tm.tm_hour = hour as i32;
    tm.tm_min = minute as i32;
    tm.tm_isdst = -1;
    unsafe { libc::mktime(&mut tm) as f64 }
}

/// The local zone's short name ("CET", "PST").
pub fn zone() -> String {
    let t = now() as libc::time_t;
    // SAFETY: as in `local`; tm_zone points at a static string of the C library.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    if tm.tm_zone.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(tm.tm_zone) }.to_string_lossy().into_owned()
}

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// "today at 9:00", "tomorrow at 9:00", "yesterday at 9:00", "Mon, Nov 24 at 9:00",
/// "Nov 24, 2027 at 9:00": a run's time, past or coming.
pub fn when(t: f64, now: f64) -> String {
    let (y, m, d, hh, mm, wd) = local(t);
    let time = format!("{hh}:{mm:02}");
    let day = |offset: f64| {
        let o = local(now + offset);
        (o.0, o.1, o.2) == (y, m, d)
    };
    if day(0.0) {
        return format!("today at {time}");
    }
    if day(86_400.0) {
        return format!("tomorrow at {time}");
    }
    if day(-86_400.0) {
        return format!("yesterday at {time}");
    }
    let month = MONTHS[(m as usize).saturating_sub(1).min(11)];
    if y == local(now).0 {
        format!("{}, {month} {d} at {time}", DAYS[wd as usize % 7])
    } else {
        format!("{month} {d}, {y} at {time}")
    }
}

// ---------------------------------------------------------------------------------------------
// Schedules

impl Schedule {
    /// The first run strictly after `after`, or None (a one-time run that's passed, a cron line
    /// that never matches).
    pub fn next_after(&self, after: f64) -> Option<f64> {
        match self {
            Schedule::Once { at } => (*at > after).then_some(*at),
            Schedule::Every { minutes, from } => {
                let period = (*minutes).max(MIN_EVERY) as f64 * 60.0;
                if *from > after {
                    return Some(*from);
                }
                let k = ((after - from) / period).floor() + 1.0;
                Some(from + k * period)
            }
            Schedule::Daily { hour, minute, weekdays } => {
                let (y, m, d, ..) = local(after);
                (0..9).map(|offset| moment(y, m, d as i32 + offset, *hour, *minute)).find(|t| *t > after && (weekdays.is_empty() || weekdays.contains(&(local(*t).5 as u8))))
            }
            Schedule::Cron { expr } => Cron::parse(expr).ok()?.next_after(after),
        }
    }

    /// "Every day at 9:00", "Weekdays at 9:00", "Mon, Wed at 18:30", "Every 2 hours", "Once,
    /// tomorrow at 9:00", "Cron 0 9 * * 1-5".
    pub fn describe(&self, now: f64) -> String {
        match self {
            Schedule::Once { at } => format!("Once, {}", when(*at, now)),
            Schedule::Every { minutes, .. } => every_words(*minutes),
            Schedule::Daily { hour, minute, weekdays } => {
                let time = format!("{hour}:{minute:02}");
                let mut days = weekdays.clone();
                days.sort_unstable();
                days.dedup();
                match days.as_slice() {
                    [] | [0, 1, 2, 3, 4, 5, 6] => format!("Every day at {time}"),
                    [1, 2, 3, 4, 5] => format!("Weekdays at {time}"),
                    [0, 6] => format!("Weekends at {time}"),
                    [one] => format!("{}s at {time}", DAY_NAMES[*one as usize % 7]),
                    many => format!("{} at {time}", many.iter().map(|d| DAYS[*d as usize % 7]).collect::<Vec<_>>().join(", ")),
                }
            }
            Schedule::Cron { expr } => format!("Cron {expr}"),
        }
    }

    /// Repeats (anything but once).
    pub fn repeats(&self) -> bool {
        !matches!(self, Schedule::Once { .. })
    }

    /// Editable form `parse` reads back into the same schedule (e.g. `daily 9:00`, `every 2h`).
    pub fn to_input_string(&self) -> String {
        match self {
            Schedule::Once { at } => {
                let (year, month, day, hour, minute, _) = local(*at);
                format!("{year:04}-{month:02}-{day:02} {hour}:{minute:02}")
            }
            Schedule::Every { minutes, .. } if minutes % 60 == 0 => format!("every {}h", minutes / 60),
            Schedule::Every { minutes, .. } => format!("every {minutes}m"),
            Schedule::Daily { hour, minute, weekdays } => {
                let mut sorted = weekdays.clone();
                sorted.sort_unstable();
                sorted.dedup();
                let days = match sorted.as_slice() {
                    [] => "daily".to_string(),
                    [1, 2, 3, 4, 5] => "weekdays".into(),
                    [0, 6] => "weekends".into(),
                    [one] => format!("{}s", ["sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"][*one as usize % 7]),
                    many => many.iter().map(|d| ["sun", "mon", "tue", "wed", "thu", "fri", "sat"][*d as usize % 7]).collect::<Vec<_>>().join(","),
                };
                format!("{days} {hour}:{minute:02}")
            }
            Schedule::Cron { expr } => format!("cron {expr}"),
        }
    }
}

const DAY_NAMES: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

fn every_words(minutes: u32) -> String {
    match minutes {
        1 => "Every minute".into(),
        1440 => "Every day".into(),
        m if m % 1440 == 0 => format!("Every {} days", m / 1440),
        60 => "Every hour".into(),
        m if m % 60 == 0 => format!("Every {} hours", m / 60),
        m => format!("Every {m} minutes"),
    }
}

/// What `parse` takes, for error messages and the tools' descriptions.
pub const FORMS: &str = "\"in 30m\", \"tomorrow 9:00\", \"2026-12-01 14:30\" (once); \"every 15m\", \"every 2h\", \"hourly\"; \"daily 9:00\", \"weekdays 8:30\", \"weekends 10am\", \"mon,thu 18:00\", \"every friday at 17:00\"; \"cron 0 9 * * 1-5\"";

/// A schedule from what someone wrote (see `FORMS`), in local time.
pub fn parse(text: &str, now: f64) -> Result<Schedule, String> {
    let lower = text.trim().to_lowercase().replace(" at ", " ").replace('\u{a0}', " ");
    let words: Vec<&str> = lower.split_whitespace().collect();
    let fail = || Err(format!("Couldn't read the schedule \"{}\". Write it like {FORMS}.", text.trim()));
    let Some(first) = words.first().copied() else { return fail() };
    if first == "cron" || (words.len() == 5 && Cron::parse(&lower).is_ok()) {
        let expr = if first == "cron" { words[1..].join(" ") } else { words.join(" ") };
        Cron::parse(&expr)?;
        return Ok(Schedule::Cron { expr });
    }
    if first == "hourly" && words.len() == 1 {
        return Ok(Schedule::Every { minutes: 60, from: start(now, 60) });
    }
    let rest = &words[1..];
    match first {
        "in" => return span(&rest.join("")).map(|m| Schedule::Once { at: (now + m as f64 * 60.0).floor() }).ok_or(()).or_else(|_| fail()),
        "once" | "on" => return once(rest, now).ok_or(()).or_else(|_| fail()),
        "every" | "each" => {
            if let Some(m) = span(&rest.join("")).or_else(|| span(&format!("1{}", rest.join("")))) {
                let m = m.max(MIN_EVERY);
                return Ok(Schedule::Every { minutes: m, from: start(now, m) });
            }
            return daily(rest, now).ok_or(()).or_else(|_| fail());
        }
        _ => {}
    }
    let one_day = words.len() <= 2 && weekday(first).is_some() && !first.ends_with('s');
    if !one_day {
        if let Some(s) = daily(&words, now) {
            return Ok(s);
        }
    }
    once(&words, now).ok_or(()).or_else(|_| fail())
}

/// Where "every …" counts from: one period on, on the minute.
fn start(now: f64, minutes: u32) -> f64 {
    (now / 60.0).floor() * 60.0 + minutes as f64 * 60.0
}

/// "30m", "2h", "1d", "90min", "2hours", "1h30m": minutes.
fn span(text: &str) -> Option<u32> {
    let mut total = 0u64;
    let mut rest = text;
    if rest.is_empty() {
        return None;
    }
    while !rest.is_empty() {
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return None;
        }
        let n: u64 = rest[..digits].parse().ok()?;
        rest = &rest[digits..];
        let unit_len = rest.chars().take_while(|c| c.is_ascii_alphabetic()).count();
        let unit = &rest[..unit_len];
        rest = &rest[unit_len..];
        total += n * match unit {
            "m" | "min" | "mins" | "minute" | "minutes" => 1,
            "h" | "hr" | "hrs" | "hour" | "hours" => 60,
            "d" | "day" | "days" => 1440,
            "w" | "week" | "weeks" => 10080,
            _ => return None,
        };
    }
    (total > 0 && total <= u32::MAX as u64).then_some(total as u32)
}

/// "9", "9:30", "09:30", "9am", "9:30pm", "21:00", "noon", "midnight": (hour, minute).
fn time_of_day(text: &str) -> Option<(u32, u32)> {
    match text {
        "noon" => return Some((12, 0)),
        "midnight" => return Some((0, 0)),
        _ => {}
    }
    let (body, pm) = match (text.strip_suffix("am"), text.strip_suffix("pm")) {
        (Some(b), _) => (b, Some(false)),
        (_, Some(b)) => (b, Some(true)),
        _ => (text, None),
    };
    let (h, m) = match body.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None if !body.is_empty() && body.len() <= 2 => (body.parse::<u32>().ok()?, 0),
        None => return None,
    };
    let h = match pm {
        Some(true) if (1..=12).contains(&h) => h % 12 + 12,
        Some(false) if (1..=12).contains(&h) => h % 12,
        Some(_) => return None,
        None => h,
    };
    (h < 24 && m < 60).then_some((h, m))
}

fn weekday(word: &str) -> Option<u8> {
    let w = word.trim_end_matches('s');
    ["sun", "mon", "tue", "wed", "thu", "fri", "sat"].iter().position(|d| w.starts_with(d) && (w.len() == 3 || DAY_NAMES.iter().any(|n| n.to_lowercase().starts_with(w)))).map(|i| i as u8)
}

/// "daily 9:00", "day 9", "weekdays 8:30", "weekends", "mon,thu 18:00", "monday 9am",
/// "weekly fri 17:00": a time on some days. A missing time is 9:00.
fn daily(words: &[&str], _now: f64) -> Option<Schedule> {
    let (days_words, time) = match words.last().and_then(|w| time_of_day(w)) {
        Some(t) if words.len() > 1 => (&words[..words.len() - 1], t),
        _ => (words, (9, 0)),
    };
    let joined = days_words.iter().filter(|w| **w != "weekly" && **w != "every").copied().collect::<Vec<_>>().join(",");
    let weekdays: Vec<u8> = match joined.as_str() {
        "daily" | "day" | "days" | "everyday" | "" if !days_words.is_empty() => vec![],
        "weekdays" | "weekday" | "workdays" => vec![1, 2, 3, 4, 5],
        "weekends" | "weekend" => vec![0, 6],
        list => {
            let mut days = vec![];
            for part in list.split(',').filter(|p| !p.is_empty()) {
                if let Some((a, b)) = part.split_once('-') {
                    let (a, b) = (weekday(a)?, weekday(b)?);
                    let mut d = a;
                    loop {
                        days.push(d);
                        if d == b {
                            break;
                        }
                        d = (d + 1) % 7;
                    }
                } else {
                    days.push(weekday(part)?);
                }
            }
            if days.is_empty() {
                return None;
            }
            days.sort_unstable();
            days.dedup();
            days
        }
    };
    Some(Schedule::Daily { hour: time.0, minute: time.1, weekdays })
}

/// "tomorrow 9:00", "today 17:30", "2026-12-01 14:30", "2026-12-01T14:30", "dec 1 14:30",
/// "monday 9:00" (the next one), "14:30" (today, or tomorrow when it's passed).
fn once(words: &[&str], now: f64) -> Option<Schedule> {
    let (y, m, d, ..) = local(now);
    let joined = words.join(" ");
    let (date_part, time_part) = match joined.split_once('t') {
        Some((a, b)) if a.len() == 10 && a.as_bytes()[4] == b'-' => (a.to_string(), Some(b.split(':').take(2).collect::<Vec<_>>().join(":"))),
        _ => match words {
            [date, time] => (date.to_string(), Some(time.to_string())),
            [one] if time_of_day(one).is_some() => (String::new(), Some(one.to_string())),
            [one] => (one.to_string(), None),
            [mon, day, time] if MONTHS.iter().any(|n| mon.starts_with(&n.to_lowercase())) => (format!("{mon} {day}"), Some(time.to_string())),
            _ => return None,
        },
    };
    let (hh, mm) = match &time_part {
        Some(t) => time_of_day(t)?,
        None => (9, 0),
    };
    let at = match date_part.as_str() {
        "" => {
            let t = moment(y, m, d as i32, hh, mm);
            if t > now { t } else { moment(y, m, d as i32 + 1, hh, mm) }
        }
        "today" => moment(y, m, d as i32, hh, mm),
        "tomorrow" => moment(y, m, d as i32 + 1, hh, mm),
        iso if iso.len() == 10 && iso.as_bytes()[4] == b'-' && iso.as_bytes()[7] == b'-' => {
            let year: i32 = iso[..4].parse().ok()?;
            let month: u32 = iso[5..7].parse().ok()?;
            let day: i32 = iso[8..10].parse().ok()?;
            if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
                return None;
            }
            moment(year, month, day, hh, mm)
        }
        other => {
            if let Some(wd) = weekday(other) {
                (1..=7).map(|o| moment(y, m, d as i32 + o, hh, mm)).find(|t| local(*t).5 as u8 == wd)?
            } else {
                let (mon, day) = other.split_once(' ')?;
                let month = MONTHS.iter().position(|n| mon.starts_with(&n.to_lowercase()))? as u32 + 1;
                let day: i32 = day.trim_end_matches(|c: char| c.is_ascii_alphabetic()).parse().ok()?;
                let t = moment(y, month, day, hh, mm);
                if t > now { t } else { moment(y + 1, month, day, hh, mm) }
            }
        }
    };
    Some(Schedule::Once { at })
}

// ---------------------------------------------------------------------------------------------
// Cron

/// A parsed cron line: which minutes, hours, days, months and weekdays match.
#[derive(Debug)]
struct Cron {
    minutes: u64,
    hours: u32,
    days: u32,
    months: u16,
    weekdays: u8,
    any_day: bool,
    any_weekday: bool,
}

impl Cron {
    fn parse(expr: &str) -> Result<Cron, String> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(format!("A cron line has five fields (minute hour day month weekday), like \"0 9 * * 1-5\"; \"{expr}\" has {}.", fields.len()));
        }
        let bad = |what: &str, f: &str| format!("Couldn't read the cron {what} field \"{f}\".");
        let months = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
        let days = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
        let minutes = field(fields[0], 0, 59, &[], 0).ok_or_else(|| bad("minute", fields[0]))?;
        let hours = field(fields[1], 0, 23, &[], 0).ok_or_else(|| bad("hour", fields[1]))?;
        let dom = field(fields[2], 1, 31, &[], 0).ok_or_else(|| bad("day", fields[2]))?;
        let mon = field(fields[3], 1, 12, &months, 1).ok_or_else(|| bad("month", fields[3]))?;
        let mut dow = field(fields[4], 0, 7, &days, 0).ok_or_else(|| bad("weekday", fields[4]))?;
        if dow & (1 << 7) != 0 {
            dow = (dow & !(1 << 7)) | 1;
        }
        Ok(Cron { minutes: minutes as u64, hours: hours as u32, days: dom as u32, months: mon as u16, weekdays: dow as u8, any_day: fields[2] == "*", any_weekday: fields[4] == "*" })
    }

    fn day_matches(&self, day: u32, weekday: u32) -> bool {
        let d = self.days & (1 << day) != 0;
        let w = self.weekdays & (1 << weekday) != 0;
        match (self.any_day, self.any_weekday) {
            (true, true) => true,
            (true, false) => w,
            (false, true) => d,
            (false, false) => d || w,
        }
    }

    fn next_after(&self, after: f64) -> Option<f64> {
        let (y, m, d, ..) = local(after);
        for offset in 0..(366 * 5) {
            let midnight = moment(y, m, d as i32 + offset, 0, 0);
            let (yy, mm, dd, _, _, wd) = local(midnight);
            if self.months & (1 << mm) == 0 || !self.day_matches(dd, wd) {
                continue;
            }
            for hour in (0..24).filter(|h| self.hours & (1 << h) != 0) {
                for minute in (0..60).filter(|mi| self.minutes & (1u64 << mi) != 0) {
                    let t = moment(yy, mm, dd as i32, hour, minute);
                    if t > after && local(t).3 == hour {
                        return Some(t);
                    }
                }
            }
        }
        None
    }
}

/// One cron field as bits: "*", "5", "1-5", "*/15", "0-30/10", "mon-fri", lists of those.
fn field(text: &str, lo: u32, hi: u32, names: &[&str], name_base: u32) -> Option<u64> {
    let value = |s: &str| -> Option<u32> {
        if let Ok(n) = s.parse::<u32>() {
            return (lo..=hi).contains(&n).then_some(n);
        }
        names.iter().position(|n| s.eq_ignore_ascii_case(n)).map(|i| i as u32 + name_base)
    };
    let mut bits = 0u64;
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => (r, s.parse::<u32>().ok().filter(|s| *s > 0)?),
            None => (part, 1),
        };
        let (a, b) = match range {
            "*" => (lo, hi),
            r => match r.split_once('-') {
                Some((a, b)) => (value(a)?, value(b)?),
                None if step > 1 => (value(r)?, hi),
                None => (value(r)?, value(r)?),
            },
        };
        if a > b {
            return None;
        }
        let mut n = a;
        while n <= b {
            bits |= 1 << n;
            n += step;
        }
    }
    (bits != 0).then_some(bits)
}

// ---------------------------------------------------------------------------------------------
// Storage

/// Where the automations are kept: the library's settings, or a stand-in in tests.
pub trait Settings: Send + Sync {
    fn get(&self) -> Option<Value>;
    fn set(&self, value: &Value);
}

impl Settings for crate::library::Library {
    fn get(&self) -> Option<Value> {
        self.setting(SETTING)
    }
    fn set(&self, value: &Value) {
        self.set_setting(SETTING, value)
    }
}

/// One change at a time: the timer/watchers on the main thread and agents' tools on theirs.
static LOCK: Mutex<()> = Mutex::new(());

/// Every automation, in the order they were made.
pub fn all(store: &dyn Settings) -> Vec<Automation> {
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    split(store).0
}

fn split(store: &dyn Settings) -> (Vec<Automation>, Vec<Value>) {
    let mut known = vec![];
    let mut unknown = vec![];
    if let Some(Value::Array(items)) = store.get() {
        for item in items {
            match serde_json::from_value::<Automation>(item.clone()) {
                Ok(a) => known.push(a),
                Err(_) => unknown.push(item),
            }
        }
    }
    (known, unknown)
}

/// Changes the automations and saves them; what this version couldn't read is kept.
pub fn edit<R>(store: &dyn Settings, change: impl FnOnce(&mut Vec<Automation>) -> R) -> R {
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut known, unknown) = split(store);
    let before = known.clone();
    let result = change(&mut known);
    if known != before {
        let mut items: Vec<Value> = known.iter().filter_map(|a| serde_json::to_value(a).ok()).collect();
        items.extend(unknown);
        store.set(&Value::Array(items));
    }
    result
}

/// Finds an automation within a slice by exact ID, 6+ char ID prefix, slug, or case-insensitive name.
pub fn find_in<'a>(candidates: &[&'a Automation], key: &str, scope_desc: &str) -> Result<&'a Automation, String> {
    let key = key.trim();
    if let Some(a) = candidates.iter().find(|a| a.id == key) {
        return Ok(a);
    }
    let by_id: Vec<&&Automation> = candidates.iter().filter(|a| key.len() >= 6 && a.id.to_lowercase().starts_with(&key.to_lowercase())).collect();
    if by_id.len() == 1 {
        return Ok(by_id[0]);
    }
    let by_name: Vec<&&Automation> = candidates.iter().filter(|a| a.name.eq_ignore_ascii_case(key) || a.slug().eq_ignore_ascii_case(key)).collect();
    match by_name.len() {
        1 => Ok(by_name[0]),
        0 => Err(format!(
            "No automation \"{key}\" {scope_desc}.{}",
            if candidates.is_empty() {
                String::new()
            } else {
                format!(" There are: {}.", candidates.iter().map(|a| format!("{} ({})", a.name, a.short_id())).collect::<Vec<_>>().join(", "))
            }
        )),
        _ => Err(format!("More than one automation is called \"{key}\"; give its id.")),
    }
}

/// An automation of `project` by its id, prefix, slug, or name.
pub fn find<'a>(list: &'a [Automation], project: &str, key: &str) -> Result<&'a Automation, String> {
    let mine: Vec<&Automation> = list.iter().filter(|a| !a.is_global && a.project_id == project).collect();
    find_in(&mine, key, "in this project")
}

/// Finds an automation across all automations (for CLI `yh trigger` and Home tools).
pub fn find_any<'a>(list: &'a [Automation], key: &str) -> Result<&'a Automation, String> {
    let all: Vec<&Automation> = list.iter().collect();
    find_in(&all, key, "")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-11-21 (a Saturday) at 10:15 local.
    fn base() -> f64 {
        moment(2026, 11, 21, 10, 15)
    }

    #[test]
    fn every_counts_from_its_start() {
        let s = Schedule::Every { minutes: 30, from: base() };
        assert_eq!(s.next_after(base() - 1.0), Some(base()));
        assert_eq!(s.next_after(base()), Some(base() + 1800.0));
        assert_eq!(s.next_after(base() + 3601.0), Some(base() + 5400.0));
    }

    #[test]
    fn daily_picks_the_next_matching_day() {
        let s = Schedule::Daily { hour: 9, minute: 0, weekdays: vec![] };
        assert_eq!(s.next_after(base()), Some(moment(2026, 11, 22, 9, 0)));
        let s = Schedule::Daily { hour: 11, minute: 0, weekdays: vec![] };
        assert_eq!(s.next_after(base()), Some(moment(2026, 11, 21, 11, 0)));
        let s = Schedule::Daily { hour: 9, minute: 0, weekdays: vec![1, 2, 3, 4, 5] };
        assert_eq!(s.next_after(base()), Some(moment(2026, 11, 23, 9, 0)));
        let s = Schedule::Daily { hour: 8, minute: 0, weekdays: vec![] };
        assert_eq!(s.next_after(moment(2026, 11, 30, 9, 0)), Some(moment(2026, 12, 1, 8, 0)));
    }

    #[test]
    fn cron_lines() {
        let next = |e: &str, after: f64| Schedule::Cron { expr: e.into() }.next_after(after);
        assert_eq!(next("0 9 * * 1-5", base()), Some(moment(2026, 11, 23, 9, 0)));
        assert_eq!(next("*/15 * * * *", base()), Some(moment(2026, 11, 21, 10, 30)));
        assert_eq!(next("30 10 * * *", base()), Some(moment(2026, 11, 21, 10, 30)));
        assert_eq!(next("0 0 1 * *", base()), Some(moment(2026, 12, 1, 0, 0)));
        assert_eq!(next("0 12 * dec sun", base()), Some(moment(2026, 12, 6, 12, 0)));
        assert_eq!(next("0 0 30 2 *", base()), None);
        assert_eq!(next("0 9 1 * mon", base()), Some(moment(2026, 11, 23, 9, 0)));
        assert!(Cron::parse("61 * * * *").is_err());
        assert!(Cron::parse("* * *").is_err());
    }

    #[test]
    fn parses_what_people_write() {
        let now = base();
        assert_eq!(parse("in 30m", now), Ok(Schedule::Once { at: now + 1800.0 }));
        assert_eq!(parse("in 1h30m", now), Ok(Schedule::Once { at: now + 5400.0 }));
        assert_eq!(parse("tomorrow 9:00", now), Ok(Schedule::Once { at: moment(2026, 11, 22, 9, 0) }));
        assert_eq!(parse("tomorrow at 9am", now), Ok(Schedule::Once { at: moment(2026, 11, 22, 9, 0) }));
        assert_eq!(parse("2026-12-01 14:30", now), Ok(Schedule::Once { at: moment(2026, 12, 1, 14, 30) }));
        assert_eq!(parse("2026-12-01T14:30:00", now), Ok(Schedule::Once { at: moment(2026, 12, 1, 14, 30) }));
        assert_eq!(parse("once dec 3 8:00", now), Ok(Schedule::Once { at: moment(2026, 12, 3, 8, 0) }));
        assert_eq!(parse("9:00", now), Ok(Schedule::Once { at: moment(2026, 11, 22, 9, 0) }));
        assert_eq!(parse("monday 9:00", now), Ok(Schedule::Once { at: moment(2026, 11, 23, 9, 0) }));
        assert_eq!(parse("every 15m", now), Ok(Schedule::Every { minutes: 15, from: start(now, 15) }));
        assert_eq!(parse("every 2 hours", now), Ok(Schedule::Every { minutes: 120, from: start(now, 120) }));
        assert_eq!(parse("every hour", now), Ok(Schedule::Every { minutes: 60, from: start(now, 60) }));
        assert_eq!(parse("hourly", now), Ok(Schedule::Every { minutes: 60, from: start(now, 60) }));
        assert_eq!(parse("daily 9:00", now), Ok(Schedule::Daily { hour: 9, minute: 0, weekdays: vec![] }));
        assert_eq!(parse("every day at 18:30", now), Ok(Schedule::Daily { hour: 18, minute: 30, weekdays: vec![] }));
        assert_eq!(parse("weekdays 8:30", now), Ok(Schedule::Daily { hour: 8, minute: 30, weekdays: vec![1, 2, 3, 4, 5] }));
        assert_eq!(parse("weekends 10am", now), Ok(Schedule::Daily { hour: 10, minute: 0, weekdays: vec![0, 6] }));
        assert_eq!(parse("mon,thu 18:00", now), Ok(Schedule::Daily { hour: 18, minute: 0, weekdays: vec![1, 4] }));
        assert_eq!(parse("every friday at 5pm", now), Ok(Schedule::Daily { hour: 17, minute: 0, weekdays: vec![5] }));
        assert_eq!(parse("weekly mon 9:00", now), Ok(Schedule::Daily { hour: 9, minute: 0, weekdays: vec![1] }));
        assert_eq!(parse("cron 0 9 * * 1-5", now), Ok(Schedule::Cron { expr: "0 9 * * 1-5".into() }));
        assert_eq!(parse("0 9 * * 1-5", now), Ok(Schedule::Cron { expr: "0 9 * * 1-5".into() }));
        assert_eq!(parse("mondays 9:00", now), Ok(Schedule::Daily { hour: 9, minute: 0, weekdays: vec![1] }));
        assert!(parse("whenever", now).is_err());
        assert!(parse("every", now).is_err());
        assert!(parse("25:00", now).is_err());
    }

    #[test]
    fn says_it_back() {
        let now = base();
        assert_eq!(Schedule::Daily { hour: 9, minute: 0, weekdays: vec![1, 2, 3, 4, 5] }.describe(now), "Weekdays at 9:00");
        assert_eq!(Schedule::Daily { hour: 9, minute: 5, weekdays: vec![5] }.describe(now), "Fridays at 9:05");
        assert_eq!(Schedule::Daily { hour: 9, minute: 0, weekdays: vec![1, 3] }.describe(now), "Mon, Wed at 9:00");
        assert_eq!(Schedule::Every { minutes: 120, from: 0.0 }.describe(now), "Every 2 hours");
        assert_eq!(Schedule::Once { at: moment(2026, 11, 22, 9, 0) }.describe(now), "Once, tomorrow at 9:00");
        assert_eq!(when(moment(2026, 11, 24, 9, 0), now), "Tue, Nov 24 at 9:00");
    }

    #[test]
    fn runs_move_the_schedule_on() {
        let now = base();
        let mut a = Automation::new("P", "Check", "Look", Schedule::Every { minutes: 60, from: now + 60.0 }, now).unwrap();
        assert!(!a.due(now));
        assert!(a.due(now + 60.0));
        let late = now + 86_400.0;
        a.ran(late, Some("T".into()), None, false);
        assert!(a.runs[0].caught_up);
        assert!(a.next_run.unwrap() > late && !a.due(late));
        assert_eq!(a.last_thread_id.as_deref(), Some("T"));
        let next = a.next_run;
        a.ran(late + 5.0, Some("T2".into()), None, true);
        assert_eq!(a.next_run, next);
        let mut once = Automation::new("P", "Once", "Look", Schedule::Once { at: now + 60.0 }, now).unwrap();
        once.ran(now + 61.0, None, None, false);
        assert!(!once.enabled && once.next_run.is_none() && once.is_done());
        assert!(Automation::new("P", "Past", "Look", Schedule::Once { at: now - 60.0 }, now).is_err());
        let mut same = Automation::new("P", "Log", "Look", Schedule::Every { minutes: 5, from: now }, now).unwrap();
        same.set_thread_policy(ThreadPolicy::Same);
        same.ran(now + 1.0, Some("T9".into()), None, false);
        assert_eq!(same.thread_id.as_deref(), Some("T9"));
    }

    #[test]
    fn v1_json_deserializes_cleanly() {
        let v1 = serde_json::json!({
            "id": "auto-1",
            "projectId": "proj-1",
            "name": "Morning PRs",
            "prompt": "Check PRs",
            "schedule": { "kind": "daily", "hour": 9, "minute": 0, "weekdays": [1,2,3,4,5] },
            "sameThread": true,
            "threadId": "t-1",
            "enabled": true,
            "createdAt": 1700000000.0,
            "nextRun": 1700003600.0,
            "runs": [{ "at": 1700000100.0, "threadId": "t-1", "caughtUp": false, "manual": false }]
        });
        let a: Automation = serde_json::from_value(v1).unwrap();
        assert_eq!(a.scope(), Scope::Project { project_id: "proj-1".into() });
        assert_eq!(a.thread_policy(), ThreadPolicy::Same);
        assert!(matches!(a.effective_trigger(), Trigger::Schedule { .. }));
        assert_eq!(a.runs[0].effective_status(), RunStatus::Done);
    }

    #[test]
    fn global_scope_and_event_triggers_and_guardrails() {
        let now = base();
        let mut a = Automation::new_with_trigger(
            Scope::Global,
            "Morning Briefing",
            "Prep day",
            Trigger::System(SystemTrigger { event: SystemEventKind::MacWake, app: None, power: None }),
            now,
        )
        .unwrap();
        assert!(a.scope().is_global());
        assert!(!a.due(now));
        a.set_thread_policy(ThreadPolicy::NewSettlePrevious);
        assert_eq!(a.thread_policy(), ThreadPolicy::NewSettlePrevious);
        a.guardrails.cooldown_secs = 600;
        assert!(a.cooldown_remaining(now).is_none());
        a.record_run(now, Some("T1".into()), None, false, "Mac woke".into(), Some(RunStatus::Running), None);
        assert!(a.cooldown_remaining(now + 100.0).is_some());
        assert!(a.cooldown_remaining(now + 601.0).is_none());
        assert!(a.finish_run("T1", RunStatus::Done, now + 45.0));
        assert_eq!(a.runs[0].duration_secs, Some(45.0));
        assert_eq!(a.runs[0].effective_status(), RunStatus::Done);
    }

    #[test]
    fn glob_and_prompt_rendering() {
        assert!(glob_matches("*.pdf", "Downloads/invoice.PDF"));
        assert!(glob_matches("docs/**/*.md", "docs/a/b/readme.md"));
        assert!(glob_matches("docs/**/*.md", "docs/readme.md"));
        assert!(!glob_matches("docs/*.md", "docs/a/readme.md"));
        assert!(glob_matches("*.rs, *.toml", "Cargo.toml"));
        assert!(is_ignored_path("target/debug/app", None));
        assert!(!is_ignored_path("src/main.rs", None));

        let ev = TriggerEvent {
            reason: "File changed".into(),
            kind: "fileWatch".into(),
            fields: vec![("folder".into(), "~/Downloads".into())],
            files: vec!["~/Downloads/a.pdf".into()],
        };
        let rendered = render_prompt("Summarize it.", Some(&ev), base());
        assert!(rendered.contains("<automation_event"));
        assert!(rendered.contains("a.pdf"));

        let custom = render_prompt("Read {{event.files}} in {{event.folder}}", Some(&ev), base());
        assert!(!custom.contains("<automation_event"));
        assert_eq!(custom, "Read ~/Downloads/a.pdf in ~/Downloads");
    }

    #[test]
    fn parses_trigger_shorthands() {
        let now = base();
        assert!(matches!(parse_trigger("wake", now), Ok(Trigger::System(SystemTrigger { event: SystemEventKind::MacWake, .. }))));
        assert!(matches!(parse_trigger("app_quit:Zoom", now), Ok(Trigger::System(SystemTrigger { event: SystemEventKind::AppQuit, app: Some(a), .. })) if a == "Zoom"));
        assert!(matches!(parse_trigger("watch:~/Downloads|*.pdf", now), Ok(Trigger::FileWatch(FileWatchTrigger { path, glob: Some(g), .. })) if path == "~/Downloads" && g == "*.pdf"));
        assert!(matches!(parse_trigger("pr_failed", now), Ok(Trigger::Repo(RepoTrigger { event: RepoEventKind::PrChecksFailed, .. }))));
        assert!(matches!(parse_trigger("manual", now), Ok(Trigger::Manual)));
        assert!(matches!(parse_trigger("weekdays 9:00", now), Ok(Trigger::Schedule { .. })));
    }

    struct Fake(Mutex<Option<Value>>);
    impl Settings for Fake {
        fn get(&self) -> Option<Value> {
            self.0.lock().unwrap().clone()
        }
        fn set(&self, value: &Value) {
            *self.0.lock().unwrap() = Some(value.clone());
        }
    }

    #[test]
    fn storage_keeps_unknown_entries() {
        let store = Fake(Mutex::new(Some(serde_json::json!([{ "id": "future", "weird": 1 }]))));
        let a = Automation::new("P", "One", "Hi", Schedule::Daily { hour: 9, minute: 0, weekdays: vec![] }, base()).unwrap();
        edit(&store, |list| list.push(a.clone()));
        assert_eq!(all(&store).len(), 1);
        assert_eq!(store.get().unwrap().as_array().unwrap().len(), 2);
    }
}
