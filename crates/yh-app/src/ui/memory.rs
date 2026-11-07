//! A project's Memory page: what its agents remember (search, filter by kind, read, edit, pin,
//! archive, delete, write a new one, see how each changed), the tasks they track, and what
//! they did with memory. Shown in place of the thread when you click a project's name.
//! Memory is read off the main thread, again whenever it changes (`memory_revision`).

use crate::state::workspace::Workspace;
use crate::theme::{radius, space, text, Theme, MONO};
use crate::ui::page_icons as icons;
use crate::ui::page_kit::{self, Segment};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenuItem};
use gpui_kit::component::{Disableable, IconName, Sizable, WindowExt};
use gpui_kit::{div, prelude::*, px, App, Context, Entity, FontWeight, Hsla, SharedString, Subscription, Window};
use yh_core::memory::{Actor, Memory, Store};

const KINDS: [(&str, &str); 5] =
    [("user", "About You"), ("feedback", "Feedback"), ("project", "Project"), ("reference", "Reference"), ("task", "Task")];

fn kind_label(kind: &str) -> &'static str {
    KINDS.iter().find(|(k, _)| *k == kind).map_or("Project", |(_, l)| l)
}

fn kind_color(kind: &str, theme: &Theme) -> Hsla {
    match kind {
        "user" => gpui_kit::rgb(0xaf52de).into(),
        "feedback" => theme.warning,
        "reference" => gpui_kit::rgb(0x30b0c7).into(),
        "task" => theme.success,
        _ => theme.accent,
    }
}

fn state_color(state: &str, theme: &Theme) -> Hsla {
    match state {
        "blocked" => theme.warning,
        "done" => theme.success,
        "todo" => theme.secondary,
        _ => theme.accent,
    }
}

/// A task state's icon: a dot in a ring under way, paused when blocked, dashed to do, a tick done.
fn state_icon(state: &str) -> gpui_kit::component::Icon {
    match state {
        "blocked" => icons::circle_pause(),
        "done" => icons::circle_check(),
        "todo" => icons::circle_dashed(),
        _ => icons::circle_dot(),
    }
}

fn user() -> Actor {
    Actor { kind: "user".into(), thread: None }
}

/// "5 minutes ago", from seconds since 1970.
fn ago(t: f64) -> String {
    crate::state::thread_tools::ago(t * 1000.0)
}

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Memories,
    Tasks,
    Insights,
    Activity,
    Graph,
}

/// The memory as a map: active memories' IDs, their links as index pairs, their places (0…1).
/// Claude Code's own memory for the project's folder, the Swift app's `ClaudeMemoryStatus`: on
/// or off, its files, and the ones YourHarness doesn't have.
#[derive(Clone, Default)]
pub struct ClaudeMemory {
    pub is_on: bool,
    pub files: usize,
    pub unimported: Vec<std::path::PathBuf>,
    pub is_home: bool,
}

impl ClaudeMemory {
    pub fn needs_nudge(&self) -> bool {
        self.is_on || !self.unimported.is_empty()
    }
}

/// When a project's Claude Code memory was last brought in (seconds since 1970). The Swift app
/// saved dates from 2001; those are moved over.
fn synced_at(lib: &yh_core::library::Library, project: &str) -> Option<f64> {
    let v = lib.setting("claudeMemorySyncedAt")?.get(project)?.as_f64()?;
    Some(if v < 1_500_000_000. { v + 978_307_200. } else { v })
}

#[derive(Default)]
struct Graph {
    ids: Vec<String>,
    edges: Vec<(usize, usize)>,
    points: Vec<(f64, f64)>,
}

pub struct MemoryPage {
    workspace: Entity<Workspace>,
    pub project: String,
    store: Option<Store>,
    pub tab: Tab,
    /// Every memory of the project, archived ones too; read again when memory changes.
    pub memories: Vec<Memory>,
    events: Vec<(String, Option<String>, Option<String>, f64, Option<String>, Option<i64>)>,
    loaded_revision: Option<u64>,
    pub loaded: bool,
    /// Claude Code's own memory here, read with the rest.
    claude: Option<ClaudeMemory>,
    pressed_one_memory: bool,
    search: Entity<InputState>,
    kind_filter: Option<&'static str>,
    archived: bool,
    pub selection: Option<String>,
    /// The selected memory's fields as you edit them, and its kind.
    title: Entity<InputState>,
    description: Entity<InputState>,
    body: Entity<TextareaState>,
    draft_kind: String,
    /// The editors hold this memory as it was when they were filled.
    editing: Option<Memory>,
    versions: Vec<(i64, String, String, String, f64, Option<String>)>,
    shown_version: Option<i64>,
    /// A memory to put in the editors at the next render (setting a value needs the window).
    pending_fill: Option<Memory>,
    /// What an import or export just did, under the title.
    notice: Option<String>,
    stats: Option<yh_core::memory_stats::Stats>,
    graph: Graph,
    /// The graph's place, from its last paint: which dot a click is on.
    graph_bounds: std::rc::Rc<std::cell::Cell<gpui_kit::Bounds<gpui_kit::Pixels>>>,
    /// Add a step, under a task's steps.
    new_step: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl MemoryPage {
    pub fn new(workspace: Entity<Workspace>, project: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = workspace.read(cx).memory_store();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search memories"));
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("Title"));
        let description = cx.new(|cx| InputState::new(window, cx).placeholder("One line: when it matters"));
        let body = cx.new(|cx| TextareaState::new(window, cx).placeholder("What to remember"));
        let new_step = cx.new(|cx| InputState::new(window, cx).placeholder("Add a step"));
        let subscriptions = vec![
            cx.subscribe_in(&new_step, window, |this, input, e: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = e {
                    let text = input.read(cx).value().trim().to_string();
                    if let (false, Some(id)) = (text.is_empty(), this.selection.clone()) {
                        input.update(cx, |i, cx| i.set_value("", window, cx));
                        this.add_step(&id, text, cx);
                    }
                }
            }),
            cx.observe(&workspace, |this, ws, cx| {
                if this.loaded_revision != Some(ws.read(cx).memory_revision) {
                    this.reload(cx);
                }
            }),
            cx.subscribe(&search, |_, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&title, |_, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&description, |_, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&body, |_, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        let mut page = MemoryPage {
            workspace,
            project,
            store,
            tab: Tab::Memories,
            memories: vec![],
            events: vec![],
            loaded_revision: None,
            claude: None,
            pressed_one_memory: false,
            loaded: false,
            search,
            kind_filter: None,
            archived: false,
            selection: None,
            title,
            description,
            body,
            draft_kind: "project".into(),
            editing: None,
            versions: vec![],
            shown_version: None,
            pending_fill: None,
            notice: None,
            stats: None,
            graph: Graph::default(),
            graph_bounds: Default::default(),
            new_step,
            _subscriptions: subscriptions,
        };
        page.reload(cx);
        page
    }

    fn scope(&self) -> String {
        yh_core::memory::scope_of(&self.project)
    }

    /// Reads the project's memory and activity again, off the main thread, after any queued
    /// writes have landed.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            self.loaded = true;
            return;
        };
        let revision = self.workspace.read(cx).memory_revision;
        self.loaded_revision = Some(revision);
        let scope = self.scope();
        let (folder, since) = (self.project_path(cx), synced_at(&self.workspace.read(cx).library, &self.project));
        let is_home = {
            let ws = self.workspace.read(cx);
            ws.home_project().as_deref() == Some(self.project.as_str()) || folder.as_deref() == std::env::var("HOME").ok().as_deref()
        };
        cx.spawn(async move |this, cx| {
            let (memories, events, stats, graph, claude) = cx
                .background_executor()
                .spawn(async move {
                    store.db.flush();
                    let mut all = store.memories(Some(&scope), None);
                    all.retain(|m| m.status != "proposed" || m.merged_into.is_none());
                    all.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.updated_at.total_cmp(&a.updated_at)));
                    let stats = yh_core::memory_stats::compute(&store, &scope, 30);
                    let active: Vec<&Memory> = all.iter().filter(|m| m.status == "active").collect();
                    let ids: Vec<String> = active.iter().map(|m| m.id.clone()).collect();
                    let edges: Vec<(usize, usize)> = active
                        .iter()
                        .enumerate()
                        .flat_map(|(i, m)| m.links.iter().filter_map(|(to, _)| ids.iter().position(|x| x == to)).map(move |j| (i, j)).collect::<Vec<_>>())
                        .collect();
                    let points = yh_core::memory_stats::graph_layout(&ids, &edges);
                    let claude = folder.map(|folder| {
                        let config = yh_core::history::config_directory();
                        let active: Vec<Memory> = all.iter().filter(|m| m.status == "active").cloned().collect();
                        use yh_core::memory_archive::claude_code;
                        ClaudeMemory {
                            is_on: !claude_code::setting(&folder, &config).is_off(),
                            files: claude_code::files(&folder, &config).len(),
                            unimported: claude_code::unimported(&folder, &config, &active, since),
                            is_home,
                        }
                    });
                    (all, store.events(&scope, 300), stats, Graph { ids, edges, points }, claude)
                })
                .await;
            let _ = this.update(cx, |page, cx| {
                // `YH_USE_ONE_MEMORY=1` presses the nudge's main button once, for checking it.
                let press = claude.as_ref().is_some_and(|c| c.needs_nudge()) && std::env::var("YH_USE_ONE_MEMORY").is_ok() && !page.pressed_one_memory;
                page.claude = claude;
                if press {
                    page.pressed_one_memory = true;
                    if let Some(c) = page.claude.clone() {
                        eprintln!("USE ONE MEMORY: on {}, {} to import", c.is_on, c.unimported.len());
                        page.use_one_memory(&c, cx);
                    }
                }
                page.memories = memories;
                page.events = events;
                page.stats = Some(stats);
                page.graph = graph;
                page.loaded = true;
                page.after_reload(cx);
                // `YH_MEMORY_ADD_STEP=<text>`: opens the first task and adds a step to it, once.
                static ADDED_STEP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                if let Ok(text) = std::env::var("YH_MEMORY_ADD_STEP") {
                    if let Some(id) = page.memories.iter().find(|m| m.is_task() && m.status == "active").map(|m| m.id.clone()) {
                        if !ADDED_STEP.swap(true, std::sync::atomic::Ordering::Relaxed) {
                            page.tab = Tab::Memories;
                            page.select(&id, cx);
                            page.add_step(&id, text, cx);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Keeps the selection on a memory that's still listed, and shows changes made elsewhere
    /// in the editors unless you've edited them.
    fn after_reload(&mut self, cx: &mut Context<Self>) {
        let visible = self.visible(cx);
        if self.selection.as_ref().is_none_or(|id| !self.memories.iter().any(|m| &m.id == id)) {
            self.selection = visible.first().map(|m| m.id.clone());
        }
        let current = self.selected().cloned();
        let edited = self.is_dirty(cx);
        if let Some(m) = current {
            if self.editing.as_ref() != Some(&m) && !edited {
                self.pending_fill = Some(m);
            }
        }
    }

    pub fn selected(&self) -> Option<&Memory> {
        let id = self.selection.as_ref()?;
        self.memories.iter().find(|m| &m.id == id)
    }

    fn visible(&self, cx: &App) -> Vec<&Memory> {
        let words: Vec<String> = self.search.read(cx).value().to_lowercase().split_whitespace().map(str::to_string).collect();
        self.memories
            .iter()
            .filter(|m| (m.status == "archived") == self.archived)
            .filter(|m| self.kind_filter.is_none_or(|k| m.kind == k))
            .filter(|m| {
                let text = format!("{} {} {}", m.title, m.description, m.body).to_lowercase();
                words.iter().all(|w| text.contains(w))
            })
            .collect()
    }

    pub fn select(&mut self, id: &str, cx: &mut Context<Self>) {
        self.selection = Some(id.to_string());
        self.pending_fill = self.selected().cloned();
        self.shown_version = None;
        cx.notify();
    }

    fn is_dirty(&self, cx: &App) -> bool {
        let Some(m) = &self.editing else { return false };
        self.title.read(cx).value().trim() != m.title
            || self.description.read(cx).value().trim() != m.description
            || self.body.read(cx).value().trim() != m.body
            || self.draft_kind != m.kind
    }

    /// Puts a memory in the editors (needs the window, so it's done at the next render).
    fn fill(&mut self, m: Memory, window: &mut Window, cx: &mut Context<Self>) {
        self.title.update(cx, |i, cx| i.set_value(m.title.clone(), window, cx));
        self.description.update(cx, |i, cx| i.set_value(m.description.clone(), window, cx));
        self.body.update(cx, |i, cx| i.set_value(m.body.clone(), window, cx));
        self.draft_kind = m.kind.clone();
        self.versions = self.store.as_ref().map(|s| s.versions(&m.id)).unwrap_or_default();
        self.editing = Some(m);
    }

    fn written(&mut self, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| ws.memory_changed(cx));
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let (Some(store), Some(mut m)) = (self.store.clone(), self.editing.clone()) else { return };
        let title = self.title.read(cx).value().trim().to_string();
        if title.is_empty() {
            return;
        }
        m.title = title;
        m.description = self.description.read(cx).value().trim().to_string();
        m.body = self.body.read(cx).value().trim().to_string();
        if m.kind != self.draft_kind {
            m.kind = self.draft_kind.clone();
            if m.kind == "task" && m.task_state.is_none() {
                m.task_state = Some("active".into());
            }
        }
        m.updated_at = yh_core::library::now_ms() / 1000.0;
        let created = !self.memories.iter().any(|x| x.id == m.id);
        store.save(m.clone(), &user(), Some(if created { "created" } else { "updated" }), None);
        self.editing = Some(m);
        self.written(cx);
    }

    fn revert(&mut self, cx: &mut Context<Self>) {
        let restored = self.selected().cloned().or(self.editing.clone());
        if let Some(m) = restored {
            self.pending_fill = Some(m);
        }
        cx.notify();
    }

    fn project_path(&self, cx: &App) -> Option<String> {
        self.workspace.read(cx).project(&self.project).map(|p| p.path.clone())
    }

    fn workspace_name(&self, cx: &App) -> String {
        let ws = self.workspace.read(cx);
        ws.project(&self.project).map(|p| if ws.home_project().as_deref() == Some(p.id.as_str()) { "Home".to_string() } else { p.name.clone() }).unwrap_or_default()
    }

    /// Claude Code's own memory files for this project's folder, not counting its index.
    fn claude_code_files(&self, cx: &App) -> Vec<std::path::PathBuf> {
        self.project_path(cx).map(|p| yh_core::memory_archive::claude_code::files(&p, &yh_core::history::config_directory())).unwrap_or_default()
    }

    /// Imports a file or folder: an export (JSON) or memory files (Markdown with memory
    /// frontmatter) straight in; anything else goes to the Importer.
    pub fn import_path(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else { return };
        let scope = self.scope();
        let (ws, project) = (self.workspace.clone(), self.project.clone());
        cx.spawn(async move |this, cx| {
            let p = path.clone();
            let found: Result<Vec<Memory>, String> = cx
                .background_executor()
                .spawn(async move {
                    let now = yh_core::library::now_ms() / 1000.;
                    if p.is_dir() {
                        Ok(yh_core::memory_archive::memories_from_markdown(&yh_core::memory_archive::read_markdown_folder(&p), &scope, now))
                    } else if p.extension().is_some_and(|e| e == "json") {
                        let data = std::fs::read(&p).map_err(|e| e.to_string())?;
                        yh_core::memory_archive::memories_from_json(&data, &scope).map_err(|e| e.to_string())
                    } else if p.extension().is_some_and(|e| e == "md") {
                        let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
                        let mut files = std::collections::BTreeMap::new();
                        files.insert(p.file_name().unwrap_or_default().to_string_lossy().into_owned(), text);
                        Ok(yh_core::memory_archive::memories_from_markdown(&files, &scope, now))
                    } else {
                        Ok(vec![])
                    }
                })
                .await;
            let _ = this.update(cx, |page, cx| match found {
                Ok(memories) if !memories.is_empty() => {
                    let report = yh_core::memory_archive::merge(memories, &page.scope(), &store, &user());
                    page.notice = Some(format!("Imported from {}: {} added, {} updated, {} already here.", path.file_name().unwrap_or_default().to_string_lossy(), report.added, report.updated, report.unchanged));
                    if std::env::var("YH_MEMORY_IMPORT").is_ok() {
                        eprintln!("MEMORY {}", page.notice.clone().unwrap_or_default());
                    }
                    page.written(cx);
                }
                Ok(_) => {
                    // Not memory files: the Importer reads anything.
                    let sources = vec![path.to_string_lossy().into_owned()];
                    let message = yh_core::memory_agents::import_request(&sources, None);
                    ws.update(cx, |ws, cx| ws.open_memory_agent("importer", &project, sources, Some(message), cx));
                }
                Err(e) => {
                    page.notice = Some(e);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn import_file_dialog(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: true, directories: true, multiple: false, prompt: Some("Import".into()) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else { return };
            if let Some(path) = paths.into_iter().next() {
                let _ = this.update(cx, |p, cx| p.import_path(path, cx));
            }
        })
        .detach();
    }

    fn import_claude_code(&mut self, cx: &mut Context<Self>) {
        let Some(folder) = self.project_path(cx) else { return };
        let dir = yh_core::memory_archive::claude_code::directory(&folder, &yh_core::history::config_directory());
        self.import_path(dir, cx);
    }

    /// Writes this workspace's memory as JSON, or a Markdown folder (Claude Code's format), to `to`.
    pub fn export_to(&mut self, markdown: bool, to: std::path::PathBuf, cx: &mut Context<Self>) {
        let memories: Vec<Memory> = self.memories.clone();
        let (name, path) = (self.workspace_name(cx), self.project_path(cx));
        let result = if markdown {
            std::fs::create_dir_all(&to)
                .map_err(|e| e.to_string())
                .and_then(|_| yh_core::memory_archive::markdown_files(&memories, &name).into_iter().try_for_each(|(f, text)| std::fs::write(to.join(f), text).map_err(|e| e.to_string())))
        } else {
            std::fs::write(&to, yh_core::memory_archive::json(&memories, &name, path.as_deref(), yh_core::library::now_ms() / 1000.)).map_err(|e| e.to_string())
        };
        self.notice = Some(match result {
            Ok(()) => format!("Exported {} memories to {}.", memories.len(), to.display()),
            Err(e) => format!("Couldn't export: {e}"),
        });
        if std::env::var("YH_MEMORY_EXPORT").is_ok() {
            eprintln!("MEMORY {}", self.notice.clone().unwrap_or_default());
        }
        cx.notify();
    }

    fn export_dialog(&mut self, markdown: bool, cx: &mut Context<Self>) {
        let home = std::env::var("HOME").map(std::path::PathBuf::from).unwrap_or_default().join("Desktop");
        let name = format!("{} memory{}", self.workspace_name(cx), if markdown { "" } else { ".json" });
        let chosen = cx.prompt_for_new_path(&home, Some(&name));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(to))) = chosen.await else { return };
            let _ = this.update(cx, |p, cx| p.export_to(markdown, to, cx));
        })
        .detach();
    }

    pub fn new_memory(&mut self, cx: &mut Context<Self>) {
        let kind = self.kind_filter.unwrap_or("project");
        let m = Memory::new(&self.scope(), kind, "", "", "", "user");
        self.tab = Tab::Memories;
        self.archived = false;
        self.selection = Some(m.id.clone());
        // Listed once saved; until then it's only in the editors.
        self.pending_fill = Some(m);
        cx.notify();
    }

    fn set_pinned(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        if let Some(store) = &self.store {
            store.set_pinned(id, on, &user());
            self.written(cx);
        }
    }

    fn set_status(&mut self, id: &str, status: &str, cx: &mut Context<Self>) {
        if let Some(store) = &self.store {
            store.set_status(id, status, &user(), None);
            self.written(cx);
        }
    }

    fn delete(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(store) = &self.store {
            store.delete(id, &user());
            self.selection = None;
            self.editing = None;
            self.written(cx);
        }
    }

    fn set_task_state(&mut self, id: &str, state: &str, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else { return };
        let Some(mut m) = self.memories.iter().find(|m| m.id == id).cloned() else { return };
        m.task_state = Some(state.into());
        m.updated_at = yh_core::library::now_ms() / 1000.0;
        store.save(m, &user(), Some("updated"), None);
        self.written(cx);
    }

    fn toggle_step(&mut self, id: &str, step: usize, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else { return };
        let Some(mut m) = self.memories.iter().find(|m| m.id == id).cloned() else { return };
        let Some(s) = m.steps.get_mut(step) else { return };
        s.done = !s.done;
        if m.steps.iter().all(|s| s.done) {
            m.task_state = Some("done".into());
        } else if m.task_state.as_deref() == Some("done") {
            m.task_state = Some("active".into());
        }
        m.updated_at = yh_core::library::now_ms() / 1000.0;
        store.save(m, &user(), Some("updated"), None);
        self.written(cx);
    }

    /// A step at the end of a task's list; a done task with a new step is under way again.
    pub fn add_step(&mut self, id: &str, text: String, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else { return };
        let Some(mut m) = self.memories.iter().find(|m| m.id == id).cloned() else { return };
        m.steps.push(yh_core::memory::Step { text, done: false });
        if m.task_state.as_deref() == Some("done") {
            m.task_state = Some("active".into());
        }
        m.updated_at = yh_core::library::now_ms() / 1000.0;
        if std::env::var("YH_DATA_DIR").is_ok() {
            eprintln!("MEMORY step added to {}: {} steps", m.title, m.steps.len());
        }
        store.save(m, &user(), Some("updated"), None);
        self.written(cx);
    }

    /// The threads a task was worked on in (its sources), newest first: (id, title).
    fn threads_of(&self, m: &Memory, cx: &App) -> Vec<(String, String)> {
        let ws = self.workspace.read(cx);
        let mut seen = std::collections::HashSet::new();
        let mut out: Vec<(f64, String, String)> = m
            .sources
            .iter()
            .filter(|(t, _)| seen.insert(t.clone()))
            .filter_map(|(t, _)| ws.thread(t).map(|th| (th.read(cx).meta.updated_at, t.clone(), th.read(cx).title.clone())))
            .collect();
        out.sort_by(|a, b| b.0.total_cmp(&a.0));
        out.into_iter().map(|(_, id, title)| (id, title)).collect()
    }

    fn summary(&self) -> String {
        let active: Vec<&Memory> = self.memories.iter().filter(|m| m.status == "active").collect();
        let archived = self.memories.iter().filter(|m| m.status == "archived").count();
        if active.is_empty() && archived == 0 {
            return "Nothing remembered yet".into();
        }
        let bytes: usize = active.iter().map(|m| m.size()).sum();
        let mut parts = vec![
            if active.len() == 1 { "1 memory".to_string() } else { format!("{} memories", active.len()) },
            if bytes < 1024 { format!("{bytes} bytes") } else { format!("{:.1} KB", bytes as f64 / 1024.0) },
        ];
        let reads: i64 = active.iter().map(|m| m.recall_count).sum();
        if reads > 0 {
            parts.push(format!("{reads} reads"));
        }
        if archived > 0 {
            parts.push(format!("{archived} archived"));
        }
        parts.join(" · ")
    }
}

// MARK: Drawing

fn chip(label: impl Into<SharedString>, color: Hsla) -> gpui_kit::Div {
    page_kit::chip(label, color)
}

/// A small heading over a group of the detail (Task, Use, Linked…).
fn section(title: &str, theme: &Theme) -> gpui_kit::Div {
    div().pt(px(space::S)).text_size(px(text::FOOTNOTE)).font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).child(title.to_string())
}

/// A meta line's piece: an icon and words, both quiet.
fn meta(icon: gpui_kit::component::Icon, label: impl Into<SharedString>, theme: &Theme) -> gpui_kit::Div {
    div().flex().items_center().gap(px(3.)).child(icon.size(px(11.)).text_color(theme.tertiary)).child(label.into())
}

impl MemoryPage {
    fn header(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.workspace.read(cx).project(&self.project).map(|p| {
            if self.workspace.read(cx).home_project().as_deref() == Some(p.id.as_str()) { "Home".to_string() } else { p.name.clone() }
        });
        let memory_on = self.workspace.read(cx).library.setting("memoryEnabled").and_then(|v| v.as_bool()).unwrap_or(true);
        let tasks = self.memories.iter().filter(|m| m.is_task() && m.status == "active").count();
        // Back to the project's PRs and repos (Home has none).
        let is_home = self.workspace.read(cx).home_project().as_deref() == Some(self.project.as_str());
        let (ws_overview, project_overview) = (self.workspace.clone(), self.project.clone());
        let page = cx.entity().downgrade();
        let tab = |id: &'static str, label: &'static str, icon: gpui_kit::component::Icon, t: Tab| {
            let page = page.clone();
            Segment::new(id, label, self.tab == t, move |_, _, cx| {
                let _ = page.update(cx, |p, cx| {
                    p.tab = t;
                    cx.notify();
                });
            })
            .icon(icon)
        };
        let segments = vec![
            tab("tab-memories", "Memories", icons::brain(), Tab::Memories),
            if tasks > 0 { tab("tab-tasks", "Tasks", icons::list_checks(), Tab::Tasks).badge(tasks.to_string()) } else { tab("tab-tasks", "Tasks", icons::list_checks(), Tab::Tasks) },
            tab("tab-insights", "Insights", icons::lightbulb(), Tab::Insights),
            tab("tab-activity", "Activity", icons::activity(), Tab::Activity),
            tab("tab-graph", "Graph", icons::waypoints(), Tab::Graph),
        ];
        let tabs = div()
            .flex_1()
            .flex()
            .items_center()
            .gap(px(space::S))
            .child(page_kit::segmented(segments, theme))
            .child(div().flex_1())
            .when(!is_home, |d| {
                d.child(
                    Button::new("tab-overview")
                        .small()
                        .ghost()
                        .icon(IconName::LayoutDashboard)
                        .label("Project Overview")
                        .tooltip("The project's pull requests, repos and worktrees")
                        .on_click(move |_, _, cx| ws_overview.update(cx, |ws, cx| ws.open_project(&project_overview, cx))),
                )
            });
        // The subtitle: what's remembered, that it's the only memory, what an import just did.
        let one_memory = self.claude.as_ref().filter(|c| !c.needs_nudge()).map(|c| {
            // One memory: Claude Code's own is off here and nothing of it is left out.
            let tip = if c.files > 0 {
                format!("Claude Code's own memory is off for this folder, and its {} memory files are in YourHarness.", c.files)
            } else {
                "Claude Code's own memory is off for this folder: YourHarness keeps the only memory.".to_string()
            };
            div()
                .id("one-memory")
                .flex_none()
                .flex()
                .items_center()
                .gap(px(3.))
                .text_color(theme.success)
                .child(icons::circle_check().size(px(12.)))
                .child(div().text_color(theme.secondary).child("One memory"))
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
        });
        let subtitle = div()
            .flex()
            .items_center()
            .gap(px(space::S))
            .min_w_0()
            .child(div().min_w_0().truncate().child(if self.loaded { self.summary() } else { "Loading…".into() }))
            .children(one_memory)
            .when_some(self.notice.clone(), |d, n| d.child(div().min_w_0().truncate().text_color(theme.accent).child(n)))
            .into_any_element();
        let toolbar = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .when(!memory_on, |d| {
                d.child(
                    div()
                        .id("memory-off")
                        .mr(px(space::S))
                        .child(chip("Off for agents", theme.warning))
                        .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Agents don't get memory. Turn it on in Settings → General.").build(window, cx)),
                )
            })
            .child({
                let page = cx.entity().downgrade();
                let claude_files = self.claude_code_files(cx).len();
                Button::new("memory-import").small().ghost().icon(icons::download()).label("Import").dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
                    let (a, b, c) = (page.clone(), page.clone(), page.clone());
                    menu = menu
                        .item(PopupMenuItem::new("With the Importer…").icon(icons::sparkles()).on_click(move |_, window, cx| {
                            if let Some(p) = a.upgrade() {
                                let (ws, project) = (p.read(cx).workspace.clone(), p.read(cx).project.clone());
                                open_setup(ws, "importer", project, vec![], window, cx);
                            }
                        }))
                        .item(PopupMenuItem::new("File or Folder…").icon(IconName::FolderOpen).on_click(move |_, _, cx| {
                            let _ = b.update(cx, |p, cx| p.import_file_dialog(cx));
                        }));
                    if claude_files > 0 {
                        menu = menu.separator().item(PopupMenuItem::new(format!("Claude Code's Memory ({claude_files})")).on_click(move |_, _, cx| {
                            let _ = c.update(cx, |p, cx| p.import_claude_code(cx));
                        }));
                    }
                    menu
                })
            })
            .child({
                let page = cx.entity().downgrade();
                Button::new("memory-export").small().ghost().icon(icons::upload()).label("Export").dropdown_caret(true).dropdown_menu(move |menu, _, _| {
                    let (a, b) = (page.clone(), page.clone());
                    menu.item(PopupMenuItem::new("As JSON…").on_click(move |_, _, cx| {
                        let _ = a.update(cx, |p, cx| p.export_dialog(false, cx));
                    }))
                    .item(PopupMenuItem::new("As Markdown Folder…").on_click(move |_, _, cx| {
                        let _ = b.update(cx, |p, cx| p.export_dialog(true, cx));
                    }))
                })
            })
            .child(Button::new("memory-reconcile").small().ghost().icon(icons::sparkles()).label("Reconcile").tooltip("The Reconciler finds duplicates, stale and vague memories, and fixes them with you").on_click(cx.listener(|this, _, window, cx| {
                open_setup(this.workspace.clone(), "reconciler", this.project.clone(), vec![], window, cx);
            })))
            .child(div().ml(px(space::XS)).child(Button::new("new-memory").small().primary().icon(IconName::Plus).label("New Memory").on_click(cx.listener(|this, _, _, cx| this.new_memory(cx)))))
            .into_any_element();
        page_kit::page_header(icons::brain(), format!("{} Memory", name.unwrap_or_default()), subtitle, toolbar, Some(tabs.into_any_element()), theme)
    }

    fn list(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let visible = self.visible(cx);
        let count = visible.len();
        let mut chips = div().flex().flex_wrap().gap(px(4.));
        let filter_chip = |id: String, label: &'static str, on: bool, color: Option<Hsla>, value: Option<&'static str>, cx: &mut Context<Self>| {
            div()
                .id(SharedString::from(id))
                .flex()
                .items_center()
                .gap(px(5.))
                .h(px(22.))
                .px(px(8.))
                .rounded(px(radius::MEDIUM))
                .text_size(px(text::FOOTNOTE))
                .cursor_pointer()
                .when(on, |d| d.bg(theme.accent_soft).text_color(theme.text).font_weight(FontWeight::MEDIUM))
                .when(!on, |d| d.text_color(theme.secondary).hover(|s| s.bg(theme.hover).text_color(theme.text)))
                .when_some(color, |d, c| d.child(page_kit::dot(c, 6.)))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.kind_filter = if this.kind_filter == value { None } else { value };
                    cx.notify();
                }))
        };
        chips = chips.child(filter_chip("kind-all".into(), "All", self.kind_filter.is_none(), None, None, cx));
        for (kind, label) in KINDS {
            chips = chips.child(filter_chip(format!("kind-{kind}"), label, self.kind_filter == Some(kind), Some(kind_color(kind, theme)), Some(kind), cx));
        }
        let page = cx.entity().downgrade();
        let status = |id: &'static str, label: &'static str, archived: bool| {
            let page = page.clone();
            Segment::new(id, label, self.archived == archived, move |_, _, cx| {
                let _ = page.update(cx, |p, cx| {
                    p.archived = archived;
                    cx.notify();
                });
            })
        };
        let mut rows = div().id("memory-list").flex_1().min_h_0().overflow_y_scroll().px(px(space::S)).py(px(space::XS)).flex().flex_col().gap(px(1.));
        if count == 0 {
            rows = rows.child(div().p(px(space::L)).text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(if self.memories.is_empty() {
                "Nothing remembered yet."
            } else if self.archived {
                "Nothing archived."
            } else {
                "No memories match."
            }));
        }
        for m in visible {
            let id = m.id.clone();
            let selected = self.selection.as_deref() == Some(m.id.as_str());
            let preview = if m.description.is_empty() { m.body.clone() } else { m.description.clone() };
            let (pin_id, status_id, del_id, pinned, archived) = (id.clone(), id.clone(), id.clone(), m.pinned, m.status == "archived");
            let page = cx.entity().downgrade();
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("memory-{id}")))
                    .px(px(space::S + 2.))
                    .py(px(space::S))
                    .rounded(px(radius::MEDIUM))
                    .when(selected, |d| d.bg(theme.accent_soft))
                    .when(!selected, |d| d.hover(|s| s.bg(theme.hover)))
                    .flex()
                    .gap(px(space::S + 1.))
                    .child(div().mt(px(6.)).child(page_kit::dot(kind_color(&m.kind, theme), 7.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.))
                                    .child(div().min_w_0().font_weight(FontWeight::MEDIUM).text_color(theme.text).truncate().child(if m.title.is_empty() { "Untitled".to_string() } else { m.title.clone() }))
                                    .when(m.pinned, |d| d.child(icons::pin().size(px(11.)).text_color(theme.tertiary))),
                            )
                            .when(!preview.is_empty(), |d| d.child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).line_clamp(2).child(preview)))
                            .when(m.is_task(), |d| {
                                let state = m.task_state.clone().unwrap_or_else(|| "active".into());
                                let color = state_color(&state, theme);
                                d.child(
                                    div()
                                        .pt(px(1.))
                                        .flex()
                                        .items_center()
                                        .gap(px(4.))
                                        .text_size(px(text::CAPTION))
                                        .text_color(theme.secondary)
                                        .child(state_icon(&state).size(px(12.)).text_color(color))
                                        .child(yh_core::memory::state_label(&state).to_string())
                                        .when(!m.steps.is_empty(), |d| d.child(div().text_color(theme.tertiary).child(format!("· {} of {} steps", m.steps_done(), m.steps.len())))),
                                )
                            })
                            .child(
                                div()
                                    .pt(px(1.))
                                    .flex()
                                    .items_center()
                                    .gap(px(space::S))
                                    .text_size(px(text::CAPTION))
                                    .text_color(theme.tertiary)
                                    .child(format!("Read {}×", m.recall_count))
                                    .when(!m.links.is_empty(), |d| d.child(meta(icons::link(), m.links.len().to_string(), theme)))
                                    .child(div().flex_1())
                                    .child(ago(m.updated_at)),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.select(&id, cx)))
                    .context_menu(move |menu, _, _| {
                        let (a, b, c) = (page.clone(), page.clone(), page.clone());
                        let (i1, i2, i3) = (pin_id.clone(), status_id.clone(), del_id.clone());
                        menu.item(PopupMenuItem::new(if pinned { "Unpin" } else { "Pin (Always Load in Full)" }).icon(if pinned { icons::pin_off() } else { icons::pin() }).on_click(move |_, _, cx| {
                            let _ = a.update(cx, |p, cx| p.set_pinned(&i1, !pinned, cx));
                        }))
                        .item(PopupMenuItem::new(if archived { "Restore" } else { "Archive" }).icon(if archived { icons::archive_restore() } else { icons::archive() }).on_click(move |_, _, cx| {
                            let _ = b.update(cx, |p, cx| p.set_status(&i2, if archived { "active" } else { "archived" }, cx));
                        }))
                        .separator()
                        .item(PopupMenuItem::new("Delete").icon(icons::trash()).on_click(move |_, _, cx| {
                            let _ = c.update(cx, |p, cx| p.delete(&i3, cx));
                        }))
                    }),
            );
        }
        div()
            .w(px(300.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme.separator)
            .child(
                div()
                    .px(px(space::M))
                    .pt(px(space::M))
                    .pb(px(space::S))
                    .flex()
                    .flex_col()
                    .gap(px(space::S))
                    .child(Input::new(&self.search).small().cleanable(true).prefix(gpui_kit::component::Icon::new(IconName::Search).size(px(13.)).text_color(theme.tertiary)))
                    .child(chips)
                    .child(div().flex().child(page_kit::segmented(vec![status("in-use", "In Use", false), status("archived", "Archived", true)], theme))),
            )
            .child(rows)
            .child(
                div()
                    .py(px(6.))
                    .border_t_1()
                    .border_color(theme.separator)
                    .text_center()
                    .text_size(px(text::CAPTION))
                    .text_color(theme.tertiary)
                    .child(if count == 1 { "1 memory".to_string() } else { format!("{count} memories") }),
            )
    }

    fn detail(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(m) = self.editing.clone().filter(|e| Some(&e.id) == self.selection.as_ref()) else {
            return div()
                .flex_1()
                .child(page_kit::empty_state(icons::brain(), "No Memory Selected", "Choose a memory to see what it says, how agents use it and how it changed.", None, theme))
                .into_any_element();
        };
        let saved = self.memories.iter().find(|x| x.id == m.id).cloned();
        let listed = saved.is_some();
        let current = saved.unwrap_or_else(|| m.clone());
        let dirty = self.is_dirty(cx) || !listed;
        let id = m.id.clone();
        let page = cx.entity().downgrade();
        let kind_menu = {
            let page = page.clone();
            let current_kind = self.draft_kind.clone();
            Button::new("kind").small().ghost().label(kind_label(&self.draft_kind)).dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
                for (kind, label) in KINDS {
                    let page = page.clone();
                    menu = menu.item(PopupMenuItem::new(label).checked(current_kind == kind).on_click(move |_, _, cx| {
                        let _ = page.update(cx, |p, cx| {
                            p.draft_kind = kind.to_string();
                            cx.notify();
                        });
                    }));
                }
                menu
            })
        };
        let (pin_id, status_id, del_id) = (id.clone(), id.clone(), id.clone());
        let archived = current.status == "archived";
        let pinned = current.pinned;
        let mut body = div()
            .id("memory-detail")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(space::XL))
            .py(px(space::L))
            .flex()
            .flex_col()
            .gap(px(space::S))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(page_kit::dot(kind_color(&self.draft_kind, theme), 8.))
                    .child(div().ml(px(-4.)).child(kind_menu))
                    .when(pinned, |d| d.child(page_kit::quiet_chip("Pinned", theme)))
                    .when(current.status != "active", |d| d.child(chip(if archived { "Archived" } else { "Proposed" }, theme.warning)))
                    .when(listed, |d| d.child(div().font_family(MONO).text_size(px(text::CAPTION)).text_color(theme.tertiary).child(current.short_id())))
                    .child(div().flex_1())
                    .when(listed, |d| {
                        d.child(
                            Button::new("pin")
                                .small()
                                .ghost()
                                .icon(if pinned { icons::pin_off() } else { icons::pin() })
                                .tooltip(if pinned { "Unpin: sessions get only its index line" } else { "Pin: every session gets it in full, not only its index line" })
                                .on_click(cx.listener(move |this, _, _, cx| this.set_pinned(&pin_id, !pinned, cx))),
                        )
                        .child(Button::new("archive").small().ghost().icon(if archived { icons::archive_restore() } else { icons::archive() }).tooltip(if archived { "Restore" } else { "Archive" }).on_click(cx.listener(move |this, _, _, cx| {
                            this.set_status(&status_id, if archived { "active" } else { "archived" }, cx)
                        })))
                        .child(Button::new("delete").small().ghost().icon(icons::trash()).tooltip("Delete").on_click(cx.listener(move |this, _, _, cx| this.delete(&del_id, cx))))
                    }),
            )
            // The editors draw without a frame; their own inset is taken back so the text lines up.
            .child(div().ml(px(-10.)).child(Input::new(&self.title).appearance(false).text_size(px(text::LARGE_TITLE)).font_weight(FontWeight::SEMIBOLD)))
            .child(div().ml(px(-10.)).mt(px(-4.)).child(Input::new(&self.description).appearance(false).text_color(theme.secondary)))
            .child(page_kit::card(theme).mt(px(space::XS)).px(px(space::M)).py(px(space::S + 2.)).child(Textarea::new(&self.body).appearance(false).max_h(px(420.))));
        if current.is_task() {
            let state = current.task_state.clone().unwrap_or_else(|| "active".into());
            let page = cx.entity().downgrade();
            let states: Vec<Segment> = ["todo", "active", "blocked", "done"]
                .into_iter()
                .map(|s| {
                    let (page, tid) = (page.clone(), id.clone());
                    Segment::new(SharedString::from(format!("state-{s}")), yh_core::memory::state_label(s), state == s, move |_, _, cx| {
                        let _ = page.update(cx, |p, cx| p.set_task_state(&tid, s, cx));
                    })
                    .icon(state_icon(s))
                })
                .collect();
            let mut steps = div().flex().flex_col().gap(px(1.));
            for (i, step) in current.steps.iter().enumerate() {
                let tid = id.clone();
                steps = steps.child(
                    div()
                        .id(SharedString::from(format!("step-{i}")))
                        .flex()
                        .items_center()
                        .gap(px(space::S))
                        .rounded(px(radius::SMALL))
                        .px(px(6.))
                        .py(px(3.))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.hover))
                        .child(if step.done { icons::circle_check().size(px(15.)).text_color(theme.success) } else { icons::circle().size(px(15.)).text_color(theme.tertiary) })
                        .child(div().flex_1().when(step.done, |d| d.text_color(theme.secondary).line_through()).child(step.text.clone()))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_step(&tid, i, cx))),
                );
            }
            steps = steps.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S))
                    .px(px(6.))
                    .child(gpui_kit::component::Icon::new(IconName::Plus).size(px(15.)).text_color(theme.tertiary))
                    .child(div().flex_1().child(Input::new(&self.new_step).small().appearance(false))),
            );
            body = body.child(section("Task", theme)).child(div().flex().child(page_kit::segmented(states, theme))).child(steps);
        }
        if listed {
            body = body.child(section("Use", theme)).child(
                div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(format!(
                    "Read by agents {}×, in the index of {} sessions{}. Saved {}, changed {}.",
                    current.recall_count,
                    current.surfaced_count,
                    current.last_recalled_at.map(|t| format!(", last {}", ago(t))).unwrap_or_default(),
                    ago(current.created_at),
                    ago(current.updated_at),
                )),
            );
        }
        if !current.links.is_empty() {
            let mut links = div().flex().flex_col().gap(px(2.));
            for (to, relation) in &current.links {
                let other = self.memories.iter().find(|x| &x.id == to);
                let title = other.map(|x| x.title.clone()).unwrap_or_else(|| to.chars().take(8).collect());
                let color = other.map_or(theme.tertiary, |x| kind_color(&x.kind, theme));
                let target = to.clone();
                links = links.child(
                    div()
                        .id(SharedString::from(format!("link-{to}")))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(6.))
                        .py(px(2.))
                        .rounded(px(radius::SMALL))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.hover))
                        .child(icons::link().size(px(12.)).text_color(theme.tertiary))
                        .child(page_kit::dot(color, 6.))
                        .child(div().text_color(theme.accent).child(title))
                        .when_some(relation.clone(), |d, r| d.child(div().text_size(px(text::FOOTNOTE)).text_color(theme.tertiary).child(r)))
                        .on_click(cx.listener(move |this, _, _, cx| this.select(&target, cx))),
                );
            }
            body = body.child(section("Linked", theme)).child(links);
        }
        if !current.sources.is_empty() {
            let mut sources = div().flex().flex_col().gap(px(2.));
            let ws = self.workspace.clone();
            for (thread, _) in current.sources.iter().take(10) {
                let title = ws.read(cx).thread(thread).map(|t| t.read(cx).title.clone());
                let (ws, thread_id) = (ws.clone(), thread.clone());
                sources = sources.child(
                    div()
                        .id(SharedString::from(format!("source-{thread}")))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(6.))
                        .py(px(2.))
                        .rounded(px(radius::SMALL))
                        .child(icons::message_square().size(px(12.)).text_color(theme.tertiary))
                        .child(div().text_color(if title.is_some() { theme.accent } else { theme.tertiary }).child(title.clone().unwrap_or_else(|| "A thread that's gone".into())))
                        .when(title.is_some(), |d| d.cursor_pointer().hover(|s| s.bg(theme.hover)).on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.select(&thread_id, cx)))),
                );
            }
            body = body.child(section("Learned in", theme)).child(sources);
        }
        if self.versions.len() > 1 {
            let mut history = div().flex().flex_col().gap(px(1.));
            for (version, title, text_, status, at, actor) in self.versions.iter().rev() {
                let open = self.shown_version == Some(*version);
                let v = *version;
                history = history.child(
                    div()
                        .id(SharedString::from(format!("version-{version}")))
                        .flex()
                        .items_center()
                        .gap(px(space::S))
                        .rounded(px(radius::SMALL))
                        .px(px(6.))
                        .py(px(3.))
                        .cursor_pointer()
                        .when(open, |d| d.bg(theme.hover))
                        .hover(|s| s.bg(theme.hover))
                        .child(gpui_kit::component::Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(12.)).text_color(theme.tertiary))
                        .child(div().w(px(26.)).font_family(MONO).text_size(px(text::CAPTION)).text_color(theme.secondary).child(format!("v{version}")))
                        .child(div().flex_1().min_w_0().truncate().child(title.clone()))
                        .when(status != "active", |d| d.child(page_kit::quiet_chip(status.clone(), theme)))
                        .child(div().flex_none().text_size(px(text::CAPTION)).text_color(theme.tertiary).child(format!("{} · {}", actor.clone().unwrap_or_else(|| "agent".into()), ago(*at))))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.shown_version = if this.shown_version == Some(v) { None } else { Some(v) };
                            cx.notify();
                        })),
                );
                if open {
                    history = history.child(page_kit::card(theme).ml(px(space::XL)).my(px(space::XS)).p(px(space::M)).text_size(px(text::FOOTNOTE)).whitespace_normal().child(text_.clone()));
                }
            }
            body = body.child(section("History", theme)).child(history);
        }
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(body)
            .when(dirty, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S))
                        .px(px(space::L))
                        .py(px(space::S + 2.))
                        .border_t_1()
                        .border_color(theme.separator)
                        .child(page_kit::dot(theme.accent, 6.))
                        .child(div().flex_1().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(if listed { "Edited, not saved" } else { "New memory, not saved" }))
                        .child(Button::new("revert").small().outline().label(if listed { "Revert" } else { "Discard" }).on_click(cx.listener(move |this, _, _, cx| {
                            if listed {
                                this.revert(cx);
                            } else {
                                this.selection = None;
                                this.editing = None;
                                this.after_reload(cx);
                                cx.notify();
                            }
                        })))
                        .child(Button::new("save").small().primary().label("Save").tooltip("Save (⌘S)").on_click(cx.listener(|this, _, _, cx| this.save(cx)))),
                )
            })
            .into_any_element()
    }

    fn tasks(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let tasks: Vec<&Memory> = self.memories.iter().filter(|m| m.is_task() && m.status == "active").collect();
        if tasks.is_empty() {
            return page_kit::empty_state(icons::list_checks(), "No Tasks", "When you ask an agent for work with more than one step, it tracks it here, so every session knows where it stands.", None, theme).into_any_element();
        }
        let column = div().id("memory-tasks").size_full().overflow_y_scroll();
        let mut inner = div().flex().flex_col().gap(px(space::S));
        for (state, heading) in [("active", "In Progress"), ("blocked", "Blocked"), ("todo", "To Do"), ("done", "Done")] {
            let group: Vec<&&Memory> = tasks.iter().filter(|m| m.task_state.as_deref().unwrap_or("active") == state).collect();
            if group.is_empty() {
                continue;
            }
            let color = state_color(state, theme);
            inner = inner.child(
                div()
                    .pt(px(space::S))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(state_icon(state).size(px(14.)).text_color(color))
                    .child(page_kit::section_title(heading, Some(group.len()), theme)),
            );
            let mut list = page_kit::card(theme).overflow_hidden().flex().flex_col();
            for (n, m) in group.into_iter().enumerate() {
                let id = m.id.clone();
                let threads = self.threads_of(m, cx);
                let (done, total) = (m.steps_done(), m.steps.len());
                list = list.child(
                    div()
                        .id(SharedString::from(format!("task-{id}")))
                        .px(px(space::L))
                        .py(px(space::S + 2.))
                        .when(n > 0, |d| d.border_t_1().border_color(theme.separator))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.hover))
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::S))
                                .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::MEDIUM).text_color(if state == "done" { theme.secondary } else { theme.text }).child(m.title.clone()))
                                .when(total > 0, |d| {
                                    // How far along: a thin bar and the count.
                                    let f = done as f32 / total as f32;
                                    d.child(
                                        div()
                                            .flex_none()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.))
                                            .child(div().w(px(48.)).h(px(4.)).rounded(px(2.)).bg(theme.selected).child(div().h_full().rounded(px(2.)).w(gpui_kit::relative(f)).bg(color)))
                                            .child(div().w(px(36.)).text_size(px(text::CAPTION)).text_color(theme.secondary).child(format!("{done} of {total}"))),
                                    )
                                })
                                .child(div().flex_none().w(px(84.)).text_right().text_size(px(text::CAPTION)).text_color(theme.tertiary).child(ago(m.updated_at))),
                        )
                        .when_some(m.next_step().filter(|_| state != "done").map(str::to_string), |d, next| {
                            d.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.))
                                    .text_size(px(text::FOOTNOTE))
                                    .text_color(theme.secondary)
                                    .child(gpui_kit::component::Icon::new(IconName::ArrowRight).size(px(11.)).text_color(theme.tertiary))
                                    .child(div().min_w_0().truncate().child(next)),
                            )
                        })
                        .when(!threads.is_empty(), |d| {
                            // The threads working on it: click one to open it.
                            let mut row = div().flex().items_center().gap(px(space::S)).text_size(px(text::CAPTION));
                            for (tid, title) in threads.iter().take(3) {
                                let (ws, tid) = (self.workspace.clone(), tid.clone());
                                row = row.child(
                                    div()
                                        .id(SharedString::from(format!("task-thread-{id}-{tid}")))
                                        .flex()
                                        .items_center()
                                        .gap(px(4.))
                                        .max_w(px(220.))
                                        .text_color(theme.accent)
                                        .cursor_pointer()
                                        .child(icons::message_square().size(px(11.)).text_color(theme.tertiary))
                                        .child(div().min_w_0().truncate().child(title.clone()))
                                        .on_click(move |_, _, cx| {
                                            cx.stop_propagation();
                                            ws.update(cx, |ws, cx| ws.select(&tid, cx));
                                        }),
                                );
                            }
                            if threads.len() > 3 {
                                row = row.child(div().text_color(theme.tertiary).child(format!("+{}", threads.len() - 3)));
                            }
                            d.child(row)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.tab = Tab::Memories;
                            this.archived = false;
                            this.select(&id, cx);
                        })),
                );
            }
            inner = inner.child(list);
        }
        column.child(div().max_w(px(880.)).w_full().mx_auto().px(px(space::XL)).pt(px(space::M)).pb(px(space::XL)).child(inner)).into_any_element()
    }

    /// How memory is doing, in sentences first and figures after.
    fn insights(&self, theme: &Theme, _cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        use yh_core::memory_stats::Stats;
        let Some(st) = &self.stats else { return div().p(px(space::XL)).text_color(theme.secondary).child("Reading…").into_any_element() };
        let mut lines: Vec<(String, bool)> = vec![];
        let index_tokens = Stats::tokens(st.average_injected_bytes);
        lines.push((
            if st.sessions == 0 {
                "Every new thread here starts with an index of these memories: one line each, pinned ones in full.".into()
            } else {
                format!("Every session starts with an index of the memories, about {index_tokens} tokens. {} started in the last {} days.", if st.sessions == 1 { "1 session".into() } else { format!("{} sessions", st.sessions) }, st.window)
            },
            false,
        ));
        if st.recalls_in_window > 0 {
            lines.push((format!("Agents read {} of {} memories in full, {} times in all, when the index said they were relevant.", st.recalled_in_window, st.active, st.recalls_in_window), false));
        } else if st.sessions > 0 {
            lines.push(("No memory was read in full lately. The index line may be all agents needed, or titles and descriptions don't say enough to tell.".into(), true));
        }
        if st.searches > 0 {
            let missed = st.searches - st.searches_with_hits;
            lines.push((format!("Agents searched memory {} times; {}.", st.searches, if missed == 0 { "every search found something".to_string() } else { format!("{missed} found nothing (listed below: things they expected to know)") }), missed > 0));
        }
        let count = |k: &str| st.tasks.iter().find(|(s, _)| s == k).map_or(0, |(_, c)| *c);
        let open = count("active") + count("blocked") + count("todo");
        if open > 0 {
            let blocked = count("blocked");
            lines.push((format!("{open} task{} open{}: every session starts knowing where they stand and what's next.", if open == 1 { " is" } else { "s are" }, if blocked > 0 { format!(", {blocked} blocked") } else { String::new() }), blocked > 0));
        }
        if !st.never_recalled.is_empty() {
            let n = st.never_recalled.len();
            lines.push((format!("{n} memor{} never been read in over a week. Each still costs a line in every session.", if n == 1 { "y has" } else { "ies have" }), true));
        }
        if !st.likely_duplicates.is_empty() {
            let titles: Vec<String> = st.likely_duplicates.iter().take(2).map(|g| g.iter().take(3).map(|m| format!("“{}”", m.title)).collect::<Vec<_>>().join(", ")).collect();
            lines.push((format!("{} group{} like the same memory twice: {}.", st.likely_duplicates.len(), if st.likely_duplicates.len() == 1 { " looks" } else { "s look" }, titles.join("; ")), true));
        }
        if st.active > 3 && st.orphans * 2 > st.active {
            lines.push((format!("{} memories aren't linked to any other. Links let an agent that reads one find the rest.", st.orphans), false));
        }
        if let Some(l) = st.largest.first().filter(|l| l.bytes > 2_000) {
            lines.push((format!("“{}” is the biggest at {} bytes; memories work best short and about one thing.", l.title, l.bytes), true));
        }
        let issues = lines.iter().any(|(_, issue)| *issue);
        let mut how = page_kit::card(theme).px(px(space::L)).py(px(space::M + 2.)).flex().flex_col().gap(px(space::S + 2.)).child(
            div()
                .flex()
                .items_center()
                .gap(px(space::S))
                .child(icons::lightbulb().size(px(15.)).text_color(theme.secondary))
                .child(div().flex_1().text_size(px(text::HEADLINE)).font_weight(FontWeight::SEMIBOLD).child("How your memory is working"))
                .when(issues, |d| {
                    let (ws, project) = (self.workspace.clone(), self.project.clone());
                    d.child(Button::new("insights-reconcile").small().primary().icon(icons::sparkles()).label("Reconcile").on_click(move |_, window, cx| {
                        open_setup(ws.clone(), "reconciler", project.clone(), vec![], window, cx);
                    }))
                }),
        );
        for (text_, issue) in lines {
            how = how.child(
                div()
                    .flex()
                    .gap(px(space::S + 2.))
                    .child(div().flex_none().mt(px(2.)).child(if issue { gpui_kit::component::Icon::new(IconName::TriangleAlert).size(px(14.)).text_color(theme.warning) } else { gpui_kit::component::Icon::new(IconName::Info).size(px(14.)).text_color(theme.tertiary) }))
                    .child(div().flex_1().whitespace_normal().text_color(theme.text).child(text_)),
            );
        }
        let figures = div()
            .flex()
            .flex_wrap()
            .gap(px(space::M))
            .child(page_kit::stat_tile("Memories", st.active.to_string(), None, theme))
            .child(page_kit::stat_tile("Index each session", format!("~{index_tokens}"), Some("tokens".into()), theme))
            .child(page_kit::stat_tile("Read, last 30 days", match st.recall_rate() { Some(r) => format!("{:.0}%", r * 100.), None => "—".into() }, Some("of memories in use".into()), theme))
            .child(page_kit::stat_tile("Searches that found something", match st.search_hit_rate() { Some(r) => format!("{:.0}%", r * 100.), None => "—".into() }, Some(if st.searches == 0 { "no searches yet".into() } else { format!("of {}", st.searches) }), theme));
        // Each day of the window: sessions, reads, searches as stacked columns.
        let days = st.days.clone();
        let total: usize = days.iter().map(|d| d.sessions + d.recalled + d.searches).sum();
        let max = days.iter().map(|d| d.sessions + d.recalled + d.searches).max().unwrap_or(0).max(1) as f32;
        let colors: [Hsla; 3] = [theme.accent, gpui_kit::rgb(0x1BAF7A).into(), theme.warning];
        let (surface, base) = (theme.card, theme.separator);
        let chart = gpui_kit::canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let n = days.len().max(1);
                let w = f32::from(bounds.size.width) / n as f32;
                let h = f32::from(bounds.size.height);
                let bar = (w - 4.).clamp(1., 18.);
                // The baseline: a hairline.
                window.paint_quad(gpui_kit::fill(gpui_kit::Bounds::new(gpui_kit::point(bounds.origin.x, bounds.origin.y + px(h - 1.)), gpui_kit::size(bounds.size.width, px(1.))), base));
                for (i, d) in days.iter().enumerate() {
                    let x = f32::from(bounds.origin.x) + i as f32 * w + (w - bar) / 2.;
                    let mut y = f32::from(bounds.origin.y) + h - 1.;
                    let segs: Vec<(usize, f32)> = [d.sessions, d.recalled, d.searches].into_iter().enumerate().map(|(k, v)| (k, v as f32 / max * (h - 6.))).filter(|(_, s)| *s >= 0.5).collect();
                    let last = segs.len().saturating_sub(1);
                    for (j, (k, seg)) in segs.into_iter().enumerate() {
                        let top = j == last;
                        let quad = gpui_kit::fill(gpui_kit::Bounds::new(gpui_kit::point(px(x), px(y - seg)), gpui_kit::size(px(bar), px(seg))), colors[k]);
                        window.paint_quad(if top && bar > 4. { quad.corner_radii(gpui_kit::Corners { top_left: px(3.), top_right: px(3.), bottom_left: px(0.), bottom_right: px(0.) }) } else { quad });
                        y -= seg;
                        if !top {
                            // A 2px gap in the surface's colour between segments.
                            window.paint_quad(gpui_kit::fill(gpui_kit::Bounds::new(gpui_kit::point(px(x), px(y - 1.)), gpui_kit::size(px(bar), px(2.))), surface));
                        }
                    }
                }
            },
        )
        .size_full();
        let legend = div().flex().gap(px(space::M)).text_size(px(text::CAPTION)).text_color(theme.secondary).children(
            [("Sessions", colors[0]), ("Reads", colors[1]), ("Searches", colors[2])].into_iter().map(|(l, c)| div().flex().items_center().gap(px(5.)).child(div().w(px(8.)).h(px(8.)).rounded(px(2.)).bg(c)).child(l)),
        );
        let chart_card = page_kit::card(theme)
            .px(px(space::L))
            .py(px(space::M + 2.))
            .flex()
            .flex_col()
            .gap(px(space::S + 2.))
            .child(div().flex().items_center().child(div().flex_1().child(page_kit::section_title(format!("Last {} days", st.window), None, theme))).child(legend))
            .child(if total == 0 {
                div().h(px(110.)).flex().items_center().justify_center().text_size(px(text::FOOTNOTE)).text_color(theme.tertiary).child("No sessions, reads or searches in this window yet.").into_any_element()
            } else {
                div().h(px(110.)).child(chart).into_any_element()
            });
        // A list as a card of rows: a label left, a figure right.
        let list = |title: &str, items: Vec<(Option<Hsla>, String, Option<String>)>, empty: &str| {
            let mut d = page_kit::card(theme).flex_1().min_w(px(240.)).px(px(space::L)).py(px(space::M)).flex().flex_col().child(div().pb(px(6.)).child(page_kit::section_title(title.to_string(), None, theme)));
            if items.is_empty() {
                d = d.child(div().py(px(3.)).text_size(px(text::FOOTNOTE)).text_color(theme.tertiary).child(empty.to_string()));
            }
            for (i, (color, label, value)) in items.into_iter().enumerate() {
                d = d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S))
                        .py(px(5.))
                        .when(i > 0, |d| d.border_t_1().border_color(theme.separator))
                        .when_some(color, |d, c| d.child(page_kit::dot(c, 7.)))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(text::FOOTNOTE)).child(label))
                        .when_some(value, |d, v| d.child(div().flex_none().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(v))),
                );
            }
            d
        };
        let by_kind = st.by_kind.iter().map(|(k, c)| (Some(kind_color(k, theme)), kind_label(k).to_string(), Some(c.to_string()))).collect();
        let ages = st.age_buckets.iter().map(|(l, c)| (None, l.to_string(), Some(c.to_string()))).collect();
        let row = || div().flex().flex_wrap().gap(px(space::M));
        let content = div()
            .max_w(px(960.))
            .w_full()
            .mx_auto()
            .px(px(space::XL))
            .py(px(space::L + space::XS))
            .flex()
            .flex_col()
            .gap(px(space::M + 2.))
            .child(how)
            .child(figures)
            .child(chart_card)
            .child(row().child(list("By kind", by_kind, "No memories yet.")).child(list("By age", ages, "No memories yet.")))
            .child(
                row()
                    .child(list("Searched for, not found", st.missed_queries.iter().map(|q| (None, format!("“{q}”"), None)).collect(), "Every search found something."))
                    .child(list("Never read, over a week old", st.never_recalled.iter().take(12).map(|m| (Some(kind_color(&m.kind, theme)), m.title.clone(), None)).collect(), "Everything gets read.")),
            )
            .child(
                row()
                    .child(list("Read the most", st.top_recalled.iter().map(|m| (Some(kind_color(&m.kind, theme)), m.title.clone(), Some(format!("{}×", m.recall_count)))).collect(), "Nothing read yet."))
                    .child(list("Look like duplicates", st.likely_duplicates.iter().map(|g| (None, g.iter().map(|m| m.title.clone()).collect::<Vec<_>>().join(" · "), None)).collect(), "None.")),
            );
        div().id("memory-insights").size_full().overflow_y_scroll().child(content).into_any_element()
    }

    /// The memory as a map: a dot a memory (its kind's color, bigger the more it's read), a
    /// line a link. Click a dot to open it.
    fn graph_view(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        if self.graph.ids.is_empty() {
            return page_kit::empty_state(icons::waypoints(), "Nothing to Map Yet", "Memories and the links between them show here as a map once agents save some.", None, theme).into_any_element();
        }
        let (points, edges) = (self.graph.points.clone(), self.graph.edges.clone());
        let dots: Vec<(Hsla, f32, String)> = self
            .graph
            .ids
            .iter()
            .map(|id| {
                let m = self.memories.iter().find(|m| &m.id == id);
                let kind = m.map_or("project", |m| m.kind.as_str());
                let reads = m.map_or(0, |m| m.recall_count) as f32;
                (kind_color(kind, theme), 4. + reads.sqrt().min(8.) * 1.5, m.map(|m| m.title.clone()).unwrap_or_default())
            })
            .collect();
        let selected = self.selection.as_ref().and_then(|s| self.graph.ids.iter().position(|i| i == s));
        let (line, label, ring, cell) = (theme.secondary.opacity(0.28), theme.secondary, theme.canvas, self.graph_bounds.clone());
        let pad = 48.;
        let canvas = gpui_kit::canvas(
            move |bounds, _, _| cell.set(bounds),
            move |bounds, _, window, cx| {
                let (w, h) = (f32::from(bounds.size.width) - pad * 2., f32::from(bounds.size.height) - pad * 2.);
                let side = w.min(h);
                let (ox, oy) = (f32::from(bounds.origin.x) + pad + (w - side) / 2., f32::from(bounds.origin.y) + pad + (h - side) / 2.);
                let at = |p: (f64, f64)| (ox + p.0 as f32 * side, oy + p.1 as f32 * side);
                for &(a, b) in &edges {
                    let (pa, pb) = (at(points[a]), at(points[b]));
                    let mut path = gpui_kit::PathBuilder::stroke(px(1.));
                    path.move_to(gpui_kit::point(px(pa.0), px(pa.1)));
                    path.line_to(gpui_kit::point(px(pb.0), px(pb.1)));
                    if let Ok(p) = path.build() {
                        window.paint_path(p, line);
                    }
                }
                for (i, (color, r, title)) in dots.iter().enumerate() {
                    let (x, y) = at(points[i]);
                    let picked = Some(i) == selected;
                    let r = if picked { r + 3. } else { *r };
                    // A ring in the canvas's colour keeps dots apart where they touch.
                    let ring_r = r + 2.;
                    window.paint_quad(gpui_kit::fill(gpui_kit::Bounds::new(gpui_kit::point(px(x - ring_r), px(y - ring_r)), gpui_kit::size(px(ring_r * 2.), px(ring_r * 2.))), if picked { *color } else { ring }).corner_radii(px(ring_r)));
                    if picked {
                        let inner = r + 0.5;
                        window.paint_quad(gpui_kit::fill(gpui_kit::Bounds::new(gpui_kit::point(px(x - inner), px(y - inner)), gpui_kit::size(px(inner * 2.), px(inner * 2.))), ring).corner_radii(px(inner)));
                    }
                    window.paint_quad(gpui_kit::fill(gpui_kit::Bounds::new(gpui_kit::point(px(x - r + if picked { 2. } else { 0. }), px(y - r + if picked { 2. } else { 0. })), gpui_kit::size(px((r - if picked { 2. } else { 0. }) * 2.), px((r - if picked { 2. } else { 0. }) * 2.))), *color).corner_radii(px(r)));
                    // Titles of the read and the picked, small, beside their dots.
                    if picked || dots.len() <= 40 {
                        let run = gpui_kit::TextRun { len: title.len(), font: window.text_style().font(), color: label, background_color: None, underline: None, strikethrough: None };
                        let shaped = window.text_system().shape_line(title.clone().into(), px(text::CAPTION), &[run], None);
                        let _ = shaped.paint(gpui_kit::point(px(x + r + 5.), px(y - 7.)), px(14.), gpui_kit::TextAlign::Left, None, window, cx);
                    }
                }
            },
        )
        .size_full();
        let (cell, ids, pts) = (self.graph_bounds.clone(), self.graph.ids.clone(), self.graph.points.clone());
        let legend = div()
            .absolute()
            .top(px(space::L))
            .left(px(space::XL))
            .flex()
            .gap(px(space::M))
            .text_size(px(text::CAPTION))
            .text_color(theme.secondary)
            .children(KINDS.iter().map(|(k, l)| div().flex().items_center().gap(px(5.)).child(page_kit::dot(kind_color(k, theme), 7.)).child(*l)))
            .child(div().text_color(theme.tertiary).child("Bigger: read more · Lines: links"));
        div()
            .id("memory-graph")
            .relative()
            .size_full()
            .child(canvas)
            .child(legend)
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(move |this, e: &gpui_kit::MouseDownEvent, _, cx| {
                    let b = cell.get();
                    let (w, h) = (f32::from(b.size.width) - pad * 2., f32::from(b.size.height) - pad * 2.);
                    let side = w.min(h);
                    let (ox, oy) = (f32::from(b.origin.x) + pad + (w - side) / 2., f32::from(b.origin.y) + pad + (h - side) / 2.);
                    let (mx, my) = (f32::from(e.position.x), f32::from(e.position.y));
                    let near = pts
                        .iter()
                        .enumerate()
                        .map(|(i, p)| (i, (ox + p.0 as f32 * side - mx).powi(2) + (oy + p.1 as f32 * side - my).powi(2)))
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                        .filter(|(_, d)| *d < 144.);
                    if let Some((i, _)) = near {
                        this.tab = Tab::Memories;
                        this.archived = false;
                        let id = ids[i].clone();
                        this.select(&id, cx);
                    }
                }),
            )
            .into_any_element()
    }

    fn activity(&self, theme: &Theme) -> gpui_kit::AnyElement {
        if self.events.is_empty() {
            return page_kit::empty_state(icons::activity(), "No Activity Yet", "Agents' saves, reads and searches show here.", None, theme).into_any_element();
        }
        let mut list = page_kit::card(theme).overflow_hidden().flex().flex_col();
        for (n, (kind, title, actor, at, query, count)) in self.events.iter().enumerate() {
            let t = || title.clone().unwrap_or_default();
            let (what, icon): (String, gpui_kit::component::Icon) = match kind.as_str() {
                "created" => (format!("saved “{}”", t()), gpui_kit::component::Icon::new(IconName::Plus)),
                "updated" => (format!("updated “{}”", t()), icons::rotate_ccw()),
                "archived" => (format!("archived “{}”", t()), icons::archive()),
                "restored" => (format!("restored “{}”", t()), icons::archive_restore()),
                "deleted" => (format!("deleted “{}”", t()), icons::trash()),
                "recalled" => (format!("read “{}”", t()), gpui_kit::component::Icon::new(IconName::BookOpen)),
                "searched" => (format!("searched “{}”, {} found", query.clone().unwrap_or_default(), count.unwrap_or(0)), gpui_kit::component::Icon::new(IconName::Search)),
                "imported" => (if t().is_empty() { "imported memories".to_string() } else { format!("imported “{}”", t()) }, icons::download()),
                "injected" => (format!("started a session with {} memories in its index", count.unwrap_or(0)), icons::zap()),
                other => (format!("{other} {}", t()), icons::circle()),
            };
            let who = match actor.as_deref() {
                Some("user") => "You",
                Some("importer") => "The importer",
                Some("reconciler") => "The reconciler",
                Some("app") => "The app",
                _ => "An agent",
            };
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::M))
                    .px(px(space::L))
                    .py(px(space::S))
                    .when(n > 0, |d| d.border_t_1().border_color(theme.separator))
                    .child(div().flex_none().w(px(24.)).h(px(24.)).rounded_full().bg(theme.selected).flex().items_center().justify_center().child(icon.size(px(12.)).text_color(theme.secondary)))
                    .child(div().flex_1().min_w_0().truncate().child(div().flex().gap(px(4.)).child(div().flex_none().font_weight(FontWeight::MEDIUM).child(who)).child(div().min_w_0().truncate().text_color(theme.text).child(what))))
                    .child(div().flex_none().text_size(px(text::CAPTION)).text_color(theme.tertiary).child(ago(*at))),
            );
        }
        div()
            .id("memory-activity")
            .size_full()
            .overflow_y_scroll()
            .child(div().max_w(px(880.)).w_full().mx_auto().px(px(space::XL)).py(px(space::L + space::XS)).child(list))
            .into_any_element()
    }
}

impl MemoryPage {
    /// Toward one memory: shown while Claude Code still keeps its own for the folder, or has
    /// memories YourHarness doesn't. One button brings them in and turns it off.
    fn claude_nudge(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let status = self.claude.clone()?;
        let dismissed = self.workspace.read(cx).library.setting("claudeMemoryNudgeDismissed").and_then(|v| v.as_array().map(|a| a.iter().any(|x| x.as_str() == Some(self.project.as_str())))).unwrap_or(false);
        if !status.needs_nudge() || (dismissed && status.unimported.is_empty()) {
            return None;
        }
        let count = status.unimported.len();
        let memories = if count == 1 { "1 memory".to_string() } else { format!("{count} memories") };
        let message = match (status.is_on, count > 0) {
            (true, true) => format!("Claude Code also keeps its own memory for this folder: {memories} there aren't in YourHarness, and claude sessions you start outside the app keep adding to it. Two memories drift apart; bring them in and keep one."),
            (true, false) if status.is_home => "Claude Code still keeps its own memory when you run claude outside YourHarness, so two memories would drift apart.".into(),
            (true, false) => "Claude Code still keeps its own memory when you run claude in this folder outside YourHarness, so two memories would drift apart.".into(),
            _ => format!("Claude Code has {memories} for this folder that YourHarness doesn't."),
        };
        let primary = match (status.is_on, count > 0) {
            (true, true) => "Import & Use One Memory",
            (true, false) if status.is_home => "Use One Memory Everywhere",
            (true, false) => "Use One Memory",
            _ => "Import",
        };
        let detail = if status.is_home {
            "Turns off Claude Code's memory in your Claude Code settings (autoMemoryEnabled: false), for every folder."
        } else {
            "Turns off Claude Code's memory for this folder only: autoMemoryEnabled: false in its .claude/settings.local.json, which stays out of git. Its memory files are left as they are."
        };
        let on = status.is_on;
        let s1 = status.clone();
        Some(
            div()
                .mx(px(space::L))
                .mt(px(space::M))
                .px(px(space::L))
                .py(px(space::M))
                .rounded(px(radius::LARGE))
                .bg(theme.warning.opacity(0.07))
                .border_1()
                .border_color(theme.warning.opacity(0.28))
                .flex()
                .gap(px(space::M))
                .child(div().flex_none().mt(px(1.)).child(icons::layers().size(px(16.)).text_color(theme.warning)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(div().text_size(px(text::HEADLINE)).font_weight(FontWeight::SEMIBOLD).child(if on { "Two memories" } else { "Not in YourHarness yet" }))
                        .child(div().text_color(theme.text).child(message))
                        .child(
                            div()
                                .pt(px(2.))
                                .flex()
                                .flex_wrap()
                                .gap(px(space::S))
                                .child(Button::new("one-memory").small().primary().label(primary).tooltip(if on { detail } else { "Brings Claude Code's memory files for this folder in." }).on_click(cx.listener(move |this, _, _, cx| this.use_one_memory(&s1, cx))))
                                .when(count > 0, |d| {
                                    d.child(Button::new("memory-importer").small().outline().label("Open in the Importer…").on_click(cx.listener(|this, _, window, cx| {
                                        // Claude Code's memory for the folder is found and ticked by the setup.
                                        open_setup(this.workspace.clone(), "importer", this.project.clone(), vec![], window, cx);
                                    })))
                                    .child(Button::new("memory-already-in").small().outline().label("They're Already In").on_click(cx.listener(|this, _, _, cx| this.mark_synced(cx))))
                                })
                                .when(on, |d| {
                                    d.child(Button::new("memory-not-now").small().ghost().label("Not Now").on_click(cx.listener(|this, _, _, cx| {
                                        let lib = this.workspace.read(cx).library.clone();
                                        let mut list = lib.setting("claudeMemoryNudgeDismissed").and_then(|v| v.as_array().cloned()).unwrap_or_default();
                                        list.push(serde_json::Value::from(this.project.clone()));
                                        lib.set_setting("claudeMemoryNudgeDismissed", &serde_json::Value::Array(list));
                                        cx.notify();
                                    })))
                                }),
                        )
                        .when(on, |d| d.child(div().text_size(px(text::CAPTION)).text_color(theme.secondary).child(detail))),
                )
                .into_any_element(),
        )
    }

    fn mark_synced(&mut self, cx: &mut Context<Self>) {
        let lib = self.workspace.read(cx).library.clone();
        let mut all = lib.setting("claudeMemorySyncedAt").filter(|v| v.is_object()).unwrap_or(serde_json::json!({}));
        all[self.project.as_str()] = serde_json::Value::from(yh_core::library::now_ms() / 1000.);
        lib.set_setting("claudeMemorySyncedAt", &all);
        self.reload(cx);
    }

    /// What Claude Code had comes in, then its own memory is turned off for the folder (or
    /// everywhere, for Home), so `claude` in Terminal stops keeping a second one.
    fn use_one_memory(&mut self, status: &ClaudeMemory, cx: &mut Context<Self>) {
        if !status.unimported.is_empty() {
            self.import_claude_code(cx);
        }
        let result = match (status.is_on, self.project_path(cx)) {
            (true, Some(folder)) => {
                use yh_core::memory_archive::claude_code;
                if status.is_home {
                    claude_code::turn_off(&yh_core::history::config_directory().join("settings.json"))
                } else {
                    let r = claude_code::turn_off(&claude_code::local_settings(&folder));
                    claude_code::exclude_from_git(&folder);
                    r
                }
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            self.notice = Some(format!("Couldn't turn off Claude Code's memory: {e}"));
        }
        self.mark_synced(cx);
    }
}

impl Render for MemoryPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(m) = self.pending_fill.take() {
            self.fill(m, window, cx);
        }
        let theme = Theme::get(cx);
        let content = match self.tab {
            Tab::Memories => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .children(self.claude_nudge(&theme, cx))
                .child(div().flex_1().min_h_0().flex().child(self.list(&theme, cx)).child(self.detail(&theme, cx)))
                .into_any_element(),
            Tab::Tasks => div().flex_1().min_h_0().child(self.tasks(&theme, cx)).into_any_element(),
            Tab::Activity => div().flex_1().min_h_0().child(self.activity(&theme)).into_any_element(),
            Tab::Insights => div().flex_1().min_h_0().child(self.insights(&theme, cx)).into_any_element(),
            Tab::Graph => div().flex_1().min_h_0().child(self.graph_view(&theme, cx)).into_any_element(),
        };
        div()
            .key_context("MemoryPage")
            .on_action(cx.listener(|this, _: &SaveMemory, _, cx| this.save(cx)))
            .size_full()
            .flex()
            .flex_col()
            .child(self.header(&theme, cx))
            .child(content)
    }
}

gpui_kit::actions!(memory, [SaveMemory]);

// MARK: The specialists' setup

/// A memory specialist's thread ID, as the workspace names it.
pub fn agent_id(role: &str, project: &str) -> String {
    format!("{}{role}:{project}", crate::state::workspace::MEMORY_AGENT)
}

/// One source the Importer could read, ticked or not.
struct Source {
    path: String,
    title: String,
    detail: String,
    on: bool,
}

/// Before a specialist starts, as the Swift app's MemoryAgentWindow does: the Importer's
/// sources (Claude Code's memory for the folder, instruction files, files you add, pasted text)
/// and what it should know; the Reconciler's starting point and what to focus on; for both,
/// the model and effort. Start opens the specialist's conversation and sets it going.
pub struct SpecialistSetup {
    workspace: Entity<Workspace>,
    project: String,
    role: &'static str,
    sources: Vec<Source>,
    pasted: Entity<TextareaState>,
    note: Entity<InputState>,
    model: Option<String>,
    effort: Option<String>,
    stats: Option<yh_core::memory_stats::Stats>,
}

/// Opens the setup for `role` (`importer` or `reconciler`) over the window; `sources` come
/// ticked (a folder dropped on Import, Claude Code's memory from the nudge).
pub fn open_setup(workspace: Entity<Workspace>, role: &str, project: String, sources: Vec<String>, window: &mut Window, cx: &mut App) -> Entity<SpecialistSetup> {
    let role: &'static str = if role == "importer" { "importer" } else { "reconciler" };
    let view = cx.new(|cx| SpecialistSetup::new(workspace, role, project, sources, window, cx));
    let title = if role == "importer" { "Memory Importer" } else { "Memory Reconciler" };
    let v = view.clone();
    window.open_dialog(cx, move |d, _, _| d.title(title).w(px(640.)).child(v.clone()));
    view
}

impl SpecialistSetup {
    fn new(workspace: Entity<Workspace>, role: &'static str, project: String, given: Vec<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pasted = cx.new(|cx| TextareaState::new(window, cx).placeholder("Paste notes, an export, anything to remember"));
        let hint = if role == "importer" { "e.g. only what's about the iOS app; I'm the tech lead" } else { "e.g. merge the deploy notes; drop what's about the old API" };
        let note = cx.new(|cx| InputState::new(window, cx).placeholder(hint));
        if let Ok(n) = std::env::var("YH_MEMORY_AGENT_NOTE") {
            note.update(cx, |i, cx| i.set_value(n, window, cx));
        }
        // The specialist's model: what it had, else what Settings gives the specialists.
        let (model, effort) = {
            let ws = workspace.read(cx);
            let thread = ws.thread(&agent_id(role, &project));
            let setting = |key: &str| ws.library.setting(key).and_then(|v| v.as_str().map(str::to_string)).filter(|v| !v.is_empty());
            (
                std::env::var("YH_MEMORY_AGENT_MODEL").ok().or_else(|| thread.as_ref().and_then(|t| t.read(cx).meta.model.clone())).or_else(|| setting("memoryAgentModel")),
                thread.as_ref().and_then(|t| t.read(cx).meta.effort.clone()).or_else(|| setting("memoryAgentEffort")),
            )
        };
        let sources = given.iter().map(|p| Source { path: p.clone(), title: file_name(p), detail: p.clone(), on: true }).collect();
        let mut setup = SpecialistSetup { workspace, project, role, sources, pasted, note, model, effort, stats: None };
        setup.find(cx);
        setup
    }

    /// Looks, off the main thread, for what the Importer could read (Claude Code's memory for
    /// the folder, CLAUDE.md and AGENTS.md), or what the Reconciler starts from.
    fn find(&mut self, cx: &mut Context<Self>) {
        let folder = self.workspace.read(cx).project(&self.project).map(|p| p.path.clone());
        if self.role == "reconciler" {
            let Some(store) = self.workspace.read(cx).memory_store() else { return };
            let scope = yh_core::memory::scope_of(&self.project);
            cx.spawn(async move |this, cx| {
                let stats = cx.background_executor().spawn(async move { yh_core::memory_stats::compute(&store, &scope, 30) }).await;
                let _ = this.update(cx, |s, cx| {
                    s.stats = Some(stats);
                    cx.notify();
                });
            })
            .detach();
            return;
        }
        let Some(folder) = folder else { return };
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let config = yh_core::history::config_directory();
                    let mut found = vec![];
                    let memory = yh_core::memory_archive::claude_code::files(&folder, &config);
                    if !memory.is_empty() {
                        let dir = yh_core::memory_archive::claude_code::directory(&folder, &config);
                        found.push(Source {
                            path: dir.to_string_lossy().into_owned(),
                            title: "Claude Code's memory for this folder".into(),
                            detail: format!("{} memor{}", memory.len(), if memory.len() == 1 { "y" } else { "ies" }),
                            on: true,
                        });
                    }
                    for name in ["CLAUDE.md", "AGENTS.md", ".claude/CLAUDE.md", "CLAUDE.local.md"] {
                        let path = std::path::Path::new(&folder).join(name);
                        if path.is_file() {
                            found.push(Source {
                                path: path.to_string_lossy().into_owned(),
                                title: name.to_string(),
                                detail: "Agents already read this every session; import only what belongs in memory".into(),
                                on: false,
                            });
                        }
                    }
                    found
                })
                .await;
            let _ = this.update(cx, |s, cx| {
                for f in found {
                    if !s.sources.iter().any(|x| x.path == f.path) {
                        s.sources.insert(0, f);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn add_sources(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: true, directories: true, multiple: true, prompt: Some("Add".into()) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else { return };
            let _ = this.update(cx, |s, cx| {
                for p in paths {
                    let path = p.to_string_lossy().into_owned();
                    if !s.sources.iter().any(|x| x.path == path) {
                        s.sources.push(Source { title: file_name(&path), detail: path.clone(), path, on: true });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn can_start(&self, cx: &App) -> bool {
        self.role == "reconciler" || self.sources.iter().any(|s| s.on) || !self.pasted.read(cx).value().trim().is_empty()
    }

    /// Opens the specialist with its model, and sends what starts it.
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_start(cx) {
            return;
        }
        let note = self.note.read(cx).value().trim().to_string();
        let mut sources: Vec<String> = self.sources.iter().filter(|s| s.on).map(|s| s.path.clone()).collect();
        let pasted = self.pasted.read(cx).value().trim().to_string();
        if !pasted.is_empty() {
            // Pasted text is saved where state exports find it, then read like any file.
            let dir = self.workspace.read(cx).data_dir.join("memory-imports");
            let file = dir.join(format!("pasted-{}.txt", yh_core::library::now_ms() as u64));
            if std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, &pasted)).is_ok() {
                sources.push(file.to_string_lossy().into_owned());
            }
        }
        let message = if self.role == "importer" {
            yh_core::memory_agents::import_request(&sources, Some(&note))
        } else if note.is_empty() {
            yh_core::memory_agents::RECONCILE_REQUEST.to_string()
        } else {
            format!("{}\n{note}", yh_core::memory_agents::RECONCILE_REQUEST)
        };
        let (role, project, model, effort) = (self.role, self.project.clone(), self.model.clone(), self.effort.clone());
        let id = agent_id(role, &project);
        window.close_dialog(cx);
        if std::env::var("YH_DATA_DIR").is_ok() {
            eprintln!("MEMORY {role} starts with {} sources, model {model:?}, effort {effort:?}", sources.len());
        }
        self.workspace.update(cx, |ws, cx| {
            ws.open_memory_agent(role, &project, if role == "importer" { sources } else { vec![] }, None, cx);
            ws.set_model(&id, model, cx);
            ws.set_effort(&id, effort, cx);
            if let Some(t) = ws.thread(&id) {
                ws.send(&t, &message, false, cx);
            }
        });
    }
}

fn file_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string())
}

/// Model and effort menus for a memory specialist: `on_pick` gets (model, effort).
pub fn model_menus(id: &str, ws: &Workspace, model: Option<String>, effort: Option<String>, on_pick: impl Fn(Option<String>, Option<String>, &mut App) + 'static) -> gpui_kit::AnyElement {
    let models: Vec<(String, String, Vec<String>)> = if ws.models.is_empty() {
        [("haiku", "Haiku"), ("sonnet", "Sonnet"), ("opus", "Opus")].iter().map(|(v, n)| (v.to_string(), n.to_string(), vec![])).collect()
    } else {
        ws.models.iter().map(|m| (m.value.clone(), m.display_name.clone(), m.efforts.clone())).collect()
    };
    let default = std::env::var("YH_MODEL").ok().or_else(|| ws.library.setting("defaults").and_then(|d| d["model"].as_str().map(str::to_string))).filter(|s| !s.is_empty());
    let name = |m: &Option<String>| m.as_ref().map(|m| models.iter().find(|(v, ..)| v == m).map_or(m.clone(), |(_, n, _)| n.clone()));
    let model_label = name(&model).unwrap_or_else(|| format!("Default ({})", name(&default).unwrap_or_else(|| "Claude Code's".into())));
    let efforts = models.iter().find(|(v, ..)| Some(v) == model.as_ref().or(default.as_ref())).map(|(.., e)| e.clone()).filter(|e| !e.is_empty()).unwrap_or_else(|| vec!["low".into(), "medium".into(), "high".into()]);
    let on_pick = std::rc::Rc::new(on_pick);
    let (p1, p2) = (on_pick.clone(), on_pick);
    let (e1, m2) = (effort.clone(), model.clone());
    let effort_label = effort.clone().map(|e| { let mut c = e.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() }).unwrap_or_else(|| "Auto effort".into());
    div()
        .flex()
        .gap(px(4.))
        .child(Button::new(SharedString::from(format!("{id}-model"))).small().outline().label(model_label).dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
            let p = p1.clone();
            let e = e1.clone();
            menu = menu.item(PopupMenuItem::new("Default").on_click(move |_, _, cx| p(None, e.clone(), cx)));
            for (v, n, _) in &models {
                let (p, v, e) = (p1.clone(), v.clone(), e1.clone());
                menu = menu.item(PopupMenuItem::new(n.clone()).on_click(move |_, _, cx| p(Some(v.clone()), e.clone(), cx)));
            }
            menu
        }))
        .child(Button::new(SharedString::from(format!("{id}-effort"))).small().outline().label(effort_label).dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
            let (p, m) = (p2.clone(), m2.clone());
            menu = menu.item(PopupMenuItem::new("Auto").on_click(move |_, _, cx| p(m.clone(), None, cx)));
            for level in &efforts {
                let (p, m, l) = (p2.clone(), m2.clone(), level.clone());
                let mut label = level.clone();
                if let Some(f) = label.get_mut(0..1) {
                    f.make_ascii_uppercase();
                }
                menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| p(m.clone(), Some(l.clone()), cx)));
            }
            menu
        }))
        .into_any_element()
}

impl Render for SpecialistSetup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let importer = self.role == "importer";
        let project_name = self.workspace.read(cx).project(&self.project).map(|p| p.name.clone()).unwrap_or_default();
        let label = |t: &str| page_kit::group_label(t.to_string(), &theme);
        let (title, detail) = if importer {
            (format!("Bring memory into {project_name}"), "The Importer reads what you give it (Claude Code's memory, notes, CLAUDE.md files, exports from ChatGPT or other assistants) and turns it into this workspace's memory: one fact a memory, sorted into kinds, without duplicates or secrets. It asks when something's unclear.")
        } else {
            (format!("Tidy up {project_name}'s memory"), "The Reconciler goes through the memory with you: it merges duplicates, fixes or archives what's out of date (checking the code when a memory names it), tightens long or vague memories and links related ones. It shows you the plan before it changes anything, and every change keeps the old version.")
        };
        let mut column = div().id("specialist-setup").max_h(px(520.)).overflow_y_scroll().flex().flex_col().gap(px(space::L + 2.)).child(
            div()
                .flex()
                .gap(px(space::M))
                .child(page_kit::icon_tile(if importer { icons::download() } else { icons::sparkles() }, 36., &theme))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(div().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).child(title))
                        .child(div().text_size(px(text::FOOTNOTE)).line_height(px(17.)).text_color(theme.secondary).child(detail)),
                ),
        );
        if importer {
            let mut list = page_kit::card(&theme).overflow_hidden().flex().flex_col();
            if self.sources.is_empty() {
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S))
                        .px(px(space::M))
                        .py(px(space::M))
                        .text_size(px(text::FOOTNOTE))
                        .text_color(theme.secondary)
                        .child(gpui_kit::component::Icon::new(IconName::Inbox).size(px(15.)).text_color(theme.tertiary))
                        .child("No sources yet. Add files or folders, or paste text below."),
                );
            }
            for (i, s) in self.sources.iter().enumerate() {
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S + 2.))
                        .px(px(space::M))
                        .py(px(space::S))
                        .when(i > 0, |d| d.border_t_1().border_color(theme.separator))
                        .child(gpui_kit::component::checkbox::Checkbox::new(SharedString::from(format!("source-{i}"))).checked(s.on).on_click(cx.listener(move |this, on: &bool, _, cx| {
                            if let Some(s) = this.sources.get_mut(i) {
                                s.on = *on;
                            }
                            cx.notify();
                        })))
                        .child(gpui_kit::component::Icon::new(IconName::FileText).size(px(15.)).text_color(theme.tertiary))
                        .child(div().flex_1().min_w_0().flex().flex_col().child(div().truncate().child(s.title.clone())).child(div().text_size(px(text::CAPTION)).text_color(theme.secondary).truncate().child(s.detail.clone()))),
                );
            }
            column = column
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(space::S))
                        .child(div().flex().items_center().child(div().flex_1().child(label("Sources"))).child(Button::new("add-sources").small().ghost().icon(IconName::Plus).label("Add Files or Folders…").on_click(cx.listener(|this, _, _, cx| this.add_sources(cx)))))
                        .child(list),
                )
                .child(div().flex().flex_col().gap(px(space::S)).child(label("Or paste text")).child(page_kit::card(&theme).px(px(space::S)).py(px(6.)).child(Textarea::new(&self.pasted).appearance(false).h(px(90.)))));
        } else if let Some(st) = &self.stats {
            let mut facts: Vec<(gpui_kit::component::Icon, String)> = vec![(icons::brain(), format!("{} memories in use, {:.1} KB", st.active, st.bytes as f64 / 1024.))];
            if !st.never_recalled.is_empty() {
                facts.push((gpui_kit::component::Icon::new(IconName::EyeOff), format!("{} never read by an agent", st.never_recalled.len())));
            }
            if !st.likely_duplicates.is_empty() {
                facts.push((gpui_kit::component::Icon::new(IconName::Copy), format!("{} groups that look like duplicates", st.likely_duplicates.len())));
            }
            if !st.missed_queries.is_empty() {
                facts.push((gpui_kit::component::Icon::new(IconName::Search), format!("{} searches that found nothing", st.missed_queries.len())));
            }
            if st.orphans > 0 {
                facts.push((icons::link(), format!("{} memories without links", st.orphans)));
            }
            let mut list = page_kit::card(&theme).overflow_hidden().flex().flex_col();
            for (i, (icon, f)) in facts.into_iter().enumerate() {
                list = list.child(div().flex().items_center().gap(px(space::S + 2.)).px(px(space::M)).py(px(space::S)).when(i > 0, |d| d.border_t_1().border_color(theme.separator)).child(icon.size(px(14.)).text_color(theme.secondary)).child(f));
            }
            column = column.child(div().flex().flex_col().gap(px(space::S)).child(label("What it starts from")).child(list));
        }
        let this = cx.entity().downgrade();
        let menus = model_menus(&format!("setup-{}", self.role), self.workspace.read(cx), self.model.clone(), self.effort.clone(), move |m, e, cx| {
            let _ = this.update(cx, |s, cx| {
                s.model = m;
                s.effort = e;
                cx.notify();
            });
        });
        column = column
            .child(
                div().flex().flex_col().gap(px(space::S)).child(label("Model")).child(
                    div().flex().items_center().gap(px(space::M)).child(menus).child(div().flex_1().min_w_0().text_size(px(text::CAPTION)).text_color(theme.secondary).child("A stronger model sorts and merges memory more carefully.")),
                ),
            )
            .child(div().flex().flex_col().gap(px(space::S)).child(label(if importer { "Anything the Importer should know" } else { "Anything to focus on" })).child(Input::new(&self.note)));
        div().flex().flex_col().gap(px(space::L)).child(column).child(
            div().flex().justify_end().gap(px(space::S)).child(Button::new("setup-cancel").outline().label("Cancel").on_click(|_, window, cx| window.close_dialog(cx))).child(
                Button::new("setup-start")
                    .primary()
                    .label(if importer { "Start Import" } else { "Start Review" })
                    .disabled(!self.can_start(cx))
                    .on_click(cx.listener(|this, _, window, cx| this.start(window, cx))),
            ),
        )
    }
}
