//! A project's Memory page: what its agents remember (search, filter by kind, read, edit, pin,
//! archive, delete, write a new one, see how each changed), the tasks they track, and what
//! they did with memory. Shown in place of the thread when you click a project's name.
//! Memory is read off the main thread, again whenever it changes (`memory_revision`).

use crate::state::workspace::Workspace;
use crate::theme::{Theme, MONO};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenuItem};
use gpui_kit::component::Sizable;
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
        "task" => gpui_kit::rgb(0x34c759).into(),
        _ => theme.accent,
    }
}

fn state_color(state: &str, theme: &Theme) -> Hsla {
    match state {
        "blocked" => theme.warning,
        "done" => gpui_kit::rgb(0x34c759).into(),
        "todo" => theme.secondary,
        _ => theme.accent,
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
    _subscriptions: Vec<Subscription>,
}

impl MemoryPage {
    pub fn new(workspace: Entity<Workspace>, project: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = workspace.read(cx).memory_store();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search memories"));
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("Title"));
        let description = cx.new(|cx| InputState::new(window, cx).placeholder("One line: when it matters"));
        let body = cx.new(|cx| TextareaState::new(window, cx).placeholder("What to remember"));
        let subscriptions = vec![
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

    fn select(&mut self, id: &str, cx: &mut Context<Self>) {
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

    /// Asks for files or folders, then hands them to the Importer.
    fn import_with_agent(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: true, directories: true, multiple: true, prompt: Some("Import".into()) });
        let (ws, project) = (self.workspace.clone(), self.project.clone());
        cx.spawn(async move |_, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else { return };
            let sources: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
            let _ = cx.update(|cx| {
                ws.update(cx, |ws, cx| {
                    let message = yh_core::memory_agents::import_request(&sources, None);
                    ws.open_memory_agent("importer", &project, sources, Some(message), cx)
                })
            });
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
    div()
        .flex_none()
        .px(px(7.))
        .py(px(1.))
        .rounded(px(9.))
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .bg(color.opacity(0.14))
        .child(label.into())
}

fn section(title: &str, theme: &Theme) -> gpui_kit::Div {
    div().pt(px(6.)).text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).child(title.to_uppercase())
}

impl MemoryPage {
    fn header(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.workspace.read(cx).project(&self.project).map(|p| {
            if self.workspace.read(cx).home_project().as_deref() == Some(p.id.as_str()) { "Home".to_string() } else { p.name.clone() }
        });
        let memory_on = self.workspace.read(cx).library.setting("memoryEnabled").and_then(|v| v.as_bool()).unwrap_or(true);
        let tab_button = |id: &'static str, label: &'static str, tab: Tab, cx: &mut Context<Self>| {
            let mut b = Button::new(id).small().label(label);
            b = if self.tab == tab { b.primary() } else { b.ghost() };
            b.on_click(cx.listener(move |this, _, _, cx| {
                this.tab = tab;
                cx.notify();
            }))
        };
        let tasks = self.memories.iter().filter(|m| m.is_task() && m.status == "active").count();
        // Back to the project's PRs and repos (Home has none).
        let is_home = self.workspace.read(cx).home_project().as_deref() == Some(self.project.as_str());
        let (ws_overview, project_overview) = (self.workspace.clone(), self.project.clone());
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(14.))
            .px(px(28.))
            .py(px(16.))
            .border_b_1()
            .border_color(theme.separator)
            .child(
                div()
                    .flex_1()
                    .min_w(px(220.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).truncate().child(format!("{} Memory", name.unwrap_or_default())))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(div().text_color(theme.secondary).truncate().child(if self.loaded { self.summary() } else { "Loading…".into() }))
                            .children(self.claude.as_ref().filter(|c| !c.needs_nudge()).map(|c| {
                        // One memory: Claude Code's own is off here and nothing of it is left out.
                        let tip = if c.files > 0 {
                            format!("Claude Code's own memory is off for this folder, and its {} memory files are in YourHarness.", c.files)
                        } else {
                            "Claude Code's own memory is off for this folder: YourHarness keeps the only memory.".to_string()
                        };
                        div().id("one-memory").ml(px(10.)).text_size(px(12.)).text_color(Theme::get(cx).secondary).child("✓ One memory").tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                    })),
                    )
                    .when_some(self.notice.clone(), |d, n| d.child(div().text_size(px(12.)).text_color(theme.accent).truncate().child(n))),
            )
            .when(!memory_on, |d| d.child(chip("Off for agents", theme.warning)))
            .child(
                div()
                    .flex()
                    .gap(px(2.))
                    .when(!is_home, |d| {
                        d.child(Button::new("tab-overview").small().ghost().label("Overview").on_click(move |_, _, cx| ws_overview.update(cx, |ws, cx| ws.open_project(&project_overview, cx))))
                    })
                    .child(tab_button("tab-memories", "Memories", Tab::Memories, cx))
                    .child(tab_button("tab-tasks", if tasks > 0 { "Tasks ·" } else { "Tasks" }, Tab::Tasks, cx).when(tasks > 0, |b| b.label(format!("Tasks {tasks}"))))
                    .child(tab_button("tab-insights", "Insights", Tab::Insights, cx))
                    .child(tab_button("tab-activity", "Activity", Tab::Activity, cx))
                    .child(tab_button("tab-graph", "Graph", Tab::Graph, cx)),

            )
            .child({
                let page = cx.entity().downgrade();
                let claude_files = self.claude_code_files(cx).len();
                Button::new("memory-import").small().ghost().label("Import").dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
                    let (a, b, c) = (page.clone(), page.clone(), page.clone());
                    menu = menu
                        .item(PopupMenuItem::new("With the Importer…").on_click(move |_, _, cx| {
                            let _ = a.update(cx, |p, cx| p.import_with_agent(cx));
                        }))
                        .item(PopupMenuItem::new("File or Folder…").on_click(move |_, _, cx| {
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
                Button::new("memory-export").small().ghost().label("Export").dropdown_caret(true).dropdown_menu(move |menu, _, _| {
                    let (a, b) = (page.clone(), page.clone());
                    menu.item(PopupMenuItem::new("As JSON…").on_click(move |_, _, cx| {
                        let _ = a.update(cx, |p, cx| p.export_dialog(false, cx));
                    }))
                    .item(PopupMenuItem::new("As Markdown Folder…").on_click(move |_, _, cx| {
                        let _ = b.update(cx, |p, cx| p.export_dialog(true, cx));
                    }))
                })
            })
            .child(Button::new("memory-reconcile").small().ghost().label("Reconcile").tooltip("The Reconciler finds duplicates, stale and vague memories, and fixes them with you").on_click(cx.listener(|this, _, _, cx| {
                let project = this.project.clone();
                this.workspace.update(cx, |ws, cx| ws.open_memory_agent("reconciler", &project, vec![], Some(yh_core::memory_agents::RECONCILE_REQUEST.to_string()), cx));
            })))
            .child(Button::new("new-memory").small().label("New Memory").on_click(cx.listener(|this, _, _, cx| this.new_memory(cx))))
    }

    fn list(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let visible = self.visible(cx);
        let count = visible.len();
        let mut chips = div().flex().flex_wrap().gap(px(4.));
        let filter_chip = |id: String, label: &'static str, on: bool, color: Hsla, value: Option<&'static str>, cx: &mut Context<Self>| {
            div()
                .id(SharedString::from(id))
                .px(px(8.))
                .py(px(2.))
                .rounded(px(9.))
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if on { color } else { theme.secondary })
                .bg(if on { color.opacity(0.16) } else { theme.selected })
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.kind_filter = if this.kind_filter == value { None } else { value };
                    cx.notify();
                }))
        };
        chips = chips.child(filter_chip("kind-all".into(), "All", self.kind_filter.is_none(), theme.accent, None, cx));
        for (kind, label) in KINDS {
            chips = chips.child(filter_chip(format!("kind-{kind}"), label, self.kind_filter == Some(kind), kind_color(kind, theme), Some(kind), cx));
        }
        let status = |id: &'static str, label: &'static str, archived: bool, cx: &mut Context<Self>| {
            let b = Button::new(id).xsmall().label(label);
            (if self.archived == archived { b.primary() } else { b.ghost() }).on_click(cx.listener(move |this, _, _, cx| {
                this.archived = archived;
                cx.notify();
            }))
        };
        let mut rows = div().id("memory-list").flex_1().min_h_0().overflow_y_scroll().p(px(8.)).flex().flex_col().gap(px(2.));
        if count == 0 {
            rows = rows.child(div().p(px(16.)).text_color(theme.secondary).child(if self.memories.is_empty() { "Nothing remembered yet." } else { "No memories match." }));
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
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(7.))
                    .when(selected, |d| d.bg(theme.accent.opacity(0.16)))
                    .when(!selected, |d| d.hover(|s| s.bg(theme.selected)))
                    .flex()
                    .gap(px(9.))
                    .child(div().mt(px(6.)).w(px(7.)).h(px(7.)).flex_none().rounded_full().bg(kind_color(&m.kind, theme)))
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
                                    .gap(px(4.))
                                    .child(div().font_weight(FontWeight::MEDIUM).overflow_hidden().whitespace_nowrap().text_ellipsis().child(if m.title.is_empty() { "Untitled".to_string() } else { m.title.clone() }))
                                    .when(m.pinned, |d| d.child(div().text_size(px(10.)).text_color(theme.secondary).child("pinned"))),
                            )
                            .child(div().text_size(px(12.)).text_color(theme.secondary).line_clamp(2).child(preview))
                            .when(m.is_task(), |d| {
                                let state = m.task_state.clone().unwrap_or_else(|| "active".into());
                                d.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .child(chip(yh_core::memory::state_label(&state), state_color(&state, theme)))
                                        .when(!m.steps.is_empty(), |d| {
                                            d.child(div().text_size(px(11.)).text_color(theme.secondary).child(format!("{} of {} steps", m.steps_done(), m.steps.len())))
                                        }),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .gap(px(8.))
                                    .text_size(px(11.))
                                    .text_color(theme.secondary.opacity(0.8))
                                    .child(format!("read {}×", m.recall_count))
                                    .when(!m.links.is_empty(), |d| d.child(format!("{} links", m.links.len())))
                                    .child(div().flex_1())
                                    .child(ago(m.updated_at)),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.select(&id, cx)))
                    .context_menu(move |menu, _, _| {
                        let (a, b, c) = (page.clone(), page.clone(), page.clone());
                        let (i1, i2, i3) = (pin_id.clone(), status_id.clone(), del_id.clone());
                        menu.item(PopupMenuItem::new(if pinned { "Unpin" } else { "Pin (Always Load in Full)" }).on_click(move |_, _, cx| {
                            let _ = a.update(cx, |p, cx| p.set_pinned(&i1, !pinned, cx));
                        }))
                        .item(PopupMenuItem::new(if archived { "Restore" } else { "Archive" }).on_click(move |_, _, cx| {
                            let _ = b.update(cx, |p, cx| p.set_status(&i2, if archived { "active" } else { "archived" }, cx));
                        }))
                        .separator()
                        .item(PopupMenuItem::new("Delete").on_click(move |_, _, cx| {
                            let _ = c.update(cx, |p, cx| p.delete(&i3, cx));
                        }))
                    }),
            );
        }
        div()
            .w(px(320.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme.separator)
            .child(
                div()
                    .p(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .border_b_1()
                    .border_color(theme.separator)
                    .child(Input::new(&self.search).small().cleanable(true))
                    .child(chips)
                    .child(div().flex().gap(px(2.)).child(status("in-use", "In Use", false, cx)).child(status("archived", "Archived", true, cx))),
            )
            .child(rows)
            .child(div().py(px(5.)).text_center().text_size(px(11.)).text_color(theme.secondary).child(if count == 1 { "1 memory".to_string() } else { format!("{count} memories") }))
    }

    fn detail(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(m) = self.editing.clone().filter(|e| Some(&e.id) == self.selection.as_ref()) else {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.secondary)
                .child("Choose a memory to see what it says, how agents use it and how it changed.")
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
            Button::new("kind").xsmall().ghost().label(kind_label(&self.draft_kind)).dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
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
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().w(px(9.)).h(px(9.)).rounded_full().bg(kind_color(&self.draft_kind, theme)))
                    .child(kind_menu)
                    .when(pinned, |d| d.child(chip("Pinned", theme.secondary)))
                    .when(current.status != "active", |d| d.child(chip(if archived { "Archived" } else { "Proposed" }, theme.warning)))
                    .when(listed, |d| d.child(div().font_family(MONO).text_size(px(11.)).text_color(theme.secondary).child(current.short_id())))
                    .child(div().flex_1())
                    .when(listed, |d| {
                        d.child(Button::new("pin").xsmall().ghost().label(if pinned { "Unpin" } else { "Pin" }).tooltip("Pinned: every session gets it in full, not only its index line").on_click(
                            cx.listener(move |this, _, _, cx| this.set_pinned(&pin_id, !pinned, cx)),
                        ))
                        .child(Button::new("archive").xsmall().ghost().label(if archived { "Restore" } else { "Archive" }).on_click(cx.listener(move |this, _, _, cx| {
                            this.set_status(&status_id, if archived { "active" } else { "archived" }, cx)
                        })))
                        .child(Button::new("delete").xsmall().ghost().label("Delete").on_click(cx.listener(move |this, _, _, cx| this.delete(&del_id, cx))))
                    }),
            )
            .child(Input::new(&self.title).appearance(false).text_size(px(20.)).font_weight(FontWeight::SEMIBOLD))
            .child(Input::new(&self.description).appearance(false).text_color(theme.secondary))
            .child(div().p(px(10.)).rounded(px(8.)).bg(theme.card).child(Textarea::new(&self.body).appearance(false).max_h(px(420.))));
        if current.is_task() {
            let state = current.task_state.clone().unwrap_or_else(|| "active".into());
            let mut states = div().flex().gap(px(2.));
            for s in ["todo", "active", "blocked", "done"] {
                let tid = id.clone();
                let b = Button::new(SharedString::from(format!("state-{s}"))).xsmall().label(yh_core::memory::state_label(s));
                states = states.child((if state == s { b.primary() } else { b.ghost() }).on_click(cx.listener(move |this, _, _, cx| this.set_task_state(&tid, s, cx))));
            }
            let mut steps = div().flex().flex_col().gap(px(3.));
            for (i, step) in current.steps.iter().enumerate() {
                let tid = id.clone();
                steps = steps.child(
                    div()
                        .id(SharedString::from(format!("step-{i}")))
                        .flex()
                        .gap(px(8.))
                        .rounded(px(5.))
                        .px(px(4.))
                        .hover(|s| s.bg(theme.selected))
                        .child(div().w(px(16.)).text_color(if step.done { state_color("done", theme) } else { theme.secondary }).child(if step.done { "✓" } else { "○" }))
                        .child(div().flex_1().when(step.done, |d| d.text_color(theme.secondary).line_through()).child(step.text.clone()))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_step(&tid, i, cx))),
                );
            }
            body = body.child(section("Task", theme)).child(states).child(steps);
        }
        if listed {
            body = body.child(section("Use", theme)).child(
                div().text_color(theme.secondary).child(format!(
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
                let title = self.memories.iter().find(|x| &x.id == to).map(|x| x.title.clone()).unwrap_or_else(|| to.chars().take(8).collect());
                let target = to.clone();
                links = links.child(
                    div()
                        .id(SharedString::from(format!("link-{to}")))
                        .flex()
                        .gap(px(6.))
                        .text_color(theme.accent)
                        .child(title)
                        .when_some(relation.clone(), |d, r| d.child(div().text_color(theme.secondary).child(r)))
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
                        .text_color(if title.is_some() { theme.accent } else { theme.secondary })
                        .child(title.clone().unwrap_or_else(|| "A thread that's gone".into()))
                        .when(title.is_some(), |d| d.on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.select(&thread_id, cx)))),
                );
            }
            body = body.child(section("Learned in", theme)).child(sources);
        }
        if self.versions.len() > 1 {
            let mut history = div().flex().flex_col().gap(px(2.));
            for (version, title, text, status, at, actor) in self.versions.iter().rev() {
                let open = self.shown_version == Some(*version);
                let v = *version;
                history = history.child(
                    div()
                        .id(SharedString::from(format!("version-{version}")))
                        .flex()
                        .gap(px(8.))
                        .rounded(px(5.))
                        .px(px(4.))
                        .hover(|s| s.bg(theme.selected))
                        .child(div().w(px(28.)).text_color(theme.secondary).child(format!("v{version}")))
                        .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().child(title.clone()))
                        .when(status != "active", |d| d.child(div().text_color(theme.secondary).child(status.clone())))
                        .child(div().text_color(theme.secondary).child(format!("{} · {}", actor.clone().unwrap_or_else(|| "agent".into()), ago(*at))))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.shown_version = if this.shown_version == Some(v) { None } else { Some(v) };
                            cx.notify();
                        })),
                );
                if open {
                    history = history.child(div().ml(px(36.)).p(px(10.)).rounded(px(8.)).bg(theme.card).text_size(px(12.)).whitespace_normal().child(text.clone()));
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
                        .gap(px(8.))
                        .px(px(16.))
                        .py(px(10.))
                        .border_t_1()
                        .border_color(theme.separator)
                        .child(div().flex_1().text_color(theme.secondary).child(if listed { "Edited" } else { "New memory" }))
                        .child(Button::new("revert").small().label(if listed { "Revert" } else { "Discard" }).on_click(cx.listener(move |this, _, _, cx| {
                            if listed {
                                this.revert(cx);
                            } else {
                                this.selection = None;
                                this.editing = None;
                                this.after_reload(cx);
                                cx.notify();
                            }
                        })))
                        .child(Button::new("save").small().primary().label("Save ⌘S").on_click(cx.listener(|this, _, _, cx| this.save(cx)))),
                )
            })
            .into_any_element()
    }

    fn tasks(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let mut column = div().id("memory-tasks").size_full().overflow_y_scroll().p(px(24.)).flex().flex_col().gap(px(14.));
        let tasks: Vec<&Memory> = self.memories.iter().filter(|m| m.is_task() && m.status == "active").collect();
        if tasks.is_empty() {
            return column.child(div().text_color(theme.secondary).child("No tasks. When you ask an agent for work with more than one step, it tracks it here."));
        }
        for (state, heading) in [("active", "In Progress"), ("blocked", "Blocked"), ("todo", "To Do"), ("done", "Done")] {
            let group: Vec<&&Memory> = tasks.iter().filter(|m| m.task_state.as_deref().unwrap_or("active") == state).collect();
            if group.is_empty() {
                continue;
            }
            column = column.child(section(&format!("{heading} · {}", group.len()), theme));
            for m in group {
                let id = m.id.clone();
                column = column.child(
                    div()
                        .id(SharedString::from(format!("task-{id}")))
                        .p(px(12.))
                        .rounded(px(8.))
                        .bg(theme.card)
                        .hover(|s| s.bg(theme.selected))
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(chip(yh_core::memory::state_label(state), state_color(state, theme)))
                                .child(div().flex_1().font_weight(FontWeight::MEDIUM).child(m.title.clone()))
                                .when(!m.steps.is_empty(), |d| d.child(div().text_color(theme.secondary).child(format!("{} of {}", m.steps_done(), m.steps.len()))))
                                .child(div().text_size(px(11.)).text_color(theme.secondary).child(ago(m.updated_at))),
                        )
                        .when_some(m.next_step().filter(|_| state != "done").map(str::to_string), |d, next| {
                            d.child(div().text_size(px(12.)).text_color(theme.secondary).child(format!("Next: {next}")))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.tab = Tab::Memories;
                            this.archived = false;
                            this.select(&id, cx);
                        })),
                );
            }
        }
        column
    }

    /// How memory is doing, in sentences first and figures after.
    fn insights(&self, theme: &Theme, _cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        use yh_core::memory_stats::Stats;
        let Some(st) = &self.stats else { return div().p(px(24.)).text_color(theme.secondary).child("Reading…").into_any_element() };
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
        let mut how = div().p(px(14.)).rounded(px(8.)).bg(theme.card).flex().flex_col().gap(px(8.)).child(
            div().flex().items_center().child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child("How your memory is working")).when(issues, |d| {
                let (ws, project) = (self.workspace.clone(), self.project.clone());
                d.child(Button::new("insights-reconcile").small().primary().label("Reconcile").on_click(move |_, _, cx| {
                    ws.update(cx, |ws, cx| ws.open_memory_agent("reconciler", &project, vec![], Some(yh_core::memory_agents::RECONCILE_REQUEST.to_string()), cx))
                }))
            }),
        );
        for (text, issue) in lines {
            how = how.child(div().flex().gap(px(8.)).child(div().flex_none().w(px(8.)).h(px(8.)).mt(px(6.)).rounded_full().bg(if issue { theme.warning } else { theme.accent })).child(div().flex_1().whitespace_normal().child(text)));
        }
        let figure = |label: &str, value: String| {
            div().flex_1().p(px(12.)).rounded(px(8.)).bg(theme.card).flex().flex_col().gap(px(2.)).child(div().text_size(px(11.)).text_color(theme.secondary).child(label.to_string())).child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(value))
        };
        let figures = div()
            .flex()
            .gap(px(10.))
            .child(figure("Memories", st.active.to_string()))
            .child(figure("Index each session", format!("~{index_tokens} tokens")))
            .child(figure("Read, last 30 days", match st.recall_rate() { Some(r) => format!("{:.0}%", r * 100.), None => "—".into() }))
            .child(figure("Searches that found something", match st.search_hit_rate() { Some(r) => format!("{:.0}%", r * 100.), None => "—".into() }));
        // Each day of the window: sessions, reads, searches as stacked bars.
        let days = st.days.clone();
        let max = days.iter().map(|d| d.sessions + d.recalled + d.searches).max().unwrap_or(0).max(1) as f32;
        let colors: [Hsla; 3] = [theme.accent, gpui_kit::rgb(0x1BAF7A).into(), theme.warning];
        let chart = gpui_kit::canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let n = days.len().max(1);
                let w = f32::from(bounds.size.width) / n as f32;
                let h = f32::from(bounds.size.height);
                for (i, d) in days.iter().enumerate() {
                    let mut y = f32::from(bounds.origin.y) + h;
                    for (k, v) in [d.sessions, d.recalled, d.searches].into_iter().enumerate() {
                        let seg = v as f32 / max * (h - 4.);
                        if seg < 0.5 {
                            continue;
                        }
                        window.paint_quad(gpui_kit::fill(gpui_kit::Bounds::new(gpui_kit::point(bounds.origin.x + px(i as f32 * w + 1.), px(y - seg)), gpui_kit::size(px((w - 2.).max(1.)), px((seg - 1.).max(0.5)))), colors[k]));
                        y -= seg;
                    }
                }
            },
        )
        .size_full();
        let legend = div().flex().gap(px(12.)).text_size(px(11.)).text_color(theme.secondary).children(
            [("Sessions", colors[0]), ("Reads", colors[1]), ("Searches", colors[2])].into_iter().map(|(l, c)| div().flex().items_center().gap(px(4.)).child(div().w(px(8.)).h(px(8.)).rounded(px(2.)).bg(c)).child(l)),
        );
        let list = |title: &str, items: Vec<String>, empty: &str| {
            let mut d = div().flex_1().p(px(12.)).rounded(px(8.)).bg(theme.card).flex().flex_col().gap(px(3.)).child(div().font_weight(FontWeight::SEMIBOLD).pb(px(3.)).child(title.to_string()));
            if items.is_empty() {
                d = d.child(div().text_size(px(12.)).text_color(theme.secondary).child(empty.to_string()));
            }
            for i in items {
                d = d.child(div().text_size(px(12.)).child(i));
            }
            d
        };
        let by_kind: Vec<String> = st.by_kind.iter().map(|(k, c)| format!("{}: {c}", kind_label(k))).collect();
        let ages: Vec<String> = st.age_buckets.iter().map(|(l, c)| format!("{l}: {c}")).collect();
        div()
            .id("memory-insights")
            .size_full()
            .overflow_y_scroll()
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(how)
            .child(figures)
            .child(div().flex().flex_col().gap(px(6.)).child(div().font_weight(FontWeight::SEMIBOLD).child(format!("Last {} days", st.window))).child(div().h(px(120.)).child(chart)).child(legend))
            .child(div().flex().gap(px(10.)).child(list("By kind", by_kind, "No memories yet.")).child(list("By age", ages, "No memories yet.")))
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .child(list("Searched for, not found", st.missed_queries.iter().map(|q| format!("“{q}”")).collect(), "Every search found something."))
                    .child(list("Never read, over a week old", st.never_recalled.iter().take(12).map(|m| m.title.clone()).collect(), "Everything gets read.")),
            )
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .child(list("Read the most", st.top_recalled.iter().map(|m| format!("{} · {}×", m.title, m.recall_count)).collect(), "Nothing read yet."))
                    .child(list("Look like duplicates", st.likely_duplicates.iter().map(|g| g.iter().map(|m| m.title.clone()).collect::<Vec<_>>().join(" · ")).collect(), "None.")),
            )
            .into_any_element()
    }

    /// The memory as a map: a dot a memory (its kind's color, bigger the more it's read), a
    /// line a link. Click a dot to open it.
    fn graph_view(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        if self.graph.ids.is_empty() {
            return div().p(px(24.)).text_color(theme.secondary).child("No memories to map yet.").into_any_element();
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
        let (line, text, cell) = (theme.separator, theme.secondary, self.graph_bounds.clone());
        let pad = 40.;
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
                    let r = if Some(i) == selected { r + 3. } else { *r };
                    window.paint_quad(gpui_kit::fill(gpui_kit::Bounds::new(gpui_kit::point(px(x - r), px(y - r)), gpui_kit::size(px(r * 2.), px(r * 2.))), *color).corner_radii(px(r)));
                    // Titles of the read and the picked, small, beside their dots.
                    if Some(i) == selected || dots.len() <= 40 {
                        let run = gpui_kit::TextRun { len: title.len(), font: window.text_style().font(), color: text, background_color: None, underline: None, strikethrough: None };
                        let shaped = window.text_system().shape_line(title.clone().into(), px(11.), &[run], None);
                        let _ = shaped.paint(gpui_kit::point(px(x + r + 3.), px(y - 7.)), px(14.), gpui_kit::TextAlign::Left, None, window, cx);
                    }
                }
            },
        )
        .size_full();
        let (cell, ids, pts) = (self.graph_bounds.clone(), self.graph.ids.clone(), self.graph.points.clone());
        div()
            .id("memory-graph")
            .size_full()
            .child(canvas)
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

    fn activity(&self, theme: &Theme) -> impl IntoElement {
        let mut column = div().id("memory-activity").size_full().overflow_y_scroll().p(px(24.)).flex().flex_col().gap(px(4.));
        if self.events.is_empty() {
            return column.child(div().text_color(theme.secondary).child("Nothing yet: agents' saves, reads and searches show here."));
        }
        for (kind, title, actor, at, query, count) in &self.events {
            let what = match kind.as_str() {
                "created" => format!("saved “{}”", title.clone().unwrap_or_default()),
                "updated" => format!("updated “{}”", title.clone().unwrap_or_default()),
                "archived" => format!("archived “{}”", title.clone().unwrap_or_default()),
                "restored" => format!("restored “{}”", title.clone().unwrap_or_default()),
                "deleted" => format!("deleted “{}”", title.clone().unwrap_or_default()),
                "recalled" => format!("read “{}”", title.clone().unwrap_or_default()),
                "searched" => format!("searched “{}”, {} found", query.clone().unwrap_or_default(), count.unwrap_or(0)),
                "injected" => format!("started a session with {} memories in its index", count.unwrap_or(0)),
                other => format!("{other} {}", title.clone().unwrap_or_default()),
            };
            let who = match actor.as_deref() {
                Some("user") => "You",
                Some("importer") => "The importer",
                Some("reconciler") => "The reconciler",
                Some("app") => "The app",
                _ => "An agent",
            };
            column = column.child(
                div()
                    .flex()
                    .gap(px(10.))
                    .child(div().w(px(110.)).flex_none().text_color(theme.secondary).child(ago(*at)))
                    .child(div().flex_1().child(format!("{who} {what}"))),
            );
        }
        column
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
                .m(px(16.))
                .mb(px(0.))
                .p(px(14.))
                .rounded(px(10.))
                .bg(theme.warning.opacity(0.08))
                .border_1()
                .border_color(theme.warning.opacity(0.3))
                .flex()
                .gap(px(12.))
                .child(div().text_color(theme.warning).text_size(px(16.)).child("⧉"))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(if on { "Two memories" } else { "Not in YourHarness yet" }))
                        .child(div().child(message))
                        .child(
                            div()
                                .flex()
                                .gap(px(8.))
                                .child(Button::new("one-memory").small().primary().label(primary).tooltip(if on { detail } else { "Brings Claude Code's memory files for this folder in." }).on_click(cx.listener(move |this, _, _, cx| this.use_one_memory(&s1, cx))))
                                .when(count > 0, |d| {
                                    d.child(Button::new("memory-importer").small().outline().label("Open in the Importer…").on_click(cx.listener(|this, _, _, cx| {
                                        let dir = this.project_path(cx).map(|f| yh_core::memory_archive::claude_code::directory(&f, &yh_core::history::config_directory()));
                                        let (ws, project) = (this.workspace.clone(), this.project.clone());
                                        ws.update(cx, |ws, cx| ws.open_memory_agent("importer", &project, dir.into_iter().map(|d| d.to_string_lossy().into_owned()).collect(), None, cx));
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
                        .when(on, |d| d.child(div().text_size(px(11.)).text_color(theme.secondary).child(detail))),
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
