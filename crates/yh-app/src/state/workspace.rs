//! What the app knows and does, with no drawing: projects, threads, which one is open, each
//! thread's `claude`, and every request the UI makes (send, stop, allow, pin, rename…). Views
//! read it and call it; it tells them what changed with `WorkspaceEvent`s and `notify`.
//! The port of the web rewrite's `Backend` and `ThreadRuntime` (legacy/Sources/HarnessBackend).

use crate::probe;
use crate::state::thread::ThreadState;
use crate::state::thread_tools::{self, ToolRequest};
use gpui_kit::{App, AppContext, Context, Entity, EventEmitter, Task, WeakEntity};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use yh_core::claude::{ClaudeProcess, Decision, Event, LaunchOptions, ModelInfo, SlashCommand};
use yh_core::library::{cli_mode, control_mode, data_directory, iso_now, Library, ProjectMeta, ThreadMeta};

pub const PAGE: usize = 160;
/// A thread's `claude` is let go of after this long without a turn, and resumed on the next.
const IDLE_LIMIT: Duration = Duration::from_secs(600);

pub enum WorkspaceEvent {
    /// A thread was opened.
    Selected(String),
    /// A thread went away.
    Removed(String),
    /// A page (Memory) took the place of the thread.
    PageOpened,
    /// `/mcp` was typed: the MCP sheet opens for the open thread.
    OpenMcp,
}

/// One thread's `claude`, between turns too.
struct Runtime {
    process: ClaudeProcess,
    /// Events from an older process are ignored.
    generation: u64,
    /// Effort is a launch option: a different one starts a new process.
    effort: Option<String>,
    idle: Option<Task<()>>,
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

/// A side chat's id: this, then its thread's.
pub const SIDE: &str = "side:";
/// Settings' hidden thread in Home's folder (Settings → MCP): this, then the project.
pub const SETTINGS: &str = "settings:";
/// A memory specialist's id: this, its role (`importer`, `reconciler`), `:`, its project.
pub const MEMORY_AGENT: &str = "memory:";

/// What a side chat's agent is told.
const SIDE_CHAT_PROMPT: &str = "This is a side conversation. You are a copy of the conversation above, opened so the user can ask something on the side without interrupting or adding to the main thread; nothing said here reaches it. Answer the question directly and briefly. You have Read, Glob, Grep, Bash, WebFetch and WebSearch: use them when the answer needs a fresh look rather than what the conversation already says. The app asks the user to approve commands that could change something. You have no tools to edit files: if the answer needs changes, say what the main thread should do.";

/// A thread action that Undo takes back.
pub enum Undo {
    Delete { entity: Entity<ThreadState>, record: serde_json::Map<String, Value>, index: usize, was_selected: bool },
    Unpin(String),
    Settle(String),
}

pub struct Workspace {
    pub library: Arc<Library>,
    pub data_dir: PathBuf,
    pub projects: Vec<ProjectMeta>,
    pub threads: Vec<Entity<ThreadState>>,
    by_id: HashMap<String, Entity<ThreadState>>,
    pub selected: Option<String>,
    runtimes: HashMap<String, Runtime>,
    generation: u64,
    /// The CLI's models, as it last listed them (saved as `claudeModels`).
    pub models: Vec<ModelInfo>,
    /// Usage limits as Claude last reported them.
    pub limits: Option<Value>,
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
        let mut workspace = Workspace {
            projects: library.projects.clone(),
            data_dir: data_directory(),
            library,
            threads,
            by_id,
            selected: None,
            runtimes: HashMap::new(),
            generation: 0,
            models,
            limits: None,
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
            undo_serial: 0,
            pending_purge: vec![],
            shows_inspector: false,
            panel: Panel::SideChat,
            memory_revision: 0,
            scans: HashMap::new(),
            scanning: Default::default(),
            scanned_at: HashMap::new(),
            title_asked_at: HashMap::new(),
        };
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
        workspace
    }

    /// A data folder without a Home project gets one, as the Swift app makes it: your home
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
    pub fn set_home_folder(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(id) = self.home_project() else { return };
        self.library.update_project(&id, |p| {
            p.insert("path".into(), path.into());
        });
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == id) {
            p.path = path.to_string();
        }
        let ids: Vec<String> = self.threads.iter().filter(|t| t.read(cx).meta.project_id == id).map(|t| t.read(cx).id().to_string()).collect();
        for thread in ids {
            if !self.thread(&thread).is_some_and(|t| t.read(cx).running) {
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
        self.threads.iter().filter(|t| t.read(cx).running).count()
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
            let page = cx.background_executor().spawn(async move { library.page(&id, before, PAGE) }).await;
            let _ = weak.update(cx, |t, cx| {
                if !t.loading {
                    return; // let go of meanwhile
                }
                if t.total.is_none() {
                    t.total = Some(page.total);
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

    /// The transcript was let go of on screen: save its rows and keep only what a running turn
    /// needs.
    pub fn release(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        self.save_rows(thread, cx);
        thread.update(cx, |t, cx| {
            t.in_deck = false;
            if t.running {
                t.trim(200);
            } else {
                t.unload();
            }
            cx.notify();
        });
    }

    // MARK: Threads and projects

    pub fn new_thread(&mut self, project_id: &str, cx: &mut Context<Self>) -> String {
        self.new_count += 1;
        // Saved in harness.sqlite right away, as the Swift app does; `new:` only without one.
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
            self.end_runtime(id, cx);
        }
        let now = iso_now();
        self.update_meta(id, cx, |m, r| {
            if on {
                m.settled_at = Some(yh_core::library::now_ms());
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
        self.end_runtime(id, cx);
        crate::browser::Browsers::close_thread(id);
        let index = self.threads.iter().position(|t| t.read(cx).id() == id);
        let entity = index.map(|i| self.threads.remove(i));
        self.by_id.remove(id);
        let record = self.library.take_thread(id);
        let was_selected = self.selected.as_deref() == Some(id);
        if was_selected {
            self.selected = None;
        }
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

    fn offer_undo(&mut self, message: String, action: Undo, cx: &mut Context<Self>) {
        self.undo_serial += 1;
        let serial = self.undo_serial;
        self.undo = Some((message, action));
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
            Undo::Settle(id) => self.set_settled(&id, false, cx),
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

    /// Keep Awake follows whether Claude works anywhere; its one timer is for a deadline.
    pub fn note_working(&mut self, cx: &mut Context<Self>) {
        let working = self.threads.iter().any(|t| t.read(cx).running) || self.by_id.values().any(|t| t.read(cx).running);
        self.keep_awake.set_working(working);
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

    /// A tool call's folders join its thread's footprint (saved as the Swift app saves it).
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
                    copy
                })
                .await;
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
                t.session_id = None;
                cx.notify();
            });
        }
        cx.notify();
    }

    /// Your rules for generated titles (`titlePrompt`, the Swift app's key; `titleRules` before).
    pub fn title_prompt(&self) -> Option<String> {
        ["titlePrompt", "titleRules"].iter().find_map(|k| self.library.setting(k).and_then(|r| r.as_str().map(str::to_string))).filter(|r| !r.trim().is_empty())
    }

    /// Settles threads idle for `autoSettleDays` (3 by default; 0 never), the Swift app's way:
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
                    && !t.running
                    && t.permission.is_none()
                    && self.selected.as_deref() != Some(id.as_str())
                    && record.get("autoSettle").and_then(|v| v.as_bool()) != Some(false)
                    && record.get("reactivatedAt").is_none_or(|v| v.is_null())
                    && (t.session_id.is_some() || t.title != "New thread")
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

    /// Unpins with Undo, or pins (from the sidebar).
    pub fn toggle_pin(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(t) = self.thread(id) else { return };
        let (pinned, title) = (t.read(cx).meta.pinned, t.read(cx).title.clone());
        self.set_pinned(id, !pinned, cx);
        if pinned {
            self.offer_undo(format!("Unpinned “{title}”"), Undo::Unpin(id.to_string()), cx);
        }
    }

    /// Settles with Undo, or brings back (from the sidebar).
    pub fn toggle_settled(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(t) = self.thread(id) else { return };
        let (settled, title) = (t.read(cx).meta.settled_at.is_some(), t.read(cx).title.clone());
        self.set_settled(id, !settled, cx);
        if !settled {
            self.offer_undo(format!("Settled “{title}”"), Undo::Settle(id.to_string()), cx);
        }
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
        if let Some(rt) = self.runtimes.get_mut(id) {
            rt.process.set_model(model.as_deref().unwrap_or(""));
        }
    }

    /// Applies from the next start of its `claude` (effort is a launch option).
    pub fn set_effort(&mut self, id: &str, effort: Option<String>, cx: &mut Context<Self>) {
        self.update_meta(id, cx, |m, r| {
            m.effort = effort.clone();
            r.insert("effort".into(), effort.map(Value::from).unwrap_or(Value::Null));
        });
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
            if let Some(rt) = self.runtimes.get_mut(&id) {
                rt.process.set_mode(control_mode(mode));
            }
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
        let text = text.trim().to_string();
        if text.is_empty() && images.is_empty() {
            return;
        }
        let (id, running, title, chosen) = {
            let t = thread.read(cx);
            (t.id().to_string(), t.running, t.title.clone(), t.meta.title_chosen)
        };
        // The next row's number: its rows so far, before the first new one.
        if thread.read(cx).total.is_none() {
            let total = self.library.db.as_ref().map_or(0, |db| db.transcript_count(&id));
            thread.update(cx, |t, _| t.total = Some(total));
        }
        let first_message = thread.read(cx).total == Some(0);
        let message_id = yh_core::library::new_uuid();
        let delivery = running.then_some(if now { "interrupted" } else { "steered" });
        thread.update(cx, |t, cx| {
            t.add_user(&text, &images, &message_id, running && !now, delivery);
            t.running = true;
            t.meta.turn_in_progress = true;
            t.meta.draft = None;
            if title == "New thread" && !chosen {
                // Named after its first message until a title is generated or chosen.
                t.title = text.lines().next().unwrap_or("").trim().chars().take(60).collect();
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
        if first_message && title == "New thread" && !chosen && !text.is_empty() {
            self.generate_title(thread, &text, cx);
        }
        if let Err(error) = self.ensure_runtime(thread, cx) {
            thread.update(cx, |t, cx| {
                t.notice(&format!("Couldn't send: {error}"));
                t.running = false;
                cx.notify();
            });
            self.turn_ended(thread, cx);
            return;
        }
        if let Some(rt) = self.runtimes.get_mut(&id) {
            rt.idle = None;
            let blocks: Vec<(String, String)> = images
                .iter()
                .filter_map(|name| {
                    let bytes = std::fs::read(yh_core::attachments::path(&self.data_dir, name)).ok()?;
                    Some((yh_core::attachments::media_type(name).to_string(), yh_core::attachments::base64(&bytes)))
                })
                .collect();
            rt.process.send(&text, &blocks, &message_id, now.then_some("now"));
        }
        self.save_rows(thread, cx);
        cx.notify();
    }

    fn ensure_runtime(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) -> Result<(), String> {
        let t = thread.read(cx);
        let id = t.id().to_string();
        let effort = self.effort_of(t);
        if let Some(rt) = self.runtimes.get(&id) {
            if rt.effort == effort {
                return Ok(());
            }
        }
        self.end_runtime(&id, cx);
        let t = thread.read(cx);
        let project = self.project(&t.meta.project_id).cloned();
        let cwd = self.folder_of(t);
        let attachments = self.data_dir.join("attachments");
        let mut env = vec![];
        // Claude Code's task tools: what the plan checklist shows.
        if self.library.setting("planChecklist").and_then(|v| v.as_bool()).unwrap_or(true) {
            env.push(("CLAUDE_CODE_ENABLE_TODO_TOOLS".to_string(), "1".to_string()));
        }
        // The app's memory: its tools, and the index every agent here is given.
        let mut servers: Vec<std::sync::Arc<dyn yh_core::mcp::ToolServer>> = vec![];
        let mut prompt: Vec<String> = vec![];
        let mut allowed = vec![];
        let memory_on = self.library.setting("memoryEnabled").and_then(|v| v.as_bool()).unwrap_or(true);
        if let (true, Some(db), Some(project)) = (memory_on, self.library.db.clone(), project.as_ref()) {
            // One memory, the app's: Claude Code's own files would compete with it.
            if self.library.setting("claudeCodeMemoryOff").and_then(|v| v.as_bool()).unwrap_or(true) {
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
            let Some(db) = self.library.db.clone() else { return Err("No memory here.".into()) };
            let name = self.project(&project_id).map(|p| if self.home_project().as_deref() == Some(p.id.as_str()) { "Home".to_string() } else { p.name.clone() }).unwrap_or_default();
            let changed = self.tool_requests.clone();
            let actor = yh_core::memory::Actor { kind: if role == "importer" { "importer".into() } else { "reconciler".into() }, thread: None };
            let server = yh_core::memory_tools::MemoryTools {
                store: yh_core::memory::Store { db },
                scope: yh_core::memory::scope_of(&project_id),
                actor,
                on_change: Box::new(move || {
                    let _ = changed.send(ToolRequest::MemoryChanged);
                }),
                specialist: true,
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
            let (tx, rx) = flume::unbounded();
            let mut process = ClaudeProcess::spawn(options, tx)?;
            process.initialize();
            self.generation += 1;
            let generation = self.generation;
            self.runtimes.insert(id.clone(), Runtime { process, generation, effort, idle: None });
            self.pump(thread.clone(), id, generation, rx, cx);
            return Ok(());
        }
        // A side chat: a copy of its thread's conversation that can look but not edit and
        // leaves nothing behind. It reads freely; commands and web requests ask first.
        let side_parent = t.id().strip_prefix(SIDE).map(str::to_string);
        if let Some(parent) = &side_parent {
            let fork = self.thread(parent).and_then(|p| p.read(cx).session_id.clone());
            let mut extra = vec!["--tools".to_string(), "Read,Glob,Grep,Bash,WebFetch,WebSearch".into(), "--strict-mcp-config".into(), "--no-session-persistence".into()];
            if fork.is_some() {
                extra.push("--fork-session".into());
            }
            let t = thread.read(cx);
            let options = LaunchOptions {
                cwd,
                session_id: fork,
                model: self.model_of(t),
                mode: Some("default".into()),
                effort: effort.clone(),
                add_dirs: vec![],
                env: vec![],
                append_system_prompt: Some(SIDE_CHAT_PROMPT.into()),
                allowed_tools: vec![],
                servers: vec![],
                extra_args: extra,
            };
            let (tx, rx) = flume::unbounded();
            let mut process = ClaudeProcess::spawn(options, tx)?;
            process.initialize();
            self.generation += 1;
            let generation = self.generation;
            self.runtimes.insert(id.clone(), Runtime { process, generation, effort, idle: None });
            self.pump(thread.clone(), id, generation, rx, cx);
            return Ok(());
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
        let options = LaunchOptions {
            cwd,
            session_id: t.session_id.clone(),
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
        let (tx, rx) = flume::unbounded();
        let mut process = ClaudeProcess::spawn(options, tx)?;
        process.initialize();
        self.generation += 1;
        let generation = self.generation;
        self.runtimes.insert(id.clone(), Runtime { process, generation, effort, idle: None });
        self.pump(thread.clone(), id, generation, rx, cx);
        Ok(())
    }

    /// Ends a thread's `claude`; the next message resumes its session.
    fn end_runtime(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(rt) = self.runtimes.remove(id) else { return };
        rt.process.terminate();
        if let Some(thread) = self.thread(id) {
            if thread.read(cx).running {
                thread.update(cx, |t, cx| {
                    t.apply(Event::Exit { code: 0, tail: String::new() }, cx);
                    cx.notify();
                });
                self.turn_ended(&thread, cx);
            }
        }
    }

    /// Claude's events into the thread: what's arrived, at most once a frame, deltas joined.
    fn pump(&mut self, thread: Entity<ThreadState>, id: String, generation: u64, rx: flume::Receiver<Event>, cx: &mut Context<Self>) {
        let log = std::env::var("YH_EVENT_LOG").as_deref() == Ok("1");
        cx.spawn(async move |this, cx| {
            let mut saved_at = Instant::now();
            while let Ok(first) = rx.recv_async().await {
                let current = this.update(cx, |ws, _| ws.runtimes.get(&id).is_some_and(|r| r.generation == generation)).unwrap_or(false);
                if !current {
                    break;
                }
                let started = Instant::now();
                let mut batch: Vec<Event> = vec![first];
                while let Ok(next) = rx.try_recv() {
                    if !batch.last_mut().unwrap().merge(&next) {
                        batch.push(next);
                    }
                }
                if log {
                    for e in &batch {
                        eprintln!("event {id}: {e:?}");
                    }
                }
                let ended = batch.iter().any(|e| matches!(e, Event::Done { .. } | Event::Exit { .. }));
                let exited = batch.iter().any(|e| matches!(e, Event::Exit { .. }));
                let count = batch.len();
                // What the workspace keeps (catalog, limits, session) before the rows take the rest.
                let _ = this.update(cx, |ws, cx| {
                    for e in &batch {
                        match e {
                            Event::Catalog { models, commands } => {
                                let project = thread.read(cx).meta.project_id.clone();
                                ws.note_catalog(models, commands, &project, cx);
                            }
                            Event::Limits(info) => {
                                if ws.limits.as_ref() != Some(info) {
                                    ws.limits = Some(info.clone());
                                    cx.notify();
                                }
                            }
                            Event::Assistant { blocks, .. } => {
                                for block in blocks {
                                    if let yh_core::claude::Block::Tool { name, input, .. } = block {
                                        ws.note_footprint(&thread, name, input, cx);
                                    }
                                }
                            }
                            Event::Session { id: sid } => {
                                let sid = sid.clone();
                                ws.library.update_thread(&id, |r| {
                                    r.insert("sessionID".into(), sid.into());
                                });
                            }
                            _ => {}
                        }
                    }
                });
                let chrome = thread.update(cx, |t, cx| {
                    let mut chrome = false;
                    for event in batch {
                        chrome |= t.apply(event, cx);
                    }
                    cx.notify();
                    chrome
                });
                probe::note_batch(started.elapsed().as_secs_f64() * 1000.0, count);
                let thread2 = thread.clone();
                let id2 = id.clone();
                let _ = this.update(cx, |ws, cx| {
                    // Saved at most once a second while it streams, and when anything settles.
                    if chrome || ended || saved_at.elapsed() > Duration::from_secs(1) {
                        saved_at = Instant::now();
                        ws.save_rows(&thread2, cx);
                    }
                    if exited {
                        ws.runtimes.remove(&id2);
                    }
                    if ended {
                        ws.turn_ended(&thread2, cx);
                    }
                    if chrome {
                        cx.notify();
                        ws.note_attention(&thread2, cx);
                        ws.answer_for_tests(&thread2, cx);
                    }
                });
                if exited {
                    break;
                }
                cx.background_executor().timer(Duration::from_millis(8)).await;
            }
        })
        .detach();
    }

    /// The Dock badge counts threads waiting for you; a request to you, or a turn ending,
    /// is a notification when you're not looking at that thread.
    fn note_attention(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let waiting = self.threads.iter().filter(|t| t.read(cx).permission.is_some()).count();
        crate::platform::set_badge(waiting);
        let t = thread.read(cx);
        if t.permission.is_some() && !self.looking_at(t.id()) && self.notifications_on() {
            crate::platform::notify(&t.title, "Claude is waiting for you");
        }
    }

    fn notifications_on(&self) -> bool {
        self.library.setting("notifications").and_then(|v| v.as_bool()).unwrap_or(true)
    }

    /// The thread is open in the app you're using.
    fn looking_at(&self, id: &str) -> bool {
        // A side chat is looked at with its thread.
        let id = id.strip_prefix(SIDE).unwrap_or(id);
        self.selected.as_deref() == Some(id) && crate::platform::app_is_active()
    }

    /// The turn is over: saved, unread if you were elsewhere, its `claude` let go of later.
    fn turn_ended(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let id = thread.read(cx).id().to_string();
        thread.update(cx, |t, _| t.running = false);
        self.note_working(cx);
        let selected = self.selected.as_deref() == Some(id.strip_prefix(SIDE).unwrap_or(&id));
        if !self.looking_at(&id) && self.notifications_on() {
            crate::platform::notify(&thread.read(cx).title, "Claude finished");
        }
        let (title, session) = thread.update(cx, |t, _| {
            t.running = false;
            t.meta.turn_in_progress = false;
            t.meta.updated_at = yh_core::library::now_ms();
            if !selected {
                t.unread = true;
            }
            (t.title.clone(), t.session_id.clone())
        });
        self.library.update_thread(&id, |r| {
            r.remove("turnInProgress");
            r.insert("title".into(), title.into());
            if let Some(s) = session {
                r.insert("sessionID".into(), s.into());
            }
            r.insert("updatedAt".into(), iso_now().into());
        });
        self.save_rows(thread, cx);
        let weak = thread.downgrade();
        if let Some(rt) = self.runtimes.get_mut(&id) {
            rt.idle = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(IDLE_LIMIT).await;
                let _ = this.update(cx, |ws, cx| {
                    let waiting = weak.upgrade().is_some_and(|t| t.read(cx).running || t.read(cx).permission.is_some());
                    if !waiting {
                        ws.end_runtime(&id, cx);
                    }
                });
            }));
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
        let weak = thread.downgrade();
        cx.spawn(async move |this, cx| {
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
            let Ok(title) = result else { return };
            let _ = this.update(cx, |ws, cx| {
                let Some(thread) = weak.upgrade() else { return };
                if thread.read(cx).meta.title_chosen {
                    return;
                }
                let id = thread.read(cx).id().to_string();
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
    }

    pub fn interrupt(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let id = thread.read(cx).id().to_string();
        if let Some(rt) = self.runtimes.get_mut(&id) {
            rt.process.interrupt();
        }
    }

    /// Settles the thread's pending permission request (a tool, a question, a plan).
    pub fn respond(&mut self, thread: &Entity<ThreadState>, decision: Decision, cx: &mut Context<Self>) {
        let (id, permission) = {
            let t = thread.read(cx);
            (t.id().to_string(), t.permission.clone())
        };
        let Some(permission) = permission else { return };
        if let Some(rt) = self.runtimes.get(&id) {
            rt.process.respond(&permission.id, &permission.input, &decision);
        }
        thread.update(cx, |t, cx| {
            t.permission = None;
            cx.notify();
        });
        cx.notify();
    }

    pub fn open_mcp(&mut self, cx: &mut Context<Self>) {
        cx.emit(WorkspaceEvent::OpenMcp);
    }

    /// A control request to the thread's `claude` (started in its folder if it isn't running),
    /// and its reply: `mcp_status`, `mcp_toggle`, `mcp_authenticate`…
    pub fn mcp(&mut self, thread: &Entity<ThreadState>, request: Value, cx: &mut Context<Self>) -> Task<Result<Value, String>> {
        if let Err(e) = self.ensure_runtime(thread, cx) {
            return Task::ready(Err(e));
        }
        let id = thread.read(cx).id().to_string();
        let Some(rt) = self.runtimes.get_mut(&id) else { return Task::ready(Err("No Claude Code process.".into())) };
        let rx = rt.process.request(request);
        cx.background_executor().spawn(async move { rx.recv_timeout(Duration::from_secs(30)).map_err(|_| "Claude Code didn't answer.".to_string())? })
    }

    /// Starts the thread's `claude` over (between turns), so it reads the MCP config again; the
    /// next message resumes its session.
    pub fn restart_runtime(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        if !thread.read(cx).running {
            let id = thread.read(cx).id().to_string();
            self.end_runtime(&id, cx);
        }
    }

    /// Answers an MCP server's question: `accept` (with what you filled in), `decline` or `cancel`.
    pub fn answer_elicitation(&mut self, thread: &Entity<ThreadState>, action: &str, content: Option<Value>, cx: &mut Context<Self>) {
        let (id, asked) = {
            let t = thread.read(cx);
            (t.id().to_string(), t.elicitation.clone())
        };
        let Some(asked) = asked else { return };
        if let Some(rt) = self.runtimes.get(&id) {
            rt.process.respond_elicitation(&asked.id, action, content);
        }
        thread.update(cx, |t, cx| {
            t.elicitation = None;
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
        if let (Ok(answer), Some(e)) = (std::env::var("YH_MCP_ELICIT_ANSWER"), thread.read(cx).elicitation.clone()) {
            eprintln!("MCP elicitation from {}: {} ({}{}): {answer}", e.server, e.message, e.mode, e.url.as_deref().map(|u| format!(", {u}")).unwrap_or_default());
            let content = (answer == "accept" && e.mode == "form").then(|| crate::ui::composer::elicit_defaults(&e.schema));
            self.answer_elicitation(thread, &answer, content, cx);
        }
        let Ok(answer) = std::env::var("YH_ANSWER") else { return };
        if let Some(p) = thread.read(cx).permission.clone() {
            eprintln!("answering {} for {}: {answer}", p.id, p.tool);
            let decision = if answer == "allow" { Decision::allow() } else { Decision::deny() };
            self.respond(thread, decision, cx);
        }
    }

    /// The project's slash commands and skills.
    pub fn commands(&self, project_id: &str) -> &[SlashCommand] {
        self.commands.get(project_id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Where a thread's `claude` runs: its folder, else its project's, else the data dir's `home`.
    pub fn folder_of(&self, thread: &ThreadState) -> String {
        thread
            .meta
            .working_directory
            .clone()
            .or_else(|| self.project(&thread.meta.project_id).map(|p| p.path.clone()))
            .filter(|p| std::path::Path::new(p).is_dir())
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
            t.total = Some(0);
        });
        self.by_id.insert(format!("{SIDE}{parent}"), chat.clone());
        self.side_chats.insert(parent.to_string(), chat.clone());
        Some(chat)
    }

    /// Starts the side chat over: its `claude` ended, its conversation gone.
    pub fn clear_side_chat(&mut self, parent: &str, cx: &mut Context<Self>) {
        let id = format!("{SIDE}{parent}");
        self.end_runtime(&id, cx);
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
                // In the Swift app's shape (it decodes its own `SlashCommand`).
                let swift: Vec<Value> = list
                    .iter()
                    .filter(|c| c["name"].as_str().is_some_and(|n| !n.is_empty()))
                    .map(|c| {
                        serde_json::json!({
                            "name": c["name"], "description": c["description"].as_str().unwrap_or(""),
                            "argumentHint": c["argumentHint"].as_str().unwrap_or(""),
                            "aliases": c["aliases"].as_array().cloned().unwrap_or_default(),
                            "isBuiltin": c["builtin"].as_bool().unwrap_or(false),
                        })
                    })
                    .collect();
                all.insert(project_id.to_string(), Value::Array(swift));
                self.library.set_setting("slashCommands", &Value::Object(all));
            }
        }
        let parsed: Vec<ModelInfo> = models.as_array().map(|a| a.iter().filter_map(ModelInfo::from_json).collect()).unwrap_or_default();
        if parsed.is_empty() || parsed == self.models {
            return;
        }
        self.models = parsed;
        self.library.set_setting("claudeModels", models);
        cx.notify();
    }

    /// The thread's changed rows into harness.sqlite (in the background).
    pub fn save_rows(&self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let Some(db) = self.library.db.clone() else { return };
        let (id, total, rows) = thread.update(cx, |t, _| (t.id().to_string(), t.total, t.take_changes()));
        if rows.is_empty() || id.starts_with("new:") || id.starts_with("cc:") || id.starts_with(SIDE) || id.starts_with(MEMORY_AGENT) || id.starts_with(SETTINGS) {
            return;
        }
        if let Some(total) = total {
            db.save_rows(&id, total, rows);
        }
    }

    /// Quitting: everything saved, every `claude` ended.
    pub fn prepare_for_quit(&mut self, cx: &mut Context<Self>) {
        for id in std::mem::take(&mut self.pending_purge) {
            self.library.purge(&id);
        }
        for t in self.threads.clone() {
            self.save_rows(&t, cx);
        }
        for (_, rt) in self.runtimes.drain() {
            rt.process.terminate();
        }
        self.library.save();
        if let Some(db) = &self.library.db {
            db.flush();
        }
    }

    // MARK: Search and resuming

    /// Threads whose title has `query`, then rows that do (the full-text index), off the main
    /// thread: (thread, title, snippet; empty for a title match).
    pub fn search(&self, query: String, cx: &mut Context<Self>) -> Task<Vec<(String, String, String)>> {
        let q = query.trim().to_lowercase();
        let titles: Vec<(String, String)> = self
            .threads
            .iter()
            .map(|t| (t.read(cx).id().to_string(), t.read(cx).title.clone()))
            .collect();
        let db = self.library.db.clone();
        cx.background_executor().spawn(async move {
            if q.chars().count() < 2 {
                return vec![];
            }
            let mut out: Vec<(String, String, String)> =
                titles.iter().filter(|(_, title)| title.to_lowercase().contains(&q)).take(8).map(|(id, t)| (id.clone(), t.clone(), String::new())).collect();
            let mut seen: std::collections::HashSet<String> = out.iter().map(|(id, ..)| id.clone()).collect();
            if let Some(db) = db {
                for hit in db.search(&q, 60) {
                    if seen.insert(hit.thread.clone()) {
                        if let Some((_, title)) = titles.iter().find(|(id, _)| *id == hit.thread) {
                            let snippet = hit.snippet.split_whitespace().collect::<Vec<_>>().join(" ");
                            out.push((hit.thread, title.clone(), snippet));
                        }
                    }
                    if out.len() >= 30 {
                        break;
                    }
                }
            }
            out
        })
    }

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
        self.threads.iter().find(|t| t.read(cx).session_id.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(session))).cloned()
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
                t.session_id = Some(session.clone());
                t.meta.session_id = Some(session.clone());
            });
            if let Some(db) = &self.library.db {
                let now = yh_core::library::now_ms() / 1000.0;
                let saved: Vec<(usize, yh_core::db::Row)> =
                    rows.iter().enumerate().map(|(i, e)| (i, yh_core::recovery::row_of(e, if e.kind == yh_core::library::Kind::Tool { e.id.clone() } else { format!("h{i}") }, Some(now)))).collect();
                db.save_rows(&id, saved.len(), saved);
            }
            // Read back from what was saved, as any history.
            thread.update(cx, |t, _| t.unload());
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
                    t.total = Some(0);
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
            t.total = Some(0);
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
                                running: t.running,
                                waiting: t.permission.is_some(),
                                unread: t.unread,
                                pinned: t.meta.pinned,
                                settled: t.meta.settled_at.is_some(),
                                updated_at: t.meta.updated_at,
                                session_id: t.session_id.clone(),
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
        self.titles_rewriting.insert(caller.to_string());
        let id = caller.to_string();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { yh_core::titles::generate(&claude, path.as_deref(), &model, &prompt) }).await;
            let _ = this.update(cx, |ws, cx| {
                ws.titles_rewriting.remove(&id);
                let (Ok(title), Some(thread)) = (result, ws.thread(&id)) else { return };
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
                (t.id().to_string(), t.session_id.clone())
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
                    if thread.read(cx).running {
                        return;
                    }
                    let open = ws.selected.as_deref() == Some(id.as_str());
                    thread.update(cx, |t, cx| {
                        t.meta.turn_in_progress = false;
                        t.unload();
                        t.total = Some(count);
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

fn make_thread(meta: ThreadMeta, workspace: WeakEntity<Workspace>, cx: &mut Context<Workspace>) -> Entity<ThreadState> {
    let thread = cx.new(|_| ThreadState::new(meta));
    // Near the top of what's held: read the page before it.
    let weak = thread.downgrade();
    thread.read(cx).list.set_scroll_handler(move |event, _, cx| {
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

/// What agents are told about diagrams (the Swift app's `AgentInstructions.diagrams`).
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
