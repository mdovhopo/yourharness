//! The right sidebar (⌥⌘0), for the open thread: its sub-agents, followed step by step, and
//! its folder's files (a tree, Go to File, files opened in tabs, read-only). A cached view of
//! its own: it redraws when its tab's data changes, not with every streamed delta.

use crate::state::thread::{SubStep, ThreadState};
use crate::state::workspace::{Panel, Workspace};
use crate::ui::transcript::TranscriptView;
use crate::theme::{radius, space, text, Theme, MONO};
use crate::ui::icons;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenuItem};
use gpui_kit::component::{Disableable, Icon, IconName, Selectable, Sizable};
use gpui_kit::{
    div, prelude::*, px, uniform_list, App, Context, Entity, FontWeight, ScrollHandle, ScrollStrategy, SharedString, Subscription,
    UniformListScrollHandle, WeakEntity, Window,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

pub const WIDTH: f32 = 380.;

/// What Split into a Stack asks Claude, as the Swift app does.
const SPLIT_INTO_STACK: &str = "Split the work in this thread into a GitHub stack of small pull requests with `gh stack`. Group the changes into layers that each make sense on their own and build on the one below, bottom first, one branch per layer with its changes committed there (`gh stack init`, then `gh stack add` for each layer above). Then run `gh stack submit --auto --open` and tell me the stack number and its PRs. Don't merge anything.";

/// Where file links go: set by the window to open them in its Files tab.
pub struct FileLinks(pub Rc<dyn Fn(&str, Option<(usize, usize)>, &mut App)>);

impl gpui_kit::Global for FileLinks {}

/// Opens a link from the chat: a web address in the browser; a path (`src/a.rs:12-20`,
/// `a.rs#L12`) in the thread's Files tab.
pub fn open_link(url: &str, cx: &mut App) {
    let is_web = url.contains("://") || url.starts_with("mailto:");
    match cx.try_global::<FileLinks>().map(|f| f.0.clone()) {
        Some(open) if !is_web || url.starts_with("file://") => {
            let (path, lines) = split_lines(url.trim_start_matches("file://"));
            open(&path, lines, cx);
        }
        _ => cx.open_url(url),
    }
}

/// `a.rs:12-20` or `a.rs#L12-L20` → the path and its lines.
pub fn split_lines(link: &str) -> (String, Option<(usize, usize)>) {
    let range = |text: &str| -> Option<(usize, usize)> {
        let mut parts = text.split('-').map(|p| p.trim_start_matches('L').parse::<usize>().ok());
        let start = parts.next()??;
        Some((start, parts.next().flatten().unwrap_or(start)))
    };
    if let Some((path, frag)) = link.split_once("#L") {
        return (path.to_string(), range(frag));
    }
    if let Some((path, tail)) = link.rsplit_once(':') {
        if let Some(r) = range(tail) {
            return (path.to_string(), Some(r));
        }
    }
    (link.to_string(), None)
}

/// Biggest file shown; past it, its first part.
const MAX_FILE: u64 = 8 * 1024 * 1024;

/// A folder's files as a tree: each folder's children, folders first, by name.
struct Tree {
    children: HashMap<String, Vec<(String, bool)>>,
}

impl Tree {
    fn build(files: &[String]) -> Tree {
        let mut kids: HashMap<String, BTreeMap<(bool, String), ()>> = HashMap::new();
        for path in files {
            let mut parent = String::new();
            let parts: Vec<&str> = path.split('/').collect();
            for (i, part) in parts.iter().enumerate() {
                let is_dir = i + 1 < parts.len();
                kids.entry(parent.clone()).or_default().insert((!is_dir, part.to_string()), ());
                if is_dir {
                    parent = if parent.is_empty() { part.to_string() } else { format!("{parent}/{part}") };
                }
            }
        }
        Tree {
            children: kids.into_iter().map(|(k, v)| (k, v.into_keys().map(|(file, name)| (name, !file)).collect())).collect(),
        }
    }

    /// The rows shown with these folders open: (path, name, depth, is folder).
    fn rows(&self, open: &HashSet<String>) -> Vec<(String, String, usize, bool)> {
        let mut rows = vec![];
        let mut stack: Vec<(String, usize)> = vec![(String::new(), 0)];
        // Depth first, in order: push children reversed.
        let mut pending: Vec<(String, String, usize, bool)> = vec![];
        if let Some(top) = self.children.get("") {
            for (name, dir) in top.iter().rev() {
                pending.push((name.clone(), name.clone(), 0, *dir));
            }
        }
        stack.clear();
        while let Some((path, name, depth, dir)) = pending.pop() {
            if dir && open.contains(&path) {
                if let Some(kids) = self.children.get(&path) {
                    for (n, d) in kids.iter().rev() {
                        pending.push((format!("{path}/{n}"), n.clone(), depth + 1, *d));
                    }
                }
            }
            rows.push((path, name, depth, dir));
        }
        rows
    }
}

/// What an opened file turned out to be.
enum FileContent {
    Text(std::sync::Arc<Vec<SharedString>>),
    /// A picture, drawn from its file.
    Image(std::path::PathBuf),
    /// Not text: its size, and Open in Default App.
    Binary(u64),
    Folder,
    Missing,
    Unreadable(String),
}

struct OpenFile {
    path: String,
    content: FileContent,
    /// Only its first part is shown.
    note: Option<String>,
    /// Some lines were longer than `LINE_LIMIT` and are cut.
    cut_lines: bool,
    scroll: UniformListScrollHandle,
    /// With Wrap Lines on: a list that measures each line (made when first wrapped).
    wrapped: Option<gpui_kit::ListState>,
    /// Lines to show picked out, from a link (1-based).
    lines_picked: Option<(usize, usize)>,
    /// Opened with a single click in the tree: the next one replaces it, until it's kept
    /// (double-click its tab, or Keep Open).
    preview: bool,
}

impl OpenFile {
    fn lines(&self) -> Option<&std::sync::Arc<Vec<SharedString>>> {
        if let FileContent::Text(l) = &self.content { Some(l) } else { None }
    }
}

/// Lines longer than this are cut, as the Swift app's viewer does.
const LINE_LIMIT: usize = 5_000;

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "ico", "svg"];

/// Reads a file for the viewer. Blocks: off the main thread only.
fn read_for_viewer(full: &str) -> (FileContent, Option<String>, bool) {
    use std::io::Read;
    let path = std::path::Path::new(full);
    let Ok(meta) = std::fs::metadata(path) else { return (FileContent::Missing, None, false) };
    if meta.is_dir() {
        return (FileContent::Folder, None, false);
    }
    let size = meta.len();
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return (FileContent::Image(path.to_path_buf()), None, false);
    }
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return (FileContent::Unreadable(e.to_string()), None, false),
    };
    let mut bytes = vec![];
    let _ = file.take(MAX_FILE).read_to_end(&mut bytes);
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return (FileContent::Binary(size), None, false);
    }
    let note = (size > MAX_FILE).then(|| format!("Showing the first {} MB of {} MB", MAX_FILE >> 20, size >> 20));
    let text = String::from_utf8_lossy(&bytes);
    // A last newline ends the last line; it doesn't start another.
    let text = text.strip_suffix('\n').unwrap_or(&text);
    let mut cut = false;
    let lines: Vec<SharedString> = text
        .split('\n')
        .map(|l| {
            let l = l.strip_suffix('\r').unwrap_or(l);
            let l = if l.len() > LINE_LIMIT {
                cut = true;
                let mut end = LINE_LIMIT;
                while !l.is_char_boundary(end) {
                    end -= 1;
                }
                &l[..end]
            } else {
                l
            };
            SharedString::from(l.replace('\t', "    "))
        })
        .collect();
    (FileContent::Text(std::sync::Arc::new(lines)), note, cut)
}

fn size_text(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} bytes"),
        b if b < 1024 * 1024 => format!("{:.0} KB", b as f64 / 1024.),
        b => format!("{:.1} MB", b as f64 / 1024. / 1024.),
    }
}

/// One line of code, coloured by `yh_core::markdown::highlight` (only lines on screen are).
fn code_line(text: &SharedString, theme: &Theme) -> gpui_kit::StyledText {
    use yh_core::markdown::Span;
    let styled = yh_core::markdown::highlight(text, "");
    let highlights: Vec<(std::ops::Range<usize>, gpui_kit::HighlightStyle)> = styled
        .spans
        .into_iter()
        .filter_map(|(r, span)| {
            let color = match span {
                Span::Keyword => theme.keyword,
                Span::Str => theme.string,
                Span::Number => theme.number,
                Span::Comment => theme.comment,
                _ => return None,
            };
            Some((r, gpui_kit::HighlightStyle { color: Some(color), ..Default::default() }))
        })
        .collect();
    gpui_kit::StyledText::new(text.clone()).with_highlights(highlights)
}

/// The longest a batch of a file's lines took to make, for `YH_FILES_PROBE` (µs).
static LINES_WORST_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// One folder's Files tab: what's open in the tree, the files in tabs.
#[derive(Default)]
struct Folder {
    tree: Option<Rc<Tree>>,
    rows: Rc<Vec<(String, String, usize, bool)>>,
    open_dirs: HashSet<String>,
    files: Vec<OpenFile>,
    current: usize,
    loading: bool,
    /// Hide Folder: only the open file shows.
    hides_tree: bool,
}

pub struct Inspector {
    workspace: Entity<Workspace>,
    thread: Option<Entity<ThreadState>>,
    seen_subagents: u64,
    thread_watch: Option<Subscription>,
    prompt_open: bool,
    sub_scroll: ScrollHandle,
    folders: HashMap<String, Folder>,
    goto: Entity<InputState>,
    goto_results: Vec<String>,
    tree_scroll: UniformListScrollHandle,
    /// The open thread's side chat, its transcript and what's typed for it.
    side: Option<(Entity<ThreadState>, Entity<TranscriptView>, Subscription)>,
    question: Entity<InputState>,
    /// Each thread's pull requests as last looked up, and when.
    pulls: HashMap<String, (Vec<crate::state::pulls::ThreadPull>, std::time::Instant)>,
    pulls_loading: bool,
    /// Looks again every 30 seconds while the PRs tab shows.
    _pulls_timer: Option<gpui_kit::Task<()>>,
    /// The open thread was working when last seen: its turn ending looks again.
    was_running: bool,
    /// The side chat as last drawn: running, asking.
    /// The side chat's running state and the request it shows (its id), which change its controls.
    side_state: (bool, Option<String>, usize),
    /// The open thread's browser (`ui::browser_panel`).
    browser_panel: Entity<crate::ui::browser_panel::BrowserPanel>,
    /// Wrap Lines in the Files tab's viewer.
    wraps: bool,
    /// Whether `gh stack` is installed, asked once when the PRs tab first shows: Split into a
    /// Stack only shows with it.
    stacks_installed: Option<bool>,
    _subscriptions: Vec<Subscription>,
}

impl Inspector {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let goto = cx.new(|cx| InputState::new(window, cx).placeholder("Go to File"));
        let question = cx.new(|cx| InputState::new(window, cx).placeholder("Ask on the side…"));
        let browser_panel = cx.new(|cx| crate::ui::browser_panel::BrowserPanel::new(workspace.clone(), false, window, cx));
        let subscriptions = vec![
            cx.observe(&workspace, |this, _, cx| this.follow_selection(cx)),
            cx.subscribe_in(&question, window, |this, _, e: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = e {
                    this.ask(window, cx);
                }
            }),
            cx.subscribe(&goto, |this, _, e: &InputEvent, cx| match e {
                InputEvent::Change => this.search_files(cx),
                InputEvent::PressEnter { .. } => {
                    if let Some(first) = this.goto_results.first().cloned() {
                        this.open_file(&first, None, cx);
                    }
                }
                _ => {}
            }),
        ];
        let mut inspector = Inspector {
            workspace,
            thread: None,
            seen_subagents: 0,
            thread_watch: None,
            prompt_open: false,
            sub_scroll: ScrollHandle::new(),
            folders: HashMap::new(),
            goto,
            goto_results: vec![],
            tree_scroll: UniformListScrollHandle::new(),
            side: None,
            question,
            side_state: (false, None, 0),
            browser_panel,
            wraps: std::env::var("YH_FILES_WRAP").as_deref() == Ok("1"),
            stacks_installed: None,
            pulls: HashMap::new(),
            pulls_loading: false,
            _pulls_timer: None,
            was_running: false,
            _subscriptions: subscriptions,
        };
        inspector.follow_selection(cx);
        if std::env::var("YH_FILES_PROBE").is_ok() {
            inspector.files_probe(cx);
        }
        inspector
    }

    /// Watches the open thread: its sub-agents' changes redraw the sidebar, nothing else of it.
    fn follow_selection(&mut self, cx: &mut Context<Self>) {
        let selected = self.workspace.read(cx).selected_thread();
        if selected.as_ref().map(|t| t.entity_id()) != self.thread.as_ref().map(|t| t.entity_id()) {
            self.thread = selected.clone();
            self.prompt_open = false;
            self.seen_subagents = selected.as_ref().map_or(0, |t| t.read(cx).subagents_version);
            self.thread_watch = selected.map(|t| {
                cx.observe(&t, |this, t, cx| {
                    let running = t.read(cx).running;
                    if this.was_running && !running && this.workspace.read(cx).panel == Panel::PullRequests {
                        this.refresh_pulls(cx);
                    }
                    this.was_running = running;
                    let version = t.read(cx).subagents_version;
                    if version != this.seen_subagents && this.workspace.read(cx).panel == Panel::Subagents {
                        this.seen_subagents = version;
                        this.sub_scroll.scroll_to_bottom();
                        cx.notify();
                    }
                })
            });
            self.ensure_tree(cx);
            self.side = None;
            self.ensure_side(cx);
            self.ensure_pulls(cx);
        }
        cx.notify();
    }

    /// The PRs tab looks its thread's PRs up when it shows, then every 30 seconds.
    fn ensure_pulls(&mut self, cx: &mut Context<Self>) {
        if self.workspace.read(cx).panel != Panel::PullRequests {
            self._pulls_timer = None;
            return;
        }
        if self.stacks_installed.is_none() {
            self.stacks_installed = Some(false);
            let gh = yh_core::github::GitHubStacks::new(std::env::var("YH_PATH").ok().as_deref());
            cx.spawn(async move |this, cx| {
                let installed = cx.background_executor().spawn(async move { gh.is_installed() }).await;
                let _ = this.update(cx, |this, cx| {
                    this.stacks_installed = Some(installed);
                    cx.notify();
                });
            })
            .detach();
        }
        let Some(id) = self.thread.as_ref().map(|t| t.read(cx).id().to_string()) else { return };
        if self.pulls.get(&id).is_none_or(|(_, at)| at.elapsed().as_secs() > 25) {
            self.refresh_pulls(cx);
        }
        if self._pulls_timer.is_none() {
            self._pulls_timer = Some(cx.spawn(async move |this, cx| loop {
                cx.background_executor().timer(std::time::Duration::from_secs(30)).await;
                let Ok(showing) = this.update(cx, |this, cx| this.workspace.read(cx).panel == Panel::PullRequests && this.workspace.read(cx).shows_inspector) else { break };
                if showing {
                    let _ = this.update(cx, |this, cx| this.refresh_pulls(cx));
                }
            }));
        }
    }

    fn refresh_pulls(&mut self, cx: &mut Context<Self>) {
        let Some(thread) = self.thread.clone() else { return };
        let ws = self.workspace.read(cx);
        let Some(db) = ws.library.db.clone() else { return };
        let id = thread.read(cx).id().to_string();
        let folder = ws.folder_of(thread.read(cx));
        let path = std::env::var("YH_PATH").ok();
        self.pulls_loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let key = id.clone();
            let found = cx
                .background_executor()
                .spawn(async move {
                    let links = crate::state::pulls::links(&db, &id, &folder, path.as_deref());
                    crate::state::pulls::look_up(links, path)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.pulls_loading = false;
                this.pulls.insert(key, (found, std::time::Instant::now()));
                cx.notify();
            });
        })
        .detach();
    }

    /// The side chat's view, once the tab shows for a thread.
    fn ensure_side(&mut self, cx: &mut Context<Self>) {
        if self.side.is_some() || self.workspace.read(cx).panel != Panel::SideChat {
            return;
        }
        let Some(parent) = self.thread.as_ref().map(|t| t.read(cx).id().to_string()) else { return };
        if parent.starts_with(crate::state::workspace::SIDE) {
            return;
        }
        let Some(chat) = self.workspace.update(cx, |ws, cx| ws.side_chat(&parent, cx)) else { return };
        let ws = self.workspace.clone();
        let view = cx.new(|cx| TranscriptView::new(chat.clone(), ws, cx));
        // Its running state and permission requests change the controls under it.
        let watch = cx.observe(&chat, |this, chat, cx| {
            let c = chat.read(cx);
            let state = (c.running, c.permission().map(|p| p.id.clone()), c.permissions.len());
            if this.side_state != state {
                this.side_state = state;
                cx.notify();
            }
        });
        self.side = Some((chat, view, watch));
    }

    fn ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.question.read(cx).value().trim().to_string();
        let Some(chat) = self.side.as_ref().map(|(c, _, _)| c.clone()) else { return };
        if text.is_empty() {
            return;
        }
        self.question.update(cx, |i, cx| i.set_value("", window, cx));
        self.workspace.update(cx, |ws, cx| ws.send(&chat, &text, false, cx));
    }

    fn folder(&self, cx: &App) -> Option<String> {
        let t = self.thread.as_ref()?;
        Some(self.workspace.read(cx).folder_of(t.read(cx)))
    }

    /// The Files tab's tree for the thread's folder, made off the main thread.
    fn ensure_tree(&mut self, cx: &mut Context<Self>) {
        if self.workspace.read(cx).panel != Panel::Files {
            return;
        }
        let Some(folder) = self.folder(cx) else { return };
        let state = self.folders.entry(folder.clone()).or_default();
        if state.tree.is_some() || state.loading {
            return;
        }
        state.loading = true;
        let files = self.workspace.update(cx, |ws, cx| ws.folder_files(folder.clone(), cx));
        cx.spawn(async move |this, cx| {
            let files = files.await;
            let tree = cx.background_executor().spawn(async move { Tree::build(&files) }).await;
            let _ = this.update(cx, |this, cx| {
                let state = this.folders.entry(folder).or_default();
                state.loading = false;
                state.rows = Rc::new(tree.rows(&state.open_dirs));
                state.tree = Some(Rc::new(tree));
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_dir(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(folder) = self.folder(cx) else { return };
        let Some(state) = self.folders.get_mut(&folder) else { return };
        if !state.open_dirs.remove(path) {
            state.open_dirs.insert(path.to_string());
        }
        if let Some(tree) = &state.tree {
            state.rows = Rc::new(tree.rows(&state.open_dirs));
        }
        cx.notify();
    }

    fn search_files(&mut self, cx: &mut Context<Self>) {
        let query = self.goto.read(cx).value().to_string();
        let Some(folder) = self.folder(cx) else { return };
        if query.trim().is_empty() {
            self.goto_results.clear();
            cx.notify();
            return;
        }
        let files = self.workspace.update(cx, |ws, cx| ws.folder_files(folder, cx));
        cx.spawn(async move |this, cx| {
            let files = files.await;
            let q = query.clone();
            let found = cx.background_executor().spawn(async move { yh_core::files::matches(&files, &q, 40) }).await;
            let _ = this.update(cx, |this, cx| {
                if this.goto.read(cx).value() == query.as_str() {
                    this.goto_results = found;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Opens a file of the thread's folder (relative or absolute path) in a tab, read off the
    /// main thread; `lines` picks out lines to scroll to.
    pub fn open_file(&mut self, path: &str, lines: Option<(usize, usize)>, cx: &mut Context<Self>) {
        self.open_tab(path, lines, false, cx);
    }

    /// `preview`: from a single click in the tree, it takes the place of the last such tab.
    fn open_tab(&mut self, path: &str, lines: Option<(usize, usize)>, preview: bool, cx: &mut Context<Self>) {
        let Some(folder) = self.folder(cx) else { return };
        let relative = path.strip_prefix(&format!("{}/", folder.trim_end_matches('/'))).unwrap_or(path).to_string();
        self.workspace.update(cx, |ws, cx| ws.show_panel(Panel::Files, cx));
        // The tab may not have shown yet: its tree is read now too.
        self.ensure_tree(cx);
        let state = self.folders.entry(folder.clone()).or_default();
        if let Some(i) = state.files.iter().position(|f| f.path == relative) {
            state.current = i;
            if !preview {
                state.files[i].preview = false;
            }
            if let Some((start, _)) = lines {
                state.files[i].lines_picked = lines;
                state.files[i].scroll.scroll_to_item(start.saturating_sub(4), ScrollStrategy::Top);
                if let Some(w) = &state.files[i].wrapped {
                    w.scroll_to(gpui_kit::ListOffset { item_ix: start.saturating_sub(1), offset_in_item: px(0.) });
                }
            }
            cx.notify();
            return;
        }
        let full = if relative.starts_with('/') { relative.clone() } else { format!("{}/{relative}", folder.trim_end_matches('/')) };
        let started = std::time::Instant::now();
        cx.spawn(async move |this, cx| {
            let (content, note, cut_lines) = cx.background_executor().spawn(async move { read_for_viewer(&full) }).await;
            let _ = this.update(cx, |this, cx| {
                let state = this.folders.entry(folder).or_default();
                let scroll = UniformListScrollHandle::new();
                if let Some((start, _)) = lines {
                    scroll.scroll_to_item(start.saturating_sub(4), ScrollStrategy::Top);
                }
                if std::env::var("YH_FILES_PROBE").is_ok() {
                    let n = if let FileContent::Text(l) = &content { l.len() } else { 0 };
                    eprintln!("FILES opened {relative} ({n} lines) in {} ms", started.elapsed().as_millis());
                }
                let file = OpenFile { path: relative, content, note, cut_lines, scroll, wrapped: None, lines_picked: lines, preview };
                // A preview takes the last preview's place.
                match state.files.iter().position(|f| f.preview).filter(|_| preview) {
                    Some(i) => {
                        state.files[i] = file;
                        state.current = i;
                    }
                    None => {
                        state.files.push(file);
                        state.current = state.files.len() - 1;
                    }
                }
                this.ensure_wrapped();
                cx.notify();
            });
        })
        .detach();
    }

    fn keep_file(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(f) = self.folder(cx).and_then(|folder| self.folders.get_mut(&folder)).and_then(|s| s.files.get_mut(index)) {
            f.preview = false;
            cx.notify();
        }
    }

    fn close_others(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(folder) = self.folder(cx) else { return };
        let Some(state) = self.folders.get_mut(&folder) else { return };
        if index < state.files.len() {
            let keep = state.files.remove(index);
            state.files = vec![keep];
            state.current = 0;
        }
        cx.notify();
    }

    /// With Wrap Lines on, each open file's text gets a list that measures its lines (made once).
    fn ensure_wrapped(&mut self) {
        if !self.wraps {
            return;
        }
        for f in self.folders.values_mut().flat_map(|s| s.files.iter_mut()) {
            if f.wrapped.is_none() {
                if let FileContent::Text(l) = &f.content {
                    let state = gpui_kit::ListState::new(l.len(), gpui_kit::ListAlignment::Top, px(400.));
                    if let Some((start, _)) = f.lines_picked {
                        state.scroll_to(gpui_kit::ListOffset { item_ix: start.saturating_sub(1), offset_in_item: px(0.) });
                    }
                    f.wrapped = Some(state);
                }
            }
        }
    }

    fn toggle_tree(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.folder(cx).and_then(|folder| self.folders.get_mut(&folder)) {
            state.hides_tree = !state.hides_tree;
        }
        cx.notify();
    }

    /// `YH_FILES_PROBE=1`, with `YH_OPEN_FILE`: once it's open, scrolls the file through in
    /// steps and prints the longest any step's lines took to make (PASS under 50 ms).
    fn files_probe(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(3)).await;
            let mut worst = 0u64;
            for step in 0..40 {
                LINES_WORST_US.store(0, std::sync::atomic::Ordering::Relaxed);
                let done = this.update(cx, |this, cx| {
                    let Some(folder) = this.folder(cx) else { return true };
                    let Some(f) = this.folders.get(&folder).and_then(|s| s.files.get(s.current)) else { return true };
                    let n = f.lines().map_or(0, |l| l.len());
                    if let Some(w) = &f.wrapped {
                        w.scroll_to(gpui_kit::ListOffset { item_ix: n * step / 40, offset_in_item: px(0.) });
                    }
                    f.scroll.scroll_to_item(n * step / 40, ScrollStrategy::Top);
                    cx.notify();
                    false
                });
                if done.unwrap_or(true) {
                    break;
                }
                cx.background_executor().timer(std::time::Duration::from_millis(60)).await;
                worst = worst.max(LINES_WORST_US.load(std::sync::atomic::Ordering::Relaxed));
            }
            let ms = worst as f64 / 1000.;
            eprintln!("FILES scroll: longest lines step {ms:.2} ms {}", if ms < 50. { "PASS" } else { "FAIL" });
        })
        .detach();
    }

    fn close_file(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(folder) = self.folder(cx) else { return };
        let Some(state) = self.folders.get_mut(&folder) else { return };
        if index < state.files.len() {
            state.files.remove(index);
            state.current = state.current.min(state.files.len().saturating_sub(1));
        }
        cx.notify();
    }
}

// MARK: Drawing

impl Inspector {
    /// The tabs: a segmented control of icons, each named in its tooltip and in the panel's
    /// header under it.
    fn tabs(&self, panel: Panel, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        // The chosen segment stands up from the track, as in Settings.
        let raised = if theme.canvas.l < 0.5 { theme.bubble } else { theme.canvas };
        let mut track = div().flex_1().flex().items_center().gap(px(2.)).p(px(2.)).rounded(px(radius::MEDIUM + 1.)).bg(theme.selected);
        for p in Panel::ALL {
            let ws = self.workspace.clone();
            let on = p == panel;
            let (icon, tip): (Icon, &'static str) = match p {
                Panel::PullRequests => (crate::ui::icons::pull_request(), "Pull Requests"),
                Panel::SideChat => (crate::ui::icons::message_square(), "Side Chat"),
                Panel::Subagents => (crate::ui::icons::bot(), "Sub-agents"),
                Panel::Files => (crate::ui::icons::files(), "Files"),
                Panel::Browser => (Icon::new(IconName::Globe), "Browser"),
            };
            track = track.child(
                div()
                    .id(SharedString::from(format!("panel-{}", p.label())))
                    .flex_1()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(radius::MEDIUM - 1.))
                    .when(on, |d| d.bg(raised).border_1().border_color(theme.separator))
                    .when(!on, |d| d.hover(|s| s.bg(theme.hover)))
                    .child(icon.size(px(15.)).text_color(if on { theme.accent } else { theme.secondary }))
                    .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        ws.update(cx, |ws, cx| ws.show_panel(p, cx));
                        this.ensure_tree(cx);
                        this.ensure_side(cx);
                        this.ensure_pulls(cx);
                        cx.notify();
                    })),
            );
        }
        div().flex().items_center().px(px(space::M)).h(px(44.)).flex_none().border_b_1().border_color(theme.separator).child(track)
    }

    fn subagents(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(thread) = self.thread.clone() else { return empty("No thread open.", theme) };
        let runs = thread.read(cx).subagents();
        if runs.is_empty() {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(panel_header("Sub-agents", None, theme))
                .child(div().flex_1().min_h_0().child(empty_panel(icons::bot(), "No Sub-agents", "When Claude hands work to a sub-agent, you can follow it here, step by step.", theme)))
                .into_any_element();
        }
        let working = runs.iter().filter(|(_, e)| run_label(&e.borrow()).1 == "running").count();
        let subtitle = match (runs.len(), working) {
            (1, _) => "1 run".to_string(),
            (n, 0) => format!("{n} runs"),
            (n, w) => format!("{n} runs · {w} working"),
        };
        let picked = thread.read(cx).inspected_subagent.clone();
        let selected = picked
            .and_then(|id| runs.iter().find(|(r, _)| *r == id).cloned())
            .or_else(|| runs.iter().rev().find(|(_, e)| e.borrow().sub.as_ref().is_some_and(|s| s.status == "running")).cloned())
            .unwrap_or_else(|| runs[runs.len() - 1].clone());
        let mut column = div().size_full().flex().flex_col().child(panel_header("Sub-agents", Some(subtitle), theme));
        if runs.len() > 1 {
            let mut list = div().id("subagent-runs").max_h(px(176.)).overflow_y_scroll().px(px(space::S)).pb(px(space::S)).flex().flex_col().gap(px(1.)).border_b_1().border_color(theme.separator);
            for (id, e) in runs.iter().rev() {
                let b = e.borrow();
                let (description, status) = run_label(&b);
                let on = *id == selected.0;
                let (t, id2) = (thread.clone(), id.clone());
                list = list.child(
                    div()
                        .id(SharedString::from(format!("run-{id}")))
                        .flex()
                        .items_center()
                        .gap(px(space::S))
                        .h(px(28.))
                        .px(px(space::S))
                        .rounded(px(radius::MEDIUM))
                        .cursor_pointer()
                        .when(on, |d| d.bg(theme.selected).font_weight(FontWeight::MEDIUM))
                        .when(!on, |d| d.hover(|s| s.bg(theme.hover)))
                        .child(status_icon(&status, theme))
                        .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().child(description))
                        .on_click(move |_, _, cx| {
                            t.update(cx, |t, cx| {
                                t.inspected_subagent = Some(id2.clone());
                                t.subagents_version += 1;
                                cx.notify();
                            })
                        }),
                );
            }
            column = column.child(list);
        }
        let entry = selected.1.borrow();
        let (description, status) = run_label(&entry);
        let sub = entry.sub.clone();
        let input: serde_json::Value = entry.data.tool_input.as_deref().and_then(|i| serde_json::from_str(i).ok()).unwrap_or_default();
        let prompt = input["prompt"].as_str().unwrap_or("").to_string();
        let agent_type = input["subagent_type"].as_str().map(str::to_string);
        let log: Vec<SubStep> = sub.as_ref().map(|s| s.log.clone()).unwrap_or_default();
        // A sub-agent hands its report back with a SubagentHandback call; the Agent tool's own
        // result then only says it was delivered.
        let handback = log.iter().rev().find(|s| s.tool.as_deref() == Some("SubagentHandback")).and_then(|s| {
            serde_json::from_str::<serde_json::Value>(&s.text).ok().and_then(|v| v["message"].as_str().map(str::to_string))
        });
        let report = handback
            .or_else(|| sub.as_ref().and_then(|s| s.report.clone()))
            .or_else(|| (status != "running").then(|| entry.data.tool_result.as_deref().map(crate::ui::steps::subagent_report)).flatten());
        // Its final message is the report, shown once, below.
        let shown: Vec<&SubStep> = {
            let mut v: Vec<&SubStep> = log.iter().filter(|s| s.tool.as_deref() != Some("SubagentHandback")).collect();
            if let (Some(r), Some(last)) = (&report, v.last()) {
                if last.tool.is_none() && last.text.trim() == r.trim() {
                    v.pop();
                }
            }
            v
        };
        let tools = log.iter().filter(|s| s.tool.is_some() && s.tool.as_deref() != Some("SubagentHandback")).count();
        // Stop: `stop_task` on its task, the thread carries on.
        let task_id = sub.as_ref().filter(|_| status == "running").and_then(|s| s.task_id.clone());
        // `YH_SUBAGENT_STOP=<seconds>`: the probe presses Stop that long after it first shows.
        static STOP_PROBE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if let (Some(task), Some(secs)) = (task_id.clone(), std::env::var("YH_SUBAGENT_STOP").ok().and_then(|s| s.parse::<u64>().ok())) {
            if !STOP_PROBE.swap(true, std::sync::atomic::Ordering::Relaxed) {
                let (ws, t) = (self.workspace.clone(), thread.clone());
                cx.spawn(async move |_, cx| {
                    cx.background_executor().timer(std::time::Duration::from_secs(secs)).await;
                    eprintln!("SUBAGENT stop {task}");
                    let _ = cx.update(|cx| ws.update(cx, |ws, cx| ws.stop_job(&t, &task, cx)));
                })
                .detach();
            }
        }
        let stop = task_id.map(|task| {
            let (ws, t) = (self.workspace.clone(), thread.clone());
            Button::new("subagent-stop")
                .small()
                .outline()
                .icon(icons::circle_stop().size(px(13.)))
                .label("Stop")
                .tooltip("Stop this sub-agent. The thread carries on.")
                .on_click(move |_, _, cx| {
                    ws.update(cx, |ws, cx| ws.stop_job(&t, &task, cx));
                })
        });
        let mut detail = div()
            .id("subagent-detail")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.sub_scroll)
            .px(px(space::L))
            .pt(px(space::M))
            .pb(px(space::L))
            .flex()
            .flex_col()
            .gap(px(space::M))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(space::S))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(space::XS))
                            .child(div().text_size(px(text::TITLE)).line_height(px(20.)).font_weight(FontWeight::SEMIBOLD).whitespace_normal().child(description))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(space::XS + 2.))
                                    .text_size(px(text::FOOTNOTE))
                                    .text_color(theme.secondary)
                                    .child(status_icon(&status, theme))
                                    .child(format!("{} · {tools} tool{}", status_text(&status), if tools == 1 { "" } else { "s" }))
                                    .when_some(agent_type, |d, t| d.child(div().child(format!("· {t}")))),
                            ),
                    )
                    .when_some(stop, |d, b| d.child(b)),
            );
        if !prompt.is_empty() {
            let open = self.prompt_open;
            detail = detail.child(
                div()
                    .id("subagent-prompt")
                    .px(px(space::M))
                    .py(px(space::S + 2.))
                    .rounded(px(radius::MEDIUM))
                    .bg(theme.card)
                    .border_1()
                    .border_color(theme.separator)
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .gap(px(space::XS))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::XS))
                            .text_size(px(text::CAPTION))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.secondary)
                            .child("Task")
                            .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(12.)).text_color(theme.tertiary)),
                    )
                    .child(div().text_size(px(text::FOOTNOTE)).line_height(px(17.)).text_color(theme.secondary).whitespace_normal().when(!open, |d| d.line_clamp(3)).child(prompt))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.prompt_open = !this.prompt_open;
                        cx.notify();
                    })),
            );
        }
        for step in shown {
            detail = detail.child(match &step.tool {
                None => div().text_size(px(text::BODY)).line_height(px(text::BODY_LINE)).whitespace_normal().child(step.text.clone()),
                Some(name) => {
                    let gist = gist(name, &step.text);
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(space::XS))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::XS + 2.))
                                .text_size(px(text::FOOTNOTE))
                                .child(div().flex_none().child(Icon::new(if step.is_error { IconName::CircleX } else { IconName::ChevronRight }).size(px(12.)).text_color(if step.is_error { theme.error } else { theme.tertiary })))
                                .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(name.clone()))
                                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_family(MONO).text_size(px(text::CAPTION)).text_color(theme.secondary).child(gist)),
                        )
                        .when_some(step.result.as_ref().map(|r| first_lines(r, 3)), |d, r| {
                            d.child(
                                div()
                                    .ml(px(18.))
                                    .px(px(space::S))
                                    .py(px(space::XS + 1.))
                                    .rounded(px(radius::SMALL))
                                    .bg(theme.code)
                                    .font_family(MONO)
                                    .text_size(px(text::CAPTION))
                                    .line_height(px(15.))
                                    .text_color(if step.is_error { theme.error } else { theme.secondary })
                                    .child(r),
                            )
                        })
                }
            });
        }
        if status == "running" {
            detail = detail.child(div().flex().items_center().gap(px(space::XS + 2.)).text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(Icon::new(IconName::LoaderCircle).size(px(13.)).text_color(theme.accent)).child("Working…"));
        } else if let Some(report) = report.filter(|r| !r.trim().is_empty()) {
            detail = detail.child(
                div()
                    .p(px(space::M))
                    .rounded(px(radius::LARGE))
                    .border_1()
                    .border_color(theme.separator)
                    .bg(theme.card)
                    .flex()
                    .flex_col()
                    .gap(px(space::S))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::XS + 2.))
                            .text_size(px(text::HEADLINE))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(Icon::new(IconName::FileText).size(px(14.)).text_color(theme.secondary))
                            .child("Report"),
                    )
                    .child(div().text_size(px(text::BODY)).line_height(px(text::BODY_LINE)).whitespace_normal().child(report)),
            );
        }
        column.child(detail).into_any_element()
    }

    /// The Files tab: Show/Hide Folder, Go to File, Wrap Lines and More on top; the folder's
    /// tree; the open files in tabs (a single click in the tree opens a preview tab, in
    /// italics, that the next one replaces); and the file, coloured, or what it is instead.
    fn files(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(folder) = self.folder(cx) else { return empty("No thread open.", theme) };
        let state = self.folders.get(&folder);
        let query = !self.goto.read(cx).value().trim().is_empty();
        let current = state.and_then(|s| s.files.get(s.current)).map(|f| f.path.clone());
        let hides_tree = state.is_some_and(|s| s.hides_tree && !s.files.is_empty());
        let has_files = state.is_some_and(|s| !s.files.is_empty());
        let wraps = self.wraps;
        let folder_name = folder.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
        let header = panel_header("Files", Some(folder_name), theme)
            .child(
                tool_button("files-tree-toggle", if hides_tree { Icon::new(IconName::PanelLeftOpen) } else { Icon::new(IconName::PanelLeftClose) }, if hides_tree { "Show the folder" } else { "Hide the folder" }, theme)
                    .disabled(!has_files)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_tree(cx))),
            )
            .child(
                tool_button("files-wrap", icons::text_wrap(), if wraps { "Don't wrap lines" } else { "Wrap lines" }, theme)
                    .selected(wraps)
                    .disabled(current.is_none())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.wraps = !this.wraps;
                        this.ensure_wrapped();
                        cx.notify();
                    })),
            )
            .child({
                let (folder, path) = (folder.clone(), current.clone());
                tool_button("files-more", Icon::new(IconName::Ellipsis), "More", theme).disabled(current.is_none()).dropdown_menu(move |menu, _, _| match &path {
                    Some(p) => file_menu(menu, &folder, p),
                    None => menu,
                })
            });
        let top = div()
            .flex_none()
            .px(px(space::M))
            .pb(px(space::S))
            .child(Input::new(&self.goto).small().prefix(Icon::new(IconName::Search).size(px(13.)).text_color(theme.tertiary)).cleanable(true));
        let mut column = div().size_full().flex().flex_col().child(header).child(top);
        if query {
            let mut list = div().id("goto-results").flex_1().min_h_0().overflow_y_scroll().px(px(space::S)).flex().flex_col().gap(px(1.));
            if self.goto_results.is_empty() {
                list = list.child(div().px(px(space::S)).py(px(space::S)).text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child("No files match."));
            }
            for path in &self.goto_results {
                let p = path.clone();
                let (dir, name) = path.rsplit_once('/').map_or((String::new(), path.clone()), |(d, n)| (d.to_string(), n.to_string()));
                list = list.child(
                    div()
                        .id(SharedString::from(format!("goto-{path}")))
                        .flex()
                        .items_center()
                        .gap(px(space::S))
                        .h(px(26.))
                        .px(px(space::S))
                        .rounded(px(radius::MEDIUM))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.hover))
                        .child(div().flex_none().child(file_icon(&name).size(px(13.)).text_color(theme.tertiary)))
                        .child(div().flex_none().text_size(px(text::FOOTNOTE)).child(name))
                        .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(text::CAPTION)).text_color(theme.tertiary).child(dir))
                        .on_click(cx.listener(move |this, _, _, cx| this.open_file(&p, None, cx))),
                );
            }
            return column.child(list).into_any_element();
        }
        let Some(state) = state.filter(|s| s.tree.is_some() || !s.files.is_empty()) else { return column.child(empty("Reading the folder…", theme)).into_any_element() };
        // A file opened before the folder's been read shows at once, the tree when it's in.
        if !hides_tree && state.tree.is_none() {
            column = column.child(div().h(px(60.)).flex_none().child(empty("Reading the folder…", theme)));
        } else if !hides_tree {
            let rows = state.rows.clone();
            let open_dirs: Rc<HashSet<String>> = Rc::new(state.open_dirs.clone());
            let this = cx.entity().downgrade();
            let theme2 = *theme;
            let open_path = current.clone();
            let tree = uniform_list("file-tree", rows.len(), move |range, _, _| {
                range
                    .map(|i| {
                        let (path, name, depth, dir) = rows[i].clone();
                        let this: WeakEntity<Inspector> = this.clone();
                        let on = open_path.as_deref() == Some(path.as_str());
                        let expanded = dir && open_dirs.contains(&path);
                        let icon = if dir {
                            Icon::new(if expanded { IconName::FolderOpen } else { IconName::FolderClosed }).text_color(theme2.accent.opacity(0.85))
                        } else {
                            file_icon(&name).text_color(theme2.tertiary)
                        };
                        div()
                            .id(i)
                            .h(px(24.))
                            .mx(px(space::S - 2.))
                            .flex()
                            .items_center()
                            .gap(px(space::XS + 1.))
                            .pl(px(4. + depth as f32 * 14.))
                            .pr(px(space::S))
                            .rounded(px(radius::MEDIUM))
                            .text_size(px(text::FOOTNOTE))
                            .cursor_pointer()
                            .when(on, |d| d.bg(theme2.selected).font_weight(FontWeight::MEDIUM))
                            .when(!on, |d| d.hover(|s| s.bg(theme2.hover)))
                            .child(div().flex_none().w(px(12.)).when(dir, |d| d.child(Icon::new(if expanded { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(11.)).text_color(theme2.tertiary))))
                            .child(div().flex_none().child(icon.size(px(14.))))
                            .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(name))
                            .on_click(move |e, _, cx| {
                                // One click opens a preview; a double click keeps it.
                                let keep = e.click_count() >= 2;
                                let _ = this.update(cx, |this, cx| if dir { this.toggle_dir(&path, cx) } else { this.open_tab(&path, None, !keep, cx) });
                            })
                    })
                    .collect()
            })
            .track_scroll(&self.tree_scroll);
            column = column.child(div().when(has_files, |d| d.h(px(220.)).flex_none()).when(!has_files, |d| d.flex_1()).min_h_0().pb(px(space::XS)).child(tree.size_full()));
        }
        if !has_files {
            return column.into_any_element();
        }
        let mut tabs = div().id("file-tabs").flex_none().flex().items_center().gap(px(2.)).px(px(space::S)).py(px(space::XS + 1.)).border_t_1().border_b_1().border_color(theme.separator).overflow_x_scroll();
        let count = state.files.len();
        for (i, f) in state.files.iter().enumerate() {
            let on = i == state.current;
            let name = f.path.rsplit('/').next().unwrap_or(&f.path).to_string();
            let (this, folder, path) = (cx.entity().downgrade(), folder.clone(), f.path.clone());
            let preview = f.preview;
            tabs = tabs.child(
                strip_tab(SharedString::from(format!("file-tab-{i}")), Some(file_icon(&name)), name, on, preview, 200., theme)
                    .child(tab_close(SharedString::from(format!("close-{i}")), theme).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.close_file(i, cx)
                    })))
                    .on_click(cx.listener(move |this, e: &gpui_kit::ClickEvent, _, cx| {
                        if let Some(folder) = this.folder(cx) {
                            if let Some(s) = this.folders.get_mut(&folder) {
                                s.current = i;
                            }
                        }
                        if e.click_count() >= 2 {
                            this.keep_file(i, cx);
                        }
                        cx.notify();
                    }))
                    .context_menu(move |menu, _, _| {
                        let (t1, t2, t3) = (this.clone(), this.clone(), this.clone());
                        let menu = menu
                            .item(PopupMenuItem::new("Keep Open").disabled(!preview).on_click(move |_, _, cx| {
                                let _ = t1.update(cx, |this, cx| this.keep_file(i, cx));
                            }))
                            .item(PopupMenuItem::new("Close").on_click(move |_, _, cx| {
                                let _ = t2.update(cx, |this, cx| this.close_file(i, cx));
                            }))
                            .item(PopupMenuItem::new("Close Others").disabled(count < 2).on_click(move |_, _, cx| {
                                let _ = t3.update(cx, |this, cx| this.close_others(i, cx));
                            }))
                            .separator();
                        file_menu(menu, &folder, &path)
                    }),
            );
        }
        column = column.child(tabs);
        let file = &state.files[state.current];
        let (dir, name) = file.path.rsplit_once('/').map_or((String::new(), file.path.clone()), |(d, n)| (format!("{d}/"), n.to_string()));
        column = column.child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .h(px(28.))
                .px(px(space::M))
                .text_size(px(text::CAPTION))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(div().min_w_0().text_ellipsis().overflow_hidden().text_color(theme.secondary).child(dir))
                .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(name))
                .child(div().flex_1())
                .when_some(file.lines().map(|l| l.len()), |d, n| d.child(div().flex_none().pl(px(space::S)).text_color(theme.tertiary).child(if n == 1 { "1 line".to_string() } else { format!("{n} lines") }))),
        );
        let notice = |line: String| {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(space::XS + 2.))
                .px(px(space::M))
                .py(px(space::XS))
                .text_size(px(text::CAPTION))
                .text_color(theme.secondary)
                .bg(theme.warning.opacity(0.08))
                .child(Icon::new(IconName::Info).size(px(12.)).text_color(theme.warning))
                .child(line)
        };
        if file.cut_lines {
            column = column.child(notice(format!("Lines over {} characters are cut", "5,000")));
        }
        if let Some(n) = file.note.clone() {
            column = column.child(notice(n));
        }
        let full = if file.path.starts_with('/') { file.path.clone() } else { format!("{}/{}", folder.trim_end_matches('/'), file.path) };
        let unavailable = |icon: Icon, title: &str, detail: Option<String>, open: bool| {
            let full = full.clone();
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(space::M))
                .pb(px(space::XL))
                .child(empty_state(icon, title, detail.as_deref().unwrap_or(""), theme))
                .when(open, |d| d.child(Button::new("open-default").small().outline().icon(Icon::new(IconName::ExternalLink).size(px(13.))).label("Open in Default App").on_click(move |_, _, _| open_default(&full))))
                .into_any_element()
        };
        let body = match &file.content {
            FileContent::Missing => unavailable(Icon::new(IconName::File), "File Not Found", Some("It may have been moved or deleted.".into()), false),
            FileContent::Unreadable(why) => unavailable(Icon::new(IconName::TriangleAlert), "Can't Read This File", Some(why.clone()), false),
            FileContent::Folder => unavailable(Icon::new(IconName::Folder), "Folder", None, true),
            FileContent::Binary(size) => unavailable(Icon::new(IconName::File), "Binary File", Some(size_text(*size)), true),
            FileContent::Image(path) => div()
                .id("file-image")
                .flex_1()
                .min_h_0()
                .overflow_scroll()
                .p(px(12.))
                .child(gpui_kit::img(path.clone()).max_w_full().object_fit(gpui_kit::ObjectFit::ScaleDown))
                .into_any_element(),
            FileContent::Text(lines) => {
                let lines = lines.clone();
                let picked = file.lines_picked;
                let digits = lines.len().to_string().len();
                let theme3 = *theme;
                let gutter = digits as f32 * 7.5 + 20.;
                let row = move |i: usize, wrap: bool| {
                    let on = picked.is_some_and(|(a, b)| i + 1 >= a && i + 1 <= b.max(a));
                    div()
                        .when(!wrap, |d| d.h(px(text::CODE_LINE)).min_w_full())
                        .when(wrap, |d| d.w_full())
                        .flex()
                        .font_family(MONO)
                        .text_size(px(text::CODE))
                        .line_height(px(text::CODE_LINE))
                        .when(!wrap, |d| d.whitespace_nowrap())
                        .when(on, |d| d.bg(theme3.warning.opacity(0.16)))
                        .child(div().flex_none().w(px(gutter)).pr(px(space::S + 2.)).text_right().text_color(if on { theme3.secondary } else { theme3.tertiary }).child(format!("{}", i + 1)))
                        .child(div().when(!wrap, |d| d.flex_none()).when(wrap, |d| d.flex_1().min_w_0().overflow_hidden().whitespace_normal()).child(code_line(&lines[i], &theme3)))
                };
                if let Some(state) = file.wrapped.clone().filter(|_| wraps) {
                    gpui_kit::list(state, move |i, _, _| {
                        let started = std::time::Instant::now();
                        let el = row(i, true).into_any_element();
                        LINES_WORST_US.fetch_max(started.elapsed().as_micros() as u64, std::sync::atomic::Ordering::Relaxed);
                        el
                    })
                    .flex_1()
                    .min_h_0()
                    .into_any_element()
                } else {
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(
                            uniform_list("file-lines", lines_len(&file.content), move |range, _, _| {
                                let started = std::time::Instant::now();
                                let rows: Vec<_> = range.map(|i| row(i, false)).collect();
                                LINES_WORST_US.fetch_max(started.elapsed().as_micros() as u64, std::sync::atomic::Ordering::Relaxed);
                                rows
                            })
                            .track_scroll(&file.scroll)
                            .size_full(),
                        )
                        .into_any_element()
                }
            }
        };
        column.child(body).into_any_element()
    }
}

fn lines_len(content: &FileContent) -> usize {
    if let FileContent::Text(l) = content { l.len() } else { 0 }
}

fn open_default(path: &str) {
    let _ = std::process::Command::new("/usr/bin/open").arg(path).spawn();
}

/// A file's menu: Open in Default App, Show in Finder, Copy Path, Copy Relative Path.
fn file_menu(menu: gpui_kit::component::menu::PopupMenu, folder: &str, path: &str) -> gpui_kit::component::menu::PopupMenu {
    let full = if path.starts_with('/') { path.to_string() } else { format!("{}/{path}", folder.trim_end_matches('/')) };
    let (f1, f2, f3, rel) = (full.clone(), full.clone(), full, path.to_string());
    menu.item(PopupMenuItem::new("Open in Default App").on_click(move |_, _, _| open_default(&f1)))
        .item(PopupMenuItem::new("Show in Finder").icon(IconName::Folder).on_click(move |_, _, cx| cx.reveal_path(std::path::Path::new(&f2))))
        .separator()
        .item(PopupMenuItem::new("Copy Path").icon(IconName::Copy).on_click(move |_, _, cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(f3.clone()))))
        .item(PopupMenuItem::new("Copy Relative Path").on_click(move |_, _, cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(rel.clone()))))
}

impl Inspector {
    /// The open thread's browser, drawn by its own view (`ui::browser_panel`).
    fn browser(&mut self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(thread) = self.thread.as_ref().map(|t| {
            let t = t.read(cx);
            (t.id().to_string(), t.meta.project_id.clone())
        }) else {
            crate::browser::Browsers::show(None);
            return empty("No thread open.", theme);
        };
        self.browser_panel.update(cx, |p, cx| p.set_thread(Some(thread), cx));
        self.browser_panel.clone().into_any_element()
    }

    /// The element picker in the open thread's browser (`YH_BROWSER_PICK`).
    pub fn pick(&mut self, _thread: &str, cx: &mut Context<Self>) {
        self.browser_panel.update(cx, |p, cx| p.pick(cx));
    }

    /// Split into a Stack: asks Claude to make the thread's work a stack of small PRs with
    /// `gh stack`. Only with `gh stack` there, never in a side chat, and not while Claude works.
    fn split_button(&self, compact: bool, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let thread = self.thread.clone()?;
        if self.stacks_installed != Some(true) || thread.read(cx).id().starts_with(crate::state::workspace::SIDE) {
            return None;
        }
        let running = thread.read(cx).running;
        let ws = self.workspace.clone();
        let theme = Theme::get(cx);
        let tip = "Ask Claude to split this thread's work into a stack of small pull requests, with gh stack";
        let b = if compact {
            tool_button("split-stack-compact", icons::layers(), tip, &theme)
        } else {
            Button::new("split-stack").small().outline().icon(icons::layers().size(px(13.))).label("Split into a Stack").tooltip(tip)
        };
        Some(
            b.disabled(running)
                .on_click(move |_, _, cx| {
                    let t = thread.clone();
                    ws.update(cx, |ws, cx| ws.send(&t, SPLIT_INTO_STACK, false, cx));
                })
                .into_any_element(),
        )
    }

    fn pull_requests(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(thread) = &self.thread else { return empty("No thread open.", theme) };
        let id = thread.read(cx).id().to_string();
        let column = div().size_full().flex().flex_col();
        let Some((pulls, at)) = self.pulls.get(&id) else {
            return column.child(panel_header("Pull Requests", None, theme)).child(div().flex_1().min_h_0().child(empty("Looking for this thread's pull requests…", theme))).into_any_element();
        };
        let refresh = tool_button("pulls-refresh", Icon::new(IconName::RefreshCw), "Refresh", theme).loading(self.pulls_loading).on_click(cx.listener(|this, _, _, cx| this.refresh_pulls(cx)));
        if pulls.is_empty() {
            return column
                .child(panel_header("Pull Requests", None, theme).child(refresh))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .pb(px(space::XL * 2.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(space::L))
                        .child(empty_state(
                            icons::pull_request(),
                            "No Pull Requests",
                            "When Claude opens, merges or comments on one in this thread, or one's linked here, it shows here and stays current.",
                            theme,
                        ))
                        .when_some(self.split_button(false, cx), |d, b| d.child(b)),
                )
                .into_any_element();
        }
        let state = |p: &crate::state::pulls::ThreadPull| p.pull.as_ref().map(|p| p.state.clone()).ok();
        // Open ones (and ones whose state isn't known) first; then merged and closed, newest first.
        let open: Vec<&crate::state::pulls::ThreadPull> = pulls.iter().filter(|p| p.worked && state(p).is_none_or(|s| s == "open")).collect();
        let mut done: Vec<&crate::state::pulls::ThreadPull> = pulls.iter().filter(|p| p.worked && state(p).is_some_and(|s| s != "open")).collect();
        let when = |p: &crate::state::pulls::ThreadPull| p.pull.as_ref().ok().and_then(|p| p.merged_at.clone().or(p.updated_at.clone())).unwrap_or_default();
        done.sort_by_key(|p| std::cmp::Reverse(when(p)));
        let mentioned: Vec<&crate::state::pulls::ThreadPull> = pulls.iter().filter(|p| !p.worked).collect();
        let open_count = open.iter().filter(|p| state(p).is_some()).count();
        let subtitle = format!("{open_count} open · updated {}", if at.elapsed().as_secs() < 5 { "just now".to_string() } else { format!("{} s ago", at.elapsed().as_secs()) });
        let header = panel_header("Pull Requests", Some(subtitle), theme).when_some(self.split_button(true, cx), |d, b| d.child(b)).child(refresh);
        let mut list = div().id("pulls").flex_1().min_h_0().overflow_y_scroll().px(px(space::M)).pb(px(space::M)).flex().flex_col().gap(px(space::S));
        for p in open {
            list = list.child(pull_card(p, theme));
        }
        let section = |t: &str| div().pt(px(space::S)).px(px(space::XS)).text_size(px(text::FOOTNOTE)).font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).child(t.to_string());
        if !done.is_empty() {
            list = list.child(section("Merged and Closed"));
            for p in done {
                list = list.child(pull_card(p, theme));
            }
        }
        if !mentioned.is_empty() {
            list = list.child(section("Mentioned"));
            for p in mentioned {
                list = list.child(pull_card(p, theme));
            }
        }
        column.child(header).child(list).into_any_element()
    }

    fn side_chat(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some((chat, view, _)) = &self.side else { return empty("No thread open.", theme) };
        let (running, permission, empty_chat) = {
            let c = chat.read(cx);
            (c.running, c.permission().cloned(), c.items.is_empty())
        };
        let ws = self.workspace.clone();
        let parent = self.thread.as_ref().map(|t| t.read(cx).id().to_string()).unwrap_or_default();
        let (stop_ws, stop_chat, clear_ws) = (ws.clone(), chat.clone(), ws.clone());
        let header = panel_header("Side Chat", Some("A copy of this thread answers".into()), theme).when(!running && !empty_chat, |d| {
            d.child(tool_button("side-clear", icons::rotate_ccw(), "Clear: start the side chat over", theme).on_click(cx.listener(move |this, _, _, cx| {
                clear_ws.update(cx, |ws, cx| ws.clear_side_chat(&parent, cx));
                this.side = None;
                this.ensure_side(cx);
                cx.notify();
            })))
        });
        let mut column = div().size_full().flex().flex_col().child(header);
        if empty_chat {
            column = column.child(div().flex_1().min_h_0().child(empty_panel(
                icons::message_square(),
                "Ask on the Side",
                "A copy of this thread's conversation answers: it can read files and run commands you approve, can't edit anything, and nothing said here reaches the thread.",
                theme,
            )));
        } else {
            column = column.child(div().flex_1().min_h_0().child(gpui_kit::AnyView::from(view.clone()).cached(gpui_kit::StyleRefinement::default().size_full())));
        }
        if let Some(p) = permission {
            let more = chat.read(cx).permissions.len().saturating_sub(1);
            column = column.child(div().px(px(space::M)).pt(px(space::S)).child(crate::ui::composer::permission_card(&p, chat, ws.downgrade(), more, theme)));
        }
        column
            .child(
                div().flex_none().px(px(space::M)).pb(px(space::M)).pt(px(space::XS)).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::XS))
                        .pl(px(space::XS))
                        .pr(px(space::XS))
                        .py(px(space::XS))
                        .rounded(px(radius::LARGE))
                        .border_1()
                        .border_color(theme.separator)
                        .bg(theme.card)
                        .child(div().flex_1().min_w_0().child(Input::new(&self.question).appearance(false).small()))
                        .when(running, |d| {
                            d.child(
                                tool_button("side-stop", icons::circle_stop(), "Stop", theme).on_click(move |_, _, cx| stop_ws.update(cx, |ws, cx| ws.interrupt(&stop_chat, cx))),
                            )
                        }),
                ),
            )
            .into_any_element()
    }
}

/// A PR in the narrow sidebar: what it is, where it's going, what stands in its way.
fn pull_card(p: &crate::state::pulls::ThreadPull, theme: &Theme) -> impl IntoElement {
    let url = p.link.url.clone();
    let purple: gpui_kit::Hsla = gpui_kit::rgb(0xaf52de).into();
    let card = div()
        .id(SharedString::from(format!("pull-{}", p.link.id())))
        .px(px(space::M))
        .py(px(space::S + 2.))
        .rounded(px(radius::LARGE))
        .border_1()
        .border_color(theme.separator)
        .bg(theme.card)
        .cursor_pointer()
        .hover(|s| s.bg(theme.selected))
        .flex()
        .flex_col()
        .gap(px(space::XS + 1.))
        .on_click(move |_, _, cx| cx.open_url(&url));
    // Open in Browser, Copy Link, Copy Branch Name.
    let (u1, u2) = (p.link.url.clone(), p.link.url.clone());
    let head = p.pull.as_ref().ok().map(|pr| pr.head.clone());
    let mut card = card.context_menu(move |menu, _, _| {
        let (u1, u2) = (u1.clone(), u2.clone());
        let menu = menu
            .item(PopupMenuItem::new("Open in Browser").icon(IconName::ExternalLink).on_click(move |_, _, cx| cx.open_url(&u1)))
            .item(PopupMenuItem::new("Copy Link").icon(IconName::Copy).on_click(move |_, _, cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(u2.clone()))));
        match head.clone() {
            Some(h) => menu.item(PopupMenuItem::new("Copy Branch Name").on_click(move |_, _, cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(h.clone())))),
            None => menu,
        }
    });
    let fact = |icon: Icon, label: &str, color: gpui_kit::Hsla| div().flex().items_center().gap(px(3.)).text_color(color).child(icon.size(px(11.)).text_color(color)).child(label.to_string());
    match &p.pull {
        Ok(pr) => {
            let (icon, tint, chip): (Icon, gpui_kit::Hsla, Option<&str>) = match pr.state.as_str() {
                "merged" => (icons::merged(), purple, Some("Merged")),
                "closed" => (icons::pull_request_closed(), theme.error, Some("Closed")),
                _ if pr.draft => (icons::pull_request_draft(), theme.secondary, Some("Draft")),
                _ if pr.auto_merge => (icons::pull_request(), purple, Some("Auto-merge")),
                _ => (icons::pull_request(), theme.success, None),
            };
            let blocker = match pr.merge_status.as_deref().filter(|_| pr.state == "open") {
                Some("DIRTY") => Some("Conflicts"),
                Some("BEHIND") => Some("Behind base"),
                Some("BLOCKED") => Some("Blocked"),
                _ => None,
            };
            let review = match pr.review.as_str() {
                "approved" => Some((Icon::new(IconName::Check), "Approved", theme.success)),
                "changesRequested" => Some((Icon::new(IconName::CircleAlert), "Changes requested", theme.error)),
                "required" => Some((Icon::new(IconName::Eye), "Review required", theme.secondary)),
                _ => None,
            };
            let checks = match pr.checks.as_str() {
                "passing" => Some((Icon::new(IconName::CircleCheck), "Checks pass", theme.success)),
                "failing" => Some((Icon::new(IconName::CircleX), "Checks fail", theme.error)),
                "pending" => Some((icons::circle_dashed(), "Checks running", theme.warning)),
                _ => None,
            };
            card = card
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(space::S))
                        .child(div().flex_none().pt(px(2.)).child(icon.size(px(14.)).text_color(tint)))
                        .child(div().flex_1().min_w_0().font_weight(FontWeight::MEDIUM).line_clamp(2).child(pr.title.clone()))
                        .when_some(chip, |d, c| {
                            d.child(
                                div()
                                    .flex_none()
                                    .mt(px(1.))
                                    .px(px(space::XS + 2.))
                                    .rounded(px(radius::SMALL))
                                    .bg(tint.opacity(0.14))
                                    .text_size(px(text::CAPTION))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(tint)
                                    .child(c),
                            )
                        }),
                )
                .child(
                    div()
                        .pl(px(22.))
                        .flex()
                        .gap(px(space::XS + 2.))
                        .text_size(px(text::CAPTION))
                        .text_color(theme.secondary)
                        .child(div().flex_none().child(format!("{} #{}", p.link.path, p.link.number)))
                        .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_family(MONO).text_color(theme.tertiary).child(format!("{} → {}", pr.head, pr.base))),
                )
                .when(blocker.is_some() || review.is_some() || checks.is_some(), |d| {
                    d.child(
                        div()
                            .pl(px(22.))
                            .flex()
                            .flex_wrap()
                            .gap(px(space::S + 2.))
                            .text_size(px(text::CAPTION))
                            .when_some(blocker, |d, b| d.child(fact(Icon::new(IconName::TriangleAlert), b, theme.warning)))
                            .when_some(review, |d, (i, t, c)| d.child(fact(i, t, c)))
                            .when_some(checks, |d, (i, t, c)| d.child(fact(i, t, c))),
                    )
                });
        }
        Err(e) => {
            card = card
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S))
                        .child(icons::pull_request().size(px(14.)).text_color(theme.tertiary))
                        .child(div().font_weight(FontWeight::MEDIUM).child(format!("{} #{}", p.link.path, p.link.number))),
                )
                .child(div().pl(px(22.)).text_size(px(text::CAPTION)).text_color(theme.secondary).line_clamp(2).child(format!("Couldn't look it up: {}", e.lines().next().unwrap_or(""))));
        }
    }
    card
}

fn run_label(e: &crate::state::thread::Entry) -> (String, String) {
    let sub = e.sub.as_ref();
    let input: serde_json::Value = e.data.tool_input.as_deref().and_then(|i| serde_json::from_str(i).ok()).unwrap_or_default();
    let description = sub
        .map(|s| s.description.clone())
        .filter(|d| !d.is_empty() && d != "Sub-agent")
        .or_else(|| input["description"].as_str().map(str::to_string))
        .unwrap_or_else(|| "Sub-agent".into());
    let status = match sub.map(|s| s.status.as_str()) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ if e.data.tool_result.is_some() => if e.data.is_error { "failed".into() } else { "completed".into() },
        _ => "running".into(),
    };
    (description, status)
}

fn status_text(status: &str) -> &str {
    match status {
        "running" => "Working",
        "completed" => "Done",
        "failed" => "Failed",
        "stopped" | "killed" => "Stopped",
        other => other,
    }
}

/// A sub-agent's state as a small still icon: working, done, failed, stopped.
fn status_icon(status: &str, theme: &Theme) -> gpui_kit::Div {
    let (icon, color) = match status {
        "running" => (Icon::new(IconName::LoaderCircle), theme.accent),
        "failed" => (Icon::new(IconName::CircleX), theme.error),
        "stopped" | "killed" => (Icon::new(IconName::Ban), theme.warning),
        _ => (Icon::new(IconName::CircleCheck), theme.success),
    };
    div().flex_none().child(icon.size(px(13.)).text_color(color))
}

/// What a tool call is about, in a line: its file, command or pattern.
fn gist(name: &str, input: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(input).unwrap_or_default();
    let pick = ["file_path", "command", "pattern", "path", "url", "query", "description"].iter().find_map(|k| v[*k].as_str().map(str::to_string));
    let _ = name;
    pick.unwrap_or_else(|| input.chars().take(120).collect()).replace('\n', " ")
}

fn first_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = lines.iter().take(n).map(|l| l.chars().take(160).collect::<String>()).collect::<Vec<_>>().join("\n");
    if lines.len() > n {
        out += &format!("\n… {} more lines", lines.len() - n);
    }
    out
}

/// A file's icon by its kind: code, data, a picture, text.
fn file_icon(name: &str) -> Icon {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        _ if IMAGE_EXTENSIONS.contains(&ext.as_str()) => icons::file_image(),
        "json" | "toml" | "yaml" | "yml" | "lock" | "xml" | "plist" => icons::file_braces(),
        "md" | "txt" | "rst" | "log" | "csv" => Icon::new(IconName::FileText),
        "rs" | "swift" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "py" | "go" | "c" | "h" | "cc" | "cpp" | "hpp" | "java" | "kt" | "rb" | "sh" | "zsh" | "html" | "css" | "scss" | "sql" | "vue" | "svelte" | "lua" | "php" => icons::file_code(),
        _ => Icon::new(IconName::File),
    }
}

/// A line of quiet text in the middle of a panel: while something loads.
fn empty(line: &str, theme: &Theme) -> gpui_kit::AnyElement {
    div()
        .size_full()
        .p(px(space::XL))
        .flex()
        .items_center()
        .justify_center()
        .child(div().w_full().text_center().whitespace_normal().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(line.to_string()))
        .into_any_element()
}

/// A panel with nothing in it yet: its icon, what it is, and what will show here.
fn empty_state(icon: Icon, title: &str, detail: &str, theme: &Theme) -> gpui_kit::Div {
    div()
        .w_full()
        .px(px(space::XL + 4.))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(space::S))
        .child(
            div()
                .mb(px(2.))
                .w(px(36.))
                .h(px(36.))
                .rounded(px(radius::LARGE - 2.))
                .bg(theme.selected)
                .flex()
                .items_center()
                .justify_center()
                .child(icon.size(px(16.)).text_color(theme.secondary)),
        )
        .child(div().text_size(px(text::HEADLINE)).font_weight(FontWeight::SEMIBOLD).child(title.to_string()))
        .child(div().w_full().text_center().whitespace_normal().text_size(px(text::FOOTNOTE)).line_height(px(17.)).text_color(theme.secondary).child(detail.to_string()))
}

/// `empty_state` centred in the whole panel.
fn empty_panel(icon: Icon, title: &str, detail: &str, theme: &Theme) -> gpui_kit::AnyElement {
    div().size_full().pb(px(space::XL * 2.)).flex().items_center().justify_center().child(empty_state(icon, title, detail, theme)).into_any_element()
}

/// A panel's header, as the thread's: its title, a secondary line, its actions after.
fn panel_header(title: &str, subtitle: Option<String>, theme: &Theme) -> gpui_kit::Div {
    div()
        .flex_none()
        .h(px(40.))
        .flex()
        .items_center()
        .gap(px(2.))
        .pl(px(space::L))
        .pr(px(space::S + 2.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_baseline()
                .gap(px(space::S))
                .child(div().flex_none().text_size(px(text::HEADLINE)).font_weight(FontWeight::SEMIBOLD).child(title.to_string()))
                .when_some(subtitle, |d, s| d.child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(s))),
        )
}

/// A small ghost button with an icon and a tooltip, as in the thread's header.
pub(crate) fn tool_button(id: impl Into<gpui_kit::ElementId>, icon: Icon, tip: impl Into<SharedString>, theme: &Theme) -> Button {
    Button::new(id).small().ghost().icon(icon.size(px(14.)).text_color(theme.secondary)).tooltip(tip)
}

/// A tab in a strip (open files, browser pages): flat, the open one on a soft fill. Its close
/// button (`tab_close`) is always there: hover never moves anything.
pub(crate) fn strip_tab(id: SharedString, icon: Option<Icon>, title: String, on: bool, italic: bool, max_w: f32, theme: &Theme) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .flex_none()
        .max_w(px(max_w))
        .h(px(26.))
        .flex()
        .items_center()
        .gap(px(space::XS + 2.))
        .pl(px(space::S + 2.))
        .pr(px(3.))
        .rounded(px(radius::MEDIUM))
        .text_size(px(text::FOOTNOTE))
        .cursor_pointer()
        .when(on, |d| d.bg(theme.selected).text_color(theme.text).font_weight(FontWeight::MEDIUM))
        .when(!on, |d| d.text_color(theme.secondary).hover(|s| s.bg(theme.hover)))
        .when(italic, |d| d.italic())
        .when_some(icon, |d, i| d.child(div().flex_none().child(i.size(px(13.)).text_color(if on { theme.secondary } else { theme.tertiary }))))
        .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(title))
}

/// The × on a tab.
pub(crate) fn tab_close(id: SharedString, theme: &Theme) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .flex_none()
        .w(px(18.))
        .h(px(18.))
        .rounded(px(radius::SMALL))
        .flex()
        .items_center()
        .justify_center()
        .hover(|s| s.bg(theme.selected))
        .child(crate::ui::icons::x().size(px(11.)).text_color(theme.secondary))
}

impl Render for Inspector {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let panel = self.workspace.read(cx).panel;
        if panel == Panel::SideChat {
            self.ensure_side(cx);
        }
        if panel == Panel::PullRequests && self._pulls_timer.is_none() {
            self.ensure_pulls(cx);
        }
        let content = match panel {
            Panel::Browser => self.browser(&theme, cx),
            Panel::PullRequests => self.pull_requests(&theme, cx),
            Panel::SideChat => self.side_chat(&theme, cx),
            Panel::Subagents => self.subagents(&theme, cx),
            Panel::Files => self.files(&theme, cx),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.canvas)
            .border_l_1()
            .border_color(theme.separator)
            .child(self.tabs(panel, &theme, cx))
            .child(div().flex_1().min_h_0().child(content))
    }
}
