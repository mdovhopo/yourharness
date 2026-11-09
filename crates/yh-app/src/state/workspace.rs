//! What the app knows and does, with no drawing: projects, threads, which one is open, each
//! thread's `claude`, and every request the UI makes (send, stop, allow, pin, rename…). Views
//! read it and call it; it tells them what changed with `WorkspaceEvent`s and `notify`.

use crate::probe;
use crate::state::thread::ThreadState;
use crate::state::thread_tools::{self, ToolRequest};
use gpui_kit::{App, AppContext, Context, Entity, EventEmitter, Task, WeakEntity};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use yh_core::claude::{Decision, Event, LaunchOptions, ModelInfo, SlashCommand};
use yh_engine::{Command, Delivery, Output, ThreadUpdate, TurnEnd};
use yh_core::library::{cli_mode, control_mode, data_directory, iso_now, Library, ProjectMeta, ThreadMeta};

pub const PAGE: usize = 160;

pub enum WorkspaceEvent {
    /// A thread was opened.
    Selected(String),
    /// A thread went away.
    Removed(String),
    /// A page (Memory) took the place of the thread.
    PageOpened,
    /// `/mcp` was typed: the MCP sheet opens for the open thread.
    OpenMcp,
    /// Text goes at the end of the open thread's composer (an element picked in the browser).
    AppendToComposer(String),
    /// Pictures attached to the open thread's composer (an element picked in the browser): JPEG
    /// or PNG bytes.
    AttachImages(Vec<Vec<u8>>),
}

/// The sidebar's order within a project: pinned first, then by when it was made, newest first,
/// so rows stay where you left them (sorted by last activity, every turn moved them).
pub fn sidebar_order(a: &ThreadMeta, b: &ThreadMeta) -> std::cmp::Ordering {
    b.pinned.cmp(&a.pinned).then(b.created_at.total_cmp(&a.created_at))
}

/// What can stand in place of the open thread.
#[derive(Clone, PartialEq, Debug)]
pub enum Page {
    /// A project's memory, by its id.
    Memory(String),
    /// A project's pull requests and repos, by its id.
    Project(String),
    Usage,
}

/// The right sidebar's tabs.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Panel {
    PullRequests,
    SideChat,
    Subagents,
    Files,
    Browser,
}

impl Panel {
    pub const ALL: [Panel; 5] = [Panel::PullRequests, Panel::SideChat, Panel::Subagents, Panel::Files, Panel::Browser];

    pub fn label(self) -> &'static str {
        match self {
            Panel::PullRequests => "PRs",
            Panel::SideChat => "Side Chat",
            Panel::Subagents => "Sub-agents",
            Panel::Files => "Files",
            Panel::Browser => "Browser",
        }
    }

    pub fn from_name(name: &str) -> Option<Panel> {
        match name {
            "pullRequests" | "prs" => Some(Panel::PullRequests),
            "sideChat" | "side-chat" => Some(Panel::SideChat),
            "subagents" => Some(Panel::Subagents),
            "files" => Some(Panel::Files),
            "browser" => Some(Panel::Browser),
            _ => None,
        }
    }
}

pub use yh_engine::ids::{MEMORY_AGENT, SETTINGS, SIDE};

/// What a side chat's agent is told.
const SIDE_CHAT_PROMPT: &str = "This is a side conversation. You are a copy of the conversation above, opened so the user can ask something on the side without interrupting or adding to the main thread; nothing said here reaches it. Answer the question directly and briefly. You have Read, Glob, Grep, Bash, WebFetch and WebSearch: use them when the answer needs a fresh look rather than what the conversation already says. The app asks the user to approve commands that could change something. You have no tools to edit files: if the answer needs changes, say what the main thread should do.";

/// The side chat's view of its thread's browser.
const SIDE_BROWSER_PROMPT: &str = "You can look at the thread's browser, built into YourHarness, with the mcp__browser__ tools: status, snapshot, screenshot, console and network. You can't click or type there; that's for the main thread.";

/// The side chat's view of the workspace's memory.
const SIDE_MEMORY_PROMPT: &str = "This workspace has a memory kept by YourHarness: mcp__memory__search finds what earlier sessions learned and mcp__memory__read reads it. You can't change it from here.";

/// When `gh stack` is installed: how GitHub's stacked PRs are made here.
const STACKS_PROMPT: &str = "The user works with GitHub's stacked pull requests through the `gh stack` extension. For a stack (or stacked PRs): `gh stack init --base <trunk> [branches…]` starts one or adopts existing branches, bottom first; `gh stack add <branch>` adds a branch on top (run it on the top branch, then commit there); `gh stack submit --auto --open` pushes and opens or updates the PRs; `gh stack sync` rebases and pushes after changes below; `gh stack view --json` shows the stack. Pass flags so nothing prompts. Don't merge a stack unless asked: YourHarness shows the stack with its merge buttons.";

/// A thread action that Undo takes back.
pub enum Undo {
    Delete { entity: Entity<ThreadState>, record: serde_json::Map<String, Value>, index: usize, was_selected: bool },
    Unpin(String),
    /// Pinning: ⌘Z unpins (no notice).
    Pin(String),
    /// Settling: back as it was, its pin and the selection too.
    Settle { id: String, pinned: bool, was_selected: bool },
    /// Bringing back from the shelf: ⌘Z settles it again (no notice).
    Unsettle(String),
}

pub struct Workspace {
    pub library: Arc<Library>,
    pub data_dir: PathBuf,
    pub projects: Vec<ProjectMeta>,
    pub threads: Vec<Entity<ThreadState>>,
    by_id: HashMap<String, Entity<ThreadState>>,
    pub selected: Option<String>,
    /// Every thread's run and its `claude`, on a thread of its own (`yh_engine`).
    engine: yh_engine::EngineHandle,
    /// Threads the engine was told of (`Command::Open`).
    opened: std::collections::HashSet<String>,
    /// Each thread's `claude` process id, while it has one, as the engine says.
    pids: HashMap<String, u32>,
    /// The CLI's models, as it last listed them (saved as `claudeModels`).
    pub models: Vec<ModelInfo>,
    /// Usage limits as Claude last reported them.
    pub limits: Option<Value>,
    /// When they were (seconds), a refresh asked of Claude under way, and why the last failed
    /// (`state/limits.rs`).
    pub limits_updated_at: Option<f64>,
    pub limits_refreshing: bool,
    pub limits_error: Option<String>,
    /// Each project's slash commands and skills, as the CLI last listed them (`slashCommands`).
    commands: HashMap<String, Vec<SlashCommand>>,
    /// A folder's files for `@` mentions, kept 30 seconds.
    files: HashMap<String, (Instant, Arc<Vec<String>>)>,
    new_count: usize,
    /// Where agents' thread tools ask the main thread (`thread_tools`).
    tool_requests: flume::Sender<ToolRequest>,
    /// Threads whose title is being rewritten (`update_title`), and how many messages the user
    /// had sent when it was last asked for.
    titles_rewriting: std::collections::HashSet<String>,
    /// Keeps the Mac awake while Claude works (Settings), or as asked from the sidebar.
    pub keep_awake: crate::keep_awake::KeepAwake,
    keep_awake_timer: Option<Task<()>>,
    /// The last thread action that can be undone, and whether its notice still shows.
    undo: Option<(String, Undo)>,
    undo_shown: bool,
    /// When the last undoable thread action was done: ⌘Z in the composer undoes it rather than
    /// the typing when it came after the last key typed (`undo_beats_typing`).
    pub undo_at: Option<Instant>,
    undo_serial: u64,
    /// Deleted threads whose rows go when the app quits (kept until then for Undo).
    pending_purge: Vec<String>,
    /// Folders and files an importer may read, by its id.
    memory_agent_sources: HashMap<String, Vec<String>>,
    /// Each thread's side chat (`/btw`), made when first opened; never saved.
    side_chats: HashMap<String, Entity<ThreadState>>,
    /// The right sidebar: open, and on which tab.
    pub shows_inspector: bool,
    pub panel: Panel,
    /// A page open in place of the thread: a project's Memory, or Usage.
    pub page: Option<Page>,
    /// Goes up whenever memory changes (an agent's tools, the Memory page), for views of it.
    pub memory_revision: u64,
    /// Each project's repos and worktrees with their status, read aside when one of its threads
    /// opens: which checkouts a thread works in, for the toolbar.
    pub scans: HashMap<String, yh_core::workspace::Workspace>,
    scanning: std::collections::HashSet<String>,
    scanned_at: HashMap<String, Instant>,
    title_asked_at: HashMap<String, usize>,
    /// The composer's: `/rename` waiting for the CLI's new title, skills by project.
    composer: ComposerJobs,
    /// Each thread's PRs its tool calls worked on, by how many rows it had then: for settling
    /// threads whose PRs merged (`settle_merged_threads`).
    merge_links: Arc<std::sync::Mutex<HashMap<String, (usize, Vec<yh_core::forge::Link>)>>>,
    /// Why the last generated title failed, until one works.
    pub title_error: Option<String>,
    /// Whether `gh stack` is installed, once looked (aside, at launch): its instructions then
    /// go to every thread's agent.
    stacks_installed: Option<bool>,
    /// Projects whose commands were read with a throwaway `claude` this launch.
    command_loads: std::collections::HashSet<String>,
    /// Where each thread's working folder stood when its turn began, for what git sees change
    /// in the turn (`turn_changes`), by thread.
    turn_baselines: HashMap<String, TurnBaseline>,
    /// Each thread's PRs as its rows link them, read aside: (worked on, all), for the
    /// toolbar's button.
    pub pull_counts: HashMap<String, (usize, usize)>,
}

/// A turn's start, for the files it changed.
struct TurnBaseline {
    /// The message that started it.
    message: String,
    folder: String,
    /// When (ms since 1970).
    started: f64,
    /// Another thread was working in an overlapping folder then.
    shared: bool,
    /// The folder's snapshot, taken aside.
    snapshot: flume::Receiver<Option<yh_core::turn_changes::Snapshot>>,
}

/// Whether two folders are the same, or one is inside the other.
fn overlaps(a: &str, b: &str) -> bool {
    a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
}

fn real_path(folder: &str) -> String {
    std::fs::canonicalize(folder).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| folder.to_string())
}

impl EventEmitter<WorkspaceEvent> for Workspace {}

impl Workspace {
    pub fn new(library: Arc<Library>, cx: &mut Context<Self>) -> Self {
        let weak = cx.entity().downgrade();
        let threads: Vec<Entity<ThreadState>> =
            library.threads.iter().map(|meta| make_thread(meta.clone(), weak.clone(), cx)).collect();
        let by_id = threads.iter().map(|t| (t.read(cx).id().to_string(), t.clone())).collect();
        let models = library
            .setting("claudeModels")
            .and_then(|v| v.as_array().map(|a| a.iter().filter_map(ModelInfo::from_json).collect()))
            .unwrap_or_default();
        let (tool_requests, requests) = flume::unbounded();
        cx.spawn(async move |this, cx| {
            while let Ok(request) = requests.recv_async().await {
                if this.update(cx, |ws, cx| ws.handle_tool_request(request, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let engine = yh_engine::start(library.db.clone());
        // What the engine says, at most once a frame.
        let outputs = engine.outputs.clone();
        cx.spawn(async move |this, cx| {
            while let Ok(first) = outputs.recv_async().await {
                let mut all = vec![first];
                all.extend(outputs.try_iter());
                if this.update(cx, |ws, cx| ws.on_outputs(all, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(Duration::from_millis(8)).await;
            }
        })
        .detach();
        // The server for the app's clients (the phone): off unless asked for.
        if let Some(remote) = start_server(&engine) {
            cx.spawn(async move |this, cx| {
                while let Ok(action) = remote.recv_async().await {
                    if this.update(cx, |ws, cx| ws.remote_action(action, cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        let mut workspace = Workspace {
            projects: library.projects.clone(),
            data_dir: data_directory(),
            library,
            threads,
            by_id,
            selected: None,
            engine,
            opened: Default::default(),
            pids: HashMap::new(),
            models,
            limits: None,
            limits_updated_at: None,
            limits_refreshing: false,
            limits_error: None,
            commands: HashMap::new(),
            files: HashMap::new(),
            new_count: 0,
            tool_requests,
            titles_rewriting: Default::default(),
            page: None,
            side_chats: HashMap::new(),
            memory_agent_sources: HashMap::new(),
            keep_awake: crate::keep_awake::KeepAwake::new(crate::keep_awake::Settings::from(None)),
            keep_awake_timer: None,
            undo: None,
            undo_shown: false,
            undo_at: None,
            undo_serial: 0,
            pending_purge: vec![],
            shows_inspector: false,
            panel: Panel::SideChat,
            memory_revision: 0,
            scans: HashMap::new(),
            scanning: Default::default(),
            scanned_at: HashMap::new(),
            title_asked_at: HashMap::new(),
            composer: ComposerJobs::default(),
            merge_links: Default::default(),
            title_error: None,
            stacks_installed: None,
            command_loads: Default::default(),
            turn_baselines: HashMap::new(),
            pull_counts: HashMap::new(),
        };
        // Is `gh stack` there? Looked aside, so the first thread's agent knows how stacks are made.
        cx.spawn(async move |this, cx| {
            let installed = cx
                .background_executor()
                .spawn(async move {
                    let path = std::env::var("YH_PATH").ok();
                    yh_core::git::which("gh", path.as_deref()).is_some() && yh_core::github::GitHubStacks::new(path.as_deref()).is_installed()
                })
                .await;
            let _ = this.update(cx, |ws, _| ws.stacks_installed = Some(installed));
        })
        .detach();
        if let Some(Value::Object(all)) = workspace.library.setting("slashCommands") {
            for (project, list) in all {
                let list = list.as_array().map(|a| a.iter().filter_map(SlashCommand::from_json).collect()).unwrap_or_default();
                workspace.commands.insert(project, list);
            }
        }
        workspace.shows_inspector = workspace.library.setting("showsInspector").and_then(|v| v.as_bool()).unwrap_or(false);
        if let Some(panel) = std::env::var("YH_INSPECTOR_PANEL").ok().and_then(|p| Panel::from_name(&p)) {
            workspace.panel = panel;
            workspace.shows_inspector = true;
        }
        workspace.keep_awake.configure(crate::keep_awake::Settings::from(workspace.library.setting("keepAwake")));
        let chosen = workspace.library.setting("cliPathOverrides").and_then(|o| o["claude"].as_str().map(std::path::PathBuf::from));
        yh_core::claude::set_chosen_claude(chosen);
        // Threads whose PRs merged settle: 20 s after launch, then every five minutes (not in
        // perf runs, which measure the transcript).
        if std::env::var("YH_PERF").is_err() {
            let first = std::env::var("YH_SETTLE_MERGED_AFTER").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(20.);
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs_f64(first)).await;
                loop {
                    if this.update(cx, |ws, cx| ws.settle_merged_threads(cx)).is_err() {
                        break;
                    }
                    cx.background_executor().timer(Duration::from_secs(300)).await;
                }
            })
            .detach();
        }
        // Idle threads settle on their own: now, and every hour.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(5)).await;
            if this.update(cx, |ws, cx| ws.settle_idle_threads(cx)).is_err() {
                break;
            }
            cx.background_executor().timer(Duration::from_secs(3600)).await;
        })
        .detach();
        workspace.ensure_home(cx);
        workspace.recover_cut_off_turns(cx);
        workspace.load_saved_limits();
        workspace.listen_for_notification_clicks(cx);
        workspace
    }

    /// A data folder without a Home project gets one: your home
    /// folder, or in a development run (`YH_DATA_DIR`) a `home` folder inside the data folder, so a
    /// test never runs `claude` in your real home.
    fn ensure_home(&mut self, cx: &mut Context<Self>) {
        if self.library.db.is_none() || self.home_project().is_some_and(|id| self.project(&id).is_some()) {
            return;
        }
        let path = if std::env::var("YH_DATA_DIR").is_ok() {
            let home = self.data_dir.join("home");
            let _ = std::fs::create_dir_all(&home);
            home.to_string_lossy().into_owned()
        } else {
            std::env::var("HOME").unwrap_or_default()
        };
        if path.is_empty() {
            return;
        }
        let id = self.add_project(&path, cx);
        self.library.update_project(&id, |p| {
            p.insert("name".into(), "Home".into());
        });
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == id) {
            p.name = "Home".into();
        }
        self.library.set_setting("homeProjectID", &Value::from(id));
    }

    pub fn thread(&self, id: &str) -> Option<Entity<ThreadState>> {
        self.by_id.get(id).cloned()
    }

    /// The thread's `claude` process, while it has one: what it started is under it (the dev
    /// servers its browser offers).
    pub fn claude_pid(&self, id: &str) -> Option<u32> {
        self.pids.get(id).copied()
    }

    pub fn selected_thread(&self) -> Option<Entity<ThreadState>> {
        self.selected.as_deref().and_then(|id| self.thread(id))
    }

    pub fn project(&self, id: &str) -> Option<&ProjectMeta> {
        self.projects.iter().find(|p| p.id == id)
    }

    /// The Home project's ID, shown first and named Home.
    pub fn home_project(&self) -> Option<String> {
        self.library.setting("homeProjectID").and_then(|v| v.as_str().map(str::to_string))
    }

    /// Settings → General: Home, where threads outside a project run, moves to another folder.
    /// Its threads keep their conversations; their next message starts there.
    /// Where Home starts and Reset puts it: your home folder, or in a development run a `home`
    /// folder inside the data folder.
    pub fn default_home_folder(&self) -> String {
        if std::env::var("YH_DATA_DIR").is_ok() {
            let home = self.data_dir.join("home");
            let _ = std::fs::create_dir_all(&home);
            return home.to_string_lossy().into_owned();
        }
        std::env::var("HOME").unwrap_or_default()
    }

    pub fn set_home_folder(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(id) = self.home_project() else { return };
        // Named for its folder; "Home" for your home folder.
        let name = if Some(path) == std::env::var("HOME").ok().as_deref() {
            "Home".to_string()
        } else {
            std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Home".into())
        };
        self.library.update_project(&id, |p| {
            p.insert("path".into(), path.into());
            p.insert("name".into(), name.clone().into());
        });
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == id) {
            p.path = path.to_string();
            p.name = name;
        }
        let ids: Vec<String> = self.threads.iter().filter(|t| t.read(cx).meta.project_id == id).map(|t| t.read(cx).id().to_string()).collect();
        for thread in ids {
            if !self.thread(&thread).is_some_and(|t| t.read(cx).run.running) {
                self.end_runtime(&thread, cx);
            }
        }
        cx.notify();
    }

    /// Settings → General → Sleep changed.
    pub fn configure_keep_awake(&mut self, settings: crate::keep_awake::Settings, cx: &mut Context<Self>) {
        self.library.set_setting("keepAwake", &settings.to_json());
        self.keep_awake.configure(settings);
        self.schedule_keep_awake(cx);
    }

    /// Settings → Agents: the `claude` to run (None: the one found). Running threads keep theirs.
    pub fn choose_claude(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        let mut overrides = self.library.setting("cliPathOverrides").filter(Value::is_object).unwrap_or(serde_json::json!({}));
        match &path {
            Some(p) => overrides["claude"] = Value::from(p.clone()),
            None => {
                overrides.as_object_mut().unwrap().remove("claude");
            }
        }
        self.library.set_setting("cliPathOverrides", &overrides);
        yh_core::claude::set_chosen_claude(path.map(Into::into));
        cx.notify();
    }

    pub fn running_count(&self, cx: &App) -> usize {
        self.threads.iter().filter(|t| t.read(cx).run.running).count()
    }

    /// The default model or effort from Settings (`YH_MODEL`/`YH_EFFORT` in test runs).
    fn default_setting(&self, key: &str) -> Option<String> {
        std::env::var(format!("YH_{}", key.to_uppercase()))
            .ok()
            .or_else(|| self.library.setting("defaults").and_then(|d| d[key].as_str().map(str::to_string)))
            .filter(|s| !s.is_empty())
    }

    /// The thread's model: its own, else the default.
    pub fn model_of(&self, thread: &ThreadState) -> Option<String> {
        thread.meta.model.clone().or_else(|| self.default_setting("model"))
    }

    /// Whether the thread's model can run in Auto (unknown models are given the benefit of the
    /// doubt).
    pub fn supports_auto_mode(&self, thread: &ThreadState) -> bool {
        let model = self.model_of(thread).unwrap_or_else(|| "default".into());
        self.models.iter().find(|m| m.value == model).is_none_or(|m| m.supports_auto_mode)
    }

    /// The model's name as the catalog has it ("Haiku"), else as set.
    /// "Default" when none is chosen (the catalog's "Default (recommended)" is long for a chip).
    pub fn model_name(&self, thread: &ThreadState) -> String {
        match self.model_of(thread).filter(|m| m != "default") {
            Some(model) => self.models.iter().find(|m| m.value == model).map(|m| m.display_name.clone()).unwrap_or(model),
            None => "Default".into(),
        }
    }

    pub fn effort_of(&self, thread: &ThreadState) -> Option<String> {
        thread.meta.effort.clone().or_else(|| self.default_setting("effort"))
    }

    // MARK: Opening and reading

    /// The click is answered in the next frame (selection, title, a held transcript); reading a
    /// history that isn't held follows, off the main thread.
    pub fn select(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(thread) = self.thread(id) else { return };
        // Choosing a thread closes a page.
        if self.page.take().is_some() {
            cx.notify();
        }
        if self.selected.as_deref() == Some(id) {
            return;
        }
        if let Some(old) = self.selected_thread() {
            // A draft stays with its thread; the composer saves it on the way out.
            old.update(cx, |t, _| t.unread = false);
        }
        self.selected = Some(id.to_string());
        // Its notifications have done their job.
        crate::platform::clear_notifications(id);
        // Opened again at the next launch, unless it's a thread that isn't kept.
        if self.library.thread_record(id).is_some() {
            self.library.set_setting("selectedThreadID", &Value::from(id));
        }
        thread.update(cx, |t, cx| {
            if t.unread {
                t.unread = false;
                cx.notify();
            }
        });
        self.ensure_loaded(&thread, cx);
        let project = thread.read(cx).meta.project_id.clone();
        self.scan_project(&project, cx);
        self.refresh_mcp_attention(&thread, cx);
        cx.emit(WorkspaceEvent::Selected(id.to_string()));
        cx.notify();
    }

    pub fn ensure_loaded(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let (id, needed) = {
            let t = thread.read(cx);
            (t.id().to_string(), !t.loaded && !t.loading)
        };
        if !needed {
            return;
        }
        if id.starts_with("new:") {
            thread.update(cx, |t, _| t.loaded = true);
            return;
        }
        // Rows held from a live turn are newer than the history: read from before them.
        let before = thread.update(cx, |t, _| {
            t.loading = true;
            t.first_seq()
        });
        let library = self.library.clone();
        let weak = thread.downgrade();
        cx.spawn(async move |_, cx| {
            let mut page = cx.background_executor().spawn(async move { library.page(&id, before, PAGE) }).await;
            let _ = weak.update(cx, |t, cx| {
                if !t.loading {
                    return; // let go of meanwhile
                }
                // A message sent while this was read is held already and may be saved by now:
                // its saved copy, at the end of the page, isn't added again.
                let held: std::collections::HashSet<String> = t.items.iter().flat_map(|i| i.entries.iter().map(|e| e.borrow().data.id.clone())).collect();
                let mut dropped = 0;
                while page.entries.last().is_some_and(|e| held.contains(&e.id)) {
                    page.entries.pop();
                    dropped += 1;
                }
                if t.run.total.is_none() {
                    t.run.total = Some(page.total.saturating_sub(dropped));
                }
                t.prepend(page.entries, page.from);
                t.loaded = true;
                t.loading = false;
                cx.notify();
            });
        })
        .detach();
    }

    pub fn load_earlier(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let (id, before) = {
            let t = thread.read(cx);
            if !t.loaded || t.loading_earlier || t.before == 0 {
                return;
            }
            (t.id().to_string(), t.before)
        };
        thread.update(cx, |t, _| t.loading_earlier = true);
        let library = self.library.clone();
        let weak = thread.downgrade();
        cx.spawn(async move |_, cx| {
            let page = cx.background_executor().spawn(async move { library.page(&id, Some(before), PAGE) }).await;
            let _ = weak.update(cx, |t, cx| {
                t.loading_earlier = false;
                if t.loaded && t.before == before {
                    t.prepend(page.entries, page.from);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Every sub-agent call of the thread for the Sub-agents panel, held or not: the saved
    /// ones summarized, read once, aside.
    pub fn ensure_subagent_runs(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        if !thread.read(cx).needs_saved_runs() {
            return;
        }
        // Held runs list as they are, and let go of ones as they were let go of (`trim`): what's
        // read only adds the others.
        let id = thread.update(cx, |t, _| {
            t.mark_saved_runs_read();
            t.id().to_string()
        });
        let library = self.library.clone();
        let weak = thread.downgrade();
        cx.spawn(async move |_, cx| {
            let t0 = std::time::Instant::now();
            let calls = cx.background_executor().spawn(async move { library.subagent_calls(&id) }).await;
            if std::env::var("YH_TIMING").as_deref() == Ok("1") {
                eprintln!("subagent calls: {} in {:.1} ms", calls.len(), t0.elapsed().as_secs_f64() * 1000.0);
            }
            let _ = weak.update(cx, |t, cx| {
                t.set_saved_runs(calls);
                cx.notify();
            });
        })
        .detach();
    }

    /// A run the panel shows that's listed from its summary: its row read whole, aside.
    pub fn ensure_run_detail(&mut self, thread: &Entity<ThreadState>, run: &str, cx: &mut Context<Self>) {
        if !thread.read(cx).wants_run_detail(run) {
            return;
        }
        let id = thread.update(cx, |t, _| {
            t.mark_run_detail(run);
            t.id().to_string()
        });
        let (library, run) = (self.library.clone(), run.to_string());
        let weak = thread.downgrade();
        cx.spawn(async move |_, cx| {
            let call = run.clone();
            let row = cx.background_executor().spawn(async move { library.entry(&id, &call) }).await;
            let _ = weak.update(cx, |t, cx| {
                t.set_run_detail(&run, row);
                cx.notify();
            });
        })
        .detach();
    }

    /// The transcript was let go of on screen: save its rows and keep only what a running turn
    /// needs.
    pub fn release(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        self.save_rows(thread, cx);
        thread.update(cx, |t, cx| {
            t.in_deck = false;
            if t.run.running {
                t.trim(200);
            } else {
                t.unload();
            }
            cx.notify();
        });
    }

    // MARK: Threads and projects

    pub fn new_thread(&mut self, project_id: &str, cx: &mut Context<Self>) -> String {
        // An untouched blank in the project is used again instead of piling up blanks.
        let blank = self.threads.iter().find(|t| {
            let t = t.read(cx);
            t.meta.project_id == project_id && t.meta.settled_at.is_none() && t.run.session_id.is_none() && t.title == "New thread" && !t.run.running && t.items.is_empty() && t.run.total.unwrap_or(0) == 0
        });
        if let Some(blank) = blank {
            let id = blank.read(cx).id().to_string();
            self.select(&id, cx);
            return id;
        }
        self.new_count += 1;
        // Saved in harness.sqlite right away; `new:` only without one.
        let meta = if self.library.db.is_some() && !project_id.starts_with("dir:") {
            self.library.add_thread(project_id, "New thread")
        } else {
            ThreadMeta::new(format!("new:{}", self.new_count), project_id.into(), "New thread".into())
        };
        let thread = make_thread(meta, cx.entity().downgrade(), cx);
        let id = thread.read(cx).id().to_string();
        thread.update(cx, |t, _| t.loaded = true);
        self.threads.insert(0, thread.clone());
        self.by_id.insert(id.clone(), thread);
        self.select(&id, cx);
        id
    }

    pub fn add_project(&mut self, path: &str, cx: &mut Context<Self>) -> String {
        let project = self.library.add_project(path);
        let id = project.id.clone();
        if self.project(&id).is_none() {
            self.projects.push(project);
        }
        cx.notify();
        id
    }

    /// The sidebar's live threads in its order, folded projects skipped: Home first, then the
    /// projects as listed; in each, pinned first, then newest made first (`sidebar_order`).
    pub fn visible_thread_ids(&self, cx: &App) -> Vec<String> {
        let home = self.home_project();
        let collapsed: std::collections::HashSet<String> = self
            .library
            .setting("collapsedProjectIDs")
            .and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()))
            .unwrap_or_default();
        let mut projects: Vec<&ProjectMeta> = self.projects.iter().filter(|p| !collapsed.contains(&p.id)).collect();
        projects.sort_by_key(|p| Some(&p.id) != home.as_ref());
        let mut out = vec![];
        for p in projects {
            let mut live: Vec<&ThreadMeta> = self.threads.iter().map(|t| &t.read(cx).meta).filter(|m| m.project_id == p.id && m.settled_at.is_none()).collect();
            live.sort_by(|a, b| sidebar_order(a, b));
            out.extend(live.into_iter().map(|m| m.id.clone()));
        }
        out
    }

    /// Before the open thread goes (settled, deleted), the one below it opens, or above at the
    /// end, so the window doesn't go blank.
    fn select_neighbour(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.selected.as_deref() != Some(id) || self.page.is_some() {
            return;
        }
        let ids = self.visible_thread_ids(cx);
        let next = match ids.iter().position(|t| t == id) {
            Some(i) if i + 1 < ids.len() => Some(ids[i + 1].clone()),
            Some(i) if i > 0 => Some(ids[i - 1].clone()),
            Some(_) => None,
            None => ids.first().cloned(),
        };
        match next {
            Some(next) => self.select(&next, cx),
            None => self.selected = None,
        }
    }

    fn update_meta(&mut self, id: &str, cx: &mut Context<Self>, change: impl FnOnce(&mut ThreadMeta, &mut serde_json::Map<String, Value>)) {
        let Some(thread) = self.thread(id) else { return };
        let mut record_change: Option<serde_json::Map<String, Value>> = None;
        thread.update(cx, |t, cx| {
            let mut patch = serde_json::Map::new();
            change(&mut t.meta, &mut patch);
            t.title = t.meta.title.clone();
            record_change = Some(patch);
            cx.notify();
        });
        if let Some(patch) = record_change {
            self.library.update_thread(id, |record| {
                for (k, v) in patch {
                    if v.is_null() {
                        record.remove(&k);
                    } else {
                        record.insert(k, v);
                    }
                }
            });
        }
        cx.notify();
    }

    pub fn rename(&mut self, id: &str, title: &str, cx: &mut Context<Self>) {
        let title = title.trim().to_string();
        if title.is_empty() {
            return;
        }
        self.update_meta(id, cx, |m, r| {
            m.title = title.clone();
            m.title_chosen = true;
            r.insert("title".into(), title.into());
            r.insert("titleChosen".into(), true.into());
        });
    }

    pub fn set_pinned(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        self.update_meta(id, cx, |m, r| {
            m.pinned = on;
            r.insert("isPinned".into(), if on { true.into() } else { Value::Null });
        });
    }

    /// On its project's Settled shelf, its `claude` ended, history kept; or brought back.
    pub fn set_settled(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        if on {
            // Already on the shelf: nothing to do (and the selection stays).
            if self.thread(id).is_some_and(|t| t.read(cx).meta.settled_at.is_some()) {
                return;
            }
            self.select_neighbour(id, cx);
            self.end_runtime(id, cx);
            // Its pages go with its `claude`.
            crate::browser::Browsers::close_thread(id);
        }
        let now = iso_now();
        self.update_meta(id, cx, |m, r| {
            if on {
                m.settled_at = Some(yh_core::library::now_ms());
                // Settling drops the pin.
                m.pinned = false;
                r.insert("isPinned".into(), Value::Null);
                r.insert("settledAt".into(), now.into());
            } else {
                m.settled_at = None;
                m.updated_at = yh_core::library::now_ms();
                r.insert("settledAt".into(), Value::Null);
                r.insert("reactivatedAt".into(), now.into());
            }
        });
    }

    /// Deletes a thread from the sidebar, with Undo for a few seconds (and ⌘Z): its record goes
    /// now, its rows when the app quits, so Undo brings it back whole. Without the store (a
    /// thread of no project), at once.
    pub fn delete(&mut self, id: &str, cx: &mut Context<Self>) {
        // Its run is read again from what's saved if Undo brings it back.
        self.forget_in_engine(id);
        crate::browser::Browsers::close_thread(id);
        let was_selected = self.selected.as_deref() == Some(id);
        if was_selected {
            // To the neighbour first, while the thread is still listed.
            self.select_neighbour(id, cx);
            if self.selected.as_deref() == Some(id) {
                self.selected = None;
            }
        }
        let index = self.threads.iter().position(|t| t.read(cx).id() == id);
        let entity = index.map(|i| self.threads.remove(i));
        self.by_id.remove(id);
        let record = self.library.take_thread(id);
        match (entity, record) {
            (Some(entity), Some(record)) => {
                let title = entity.read(cx).title.clone();
                self.pending_purge.push(id.to_string());
                self.offer_undo(format!("Deleted “{title}”"), Undo::Delete { entity, record, index: index.unwrap_or(0), was_selected }, cx);
            }
            _ => self.library.purge(id),
        }
        cx.emit(WorkspaceEvent::Removed(id.to_string()));
        cx.notify();
    }

    /// Removes a project and its threads (not Home); its folder is left alone.
    pub fn remove_project(&mut self, project: &str, cx: &mut Context<Self>) {
        if self.home_project().as_deref() == Some(project) {
            return;
        }
        let ids: Vec<String> = self.threads.iter().filter(|t| t.read(cx).meta.project_id == project).map(|t| t.read(cx).id().to_string()).collect();
        for id in ids {
            self.delete(&id, cx);
        }
        // No Undo for those: the project is going too.
        self.undo = None;
        self.projects.retain(|p| p.id != project);
        self.library.remove_project(project);
        if matches!(&self.page, Some(Page::Project(p)) | Some(Page::Memory(p)) if p == project) {
            self.page = None;
        }
        cx.notify();
    }

    /// Puts an action on Undo (⌘Z, Edit → Undo) without the notice.
    fn register_undo(&mut self, action: Undo, cx: &mut Context<Self>) {
        self.undo_serial += 1;
        self.undo = Some((String::new(), action));
        self.undo_at = Some(Instant::now());
        self.undo_shown = false;
        cx.notify();
    }

    /// Whether there's a thread action to undo (Edit → Undo is on for it).
    pub fn can_undo(&self) -> bool {
        self.undo.is_some()
    }

    fn offer_undo(&mut self, message: String, action: Undo, cx: &mut Context<Self>) {
        self.undo_serial += 1;
        let serial = self.undo_serial;
        self.undo = Some((message, action));
        self.undo_at = Some(Instant::now());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(5)).await;
            let _ = this.update(cx, |ws, cx| {
                if ws.undo_serial == serial && ws.undo.is_some() {
                    // The notice goes; ⌘Z still undoes until something else is done.
                    ws.undo_shown = false;
                    cx.notify();
                }
            });
        })
        .detach();
        self.undo_shown = true;
        cx.notify();
    }

    /// The Undo notice's text while it shows.
    pub fn undo_notice(&self) -> Option<&str> {
        self.undo.as_ref().filter(|_| self.undo_shown).map(|(m, _)| m.as_str())
    }

    /// Undoes the last delete, unpin or settle.
    pub fn undo(&mut self, cx: &mut Context<Self>) {
        let Some((_, action)) = self.undo.take() else { return };
        self.undo_shown = false;
        match action {
            Undo::Delete { entity, record, index, was_selected } => {
                let id = entity.read(cx).id().to_string();
                self.pending_purge.retain(|p| *p != id);
                self.library.restore_thread(record);
                self.threads.insert(index.min(self.threads.len()), entity.clone());
                self.by_id.insert(id.clone(), entity);
                if was_selected {
                    self.select(&id, cx);
                }
            }
            Undo::Unpin(id) => self.set_pinned(&id, true, cx),
            Undo::Pin(id) => self.set_pinned(&id, false, cx),
            Undo::Settle { id, pinned, was_selected } => {
                // Back as it was: not "brought back" (that holds off settling on its own).
                self.update_meta(&id, cx, |m, r| {
                    m.settled_at = None;
                    m.pinned = pinned;
                    r.insert("settledAt".into(), Value::Null);
                    r.insert("isPinned".into(), if pinned { true.into() } else { Value::Null });
                });
                if was_selected {
                    self.select(&id, cx);
                }
            }
            Undo::Unsettle(id) => self.set_settled(&id, true, cx),
        }
        if std::env::var("YH_DATA_DIR").is_ok() {
            eprintln!("UNDO done");
        }
        cx.notify();
    }

    /// The thread's title written again from its conversation (the sidebar's Regenerate Title).
    pub fn regenerate_title(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(db) = self.library.db.clone() else { return };
        let id = id.to_string();
        // A title asked for by hand replaces a chosen one.
        self.update_meta(&id, cx, |m, r| {
            m.title_chosen = false;
            r.remove("titleChosen");
        });
        self.title_asked_at.remove(&id);
        cx.spawn(async move |this, cx| {
            let i = id.clone();
            let messages: Vec<(bool, String)> = cx
                .background_executor()
                .spawn(async move {
                    let total = db.transcript_count(&i);
                    db.read_rows(&i, 0, total)
                        .into_iter()
                        .filter_map(yh_core::library::EntryData::from_row)
                        .filter_map(|e| match e.kind {
                            yh_core::library::Kind::User => Some((true, e.text)),
                            yh_core::library::Kind::Assistant => Some((false, e.text)),
                            _ => None,
                        })
                        .collect()
                })
                .await;
            let _ = this.update(cx, |ws, cx| {
                let sent = messages.iter().filter(|m| m.0).count();
                ws.update_title_for_agent(&id, messages, sent, None, cx);
            });
        })
        .detach();
    }

    /// A first title that was a guess, written again from the conversation (the rows held: it's
    /// the end of the thread's first turn). One you chose stays.
    fn refine_title(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let (id, messages) = {
            let t = thread.read(cx);
            let messages: Vec<(bool, String)> = t
                .items
                .iter()
                .flat_map(|i| i.entries.iter())
                .filter_map(|e| {
                    let b = e.borrow();
                    match b.data.kind {
                        yh_core::library::Kind::User => Some((true, b.data.text.clone())),
                        yh_core::library::Kind::Assistant => Some((false, b.data.text.clone())),
                        _ => None,
                    }
                })
                .collect();
            (t.id().to_string(), messages)
        };
        thread.update(cx, |t, _| t.title_needs_refinement = false);
        let sent = messages.iter().filter(|m| m.0).count();
        self.update_title_for_agent(&id, messages, sent, None, cx);
    }

    /// How many PRs the thread worked on, of those its rows link: read aside (as the PRs tab
    /// reads them, without looking each up), for the toolbar's count.
    pub fn count_pulls(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let Some(db) = self.library.db.clone() else { return };
        let id = thread.read(cx).id().to_string();
        if id.starts_with(SIDE) || id.starts_with(MEMORY_AGENT) || id.starts_with(SETTINGS) {
            return;
        }
        let folder = self.folder_of(thread.read(cx));
        let path = std::env::var("YH_PATH").ok();
        // The links its record keeps (as the PRs tab reads them), so a merged branch PR counts.
        let strings = |key: &str| -> Vec<String> {
            self.library.thread_field(&id, key).and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())).unwrap_or_default()
        };
        let saved = (strings("workedPullRequests"), strings("pullRequests"));
        cx.spawn(async move |this, cx| {
            let key = id.clone();
            let links = cx.background_executor().spawn(async move { crate::state::pulls::links(&db, &id, &folder, path.as_deref(), &[], (&saved.0, &saved.1)) }).await;
            let counts = (links.iter().filter(|(_, worked)| *worked).count(), links.len());
            let _ = this.update(cx, |ws, cx| {
                if ws.pull_counts.insert(key, counts) != Some(counts) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Other threads working in a folder overlapping `folder` now, or active there since `since`.
    fn others_working_in(&self, id: &str, folder: &str, since: Option<f64>, cx: &App) -> bool {
        self.threads.iter().any(|t| {
            let t = t.read(cx);
            // The cheap checks first: a folder is looked up only for threads that worked lately.
            t.id() != id && (t.run.running || since.is_some_and(|s| t.meta.updated_at > s)) && overlaps(&real_path(&self.folder_of(t)), folder)
        })
    }

    /// A turn begins: its folder as it stands, read aside while Claude reads the message.
    fn begin_turn_changes(&mut self, thread: &Entity<ThreadState>, message: &str, cx: &mut Context<Self>) {
        let id = thread.read(cx).id().to_string();
        if id.starts_with(SIDE) || id.starts_with(MEMORY_AGENT) || id.starts_with(SETTINGS) {
            return;
        }
        let folder = real_path(&self.folder_of(thread.read(cx)));
        let shared = self.others_working_in(&id, &folder, None, cx);
        let (tx, rx) = flume::bounded(1);
        let (f, path) = (folder.clone(), std::env::var("YH_PATH").ok());
        cx.background_executor()
            .spawn(async move {
                let _ = tx.send(yh_core::turn_changes::snapshot(std::path::Path::new(&f), path.as_deref()));
            })
            .detach();
        self.turn_baselines.insert(id, TurnBaseline { message: message.to_string(), folder, started: yh_core::library::now_ms(), shared, snapshot: rx });
    }

    /// The turn is over: the files git saw change since it began go on its message. With another
    /// thread working in the same folder meanwhile, only those this turn's tool calls name.
    fn finish_turn_changes(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let id = thread.read(cx).id().to_string();
        let Some(base) = self.turn_baselines.remove(&id) else { return };
        let shared = base.shared || self.others_working_in(&id, &base.folder, Some(base.started), cx);
        let named = shared.then(|| thread.read(cx).turn_start_and_inputs().map(|(_, inputs)| inputs).unwrap_or_default());
        let weak = thread.downgrade();
        cx.spawn(async move |this, cx| {
            let Ok(Some(start)) = base.snapshot.recv_async().await else { return };
            let path = std::env::var("YH_PATH").ok();
            let changed = cx
                .background_executor()
                .spawn(async move {
                    let changed = yh_core::turn_changes::changes(&start, path.as_deref())?;
                    Some(match named {
                        Some(named) => yh_core::turn_changes::named_only(changed, &start.top, &named),
                        None => changed,
                    })
                })
                .await;
            let Some(changed) = changed else { return };
            if std::env::var("YH_DATA_DIR").is_ok() {
                eprintln!("TURN changed {} files (shared: {shared}): {changed:?}", changed.len());
            }
            let _ = this.update(cx, |ws, cx| {
                let Some(thread) = weak.upgrade() else { return };
                let id = thread.read(cx).id().to_string();
                ws.command(&id, Command::ChangedFiles { thread: id.clone(), message: base.message.clone(), files: changed }, cx);
            });
        })
        .detach();
    }

    /// Keep Awake follows whether Claude works anywhere; its one timer is for a deadline.
    pub fn note_working(&mut self, cx: &mut Context<Self>) {
        let busy = |t: &ThreadState| t.run.running || t.run.jobs.agents_running() > 0;
        let working = self.threads.iter().any(|t| busy(t.read(cx))) || self.by_id.values().any(|t| busy(t.read(cx)));
        self.keep_awake.set_working(working);
        // App Nap held off while Claude works (test runs hold it off for good, `hold_off_app_nap`).
        crate::platform::set_working_activity(working);
        if std::env::var("YH_KEEP_AWAKE_LOG").is_ok() {
            eprintln!("Keep awake: {}", self.keep_awake.describe());
        }
        self.schedule_keep_awake(cx);
    }

    pub fn schedule_keep_awake(&mut self, cx: &mut Context<Self>) {
        self.keep_awake_timer = self.keep_awake.deadline().map(|at| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(at.saturating_duration_since(std::time::Instant::now()) + Duration::from_millis(500)).await;
                let _ = this.update(cx, |ws, cx| {
                    ws.keep_awake.update();
                    ws.schedule_keep_awake(cx);
                    cx.notify();
                });
            })
        });
        cx.notify();
    }

    /// A tool call's folders join its thread's footprint (saved as 0.0.x saved it).
    fn note_footprint(&mut self, thread: &Entity<ThreadState>, tool: &str, input: &str, cx: &mut Context<Self>) {
        let cwd = self.folder_of(thread.read(cx));
        let touched = yh_core::footprint::folders(tool, input, &cwd);
        let written = yh_core::footprint::written_folders(tool, input, &cwd);
        let (mut footprint, mut written_footprint) = {
            let m = &thread.read(cx).meta;
            (m.footprint.clone(), m.written_footprint.clone())
        };
        let before = (footprint.len(), written_footprint.len());
        for f in touched {
            if !footprint.contains(&f) {
                footprint.push(f);
            }
        }
        for f in written {
            if !written_footprint.contains(&f) {
                written_footprint.push(f);
            }
        }
        if before == (footprint.len(), written_footprint.len()) {
            return;
        }
        let id = thread.read(cx).id().to_string();
        self.library.update_thread(&id, |r| {
            r.insert("footprint".into(), Value::from(footprint.clone()));
            r.insert("writtenFootprint".into(), Value::from(written_footprint.clone()));
        });
        thread.update(cx, |t, cx| {
            t.meta.footprint = footprint;
            t.meta.written_footprint = written_footprint;
            cx.notify();
        });
        // The header's checkouts follow it.
        cx.notify();
    }

    /// Reads a project's repos, worktrees and their status again, aside (no pull requests).
    pub fn scan_project(&mut self, project: &str, cx: &mut Context<Self>) {
        // Home is your home folder: not a workspace to look through. And once every 30 s at most.
        let recent = self.scanned_at.get(project).is_some_and(|t| t.elapsed() < Duration::from_secs(30));
        if recent || self.home_project().as_deref() == Some(project) {
            return;
        }
        let Some(root) = self.project(project).map(|p| std::path::PathBuf::from(&p.path)) else { return };
        if !root.is_dir() || !self.scanning.insert(project.to_string()) {
            return;
        }
        self.scanned_at.insert(project.to_string(), Instant::now());
        let mut copy = self.scans.get(project).cloned().unwrap_or_else(|| yh_core::workspace::Workspace::new(&root));
        let project = project.to_string();
        cx.spawn(async move |this, cx| {
            let done = cx
                .background_executor()
                .spawn(async move {
                    let tools = yh_core::workspace::WorkspaceTools::find(std::env::var("YH_PATH").ok().as_deref());
                    copy.scan(&tools);
                    copy.refresh_status(&tools, Duration::ZERO);
                    // Open PRs too, every two minutes at most: the sidebar's review badges.
                    copy.refresh_pulls(&tools, Duration::from_secs(120));
                    copy
                })
                .await;
            if std::env::var("YH_DATA_DIR").is_ok() {
                eprintln!("SCAN {} repos, worktrees {:?}", done.cloned_repos().len(), done.all_worktrees().iter().map(|w| w.path.clone()).collect::<Vec<_>>());
            }
            let _ = this.update(cx, |ws, cx| {
                ws.scanning.remove(&project);
                ws.scans.insert(project, done);
                cx.notify();
            });
        })
        .detach();
    }

    /// The checkouts a thread works in, as its project was last read.
    pub fn checkouts_of(&self, thread: &ThreadState) -> Vec<yh_core::workspace::Checkout> {
        let Some(scan) = self.scans.get(&thread.meta.project_id) else { return vec![] };
        let folders = yh_core::workspace::ThreadFolders {
            id: thread.id(),
            footprint: &thread.meta.footprint,
            written: &thread.meta.written_footprint,
            working_directory: thread.meta.working_directory.as_deref(),
            last_active: thread.meta.updated_at,
        };
        scan.checkouts_for(&folders)
    }

    /// Runs a thread somewhere else (a worktree), or in its project's folder again with None.
    /// A Claude Code session belongs to its folder, so the next message starts a new one.
    pub fn set_working_directory(&mut self, id: &str, folder: Option<String>, cx: &mut Context<Self>) {
        self.end_runtime(id, cx);
        self.update_meta(id, cx, |m, r| {
            m.working_directory = folder.clone();
            m.session_id = None;
            r.insert("workingDirectory".into(), folder.clone().map_or(Value::Null, Value::from));
            r.insert("sessionID".into(), Value::Null);
        });
        if let Some(t) = self.thread(id) {
            t.update(cx, |t, cx| {
                t.run.session_id = None;
                cx.notify();
            });
        }
        self.command(id, Command::SetSession { thread: id.to_string(), session_id: None }, cx);
        cx.notify();
    }

    /// Your rules for generated titles (`titlePrompt`, 0.0.x's key; `titleRules` before).
    pub fn title_prompt(&self) -> Option<String> {
        ["titlePrompt", "titleRules"].iter().find_map(|k| self.library.setting(k).and_then(|r| r.as_str().map(str::to_string))).filter(|r| !r.trim().is_empty())
    }

    /// Settles threads idle for `autoSettleDays` (3 by default; 0 never):
    /// never while working or waiting on you, never the one on screen, never one opted out
    /// or brought back since its last message, never an untouched blank.
    pub fn settle_idle_threads(&mut self, cx: &mut Context<Self>) {
        let days = self.library.setting("autoSettleDays").and_then(|v| v.as_i64()).unwrap_or(3);
        if days <= 0 {
            return;
        }
        let cutoff = yh_core::library::now_ms() - days as f64 * 86_400_000.;
        let candidates: Vec<String> = self
            .threads
            .iter()
            .filter_map(|t| {
                let t = t.read(cx);
                let id = t.id().to_string();
                let record = self.library.thread_record(&id).unwrap_or_default();
                let ok = t.meta.settled_at.is_none()
                    && !t.run.running
                    && !t.waiting_for_you()
                    && self.selected.as_deref() != Some(id.as_str())
                    && record.get("autoSettle").and_then(|v| v.as_bool()) != Some(false)
                    && record.get("reactivatedAt").is_none_or(|v| v.is_null())
                    && (t.run.session_id.is_some() || t.title != "New thread")
                    && t.meta.updated_at < cutoff;
                ok.then_some(id)
            })
            .collect();
        if std::env::var("YH_DATA_DIR").is_ok() && !candidates.is_empty() {
            eprintln!("Settled {} idle threads", candidates.len());
        }
        for id in candidates {
            self.set_pinned(&id, false, cx);
            self.set_settled(&id, true, cx);
        }
    }

    /// Settles threads whose PRs have all merged, with nothing new since (the work landed), a
    /// little after launch and every five minutes: the same exceptions as
    /// idle settling, and a PR closed without merging, or still open, keeps it. Each thread's PR
    /// links are read from its rows aside, again only when it has new rows (`merge_links`).
    pub fn settle_merged_threads(&mut self, cx: &mut Context<Self>) {
        let Some(db) = self.library.db.clone() else { return };
        let candidates: Vec<(String, String, f64, Vec<String>)> = self
            .threads
            .iter()
            .filter_map(|t| {
                let t = t.read(cx);
                let id = t.id().to_string();
                let record = self.library.thread_record(&id)?;
                let ok = t.meta.settled_at.is_none()
                    && !t.run.running
                    && !t.waiting_for_you()
                    && self.selected.as_deref() != Some(id.as_str())
                    && record.get("autoSettle").and_then(|v| v.as_bool()) != Some(false)
                    && record.get("reactivatedAt").is_none_or(|v| v.is_null());
                // The PRs its record keeps as worked on (the PRs tab saves them), merged ones too.
                let saved: Vec<String> = record.get("workedPullRequests").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
                ok.then(|| (id, self.folder_of(t), t.meta.updated_at, saved))
            })
            .collect();
        if candidates.is_empty() {
            return;
        }
        let cache = self.merge_links.clone();
        let path = std::env::var("YH_PATH").ok();
        cx.spawn(async move |this, cx| {
            let settle: Vec<String> = cx
                .background_executor()
                .spawn(async move {
                    let mut out = vec![];
                    for (id, folder, updated, saved) in candidates {
                        let count = db.transcript_count(&id);
                        let cached = cache.lock().unwrap().get(&id).filter(|(n, _)| *n == count).map(|(_, l)| l.clone());
                        let links = cached.unwrap_or_else(|| {
                            let links: Vec<yh_core::forge::Link> = crate::state::pulls::links(&db, &id, &folder, path.as_deref(), &[], (&saved, &[])).into_iter().filter(|(_, worked)| *worked).map(|(l, _)| l).collect();
                            cache.lock().unwrap().insert(id.clone(), (count, links.clone()));
                            links
                        });
                        if links.is_empty() {
                            continue;
                        }
                        let pulls = crate::state::pulls::look_up(links.into_iter().map(|l| (l, true)).collect(), path.clone(), &[]);
                        let merges: Vec<f64> = pulls
                            .iter()
                            .filter_map(|p| p.pull.as_ref().ok().filter(|p| p.state == "merged").map(|p| p.merged_at.as_deref().and_then(yh_core::library::parse_iso).map_or(yh_core::library::now_ms(), |t| t * 1000.)))
                            .collect();
                        // Work after the last merge (beyond the turn that merged it) is new work.
                        let last = merges.iter().cloned().fold(f64::MIN, f64::max);
                        if merges.len() == pulls.len() && updated <= last + 15. * 60_000. {
                            out.push(id);
                        }
                    }
                    out
                })
                .await;
            let _ = this.update(cx, |ws, cx| {
                for id in settle {
                    // Still as it was when looked at.
                    let still = ws.thread(&id).is_some_and(|t| t.read(cx).meta.settled_at.is_none() && !t.read(cx).run.running) && ws.selected.as_deref() != Some(id.as_str());
                    if still {
                        if std::env::var("YH_DATA_DIR").is_ok() {
                            eprintln!("SETTLED merged {id}");
                        }
                        ws.set_settled(&id, true, cx);
                    }
                }
            });
        })
        .detach();
    }

    /// Unpins with Undo, or pins (from the sidebar).
    pub fn toggle_pin(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(t) = self.thread(id) else { return };
        let (pinned, title) = (t.read(cx).meta.pinned, t.read(cx).title.clone());
        self.set_pinned(id, !pinned, cx);
        if pinned {
            self.offer_undo(format!("Unpinned “{title}”"), Undo::Unpin(id.to_string()), cx);
        } else {
            self.register_undo(Undo::Pin(id.to_string()), cx);
        }
    }

    /// Settles with Undo, or brings back (from the sidebar).
    pub fn toggle_settled(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(t) = self.thread(id) else { return };
        let (settled, title, pinned) = (t.read(cx).meta.settled_at.is_some(), t.read(cx).title.clone(), t.read(cx).meta.pinned);
        let was_selected = self.selected.as_deref() == Some(id);
        self.set_settled(id, !settled, cx);
        if !settled {
            self.offer_undo(format!("Settled “{title}”"), Undo::Settle { id: id.to_string(), pinned, was_selected }, cx);
        } else {
            self.register_undo(Undo::Unsettle(id.to_string()), cx);
        }
    }

    /// Whether idle settling may move the thread to the shelf (`autoSettle`, on unless off).
    pub fn auto_settles(&self, id: &str) -> bool {
        self.library.thread_record(id).and_then(|r| r.get("autoSettle").and_then(|v| v.as_bool())) != Some(false)
    }

    pub fn set_auto_settle(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        self.library.update_thread(id, |r| {
            if on {
                r.remove("autoSettle");
            } else {
                r.insert("autoSettle".into(), Value::Bool(false));
            }
        });
        cx.notify();
    }

    /// A title is being written for it (the first message's, or a rewrite).
    pub fn is_generating_title(&self, id: &str) -> bool {
        self.titles_rewriting.contains(id)
    }

    /// The composer's text, kept with the thread until sent.
    pub fn save_draft(&mut self, id: &str, text: &str, cx: &mut Context<Self>) {
        let Some(thread) = self.thread(id) else { return };
        let draft = (!text.trim().is_empty()).then(|| text.to_string());
        if thread.read(cx).meta.draft == draft {
            return;
        }
        thread.update(cx, |t, _| t.meta.draft = draft.clone());
        self.library.update_thread(id, |r| match draft {
            Some(d) => {
                r.insert("draft".into(), d.into());
            }
            None => {
                r.remove("draft");
            }
        });
    }

    pub fn set_model(&mut self, id: &str, model: Option<String>, cx: &mut Context<Self>) {
        let value = model.clone();
        self.update_meta(id, cx, |m, r| {
            m.model = value.clone();
            r.insert("model".into(), value.map(Value::from).unwrap_or(Value::Null));
        });
        self.command(id, Command::SetModel { thread: id.to_string(), model: model.unwrap_or_default() }, cx);
    }

    /// Applies from the next start of its `claude` (effort is a launch option).
    pub fn set_effort(&mut self, id: &str, effort: Option<String>, cx: &mut Context<Self>) {
        self.update_meta(id, cx, |m, r| {
            m.effort = effort.clone();
            r.insert("effort".into(), effort.map(Value::from).unwrap_or(Value::Null));
        });
        // A turn under way (or jobs it left running) is
        // never ended for it, its `claude` starts over once it's over; an idle one ends now, the
        // next message starting it with the new effort.
        let Some(thread) = self.thread(id) else { return };
        let effort = self.effort_of(thread.read(cx));
        self.command(id, Command::SetEffort { thread: id.to_string(), effort }, cx);
    }

    /// The project's permission mode; running threads switch at once.
    pub fn set_mode(&mut self, project_id: &str, mode: &str, cx: &mut Context<Self>) {
        self.library.update_project(project_id, |p| {
            p.insert("permissionMode".into(), mode.into());
        });
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == project_id) {
            p.mode = mode.to_string();
        }
        let ids: Vec<String> =
            self.threads.iter().filter(|t| t.read(cx).meta.project_id == project_id).map(|t| t.read(cx).id().to_string()).collect();
        for id in ids {
            self.command(&id, Command::SetMode { thread: id.clone(), mode: control_mode(mode).to_string() }, cx);
        }
        cx.notify();
    }

    // MARK: Turns

    /// Sends your message. While Claude works it waits for the current step (steering), or
    /// with `now` stops the turn for it.
    pub fn send(&mut self, thread: &Entity<ThreadState>, text: &str, now: bool, cx: &mut Context<Self>) {
        self.send_with_images(thread, text, vec![], now, cx)
    }

    /// With images already saved in `attachments/` (by name).
    pub fn send_with_images(&mut self, thread: &Entity<ThreadState>, text: &str, images: Vec<String>, now: bool, cx: &mut Context<Self>) {
        self.send_message(thread, text, images, vec![], now, cx)
    }

    /// With images (saved in `attachments/`, by name) and other files or folders, which Claude
    /// is told the paths of. Commands the app answers itself (`/btw`, `/model`…) stop here.
    pub fn send_message(
        &mut self,
        thread: &Entity<ThreadState>,
        text: &str,
        images: Vec<String>,
        files: Vec<yh_core::attachments::FileAttachment>,
        now: bool,
        cx: &mut Context<Self>,
    ) {
        let mut text = text.trim().to_string();
        if text.is_empty() && images.is_empty() && files.is_empty() {
            return;
        }
        if images.is_empty() && files.is_empty() {
            // Typed as the menu shows an MCP prompt, "/github:review", which Claude Code doesn't take.
            let project = thread.read(cx).meta.project_id.clone();
            text = yh_core::claude::translate_mcp_prompt(&text, self.commands(&project));
            match self.handle_command(thread, &text, cx) {
                CommandHandling::Done => return,
                CommandHandling::Send(rest) => text = rest,
                CommandHandling::NotOurs => {}
            }
        }
        let (id, running, title, chosen) = {
            let t = thread.read(cx);
            (t.id().to_string(), t.run.running, t.title.clone(), t.meta.title_chosen)
        };
        // Its first message: no rows yet, held or saved.
        let first_message = match thread.read(cx).run.total {
            Some(total) => total == 0,
            None => self.library.db.as_ref().map_or(0, |db| db.transcript_count(&id)) == 0,
        };
        let message_id = yh_core::library::new_uuid();
        // Kept with the message as 0.0.x kept them: images by name, then files.
        let file_names: Vec<String> = files.iter().map(|f| f.stored_name(&id)).collect();
        let attachments: Vec<String> = images.iter().chain(&file_names).cloned().collect();
        // The title is written from this: it should know what came with it.
        let attached = (!files.is_empty()).then(|| format!("Attached {}", files.iter().map(|f| f.name()).collect::<Vec<_>>().join(", ")));
        let subject = match (&attached, text.is_empty()) {
            (Some(a), false) => format!("{text} ({a})"),
            (Some(a), true) => a.clone(),
            (None, false) => text.clone(),
            (None, true) => if images.len() == 1 { "Attached image".into() } else { "Attached images".into() },
        };
        thread.update(cx, |t, cx| {
            // Working from now: the engine says so too, a moment later.
            t.run.running = true;
            t.meta.turn_in_progress = true;
            t.meta.draft = None;
            if title == "New thread" && !chosen {
                // Named after its first message until a title is generated or chosen.
                t.title = subject.lines().next().unwrap_or("").trim().chars().take(60).collect();
                t.meta.title = t.title.clone();
            }
            cx.notify();
        });
        let new_title = thread.read(cx).title.clone();
        self.library.update_thread(&id, |r| {
            r.remove("draft");
            r.remove("settledAt");
            r.insert("title".into(), new_title.into());
            r.insert("turnInProgress".into(), true.into());
            r.insert("updatedAt".into(), iso_now().into());
        });
        self.note_working(cx);
        if first_message && title == "New thread" && !chosen && !subject.is_empty() {
            self.generate_title(thread, &subject, cx);
        }
        // A new turn: where its folder stands, for the files it changes.
        if !running {
            self.begin_turn_changes(thread, &message_id, cx);
        }
        let project = thread.read(cx).meta.project_id.clone();
        let for_agent = if id.starts_with(SIDE) { text.clone() } else { self.message_for_agent(&text, &project) };
        let effort = self.effort_of(thread.read(cx));
        let blocks: Vec<(String, String)> = images
            .iter()
            .filter_map(|name| {
                let bytes = std::fs::read(yh_core::attachments::path(&self.data_dir, name)).ok()?;
                Some((yh_core::attachments::media_type(name).to_string(), yh_core::attachments::base64(&bytes)))
            })
            .collect();
        if files.is_empty() {
            let deliver = Some(Delivery { text: for_agent, blocks });
            self.command(&id, Command::Send { thread: id.clone(), text, attachments, message_id, now, effort, deliver }, cx);
        } else {
            self.command(&id, Command::Send { thread: id.clone(), text, attachments, message_id: message_id.clone(), now, effort: effort.clone(), deliver: None }, cx);
            // Copied into the thread's attachments first (off the main thread), then sent with
            // where each one is.
            let dir = self.data_dir.clone();
            let engine = self.engine.clone();
            cx.background_executor()
                .spawn(async move {
                    let paths = files.iter().zip(&file_names).map(|(f, name)| (f.copy_in(&dir, name), f.is_dir)).collect::<Vec<_>>();
                    let note = yh_core::attachments::files_note(&paths);
                    let message = if for_agent.is_empty() { note } else { format!("{for_agent}\n\n{note}") };
                    engine.send(Command::Deliver { thread: id, message_id, now, effort, delivery: Delivery { text: message, blocks } });
                })
                .detach();
        }
        cx.notify();
    }

    /// The thread's `claude`, started if it isn't.    /// How to start the thread's `claude`, when the engine asks (`Output::NeedsLaunch`): its
    /// folder, model, effort, tools and prompt; and what `YH_RUNTIME_LOG` adds to its line.
    fn launch_options(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) -> (Result<LaunchOptions, String>, Option<String>) {
        let t = thread.read(cx);
        let effort = self.effort_of(t);
        let project = self.project(&t.meta.project_id).cloned();
        let cwd = self.folder_of(t);
        let attachments = self.data_dir.join("attachments");
        let memory_on = self.library.setting("memoryEnabled").and_then(|v| v.as_bool()).unwrap_or(true);
        let memory_off_for_claude = self.library.setting("claudeCodeMemoryOff").and_then(|v| v.as_bool()).unwrap_or(true);
        // A side chat: a copy of its thread's conversation that can look but not edit and
        // leaves nothing behind. It reads freely; commands and web requests ask first. It sees
        // its thread's browser and the memory, read-only.
        if let Some(parent) = t.id().strip_prefix(SIDE).map(str::to_string) {
            let parent_thread = self.thread(&parent);
            let fork = parent_thread.as_ref().and_then(|p| {
                let p = p.read(cx);
                p.run.session_id.clone().or_else(|| p.meta.session_id.clone())
            });
            let mut extra = vec!["--tools".to_string(), "Read,Glob,Grep,Bash,WebFetch,WebSearch".into(), "--strict-mcp-config".into(), "--no-session-persistence".into()];
            if fork.is_some() {
                extra.push("--fork-session".into());
            }
            let mut servers: Vec<std::sync::Arc<dyn yh_core::mcp::ToolServer>> = vec![];
            let mut prompt = vec![SIDE_CHAT_PROMPT.to_string()];
            let mut allowed = vec![];
            let mut env = vec![];
            if self.library.setting("browserTools").and_then(|v| v.as_bool()).unwrap_or(true) {
                prompt.push(SIDE_BROWSER_PROMPT.into());
                allowed.push("mcp__browser".to_string());
                servers.push(std::sync::Arc::new(crate::browser::BrowserTools { thread: parent.clone(), project: Some(t.meta.project_id.clone()), read_only: true }));
            }
            if let (true, Some(db), Some(project)) = (memory_on, self.library.db.clone(), project.as_ref()) {
                if memory_off_for_claude {
                    env.push(("CLAUDE_CODE_DISABLE_AUTO_MEMORY".to_string(), "1".to_string()));
                }
                prompt.push(SIDE_MEMORY_PROMPT.into());
                allowed.push("mcp__memory".to_string());
                servers.push(std::sync::Arc::new(yh_core::memory_tools::MemoryTools {
                    store: yh_core::memory::Store { db },
                    scope: yh_core::memory::scope_of(&project.id),
                    actor: yh_core::memory::Actor { kind: "agent".into(), thread: Some(parent.clone()) },
                    on_change: Box::new(|| {}),
                    specialist: false,
                    read_only: true,
                }));
            }
            let options = LaunchOptions {
                cwd,
                session_id: fork,
                model: self.model_of(t),
                mode: Some("default".into()),
                effort: effort.clone(),
                add_dirs: vec![],
                env,
                append_system_prompt: Some(prompt.join("\n\n")),
                allowed_tools: allowed,
                servers,
                extra_args: extra,
            };
            return (Ok(options), None);
        }
        let mut env = vec![];
        // Claude Code's task tools: what the plan checklist shows.
        if self.library.setting("planChecklist").and_then(|v| v.as_bool()).unwrap_or(true) {
            env.push(("CLAUDE_CODE_ENABLE_TODO_TOOLS".to_string(), "1".to_string()));
        }
        // The app's memory: its tools, and the index every agent here is given.
        let mut servers: Vec<std::sync::Arc<dyn yh_core::mcp::ToolServer>> = vec![];
        let mut prompt: Vec<String> = vec![];
        let mut allowed = vec![];
        if let (true, Some(db), Some(project)) = (memory_on, self.library.db.clone(), project.as_ref()) {
            // One memory, the app's: Claude Code's own files would compete with it.
            if memory_off_for_claude {
                env.push(("CLAUDE_CODE_DISABLE_AUTO_MEMORY".into(), "1".into()));
            }
            let store = yh_core::memory::Store { db };
            let scope = yh_core::memory::scope_of(&project.id);
            let actor = yh_core::memory::Actor { kind: "agent".into(), thread: Some(t.id().to_string()) };
            let (text, listed) = yh_core::memory::prompt(&store.memories(Some(&scope), Some("active")), &project.name);
            store.note_injection(&scope, listed, text.len(), &actor);
            prompt.push(text);
            allowed.push("mcp__memory".to_string());
            let changed = self.tool_requests.clone();
            servers.push(std::sync::Arc::new(yh_core::memory_tools::MemoryTools {
                store,
                scope,
                actor,
                on_change: Box::new(move || {
                    let _ = changed.send(ToolRequest::MemoryChanged);
                }),
                specialist: false,
                read_only: false,
            }));
        }
        // Claude asks you to sign in to an MCP server, from a card in the thread.
        prompt.push(crate::state::harness_tools::INSTRUCTIONS.to_string());
        allowed.push("mcp__harness".to_string());
        servers.push(std::sync::Arc::new(crate::state::harness_tools::HarnessTools { caller: t.id().to_string(), requests: self.tool_requests.clone(), folder: cwd.clone(), attachments: attachments.clone() }));
        // Claude shows you images and videos (`show_media`).
        prompt.push(crate::state::harness_tools::MEDIA_INSTRUCTIONS.to_string());
        // The thread tools: list, read and tidy threads; this thread's title.
        if let Some(project) = project.as_ref() {
            let titles = self.library.setting("titleGeneration");
            let updates_title = self.library.setting("agentUpdatesTitles").and_then(|v| v.as_bool()).unwrap_or(true)
                && titles.as_ref().is_none_or(|t| t.get("provider").is_some_and(|p| !p.is_null()));
            let home = self.home_project().as_deref() == Some(project.id.as_str());
            prompt.push(thread_tools::instructions((!home).then_some(project.name.as_str()), updates_title));
            allowed.push("mcp__threads".to_string());
            servers.push(std::sync::Arc::new(thread_tools::ThreadTools {
                caller: t.id().to_string(),
                requests: self.tool_requests.clone(),
                db: self.library.db.clone(),
                updates_title,
            }));
        }
        // A memory specialist: its own prompt, read-only file tools, the specialist memory tools,
        // nothing saved; the importer may read the sources it was given.
        if let Some(rest) = t.id().strip_prefix(MEMORY_AGENT) {
            let (role, project_id) = rest.split_once(':').unwrap_or(("reconciler", rest));
            let (role, project_id) = (role.to_string(), project_id.to_string());
            let Some(db) = self.library.db.clone() else { return (Err("No memory here.".into()), None) };
            let name = self.project(&project_id).map(|p| if self.home_project().as_deref() == Some(p.id.as_str()) { "Home".to_string() } else { p.name.clone() }).unwrap_or_default();
            let changed = self.tool_requests.clone();
            // Its thread is named, so its window can list what this session changed.
            let actor = yh_core::memory::Actor { kind: if role == "importer" { "importer".into() } else { "reconciler".into() }, thread: Some(thread.read(cx).id().to_string()) };
            let server = yh_core::memory_tools::MemoryTools {
                store: yh_core::memory::Store { db },
                scope: yh_core::memory::scope_of(&project_id),
                actor,
                on_change: Box::new(move || {
                    let _ = changed.send(ToolRequest::MemoryChanged);
                }),
                specialist: true,
                read_only: false,
            };
            let t = thread.read(cx);
            let options = LaunchOptions {
                cwd,
                session_id: None,
                model: self.model_of(t),
                mode: Some("default".into()),
                effort: effort.clone(),
                add_dirs: self.memory_agent_sources.get(t.id()).cloned().unwrap_or_default(),
                env: vec![],
                append_system_prompt: Some(yh_core::memory_agents::prompt(&role, &name)),
                allowed_tools: vec!["mcp__memory".into(), "Read".into(), "Glob".into(), "Grep".into()],
                servers: vec![std::sync::Arc::new(server)],
                extra_args: vec!["--tools".into(), "Read,Glob,Grep,AskUserQuestion".into(), "--strict-mcp-config".into(), "--no-session-persistence".into()],
            };
            return (Ok(options), None);
        }
        // The browser the user watches, driven with mcp__browser__*.
        if self.library.setting("browserTools").and_then(|v| v.as_bool()).unwrap_or(true) {
            prompt.push(crate::browser::instructions().to_string());
            allowed.push("mcp__browser".to_string());
            servers.push(std::sync::Arc::new(crate::browser::BrowserTools { thread: t.id().to_string(), project: Some(t.meta.project_id.clone()), read_only: false }));
        }
        // Mermaid blocks are always drawn; this tells agents they may use them.
        if self.library.setting("diagramInstructions").and_then(|v| v.as_bool()).unwrap_or(true) {
            prompt.push(DIAGRAM_PROMPT.to_string());
        }
        // `gh stack` installed: how GitHub's stacked PRs are made here (checked aside at launch;
        // until that answers, by its folder, as a thread may start before it does).
        if self.stacks_installed.unwrap_or_else(yh_core::github::stack_extension_on_disk) {
            prompt.push(STACKS_PROMPT.to_string());
        }
        let options = LaunchOptions {
            cwd,
            session_id: t.run.session_id.clone(),
            model: self.model_of(t),
            mode: cli_mode(project.as_ref().map_or("auto", |p| p.mode.as_str())).map(str::to_string),
            effort: effort.clone(),
            add_dirs: vec![attachments.to_string_lossy().into_owned()],
            env,
            append_system_prompt: (!prompt.is_empty()).then(|| prompt.join("\n\n")),
            allowed_tools: allowed,
            servers,
            extra_args: vec![],
        };
        let stacks = options.append_system_prompt.as_deref().is_some_and(|p| p.contains(STACKS_PROMPT));
        (Ok(options), Some(format!(", gh stack instructions {}", if stacks { "in" } else { "not in" })))
    }

    /// Ends a thread's `claude`; the next message resumes its session.
    fn end_runtime(&mut self, id: &str, cx: &mut Context<Self>) {
        self.command(id, Command::End { thread: id.to_string(), only_idle: false }, cx);
    }

    /// The Dock badge counts threads waiting for you; a request to you, or a turn ending,
    /// is a notification when you're not looking at that thread.
    fn note_attention(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let waiting = self.threads.iter().filter(|t| t.read(cx).waiting_for_you()).count();
        crate::platform::set_badge(waiting);
        let t = thread.read(cx);
        if t.waiting_for_you() || t.elicitation().is_some() {
            let body = Self::describe_request(t);
            self.notify_about(crate::state::notify::Kind::NeedsYou, thread, body, cx);
        }
    }

    /// The turn is over (the engine saved it and lets its `claude` go later): unread if you
    /// were elsewhere, said, its title written again if it was a guess.
    fn turn_ended(&mut self, thread: &Entity<ThreadState>, end: TurnEnd, cx: &mut Context<Self>) {
        let id = thread.read(cx).id().to_string();
        self.finish_turn_changes(thread, cx);
        self.note_working(cx);
        let selected = self.selected.as_deref() == Some(id.strip_prefix(SIDE).unwrap_or(&id));
        // Waiting on jobs it started, Claude isn't done: it's back when they report. Stopped by
        // you: you're there, nothing to tell. A side chat never notifies.
        let stopped = end.stopped_by_you;
        if !end.waiting_on_jobs && !stopped && !id.starts_with(SIDE) {
            let body = Self::last_reply(thread.read(cx));
            self.notify_about(crate::state::notify::Kind::Finished, thread, body, cx);
        }
        // A first title that was a guess: written again now the conversation says more.
        if thread.read(cx).title_needs_refinement && !stopped {
            self.refine_title(thread, cx);
        }
        let (title, session) = thread.update(cx, |t, _| {
            t.meta.turn_in_progress = false;
            t.meta.updated_at = yh_core::library::now_ms();
            if !selected {
                t.unread = true;
            }
            (t.title.clone(), t.run.session_id.clone())
        });
        self.library.update_thread(&id, |r| {
            r.remove("turnInProgress");
            r.insert("title".into(), title.into());
            // None after a resume that failed: the next message starts a new session.
            r.insert("sessionID".into(), session.map_or(Value::Null, Value::from));
            r.insert("updatedAt".into(), iso_now().into());
        });
        // The toolbar's PR count follows what the turn did (once its rows are saved).
        if self.selected.as_deref() == Some(id.as_str()) {
            let t = thread.clone();
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(1500)).await;
                let _ = this.update(cx, |ws, cx| ws.count_pulls(&t, cx));
            })
            .detach();
        }
        cx.notify();
    }

    /// After the first message: a title written by a one-shot request with no tools, on the
    /// model chosen in Settings (Haiku by default), off the main thread. One you chose
    /// meanwhile stays.
    fn generate_title(&mut self, thread: &Entity<ThreadState>, message: &str, cx: &mut Context<Self>) {
        let settings = self.library.setting("titleGeneration").unwrap_or(serde_json::json!({"provider": "claude", "model": "haiku"}));
        if settings["provider"].as_str() != Some("claude") {
            return;
        }
        let model = settings["model"].as_str().unwrap_or("haiku").to_string();
        let rules = self.title_prompt();
        let prompt = yh_core::titles::initial_prompt(message, rules.as_deref());
        let Some(claude) = yh_core::claude::find_claude() else { return };
        let path = std::env::var("YH_PATH").ok();
        // Linked PRs and issues are looked up (aside) so the title says what they're about.
        let linked_text = message.to_string();
        let gh = yh_core::git::which("gh", path.as_deref());
        let weak = thread.downgrade();
        // The sidebar dims the placeholder title meanwhile.
        let generating = thread.read(cx).id().to_string();
        self.titles_rewriting.insert(generating.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let prompt = cx
                .background_executor()
                .spawn(async move { prompt + &yh_core::titles::linked_suffix(yh_core::titles::linked_context(&linked_text, gh.as_deref()).as_deref()) })
                .await;
            let mut result = Err(String::new());
            for attempt in 0..2 {
                if attempt > 0 {
                    cx.background_executor().timer(Duration::from_secs(2)).await;
                }
                let (claude, path, model, prompt) = (claude.clone(), path.clone(), model.clone(), prompt.clone());
                result = cx.background_executor().spawn(async move { yh_core::titles::generate(&claude, path.as_deref(), &model, &prompt) }).await;
                if result.is_ok() {
                    break;
                }
            }
            let _ = this.update(cx, |ws, cx| {
                ws.titles_rewriting.remove(&generating);
                // Settings → Thread Titles says why the last one failed.
                ws.title_error = result.as_ref().err().map(|e| if e.is_empty() { "no answer".to_string() } else { e.clone() });
                cx.notify();
            });
            let Ok(generated) = result else { return };
            let title = generated.title;
            let _ = this.update(cx, |ws, cx| {
                let Some(thread) = weak.upgrade() else { return };
                if thread.read(cx).meta.title_chosen {
                    return;
                }
                let id = thread.read(cx).id().to_string();
                thread.update(cx, |t, cx| {
                    t.title = title.clone();
                    t.meta.title = title.clone();
                    // The subject wasn't known yet ("fix this", a link it couldn't read): written
                    // again from the conversation once the first turn is over.
                    t.title_needs_refinement = generated.needs_refinement;
                    if std::env::var("YH_DATA_DIR").is_ok() {
                        eprintln!("TITLE generated: {title} (needs refinement: {})", generated.needs_refinement);
                    }
                    cx.notify();
                });
                // The turn already ended: now.
                let replied = thread.read(cx).items.iter().any(|i| i.ty == crate::state::thread::ItemType::Text);
                if generated.needs_refinement && !thread.read(cx).run.running && replied {
                    ws.refine_title(&thread, cx);
                }
                ws.library.update_thread(&id, |r| {
                    r.insert("title".into(), title.into());
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// Stop (the button, ⌘.): anything queued goes back into the composer first, so stopping
    /// never sends it; what waited on you is dropped; the turn ends as "Interrupted", quietly.
    /// `removing`: queued messages the composer is already taking away for good.
    pub fn stop(&mut self, thread: &Entity<ThreadState>, removing: &std::collections::HashSet<String>, cx: &mut Context<Self>) {
        if !thread.read(cx).run.running {
            return;
        }
        let queued = thread.read(cx).queued_messages();
        for (message, ..) in queued {
            if !removing.contains(&message) {
                self.withdraw(thread, &message, true, cx);
            }
        }
        let id = thread.read(cx).id().to_string();
        self.command(&id, Command::Stop { thread: id.clone() }, cx);
        thread.update(cx, |t, cx| {
            // Gone from the composer now; the engine drops them too.
            t.run.interrupt_requested = true;
            t.run.permissions.clear();
            t.run.elicitations.clear();
            if let Some(ask) = t.sign_in.take() {
                ask.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
                if let Some(decision) = ask.decision {
                    let _ = decision.send(false);
                }
            }
            cx.notify();
        });
        crate::platform::set_badge(self.threads.iter().filter(|t| t.read(cx).waiting_for_you()).count());
        cx.notify();
    }

    /// Settles the thread's first pending permission request (a tool, a question, a plan): the
    /// one the composer shows.
    pub fn respond(&mut self, thread: &Entity<ThreadState>, decision: Decision, cx: &mut Context<Self>) {
        let Some(id) = thread.read(cx).permission().map(|p| p.id.clone()) else { return };
        self.respond_to(thread, &id, decision, cx);
    }

    /// Settles one pending request by its id; the others keep waiting.
    pub fn respond_to(&mut self, thread: &Entity<ThreadState>, request_id: &str, decision: Decision, cx: &mut Context<Self>) {
        let id = thread.read(cx).id().to_string();
        let answered = thread.update(cx, |t, cx| {
            // Off the composer now; the engine answers it.
            let before = t.run.permissions.len();
            t.run.permissions.retain(|p| p.id != request_id);
            cx.notify();
            before != t.run.permissions.len()
        });
        if !answered {
            return;
        }
        self.command(&id, Command::Respond { thread: id.clone(), request_id: request_id.to_string(), decision }, cx);
        let waiting = self.threads.iter().filter(|t| t.read(cx).waiting_for_you()).count();
        crate::platform::set_badge(waiting);
        cx.notify();
    }

    pub fn open_mcp(&mut self, cx: &mut Context<Self>) {
        cx.emit(WorkspaceEvent::OpenMcp);
    }

    /// A control request to the thread's `claude` (started in its folder if it isn't running),
    /// and its reply: `mcp_status`, `mcp_toggle`, `mcp_authenticate`…
    pub fn mcp(&mut self, thread: &Entity<ThreadState>, request: Value, cx: &mut Context<Self>) -> Task<Result<Value, String>> {
        let id = thread.read(cx).id().to_string();
        let effort = self.effort_of(thread.read(cx));
        let (reply, answer) = flume::bounded(1);
        self.command(&id, Command::Mcp { thread: id.clone(), request, effort, reply }, cx);
        cx.background_executor().spawn(async move { answer.recv_async().await.unwrap_or_else(|_| Err("Claude Code didn't answer.".into())) })
    }

    /// Starts the thread's `claude` over (between turns), so it reads the MCP config again; the
    /// next message resumes its session.
    pub fn restart_runtime(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let id = thread.read(cx).id().to_string();
        self.command(&id, Command::End { thread: id.clone(), only_idle: true }, cx);
    }

    /// Answers an MCP server's question: `accept` (with what you filled in), `decline` or `cancel`.
    /// The one the composer shows: the oldest waiting.
    pub fn answer_elicitation(&mut self, thread: &Entity<ThreadState>, action: &str, content: Option<Value>, cx: &mut Context<Self>) {
        let Some(asked) = thread.read(cx).elicitation().map(|e| e.id.clone()) else { return };
        self.answer_elicitation_id(thread, &asked, action, content, cx);
    }

    /// One waiting question, by its request id; the others keep waiting.
    pub fn answer_elicitation_id(&mut self, thread: &Entity<ThreadState>, request_id: &str, action: &str, content: Option<Value>, cx: &mut Context<Self>) {
        let id = thread.read(cx).id().to_string();
        self.command(&id, Command::AnswerElicitation { thread: id.clone(), request_id: request_id.to_string(), action: action.to_string(), content }, cx);
        thread.update(cx, |t, cx| {
            t.run.elicitations.retain(|e| e.id != request_id);
            cx.notify();
        });
        cx.notify();
    }

    /// Approves a plan into a permission mode, which becomes the project's.
    pub fn approve_plan(&mut self, thread: &Entity<ThreadState>, mode: &str, cx: &mut Context<Self>) {
        let project = thread.read(cx).meta.project_id.clone();
        self.set_mode(&project, mode, cx);
        self.respond(thread, Decision::Allow { updated_input: None, mode: Some(control_mode(mode).into()) }, cx);
    }

    /// `YH_ANSWER=allow|deny`: permission prompts answered by themselves, for tests.
    fn answer_for_tests(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        // `YH_MCP_ELICIT_ANSWER=accept|decline`: what an MCP server asks, answered with its defaults.
        if let Ok(answer) = std::env::var("YH_MCP_ELICIT_ANSWER") {
            let waiting = thread.read(cx).run.elicitations.clone();
            if waiting.len() > 1 {
                eprintln!("MCP elicitations waiting: {}", waiting.len());
            }
            for e in waiting {
                eprintln!("MCP elicitation from {}: {} ({}{}): {answer}", e.server, e.message, e.mode, e.url.as_deref().map(|u| format!(", {u}")).unwrap_or_default());
                let content = (answer == "accept" && e.mode == "form").then(|| crate::ui::composer::elicit_defaults(&e.schema));
                self.answer_elicitation_id(thread, &e.id, &answer, content, cx);
            }
        }
        let Ok(answer) = std::env::var("YH_ANSWER") else { return };
        // `YH_ANSWER_DELAY=<seconds>`: answered that much later, so the cards can be seen (and
        // several pile up).
        if let Some(delay) = std::env::var("YH_ANSWER_DELAY").ok().and_then(|v| v.parse::<f64>().ok()) {
            let weak = thread.downgrade();
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let _ = this.update(cx, |ws, cx| {
                    if let Some(thread) = weak.upgrade() {
                        ws.answer_waiting(&thread, &answer, cx);
                    }
                });
            })
            .detach();
            return;
        }
        self.answer_waiting(thread, &answer, cx);
    }

    fn answer_waiting(&mut self, thread: &Entity<ThreadState>, answer: &str, cx: &mut Context<Self>) {
        // Every request waiting, by its own id (parallel sub-agents ask at once).
        let waiting: Vec<_> = thread.read(cx).run.permissions.clone();
        if waiting.len() > 1 {
            eprintln!("PERMISSIONS waiting: {}", waiting.len());
        }
        for p in waiting {
            eprintln!("answering {} for {}: {answer}", p.id, p.tool);
            let decision = if answer == "allow" { Decision::allow() } else { Decision::deny() };
            self.respond_to(thread, &p.id, decision, cx);
        }
    }

    /// The project's slash commands and skills.
    pub fn commands(&self, project_id: &str) -> &[SlashCommand] {
        self.commands.get(project_id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Where a thread's `claude` runs: its folder, else its project's, else the data dir's `home`.
    pub fn folder_of(&self, thread: &ThreadState) -> String {
        let exists = |p: &String| std::path::Path::new(p).is_dir();
        // A worktree that's since been removed falls back to the project's folder.
        thread
            .meta
            .working_directory
            .clone()
            .filter(exists)
            .or_else(|| self.project(&thread.meta.project_id).map(|p| p.path.clone()).filter(exists))
            .unwrap_or_else(|| {
                // A project that's gone runs in a folder of the data dir, never your home folder.
                let home = self.data_dir.join("home");
                let _ = std::fs::create_dir_all(&home);
                home.to_string_lossy().into_owned()
            })
    }

    /// The thread's folder's files matching `query`, best first; listed off the main thread.
    pub fn mention_files(&mut self, thread: &Entity<ThreadState>, query: String, cx: &mut Context<Self>) -> Task<Vec<String>> {
        let folder = self.folder_of(thread.read(cx));
        let files = self.folder_files(folder, cx);
        cx.spawn(async move |_, cx| {
            let files = files.await;
            cx.background_executor().spawn(async move { yh_core::files::matches(&files, &query, 12) }).await
        })
    }

    /// A folder's files (git's list, else a walk), read off the main thread and kept 30
    /// seconds: for `@` mentions, the Files tab and Go to File.
    pub fn folder_files(&mut self, folder: String, cx: &mut Context<Self>) -> Task<Arc<Vec<String>>> {
        let cached = self.files.get(&folder).filter(|(at, _)| at.elapsed() < Duration::from_secs(30)).map(|(_, f)| f.clone());
        cx.spawn(async move |this, cx| match cached {
            Some(files) => files,
            None => {
                let path = folder.clone();
                let files = Arc::new(cx.background_executor().spawn(async move { yh_core::files::list(std::path::Path::new(&path)) }).await);
                let _ = this.update(cx, |ws, _| {
                    ws.files.insert(folder, (Instant::now(), files.clone()));
                });
                files
            }
        })
    }

    // MARK: Right sidebar

    /// A thread's side chat, made the first time it's asked for: in the thread's folder, on
    /// its model, starting from its conversation as it is when the first question is sent.
    pub fn side_chat(&mut self, parent: &str, cx: &mut Context<Self>) -> Option<Entity<ThreadState>> {
        if let Some(chat) = self.side_chats.get(parent) {
            return Some(chat.clone());
        }
        let p = self.thread(parent)?;
        let p = p.read(cx);
        let mut meta = ThreadMeta::new(format!("{SIDE}{parent}"), p.meta.project_id.clone(), "Side Chat".into());
        meta.working_directory = Some(self.folder_of(p));
        meta.model = p.meta.model.clone();
        meta.effort = p.meta.effort.clone();
        let chat = make_thread(meta, cx.entity().downgrade(), cx);
        chat.update(cx, |t, _| {
            t.loaded = true;
            t.run.total = Some(0);
        });
        self.by_id.insert(format!("{SIDE}{parent}"), chat.clone());
        self.side_chats.insert(parent.to_string(), chat.clone());
        Some(chat)
    }

    /// Starts the side chat over: its `claude` ended, its conversation gone.
    pub fn clear_side_chat(&mut self, parent: &str, cx: &mut Context<Self>) {
        let id = format!("{SIDE}{parent}");
        self.forget_in_engine(&id);
        self.by_id.remove(&id);
        self.side_chats.remove(parent);
        cx.notify();
    }

    pub fn set_shows_inspector(&mut self, on: bool, cx: &mut Context<Self>) {
        self.shows_inspector = on;
        self.library.set_setting("showsInspector", &Value::from(on));
        cx.notify();
    }

    pub fn show_panel(&mut self, panel: Panel, cx: &mut Context<Self>) {
        self.panel = panel;
        if !self.shows_inspector {
            self.set_shows_inspector(true, cx);
        }
        cx.notify();
    }

    fn note_catalog(&mut self, models: &Value, commands: &Value, project_id: &str, cx: &mut Context<Self>) {
        if let Some(list) = commands.as_array() {
            let parsed: Vec<SlashCommand> = list.iter().filter_map(SlashCommand::from_json).collect();
            if !parsed.is_empty() {
                self.commands.insert(project_id.to_string(), parsed);
                let mut all = match self.library.setting("slashCommands") {
                    Some(Value::Object(m)) => m,
                    _ => serde_json::Map::new(),
                };
                // In the shape 0.0.x saved (its `SlashCommand`, MCP prompts by the name
                // Claude Code takes, with their server).
                let swift: Vec<Value> = self.commands[project_id].iter().map(SlashCommand::saved).collect();
                all.insert(project_id.to_string(), Value::Array(swift));
                self.library.set_setting("slashCommands", &Value::Object(all));
            }
        }
        let parsed: Vec<ModelInfo> = models.as_array().map(|a| a.iter().filter_map(ModelInfo::from_json).collect()).unwrap_or_default();
        if parsed.is_empty() || parsed == self.models {
            return;
        }
        self.models = parsed;
        self.library.set_setting("claudeModels", &ModelInfo::saved_list(models));
        cx.notify();
    }

    /// The thread's changed rows into harness.sqlite (in the background).
    pub fn save_rows(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let id = thread.read(cx).id().to_string();
        if self.opened.contains(&id) {
            self.engine.send(Command::Save { thread: id });
        }
    }

    /// Quitting: everything saved, every `claude` ended.
    pub fn prepare_for_quit(&mut self, _cx: &mut Context<Self>) {
        for id in std::mem::take(&mut self.pending_purge) {
            self.library.purge(&id);
        }
        // The engine cuts off what still runs, saves every thread and ends every `claude`.
        let (done, finished) = flume::bounded(1);
        self.engine.send(Command::Quit { done });
        let _ = finished.recv_timeout(Duration::from_secs(5));
        self.library.save();
        if let Some(db) = &self.library.db {
            db.flush();
        }
    }

    // MARK: Search and resuming

    /// A Claude Code session started elsewhere, as a thread that continues it (`--resume`) in
    /// its own folder, with its history. Read from `~/.claude/projects`, never written.
    pub fn resume_session(&mut self, text: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let session = session_id_in(text).ok_or("That isn't a session ID.")?;
        if let Some(t) = self.thread_for_session(&session, cx) {
            let id = t.read(cx).id().to_string();
            self.select(&id, cx);
            return Ok(());
        }
        let path = yh_core::history::locate(&yh_core::history::config_directory(), &session).ok_or("No Claude Code session with this ID.")?;
        let (cwd, _) = yh_core::history::peek(&path);
        let folder = cwd.filter(|c| std::path::Path::new(c).is_dir());
        let project = match &folder {
            Some(f) => self.add_project(f, cx),
            None => self.home_project().or_else(|| self.projects.first().map(|p| p.id.clone())).ok_or("No project to put it in.")?,
        };
        let (id, _) = self.import_session(&session, &project, cx)?;
        self.select(&id, cx);
        Ok(())
    }

    pub fn thread_for_session(&self, session: &str, cx: &App) -> Option<Entity<ThreadState>> {
        self.threads.iter().find(|t| t.read(cx).run.session_id.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(session))).cloned()
    }

    /// A Claude Code session as a new thread of `project` that continues it in the folder it
    /// ran in, with its history; the thread's id and how many messages it brought.
    pub fn import_session(&mut self, session: &str, project: &str, cx: &mut Context<Self>) -> Result<(String, usize), String> {
        let path = yh_core::history::locate(&yh_core::history::config_directory(), session).ok_or("No Claude Code session with this ID.")?;
        let (cwd, first) = yh_core::history::peek(&path);
        let rows = yh_core::history::load_with_limit(&path, usize::MAX);
        let messages = rows.iter().filter(|e| e.kind == yh_core::library::Kind::User).count();
        let folder = cwd.filter(|c| std::path::Path::new(c).is_dir());
        let title = first.unwrap_or_else(|| "Resumed session".into());
        let id = self.new_thread(project, cx);
        let project_path = self.project(project).map(|p| p.path.clone());
        let session = session.to_string();
        self.update_meta(&id, cx, |m, r| {
            m.title = title.clone();
            r.insert("title".into(), title.clone().into());
            r.insert("sessionID".into(), session.clone().into());
            if let Some(f) = folder.clone().filter(|f| Some(f) != project_path.as_ref()) {
                m.working_directory = Some(f.clone());
                r.insert("workingDirectory".into(), f.into());
            }
        });
        if let Some(thread) = self.thread(&id) {
            thread.update(cx, |t, _| {
                t.run.session_id = Some(session.clone());
                t.meta.session_id = Some(session.clone());
            });
            // Its rows are written below: the engine reads its count again, with the session.
            self.forget_in_engine(&id);
            if let Some(db) = &self.library.db {
                let now = yh_core::library::now_ms() / 1000.0;
                let saved: Vec<(usize, yh_core::db::Row)> =
                    rows.iter().enumerate().map(|(i, e)| (i, yh_core::recovery::row_of(e, if e.kind == yh_core::library::Kind::Tool { e.id.clone() } else { format!("h{i}") }, e.date.or(Some(now))))).collect();
                db.save_rows(&id, saved.len(), saved);
            }
            // Read back from what was saved, as any history. `new_thread` already selected it,
            // so selecting it again won't load it: load it here.
            thread.update(cx, |t, _| t.unload());
            self.ensure_loaded(&thread, cx);
        }
        Ok((id, messages))
    }

    // MARK: Memory

    /// A project's page, from clicking its name: Home has no repos, so its page is its memory.
    pub fn open_project(&mut self, project: &str, cx: &mut Context<Self>) {
        if self.home_project().as_deref() == Some(project) {
            return self.open_memory(project, cx);
        }
        self.page = Some(Page::Project(project.to_string()));
        cx.emit(WorkspaceEvent::PageOpened);
        cx.notify();
    }

    /// Opens a project's Memory page in place of the open thread.
    pub fn open_memory(&mut self, project: &str, cx: &mut Context<Self>) {
        self.page = Some(Page::Memory(project.to_string()));
        cx.emit(WorkspaceEvent::PageOpened);
        cx.notify();
    }

    /// Opens a project's memory importer or reconciler in place of the thread (the one already
    /// open, or a new one), and sends it `message` (an import's sources, or the review request).
    pub fn open_memory_agent(&mut self, role: &str, project: &str, sources: Vec<String>, message: Option<String>, cx: &mut Context<Self>) {
        let id = format!("{MEMORY_AGENT}{role}:{project}");
        if !sources.is_empty() {
            // New sources: a new session, so it can read them.
            self.memory_agent_sources.insert(id.clone(), sources);
            self.end_runtime(&id, cx);
        }
        let thread = match self.thread(&id) {
            Some(t) => t,
            None => {
                let mut meta = ThreadMeta::new(id.clone(), project.into(), yh_core::memory_agents::title(role).into());
                meta.working_directory = self.project(project).map(|p| p.path.clone());
                meta.title_chosen = true;
                let t = make_thread(meta, cx.entity().downgrade(), cx);
                t.update(cx, |t, _| {
                    t.loaded = true;
                    t.run.total = Some(0);
                });
                self.by_id.insert(id.clone(), t.clone());
                t
            }
        };
        self.select(&id, cx);
        if let Some(text) = message {
            self.send(&thread, &text, false, cx);
        }
    }

    /// A hidden thread in Home's folder, for asking Claude Code about what applies everywhere
    /// (Settings → MCP); never saved, never in the sidebar.
    pub fn settings_thread(&mut self, cx: &mut Context<Self>) -> Option<Entity<ThreadState>> {
        let id = format!("{SETTINGS}{}", self.home_project().or_else(|| self.projects.first().map(|p| p.id.clone()))?);
        if let Some(t) = self.thread(&id) {
            return Some(t);
        }
        let project = self.home_project().or_else(|| self.projects.first().map(|p| p.id.clone()))?;
        let mut meta = ThreadMeta::new(id.clone(), project.clone(), "Settings".into());
        meta.working_directory = self.project(&project).map(|p| p.path.clone());
        let t = make_thread(meta, cx.entity().downgrade(), cx);
        t.update(cx, |t, _| {
            t.loaded = true;
            t.run.total = Some(0);
        });
        self.by_id.insert(id, t.clone());
        Some(t)
    }

    /// Opens Usage in place of the open thread, or closes it.
    pub fn toggle_usage(&mut self, cx: &mut Context<Self>) {
        if self.page == Some(Page::Usage) {
            self.page = None;
        } else {
            self.page = Some(Page::Usage);
            cx.emit(WorkspaceEvent::PageOpened);
        }
        cx.notify();
    }

    pub fn memory_changed(&mut self, cx: &mut Context<Self>) {
        self.memory_revision += 1;
        cx.notify();
    }

    pub fn memory_store(&self) -> Option<yh_core::memory::Store> {
        self.library.db.clone().map(|db| yh_core::memory::Store { db })
    }

    // MARK: Agents' thread tools

    /// The project a thread's agent reaches: None in Home (everything).
    fn tool_scope(&self, caller: &str, cx: &App) -> Option<Option<ProjectMeta>> {
        let project = self.thread(caller)?.read(cx).meta.project_id.clone();
        Some(if self.home_project().as_deref() == Some(project.as_str()) { None } else { self.project(&project).cloned() })
    }

    fn handle_tool_request(&mut self, request: ToolRequest, cx: &mut Context<Self>) {
        match request {
            ToolRequest::Snapshot { caller, reply } => {
                let snapshot = self.tool_scope(&caller, cx).map(|scope| thread_tools::Snapshot {
                    scope,
                    projects: self.projects.clone(),
                    threads: self
                        .threads
                        .iter()
                        .map(|t| {
                            let t = t.read(cx);
                            thread_tools::ThreadInfo {
                                id: t.id().to_string(),
                                title: t.title.clone(),
                                project_id: t.meta.project_id.clone(),
                                running: t.run.running,
                                waiting: t.waiting_for_you(),
                                unread: t.unread,
                                pinned: t.meta.pinned,
                                settled: t.meta.settled_at.is_some(),
                                updated_at: t.meta.updated_at,
                                session_id: t.run.session_id.clone(),
                            }
                        })
                        .collect(),
                });
                let _ = reply.send(snapshot);
            }
            ToolRequest::MemoryChanged => self.memory_changed(cx),
            ToolRequest::Mcp { caller, request, reply } => {
                let Some(thread) = self.thread(&caller) else {
                    let _ = reply.send(Err("This thread is gone.".into()));
                    return;
                };
                let task = self.mcp(&thread, request, cx);
                cx.spawn(async move |_, _| {
                    let _ = reply.send(task.await);
                })
                .detach();
            }
            ToolRequest::SignInCard { caller, ask } => {
                if let Some(thread) = self.thread(&caller) {
                    let attention = ask.is_some();
                    thread.update(cx, |t, cx| {
                        t.sign_in = ask;
                        cx.notify();
                    });
                    if attention {
                        self.note_attention(&thread, cx);
                    }
                    cx.notify();
                }
            }
            ToolRequest::OpenUrl(url) => {
                if std::env::var("YH_MCP_BROWSER").as_deref() == Ok("log") {
                    eprintln!("MCP sign-in page: {url}");
                } else {
                    cx.open_url(&url);
                }
            }
            ToolRequest::Rename { id, title, reply } => {
                self.rename(&id, &title, cx);
                let _ = reply.send(());
            }
            ToolRequest::Settle { id, on, reply } => {
                self.set_settled(&id, on, cx);
                let _ = reply.send(());
            }
            ToolRequest::Pin { id, on, reply } => {
                self.set_pinned(&id, on, cx);
                let _ = reply.send(());
            }
            ToolRequest::Import { caller, session, project, reply } => {
                let _ = reply.send(self.import_for_agent(&caller, &session, project.as_deref(), cx));
            }
            ToolRequest::UpdateTitle { caller, messages, sent, hint, reply } => {
                let _ = reply.send(self.update_title_for_agent(&caller, messages, sent, hint, cx));
            }
        }
    }

    /// `import_claude_code_session`: never a new project. From a project's thread, into that
    /// project when the session ran inside its folder; from Home, into the project asked for,
    /// else the one for its folder (the closest around it), else Home.
    fn import_for_agent(&mut self, caller: &str, session: &str, asked: Option<&str>, cx: &mut Context<Self>) -> Result<(String, usize), String> {
        let scope = self.tool_scope(caller, cx).ok_or("This thread is gone.")?;
        if let Some(existing) = self.thread_for_session(session, cx) {
            let project = existing.read(cx).meta.project_id.clone();
            if scope.as_ref().is_some_and(|p| p.id != project) {
                return Err("That session is already a thread, in a project you can't reach from here.".into());
            }
            return Ok((existing.read(cx).id().to_string(), usize::MAX));
        }
        let path = yh_core::history::locate(&yh_core::history::config_directory(), session).ok_or("No Claude Code session with this ID.")?;
        let (cwd, _) = yh_core::history::peek(&path);
        let inside = |folder: &str| cwd.as_deref().is_some_and(|c| c == folder || c.starts_with(&format!("{}/", folder.trim_end_matches('/'))));
        let destination = match &scope {
            Some(own) => {
                if !inside(&own.path) {
                    return Err(format!("That session ran in {}, outside {} ({}). Import it from a thread in Home.", cwd.as_deref().unwrap_or("an unknown folder"), own.name, own.path));
                }
                own.id.clone()
            }
            None => {
                let chosen = asked.and_then(|given| {
                    self.projects
                        .iter()
                        .find(|p| given.len() >= 6 && p.id.to_lowercase().starts_with(&given.to_lowercase()))
                        .or_else(|| self.projects.iter().find(|p| p.name.eq_ignore_ascii_case(given)))
                });
                if asked.is_some() && chosen.is_none() {
                    return Err(format!("No project {}.", asked.unwrap_or_default()));
                }
                let home = self.home_project();
                let suggested = || {
                    let exact = self.projects.iter().find(|p| cwd.as_deref() == Some(p.path.trim_end_matches('/')));
                    exact
                        .or_else(|| {
                            self.projects
                                .iter()
                                .filter(|p| Some(&p.id) != home.as_ref() && inside(&p.path))
                                .max_by_key(|p| p.path.len())
                        })
                        .map(|p| p.id.clone())
                        .or_else(|| home.clone())
                        .or_else(|| self.projects.first().map(|p| p.id.clone()))
                };
                chosen.map(|p| p.id.clone()).or_else(suggested).ok_or("There's no project to put it in.")?
            }
        };
        self.import_session(session, &destination, cx)
    }

    /// `update_title`: the caller's title written again from the conversation, with the
    /// agent's hint, on the title model; kept if still accurate, never over a chosen one.
    fn update_title_for_agent(&mut self, caller: &str, messages: Vec<(bool, String)>, sent: usize, hint: Option<String>, cx: &mut Context<Self>) -> String {
        let Some(thread) = self.thread(caller) else { return "This thread is gone.".into() };
        let current = thread.read(cx).title.clone();
        if thread.read(cx).meta.title_chosen {
            return format!("The user chose this thread's title, “{current}”, so it stays. Rename it only if they ask.");
        }
        if self.titles_rewriting.contains(caller) {
            return "The title is already being rewritten.".into();
        }
        if self.title_asked_at.get(caller) == Some(&sent) {
            return "The title was already updated since the user's last message; once per change of subject is enough.".into();
        }
        self.title_asked_at.insert(caller.to_string(), sent);
        let settings = self.library.setting("titleGeneration").unwrap_or(serde_json::json!({"provider": "claude", "model": "haiku"}));
        let Some(claude) = yh_core::claude::find_claude().filter(|_| settings["provider"].as_str() == Some("claude") && sent > 0) else {
            return format!("The title (“{current}”) can't be rewritten right now.");
        };
        let model = settings["model"].as_str().unwrap_or("haiku").to_string();
        let rules = self.title_prompt();
        let prompt = yh_core::titles::regenerate_prompt(&messages, &current, rules.as_deref(), hint.as_deref());
        let path = std::env::var("YH_PATH").ok();
        let gh = yh_core::git::which("gh", path.as_deref());
        let linked_text: String = messages.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join("\n");
        self.titles_rewriting.insert(caller.to_string());
        let id = caller.to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let prompt = prompt + &yh_core::titles::linked_suffix(yh_core::titles::linked_context(&linked_text, gh.as_deref()).as_deref());
                    yh_core::titles::generate(&claude, path.as_deref(), &model, &prompt)
                })
                .await;
            let _ = this.update(cx, |ws, cx| {
                ws.titles_rewriting.remove(&id);
                if std::env::var("YH_DATA_DIR").is_ok() {
                    eprintln!("TITLE rewritten: {result:?}");
                }
                let (Ok(yh_core::titles::Generated { title, .. }), Some(thread)) = (result, ws.thread(&id)) else { return };
                // A title you chose meanwhile stays.
                if title.is_empty() || thread.read(cx).meta.title_chosen || thread.read(cx).title == title {
                    return;
                }
                thread.update(cx, |t, cx| {
                    t.title = title.clone();
                    t.meta.title = title.clone();
                    cx.notify();
                });
                ws.library.update_thread(&id, |r| {
                    r.insert("title".into(), title.into());
                });
                cx.notify();
            });
        })
        .detach();
        format!("The title (“{current}”) is being rewritten from the conversation; it stays as it is if it's still accurate. No need to mention it to the user.")
    }

    // MARK: Recovery

    /// Turns a quit or crash cut off (`turnInProgress` still set at launch): finished from
    /// Claude Code's session file (it writes as it goes), with a note, in the background.
    fn recover_cut_off_turns(&mut self, cx: &mut Context<Self>) {
        let Some(db) = self.library.db.clone() else { return };
        let cut: Vec<Entity<ThreadState>> = self.threads.iter().filter(|t| t.read(cx).meta.turn_in_progress).cloned().collect();
        for thread in cut {
            let (id, session) = {
                let t = thread.read(cx);
                (t.id().to_string(), t.run.session_id.clone())
            };
            let db = db.clone();
            let weak = thread.downgrade();
            cx.spawn(async move |this, cx| {
                let id2 = id.clone();
                let count = cx
                    .background_executor()
                    .spawn(async move { yh_core::recovery::recover(&db, &id2, session.as_deref()) })
                    .await;
                let _ = this.update(cx, |ws, cx| {
                    ws.library.update_thread(&id, |r| {
                        r.remove("turnInProgress");
                    });
                    let Some(thread) = weak.upgrade() else { return };
                    // Talked to meanwhile: what's there now counts.
                    if thread.read(cx).run.running {
                        return;
                    }
                    let open = ws.selected.as_deref() == Some(id.as_str());
                    thread.update(cx, |t, cx| {
                        t.meta.turn_in_progress = false;
                        t.unload();
                        t.run.total = Some(count);
                        cx.notify();
                    });
                    if open {
                        ws.ensure_loaded(&thread, cx);
                    }
                });
            })
            .detach();
        }
    }
}

// MARK: Memory specialists' sessions

impl Workspace {
    /// New Session: ends a memory specialist's conversation and opens it again empty, keeping
    /// its model and effort.
    pub fn restart_memory_agent(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some((role, project)) = id.strip_prefix(MEMORY_AGENT).and_then(|r| r.split_once(':')).map(|(a, b)| (a.to_string(), b.to_string())) else { return };
        let (model, effort) = self.thread(id).map(|t| (t.read(cx).meta.model.clone(), t.read(cx).meta.effort.clone())).unwrap_or_default();
        self.forget_in_engine(id);
        self.by_id.remove(id);
        self.memory_agent_sources.remove(id);
        self.open_memory_agent(&role, &project, vec![], None, cx);
        self.set_model(id, model, cx);
        self.set_effort(id, effort, cx);
    }
}

// MARK: Background jobs

impl Workspace {
    /// Stops a job Claude left running (`stop_task`); its end
    /// comes back as for any other.
    pub fn stop_job(&mut self, thread: &Entity<ThreadState>, task_id: &str, cx: &mut Context<Self>) {
        let id = thread.read(cx).id().to_string();
        self.command(&id, Command::StopJob { thread: id.clone(), task_id: task_id.to_string() }, cx);
    }

    /// Background jobs and sub-agents running in every thread, for asking before quitting:
    /// (title, detail).
    pub fn quit_question(&self, cx: &App) -> Option<(String, String)> {
        // Worded by who started what: a running sub-agent's commands count with it.
        let commands: Vec<String> =
            self.threads.iter().flat_map(|t| { let j = &t.read(cx).run.jobs; j.background().filter(|c| !j.in_agent(c)).map(|c| c.description.clone()).collect::<Vec<_>>() }).collect();
        let agents: Vec<String> = self.threads.iter().flat_map(|t| t.read(cx).run.jobs.agents.iter().map(|j| j.description.clone()).collect::<Vec<_>>()).collect();
        let theirs: usize = self.threads.iter().map(|t| t.read(cx).run.jobs.agents_commands()).sum();
        quit_words(&commands, &agents, theirs)
    }
}

/// "1 command and 2 sub-agents are running" / "“Build” stops if you quit YourHarness.", for
/// the quit question. `theirs`: commands the running sub-agents started.
fn quit_words(commands: &[String], agents: &[String], theirs: usize) -> Option<(String, String)> {
    let plural = |n: usize, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
    let title = match (commands.len(), agents.len()) {
        (0, 0) => return None,
        (1, 0) => "A background command is running".to_string(),
        (0, 1) => "A sub-agent is running".to_string(),
        (c, 0) => format!("{c} background commands are running"),
        (0, a) => format!("{a} sub-agents are running"),
        (c, a) => format!("{} and {} are running", plural(c, "command", "commands"), plural(a, "sub-agent", "sub-agents")),
    };
    let mut detail = match (commands, agents) {
        ([one], []) | ([], [one]) => format!("“{one}” stops if you quit YourHarness"),
        _ => "They stop if you quit YourHarness".to_string(),
    };
    if theirs > 0 {
        let whose = if agents.len() == 1 { "the sub-agent" } else { "the sub-agents" };
        detail += &format!(", with {} {whose} started", if theirs == 1 { "the command".to_string() } else { format!("the {theirs} commands") });
    }
    Some((title, detail + "."))
}

fn make_thread(meta: ThreadMeta, workspace: WeakEntity<Workspace>, cx: &mut Context<Workspace>) -> Entity<ThreadState> {
    let thread = cx.new(|_| ThreadState::new(meta));
    // Near the top of what's held: read the page before it.
    let weak = thread.downgrade();
    thread.read(cx).list.set_scroll_handler(move |event, _, cx| {
        // The jump buttons over the transcript come and go as it scrolls.
        crate::ui::working::scrolled(cx);
        if event.visible_range.start > 4 {
            return;
        }
        let (ws, weak) = (workspace.clone(), weak.clone());
        cx.defer(move |cx| {
            if let (Some(ws), Some(thread)) = (ws.upgrade(), weak.upgrade()) {
                ws.update(cx, |ws, cx| ws.load_earlier(&thread, cx));
            }
        });
    });
    thread
}

/// A Claude Code session ID in what was pasted: the ID, a `claude --resume <id>`, a path.
pub fn session_id_in(text: &str) -> Option<String> {
    let bytes: Vec<char> = text.chars().collect();
    let hex = |c: &char| c.is_ascii_hexdigit();
    (0..bytes.len().saturating_sub(35)).find_map(|i| {
        let s: String = bytes[i..i + 36].iter().collect();
        let ok = s.chars().enumerate().all(|(n, c)| if [8, 13, 18, 23].contains(&n) { c == '-' } else { hex(&c) });
        ok.then_some(s.to_lowercase())
    })
}

/// What agents are told about diagrams.
pub const DIAGRAM_PROMPT: &str = "You are running inside YourHarness, a native macOS app that draws ```mermaid code blocks in your replies \
as diagrams, right in the conversation, with zoom, copy and PNG export. When the user asks for a diagram, \
chart, flow or visualization, or a picture would explain something better than prose, put ```mermaid \
blocks in your reply. Do this instead of generating image files (Graphviz/dot, PlantUML, mermaid-cli, SVG, \
PNG) or installing tools for it, unless the user explicitly asks for a file or the diagram has to live in \
the repo. Don't add diagrams to simple answers.
Supported: `flowchart` (TD, LR, BT, RL; shapes [box] (rounded) ([pill]) {decision} ((circle)) [(database)] \
[[subroutine]] {{hexagon}}; edges --> --- -.-> ==> with |labels|; subgraph … end; classDef/class/:::name/style \
with fill, stroke, stroke-width, stroke-dasharray) and `sequenceDiagram` (participant/actor, ->> -->> -x -) \
arrows, +/- activation, Note, loop/alt/else/opt/par/critical/break). Other diagram types show as code.
For a big system, draw an overview plus focused diagrams (around 25 nodes each) and group with subgraphs. \
To highlight things such as risks or gaps, use a class, e.g. `classDef gap stroke:#e5484d,stroke-dasharray:4` \
then `class X,Y gap`. Quote labels that contain punctuation, e.g. A[\"Parse (v2)\"].";

// MARK: Composer: the app's own commands, queued messages, context, MCP servers needing you

/// What the composer's requests leave waiting.
#[derive(Default)]
struct ComposerJobs {
    /// Threads that asked for `/rename`: the CLI's next title is theirs.
    renames: std::collections::HashSet<String>,
    /// By project: the skills a session there reported, which the agent can run itself.
    skills: HashMap<String, Vec<String>>,
}

/// What became of a message that starts with a command.
pub enum CommandHandling {
    /// The app did it; nothing goes to Claude.
    Done,
    /// Goes to Claude as this (`/btw` in a side chat: just the question).
    Send(String),
    NotOurs,
}

/// Reasoning effort levels, as `/effort` takes them.
const EFFORTS: &[(&str, &str)] = &[("low", "Low"), ("medium", "Medium"), ("high", "High"), ("xhigh", "Extra High"), ("max", "Max")];

/// Commands the app answers itself, first in the "/" menu.
pub fn app_commands() -> Vec<SlashCommand> {
    vec![
        SlashCommand::new("mcp", "[enable|disable|reconnect <server>]", "See your MCP servers: sign in, turn them on or off, reconnect."),
        SlashCommand::new("btw", "<question>", "Ask a side question in the sidebar. Claude sees this thread; nothing is added to it."),
    ]
}

/// Only mean something in the terminal UI.
const TERMINAL_ONLY: &[&str] = &["color", "focus", "heapdump"];

impl Workspace {
    /// A project's commands and skills, from a throwaway `claude` in its folder (it's only asked
    /// for its catalog: no request is made, nothing's saved), once per launch: the "/" menu is
    /// right before any session there has started. Off the main thread; 20 s at most.
    pub fn load_commands(&mut self, project_id: &str, cx: &mut Context<Self>) {
        if std::env::var("YH_NO_CLAUDE").is_ok() || !self.command_loads.insert(project_id.to_string()) {
            return;
        }
        let Some(folder) = self.project(project_id).map(|p| p.path.clone()).filter(|p| std::path::Path::new(p).is_dir()) else { return };
        let project = project_id.to_string();
        cx.spawn(async move |this, cx| {
            let catalog = cx
                .background_executor()
                .spawn(async move {
                    let options = LaunchOptions {
                        cwd: folder,
                        session_id: None,
                        model: None,
                        mode: None,
                        effort: None,
                        add_dirs: vec![],
                        env: vec![],
                        append_system_prompt: None,
                        allowed_tools: vec![],
                        servers: vec![],
                        extra_args: vec!["--no-session-persistence".into()],
                    };
                    let (tx, rx) = flume::unbounded();
                    let mut process = yh_core::claude::ClaudeProcess::spawn(options, tx).ok()?;
                    process.initialize();
                    let deadline = Instant::now() + Duration::from_secs(20);
                    let mut found = None;
                    while let Ok(event) = rx.recv_deadline(deadline) {
                        match event {
                            Event::Catalog { models, commands } => {
                                found = Some((models, commands));
                                break;
                            }
                            Event::Exit { .. } => break,
                            _ => {}
                        }
                    }
                    process.terminate();
                    found
                })
                .await;
            let Some((models, commands)) = catalog else { return };
            let _ = this.update(cx, |ws, cx| {
                if std::env::var("YH_DATA_DIR").is_ok() {
                    eprintln!("COMMANDS loaded for {project}: {}", commands.as_array().map_or(0, |a| a.len()));
                }
                ws.note_catalog(&models, &commands, &project, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// What the "/" menu offers in a project: the app's own, then the CLI's.
    pub fn menu_commands(&self, project_id: &str) -> Vec<SlashCommand> {
        let own = app_commands();
        let theirs = self.commands(project_id).iter().filter(|c| !own.iter().any(|o| o.name == c.name) && !TERMINAL_ONLY.contains(&c.name.as_str())).cloned();
        own.clone().into_iter().chain(theirs).collect()
    }

    /// Commands the agent can run itself partway through a message: skills, by what a session
    /// reported; until one has, your own commands and skills.
    /// The skills a session in the project last reported, as saved.
    fn saved_skills(&self, project_id: &str) -> Option<Vec<String>> {
        let list = self.library.setting("skills")?.get(project_id)?.as_array()?.clone();
        Some(list.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
    }

    fn chainable(&self, project_id: &str) -> std::collections::HashSet<String> {
        let commands = self.commands(project_id);
        let saved = self.composer.skills.get(project_id).cloned().or_else(|| self.saved_skills(project_id));
        let skills = saved.as_ref();
        commands
            .iter()
            .filter(|c| match skills {
                Some(s) => s.contains(&c.name) || c.aliases.iter().any(|a| s.contains(a)),
                None => !c.builtin,
            })
            .flat_map(|c| std::iter::once(c.name.clone()).chain(c.aliases.iter().cloned()))
            .chain(skills.into_iter().flatten().cloned())
            .collect()
    }

    /// What the "/" menu offers partway through a message.
    pub fn chainable_commands(&self, project_id: &str) -> Vec<SlashCommand> {
        let known = self.chainable(project_id);
        self.commands(project_id).iter().filter(|c| known.contains(&c.name)).cloned().collect()
    }

    /// Your message as the agent gets it: with skills named later in it (`then /review`), a note
    /// to run them as they come up. Claude Code only runs the one a message starts with.
    pub fn message_for_agent(&self, text: &str, project_id: &str) -> String {
        let chained = yh_core::claude::chained_commands(text, &self.chainable(project_id));
        match yh_core::claude::chain_note(&chained) {
            Some(note) => format!("{text}\n\n{note}"),
            None => text.to_string(),
        }
    }

    /// Commands the app does itself, so its own state stays true: `/btw` opens the side chat,
    /// `/mcp` the servers sheet, `/model` and `/effort` go through the pickers' own calls.
    /// `/rename` goes to Claude Code, whose new title comes back (`note_composer_event`); the
    /// rest (`/clear`, `/compact`, `/context`, `/usage`…) are Claude Code's.
    fn handle_command(&mut self, thread: &Entity<ThreadState>, text: &str, cx: &mut Context<Self>) -> CommandHandling {
        let Some((name, args)) = yh_core::claude::parse_command(text) else { return CommandHandling::NotOurs };
        let id = thread.read(cx).id().to_string();
        match name.as_str() {
            // Already on the side: the question is all that's left.
            "btw" if id.starts_with(SIDE) => {
                if args.is_empty() {
                    CommandHandling::Done
                } else {
                    CommandHandling::Send(args)
                }
            }
            "btw" => {
                self.ask_aside(&id, &args, cx);
                CommandHandling::Done
            }
            // `/mcp` opens the servers sheet; `/mcp enable|disable <server|all>` and `/mcp
            // reconnect <server>` are done here, as in the terminal, with a note of what happened.
            "mcp" => {
                let (verb, target) = args.split_once(char::is_whitespace).map(|(v, t)| (v.to_lowercase(), t.trim().to_string())).unwrap_or((args.to_lowercase(), String::new()));
                if !["enable", "disable", "reconnect"].contains(&verb.as_str()) || target.is_empty() {
                    self.open_mcp(cx);
                } else {
                    self.mcp_command(thread, &verb, &target, cx);
                }
                CommandHandling::Done
            }
            "model" if !args.is_empty() => {
                let fallback = [("default", "Default"), ("haiku", "Haiku"), ("sonnet", "Sonnet"), ("opus", "Opus"), ("fable", "Fable")];
                let found = self
                    .models
                    .iter()
                    .find(|m| m.value.eq_ignore_ascii_case(&args) || m.display_name.eq_ignore_ascii_case(&args))
                    .map(|m| (m.value.clone(), m.display_name.clone()))
                    .or_else(|| fallback.iter().find(|(v, _)| v.eq_ignore_ascii_case(&args)).map(|(v, n)| (v.to_string(), n.to_string())));
                let Some((value, label)) = found else { return CommandHandling::NotOurs };
                self.set_model(&id, (value != "default").then_some(value), cx);
                self.composer_notice(thread, &format!("Model set to {label}"), cx);
                CommandHandling::Done
            }
            "effort" if !args.is_empty() => {
                let Some((level, label)) = EFFORTS.iter().find(|(v, _)| v.eq_ignore_ascii_case(&args)) else { return CommandHandling::NotOurs };
                self.set_effort(&id, Some(level.to_string()), cx);
                self.composer_notice(thread, &format!("Reasoning effort set to {label}"), cx);
                CommandHandling::Done
            }
            "rename" => {
                self.composer.renames.insert(id);
                CommandHandling::NotOurs
            }
            _ => CommandHandling::NotOurs,
        }
    }

    /// Opens the side chat in the right sidebar and asks it `question`, if there is one. It
    /// starts from a copy of this conversation as it is now.
    pub fn ask_aside(&mut self, parent: &str, question: &str, cx: &mut Context<Self>) {
        let Some(chat) = self.side_chat(parent, cx) else { return };
        self.show_panel(Panel::SideChat, cx);
        if !question.is_empty() {
            self.send(&chat, question, false, cx);
        }
    }

    /// `/mcp enable|disable|reconnect <server|all>` through the thread's `claude`, one server after
    /// another; "Turned on MCP: a, b", and what failed in the error colour.
    fn mcp_command(&mut self, thread: &Entity<ThreadState>, verb: &str, target: &str, cx: &mut Context<Self>) {
        let (verb, target) = (verb.to_string(), target.to_string());
        let weak = thread.downgrade();
        cx.spawn(async move |this, cx| {
            let Some(thread) = weak.upgrade() else { return };
            let names: Vec<String> = if target == "all" && verb != "reconnect" {
                let Ok(status) = this.update(cx, |ws, cx| ws.mcp(&thread, serde_json::json!({ "subtype": "mcp_status" }), cx)) else { return };
                match status.await {
                    Ok(v) => crate::ui::mcp::parse_status(&v).into_iter().map(|s| s.name).collect(),
                    Err(e) => {
                        let _ = this.update(cx, |ws, cx| ws.composer_error(&thread, &format!("Couldn't {verb} MCP servers: {e}"), cx));
                        return;
                    }
                }
            } else {
                vec![target.clone()]
            };
            let (mut done, mut failed) = (vec![], vec![]);
            for name in names {
                let request = match verb.as_str() {
                    "reconnect" => serde_json::json!({ "subtype": "mcp_reconnect", "serverName": name }),
                    v => serde_json::json!({ "subtype": "mcp_toggle", "serverName": name, "enabled": v == "enable" }),
                };
                let Ok(task) = this.update(cx, |ws, cx| ws.mcp(&thread, request, cx)) else { return };
                match task.await {
                    Ok(_) => done.push(name),
                    Err(e) => failed.push(format!("{name}: {e}")),
                }
            }
            let past = match verb.as_str() {
                "enable" => "Turned on",
                "disable" => "Turned off",
                _ => "Reconnected",
            };
            let _ = this.update(cx, |ws, cx| {
                if !done.is_empty() {
                    ws.composer_notice(&thread, &format!("{past} MCP: {}", done.join(", ")), cx);
                }
                if !failed.is_empty() {
                    ws.composer_error(&thread, &format!("Couldn't {verb} MCP {}", failed.join("; ")), cx);
                }
                ws.refresh_mcp_attention(&thread, cx);
            });
        })
        .detach();
    }

    /// A note in the error colour from the app, saved with its rows.
    fn composer_error(&mut self, thread: &Entity<ThreadState>, text: &str, cx: &mut Context<Self>) {
        let id = thread.read(cx).id().to_string();
        self.command(&id, Command::Notice { thread: id.clone(), text: text.to_string(), error: true }, cx);
    }

    /// A note in the thread from the app (`Model set to Haiku`), saved with its rows.
    fn composer_notice(&mut self, thread: &Entity<ThreadState>, text: &str, cx: &mut Context<Self>) {
        let id = thread.read(cx).id().to_string();
        self.command(&id, Command::Notice { thread: id.clone(), text: text.to_string(), error: false }, cx);
    }

    /// What the workspace keeps from a thread's events for the composer.
    fn note_composer_event(&mut self, thread: &Entity<ThreadState>, event: &Event, cx: &mut Context<Self>) {
        let id = thread.read(cx).id().to_string();
        match event {
            // Only for your `/rename`: a title the CLI picks itself doesn't replace ours.
            Event::TitleChanged(title) => {
                if self.composer.renames.remove(&id) {
                    self.rename(&id, title, cx);
                }
            }
            Event::Skills(skills) => {
                let project = thread.read(cx).meta.project_id.clone();
                if self.saved_skills(&project).as_ref() != Some(skills) {
                    // Saved as 0.0.x saved them (`skills`, by project).
                    let mut all = match self.library.setting("skills") {
                        Some(Value::Object(m)) => m,
                        _ => serde_json::Map::new(),
                    };
                    all.insert(project.clone(), serde_json::json!(skills));
                    self.library.set_setting("skills", &Value::Object(all));
                }
                self.composer.skills.insert(project, skills.clone());
            }
            // Remembered by model (`contextWindows`, as 0.0.x kept them).
            Event::ContextWindows(windows) => {
                let mut all = match self.library.setting("contextWindows") {
                    Some(Value::Object(m)) => m,
                    _ => serde_json::Map::new(),
                };
                let before = all.clone();
                for (model, window) in windows {
                    all.insert(model.clone(), (*window).into());
                }
                if all != before {
                    self.library.set_setting("contextWindows", &Value::Object(all));
                }
            }
            _ => {}
        }
    }

    /// A turn's end: how full the context is, saved with the thread; the MCP servers asked
    /// again (only of a `claude` that's running anyway, never one started for it).
    fn composer_turn_ended(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let (id, tokens, window) = {
            let t = thread.read(cx);
            (t.id().to_string(), t.run.composer.context_tokens, t.run.composer.context_window)
        };
        thread.update(cx, |t, _| {
            t.meta.context_tokens = tokens;
            t.meta.context_window = window;
        });
        self.library.update_thread(&id, |r| {
            match tokens {
                Some(t) => r.insert("contextTokens".into(), t.into()),
                None => r.remove("contextTokens"),
            };
            if let Some(w) = window {
                r.insert("contextWindow".into(), w.into());
            }
        });
        self.refresh_mcp_attention(thread, cx);
    }

    /// The thread's MCP servers from its running `claude` (`mcp_status`), for the attention chip.
    pub fn refresh_mcp_attention(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let id = thread.read(cx).id().to_string();
        if id.starts_with(SIDE) || id.starts_with(MEMORY_AGENT) || !self.pids.contains_key(&id) {
            return;
        }
        let task = self.mcp(thread, serde_json::json!({ "subtype": "mcp_status" }), cx);
        let weak = thread.downgrade();
        cx.spawn(async move |this, cx| {
            let Ok(reply) = task.await else { return };
            let servers: Vec<(String, String, String)> = crate::ui::mcp::parse_status(&reply).into_iter().map(|s| (s.name, s.status, s.scope)).collect();
            let _ = this.update(cx, |ws, cx| {
                if let Some(t) = weak.upgrade() {
                    ws.note_mcp_servers(&t, servers, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The `/mcp` sheet heard the servers: the chip follows.
    pub fn note_mcp_servers(&mut self, thread: &Entity<ThreadState>, servers: Vec<(String, String, String)>, cx: &mut Context<Self>) {
        if thread.read(cx).run.composer.mcp_servers != servers {
            let id = thread.read(cx).id().to_string();
            thread.update(cx, |t, _| t.run.composer.mcp_servers = servers.clone());
            self.command(&id, Command::SetMcpServers { thread: id.clone(), servers }, cx);
            cx.notify();
        }
    }

    /// The thread's context window: as its last turn reported, else what was seen for its model.
    pub fn context_window_of(&self, thread: &ThreadState) -> Option<u64> {
        thread.run.composer.context_window.or_else(|| {
            let model = self.model_of(thread)?;
            let all = self.library.setting("contextWindows")?;
            let all = all.as_object()?;
            all.iter().find(|(k, _)| k.contains(&model)).and_then(|(_, v)| v.as_u64())
        })
    }

    /// Takes back a message still waiting for Claude's current step; with `to_composer` its text
    /// goes back into the composer once Claude Code lets go of it.
    pub fn withdraw(&mut self, thread: &Entity<ThreadState>, message: &str, to_composer: bool, cx: &mut Context<Self>) {
        let id = thread.read(cx).id().to_string();
        self.command(&id, Command::Withdraw { thread: id.clone(), message: message.to_string(), to_composer }, cx);
        cx.notify();
    }

    /// A queued message, right away: Claude is stopped, and the CLI hands it the message.
    pub fn send_now(&mut self, thread: &Entity<ThreadState>, message: &str, cx: &mut Context<Self>) {
        let id = thread.read(cx).id().to_string();
        self.command(&id, Command::SendNow { thread: id.clone(), message: message.to_string() }, cx);
        cx.notify();
    }
}

/// Servers waiting on you: to sign in, or to fix. Not claude.ai connectors you haven't
/// connected, which stay that way on purpose more often than not.
pub fn mcp_needing_you(servers: &[(String, String, String)]) -> Vec<(String, bool)> {
    servers
        .iter()
        .filter(|(name, status, scope)| {
            let claude_ai = scope == "claudeai" || name.starts_with("claude.ai ");
            status == "failed" || (status == "needs-auth" && !claude_ai)
        })
        .map(|(name, status, _)| (name.clone(), status == "failed"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::quit_words;

    #[test]
    fn the_quit_question_counts_sub_agents_with_commands() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(quit_words(&[], &[], 0), None);
        assert_eq!(quit_words(&s(&["Build"]), &[], 0).unwrap(), ("A background command is running".into(), "“Build” stops if you quit YourHarness.".into()));
        assert_eq!(quit_words(&[], &s(&["Survey"]), 0).unwrap(), ("A sub-agent is running".into(), "“Survey” stops if you quit YourHarness.".into()));
        assert_eq!(quit_words(&[], &s(&["a", "b"]), 0).unwrap().0, "2 sub-agents are running");
        assert_eq!(quit_words(&s(&["x"]), &s(&["a", "b"]), 0).unwrap().0, "1 command and 2 sub-agents are running");
        assert_eq!(
            quit_words(&s(&["x", "y"]), &s(&["a"]), 1).unwrap(),
            ("2 commands and 1 sub-agent are running".into(), "They stop if you quit YourHarness, with the command the sub-agent started.".into())
        );
    }
}

// MARK: The engine

impl Workspace {
    /// A command about a thread to the engine, telling it of the thread first.
    fn command(&mut self, thread_id: &str, command: Command, cx: &mut Context<Self>) {
        if !self.opened.contains(thread_id) {
            if let Some(t) = self.thread(thread_id) {
                let t = t.read(cx);
                self.engine.send(Command::Open { thread: thread_id.to_string(), session_id: t.run.session_id.clone(), context_tokens: t.run.composer.context_tokens, context_window: t.run.composer.context_window });
            }
            self.opened.insert(thread_id.to_string());
        }
        self.engine.send(command);
    }

    /// The engine lets go of a thread: its `claude` ends, its run is read again when next needed.
    fn forget_in_engine(&mut self, thread_id: &str) {
        if self.opened.remove(thread_id) || self.pids.contains_key(thread_id) {
            self.engine.send(Command::Forget { thread: thread_id.to_string() });
        }
        self.pids.remove(thread_id);
    }

    /// The composer took back the messages the engine handed back (`composer.restored`).
    pub fn took_restored(&mut self, thread: &Entity<ThreadState>, ids: Vec<String>, cx: &mut Context<Self>) {
        if ids.is_empty() {
            return;
        }
        let id = thread.read(cx).id().to_string();
        self.command(&id, Command::TookRestored { thread: id.clone(), ids }, cx);
    }

    /// What the engine said since the last frame: each thread's updates folded into one.
    fn on_outputs(&mut self, outputs: Vec<Output>, cx: &mut Context<Self>) {
        let mut updates: Vec<ThreadUpdate> = vec![];
        for output in outputs {
            match output {
                Output::Thread(u) => match updates.iter_mut().find(|x| x.thread == u.thread) {
                    Some(x) => x.merge(u),
                    None => updates.push(u),
                },
                Output::NeedsLaunch { thread } => {
                    let launch = match self.thread(&thread) {
                        Some(t) => {
                            let effort = self.effort_of(t.read(cx));
                            let (options, note) = self.launch_options(&t, cx);
                            Command::Launch { thread: thread.clone(), options, effort, note }
                        }
                        None => Command::Launch { thread: thread.clone(), options: Err("The thread is gone.".into()), effort: None, note: None },
                    };
                    self.engine.send(launch);
                }
                Output::Snapshot { .. } => {}
                Output::Runtime { thread, pid } => match pid {
                    Some(pid) => {
                        self.pids.insert(thread, pid);
                    }
                    None => {
                        self.pids.remove(&thread);
                    }
                },
            }
        }
        for u in updates {
            self.on_update(u, cx);
        }
    }

    /// One thread's update: what the workspace keeps from its events, its rows and state into
    /// the thread, then a turn's end, the badge and notifications.
    fn on_update(&mut self, u: ThreadUpdate, cx: &mut Context<Self>) {
        let Some(thread) = self.thread(&u.thread) else { return };
        let id = u.thread.clone();
        for e in &u.events {
            match e {
                Event::Catalog { models, commands } => {
                    let project = thread.read(cx).meta.project_id.clone();
                    self.note_catalog(models, commands, &project, cx);
                }
                Event::Limits(info) => self.note_limits(info, cx),
                Event::Assistant { blocks, .. } => {
                    for block in blocks {
                        if let yh_core::claude::Block::Tool { name, input, .. } = block {
                            self.note_footprint(&thread, name, input, cx);
                        }
                    }
                }
                Event::Session { id: sid } => {
                    let sid = sid.clone();
                    self.library.update_thread(&id, |r| {
                        r.insert("sessionID".into(), sid.into());
                    });
                }
                Event::TitleChanged(_) | Event::Skills(_) | Event::ContextWindows(_) => self.note_composer_event(&thread, e, cx),
                _ => {}
            }
        }
        let ended = u.turn_ended.is_some();
        thread.update(cx, |t, cx| {
            t.apply_update(u.changes, u.live, u.plan, u.session_reset, ended);
            cx.notify();
        });
        if u.pulls {
            crate::state::pulls::note_activity(&id);
        }
        if let Some((count, ms)) = u.batch {
            probe::note_batch(ms, count);
        }
        // Sub-agents came or went: Keep Awake follows them as it follows turns.
        if u.agents_changed {
            self.note_working(cx);
        }
        if let Some(end) = u.turn_ended {
            self.turn_ended(&thread, end, cx);
            self.composer_turn_ended(&thread, cx);
        }
        // The window around the thread draws again only when what it shows changed (status,
        // cards, a turn's end): the thread's rows and status redraw through the thread itself.
        // On every update, 20 threads streaming redrew the whole window each frame.
        let chrome = u.chrome || u.turn_ended.is_some() || u.agents_changed;
        if u.chrome {
            self.note_attention(&thread, cx);
            self.answer_for_tests(&thread, cx);
        }
        if chrome {
            cx.notify();
        }
    }
}

// MARK: The server

/// `YH_SERVER=<port>` (`1`: 7878): the server for the app's clients (`yh_server`, the phone app),
/// on loopback, its token `YH_SERVER_TOKEN` or made up. Prints `SERVER <url>` and writes it to
/// `server.json` in the data folder (only you can read it). Its actions come back here.
fn start_server(engine: &yh_engine::EngineHandle) -> Option<flume::Receiver<yh_server::Asked>> {
    let asked = std::env::var("YH_SERVER").ok()?;
    let port: u16 = if asked == "1" { 7878 } else { asked.parse().ok()? };
    let token = std::env::var("YH_SERVER_TOKEN").ok().filter(|t| !t.is_empty()).unwrap_or_else(|| yh_core::library::new_uuid().replace('-', "").to_lowercase());
    let listener = match std::net::TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("SERVER can't listen on 127.0.0.1:{port}: {e}");
            return None;
        }
    };
    let data = data_directory();
    let url = format!("ws://127.0.0.1:{port}/?token={token}");
    eprintln!("SERVER {url}");
    let file = data.join("server.json");
    if std::fs::write(&file, serde_json::json!({ "url": url }).to_string()).is_ok() {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
    }
    let (actions, remote) = flume::unbounded();
    let links = yh_server::Links { engine: engine.clone(), actions };
    // `YH_SERVER_NAME`: what phones call this Mac (tests run two on one Mac); else its name.
    let name = std::env::var("YH_SERVER_NAME").ok().filter(|s| !s.is_empty());
    let name = name.unwrap_or_else(|| std::process::Command::new("scutil").args(["--get", "ComputerName"]).output().ok().and_then(|o| String::from_utf8(o.stdout).ok()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "Mac".into()));
    std::thread::Builder::new()
        .name("server".into())
        .spawn(move || {
            if let Err(e) = yh_server::serve(listener, data, yh_server::Options { token: Some(token), name }, Some(links)) {
                eprintln!("SERVER stopped: {e}");
            }
        })
        .ok()?;
    Some(remote)
}

impl Workspace {
    /// What a client of the server asked (the phone): taken as the same click here would be, and
    /// answered with how it went.
    fn remote_action(&mut self, asked: yh_server::Asked, cx: &mut Context<Self>) {
        if std::env::var("YH_DATA_DIR").is_ok() {
            let mut line = format!("{:?}", asked.action);
            line.truncate(300);
            eprintln!("REMOTE {line}");
        }
        let result = self.take_remote(asked.action, cx);
        let _ = asked.reply.send(result);
    }

    fn take_remote(&mut self, action: yh_server::Action, cx: &mut Context<Self>) -> Result<Option<String>, String> {
        use yh_server::Action as A;
        if let A::NewThread { project, text } = &action {
            if self.project(project).is_none() {
                return Err("No such project.".into());
            }
            let id = self.new_thread(project, cx);
            if let (Some(text), Some(thread)) = (text.as_deref().filter(|t| !t.trim().is_empty()), self.thread(&id)) {
                self.send(&thread, text, false, cx);
            }
            return Ok(Some(id));
        }
        let thread_id = match &action {
            A::AskAside { thread, .. } | A::Send { thread, .. } | A::Stop { thread } | A::Respond { thread, .. } | A::Withdraw { thread, .. } | A::SendNow { thread, .. } | A::Answer { thread, .. } | A::SkipQuestion { thread, .. } | A::ApprovePlan { thread, .. } | A::KeepPlanning { thread, .. } | A::AnswerQuestion { thread, .. } | A::SetModel { thread, .. } | A::SetEffort { thread, .. } | A::SetMode { thread, .. } | A::Rename { thread, .. } | A::Pin { thread, .. } | A::Settle { thread, .. } | A::Delete { thread } => thread.clone(),
            A::NewThread { .. } => unreachable!("taken above"),
        };
        let Some(thread) = self.thread(&thread_id) else { return Err("No such thread.".into()) };
        let request = |id: &str, cx: &App| thread.read(cx).run.permissions.iter().find(|p| p.id == id).cloned();
        match action {
            A::Send { text, now, images, .. } => {
                if images.is_empty() {
                    self.send(&thread, &text, now, cx);
                } else {
                    let names: Vec<String> = images.iter().filter_map(|(_, bytes)| yh_core::attachments::save(&self.data_dir, &thread_id, bytes)).collect();
                    if names.len() != images.len() {
                        return Err("A picture couldn't be read.".into());
                    }
                    self.send_with_images(&thread, &text, names, now, cx);
                }
            }
            A::Stop { .. } => self.stop(&thread, &Default::default(), cx),
            A::AskAside { text, .. } => self.ask_aside(&thread_id, &text, cx),
            A::Respond { request_id, allow, .. } => {
                request(&request_id, cx).ok_or("That request isn't waiting any more.")?;
                self.respond_to(&thread, &request_id, if allow { Decision::allow() } else { Decision::deny() }, cx);
            }
            A::Withdraw { message, .. } => self.withdraw(&thread, &message, false, cx),
            A::SendNow { message, .. } => self.send_now(&thread, &message, cx),
            A::Answer { request_id, answers, .. } => {
                let p = request(&request_id, cx).ok_or("That question isn't waiting any more.")?;
                let mut input: Value = serde_json::from_str(&p.input).unwrap_or(serde_json::json!({}));
                input["answers"] = Value::Object(answers.into_iter().map(|(q, a)| (q, Value::from(a))).collect());
                self.respond_to(&thread, &request_id, Decision::Allow { updated_input: Some(input.to_string()), mode: None }, cx);
            }
            A::SkipQuestion { request_id, .. } => {
                request(&request_id, cx).ok_or("That question isn't waiting any more.")?;
                self.respond_to(&thread, &request_id, Decision::Deny { message: crate::ui::composer::SKIP_MESSAGE.into() }, cx);
            }
            A::ApprovePlan { request_id, mode, feedback, .. } => {
                request(&request_id, cx).ok_or("That plan isn't waiting any more.")?;
                let project = thread.read(cx).meta.project_id.clone();
                self.set_mode(&project, &mode, cx);
                self.respond_to(&thread, &request_id, Decision::Allow { updated_input: None, mode: Some(control_mode(&mode).into()) }, cx);
                if let Some(feedback) = feedback.filter(|f| !f.trim().is_empty()) {
                    self.send(&thread, &feedback, false, cx);
                }
            }
            A::KeepPlanning { request_id, feedback, .. } => {
                request(&request_id, cx).ok_or("That plan isn't waiting any more.")?;
                let message = crate::ui::composer::keep_planning_message(feedback.as_deref().unwrap_or(""));
                self.respond_to(&thread, &request_id, Decision::Deny { message }, cx);
            }
            A::AnswerQuestion { request_id, accept, .. } => {
                let asked = thread.read(cx).run.elicitations.iter().find(|e| e.id == request_id).cloned().ok_or("That question isn't waiting any more.")?;
                let content = (accept && asked.mode == "form").then(|| crate::ui::composer::elicit_defaults(&asked.schema));
                self.answer_elicitation_id(&thread, &request_id, if accept { "accept" } else { "decline" }, content, cx);
            }
            A::SetModel { model, .. } => self.set_model(&thread_id, model, cx),
            A::SetEffort { effort, .. } => self.set_effort(&thread_id, effort, cx),
            A::SetMode { mode, .. } => {
                let project = thread.read(cx).meta.project_id.clone();
                self.set_mode(&project, &mode, cx);
            }
            A::Rename { title, .. } => {
                let title = title.trim();
                if title.is_empty() {
                    return Err("A title can't be empty.".into());
                }
                self.rename(&thread_id, title, cx);
            }
            A::Pin { on, .. } => self.set_pinned(&thread_id, on, cx),
            A::Settle { on, .. } => self.set_settled(&thread_id, on, cx),
            A::Delete { .. } => self.delete(&thread_id, cx),
            A::NewThread { .. } => unreachable!("taken above"),
        }
        Ok(None)
    }
}
