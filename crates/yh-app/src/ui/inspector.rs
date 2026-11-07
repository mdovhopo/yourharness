//! The right sidebar (⌥⌘0), for the open thread: its sub-agents, followed step by step, and
//! its folder's files (a tree, Go to File, files opened in tabs, read-only). A cached view of
//! its own: it redraws when its tab's data changes, not with every streamed delta.

use crate::state::thread::{SubStep, ThreadState};
use crate::state::workspace::{Panel, Workspace};
use crate::ui::transcript::TranscriptView;
use crate::theme::{Theme, MONO};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenuItem};
use gpui_kit::component::{Disableable, IconName, Selectable, Sizable};
use gpui_kit::{
    div, prelude::*, px, uniform_list, App, Context, Entity, FontWeight, ScrollHandle, ScrollStrategy, SharedString, Subscription,
    UniformListScrollHandle, WeakEntity, Window,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

pub const WIDTH: f32 = 380.;

/// What Split into a Stack asks Claude, as the Swift app does.
const SPLIT_INTO_STACK: &str = "Split the work in this thread into a GitHub stack of small pull requests with `gh stack`. Group the changes into layers that each make sense on their own and build on the one below, bottom first, one branch per layer with its changes committed there (`gh stack init`, then `gh stack add` for each layer above). Then run `gh stack submit --auto --open` and tell me the stack number and its PRs. Don't merge anything.";

/// Import Sign-ins: pick a browser, then the sites whose cookies to copy.
enum CookieImport {
    Sources(Vec<yh_core::cookies::Source>),
    Reading(String),
    Sites { source: String, cookies: Vec<yh_core::cookies::Cookie>, sites: Vec<(String, usize)>, picked: HashSet<String> },
    Done(String),
}

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

#[derive(Clone, Copy, PartialEq)]
enum BrowserPane {
    Console,
    Network,
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
    /// Import Sign-ins, when open: which step it's at.
    cookie_import: Option<CookieImport>,
    /// The side chat as last drawn: running, asking.
    /// The side chat's running state and the request it shows (its id), which change its controls.
    side_state: (bool, Option<String>, usize),
    /// The browser's address field, the console or network pane under the page, and the local
    /// dev servers it offers when nothing's open.
    address: Entity<InputState>,
    browser_pane: Option<BrowserPane>,
    dev_servers: Option<Vec<yh_core::dev_servers::Server>>,
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
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("Address or search"));
        let subscriptions = vec![
            cx.subscribe(&address, |this, input, e: &InputEvent, cx| {
                if let InputEvent::PressEnter { .. } = e {
                    let text = input.read(cx).value().to_string();
                    this.go_to(&text, cx);
                }
            }),
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
            address,
            // `YH_BROWSER_PANE=console|network` opens that pane, for checking it.
            browser_pane: match std::env::var("YH_BROWSER_PANE").as_deref() {
                Ok("console") => Some(BrowserPane::Console),
                Ok("network") => Some(BrowserPane::Network),
                _ => None,
            },
            dev_servers: None,
            wraps: std::env::var("YH_FILES_WRAP").as_deref() == Ok("1"),
            stacks_installed: None,
            cookie_import: None,
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
    fn tabs(&self, panel: Panel, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let mut row = div().flex().items_center().gap(px(2.)).px(px(10.)).h(px(44.)).flex_none().border_b_1().border_color(theme.separator);
        for p in Panel::ALL {
            let ws = self.workspace.clone();
            let b = Button::new(SharedString::from(format!("panel-{}", p.label()))).small().label(p.label());
            row = row.child((if p == panel { b.primary() } else { b.ghost() }).on_click(cx.listener(move |this, _, _, cx| {
                ws.update(cx, |ws, cx| ws.show_panel(p, cx));
                this.ensure_tree(cx);
                this.ensure_side(cx);
                this.ensure_pulls(cx);
                cx.notify();
            })));
        }
        row
    }

    fn subagents(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(thread) = self.thread.clone() else { return empty("No thread open.", theme) };
        let runs = thread.read(cx).subagents();
        if runs.is_empty() {
            return empty("No sub-agents. When Claude hands work to a sub-agent, you can follow it here step by step.", theme);
        }
        let picked = thread.read(cx).inspected_subagent.clone();
        let selected = picked
            .and_then(|id| runs.iter().find(|(r, _)| *r == id).cloned())
            .or_else(|| runs.iter().rev().find(|(_, e)| e.borrow().sub.as_ref().is_some_and(|s| s.status == "running")).cloned())
            .unwrap_or_else(|| runs[runs.len() - 1].clone());
        let mut column = div().size_full().flex().flex_col();
        if runs.len() > 1 {
            let mut list = div().id("subagent-runs").max_h(px(168.)).overflow_y_scroll().p(px(8.)).flex().flex_col().gap(px(2.)).border_b_1().border_color(theme.separator);
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
                        .gap(px(8.))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .when(on, |d| d.bg(theme.accent.opacity(0.15)))
                        .when(!on, |d| d.hover(|s| s.bg(theme.selected)))
                        .child(status_dot(&status, theme))
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
            .or_else(|| (status != "running").then(|| entry.data.tool_result.clone()).flatten());
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
            Button::new("subagent-stop").small().label("Stop").tooltip("Stop this sub-agent. The thread carries on.").on_click(move |_, _, cx| {
                ws.update(cx, |ws, cx| ws.stop_job(&t, &task, cx));
            })
        });
        let mut detail = div()
            .id("subagent-detail")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.sub_scroll)
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(div().flex().items_start().gap(px(8.)).child(div().flex_1().min_w_0().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(description)).when_some(stop, |d, b| d.child(b)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_color(theme.secondary)
                    .child(status_dot(&status, theme))
                    .child(format!("{} · {tools} tool{}", status_text(&status), if tools == 1 { "" } else { "s" }))
                    .when_some(agent_type, |d, t| d.child(div().child(format!("· {t}")))),
            );
        if !prompt.is_empty() {
            let open = self.prompt_open;
            detail = detail.child(
                div()
                    .id("subagent-prompt")
                    .p(px(10.))
                    .rounded(px(8.))
                    .bg(theme.card)
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).child(if open { "TASK ▾" } else { "TASK ▸" }))
                    .child(div().text_size(px(12.)).text_color(theme.secondary).when(!open, |d| d.line_clamp(3)).child(prompt))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.prompt_open = !this.prompt_open;
                        cx.notify();
                    })),
            );
        }
        for step in shown {
            detail = detail.child(match &step.tool {
                None => div().text_size(px(13.)).whitespace_normal().child(step.text.clone()),
                Some(name) => {
                    let gist = gist(name, &step.text);
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .flex()
                                .gap(px(6.))
                                .text_size(px(12.))
                                .child(div().font_weight(FontWeight::SEMIBOLD).child(name.clone()))
                                .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().font_family(MONO).text_color(theme.secondary).child(gist)),
                        )
                        .when_some(step.result.as_ref().map(|r| first_lines(r, 3)), |d, r| {
                            d.child(
                                div()
                                    .px(px(8.))
                                    .py(px(4.))
                                    .rounded(px(5.))
                                    .bg(theme.code)
                                    .font_family(MONO)
                                    .text_size(px(11.))
                                    .text_color(if step.is_error { theme.error } else { theme.secondary })
                                    .child(r),
                            )
                        })
                }
            });
        }
        if status == "running" {
            detail = detail.child(div().text_color(theme.secondary).child("Working…"));
        } else if let Some(report) = report.filter(|r| !r.trim().is_empty()) {
            detail = detail.child(
                div()
                    .p(px(12.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme.accent.opacity(0.4))
                    .bg(theme.accent.opacity(0.06))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Report"))
                    .child(div().whitespace_normal().child(report)),
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
        let top = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .p(px(8.))
            .child(
                Button::new("files-tree-toggle")
                    .small()
                    .ghost()
                    .icon(if hides_tree { IconName::PanelLeftOpen } else { IconName::PanelLeftClose })
                    .disabled(!has_files)
                    .tooltip(if hides_tree { "Show the folder" } else { "Hide the folder" })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_tree(cx))),
            )
            .child(div().flex_1().min_w_0().child(Input::new(&self.goto).small().cleanable(true)))
            .child(
                Button::new("files-wrap")
                    .small()
                    .ghost()
                    .label("Wrap")
                    .selected(wraps)
                    .disabled(current.is_none())
                    .tooltip(if wraps { "Don't wrap lines" } else { "Wrap lines" })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.wraps = !this.wraps;
                        this.ensure_wrapped();
                        cx.notify();
                    })),
            )
            .child({
                let (folder, path) = (folder.clone(), current.clone());
                Button::new("files-more").small().ghost().icon(IconName::Ellipsis).disabled(current.is_none()).tooltip("More").dropdown_menu(move |menu, _, _| match &path {
                    Some(p) => file_menu(menu, &folder, p),
                    None => menu,
                })
            });
        let mut column = div().size_full().flex().flex_col().child(top);
        if query {
            let mut list = div().id("goto-results").flex_1().min_h_0().overflow_y_scroll().px(px(8.)).flex().flex_col();
            if self.goto_results.is_empty() {
                list = list.child(div().p(px(8.)).text_color(theme.secondary).child("No files match."));
            }
            for path in &self.goto_results {
                let p = path.clone();
                let (dir, name) = path.rsplit_once('/').map_or((String::new(), path.clone()), |(d, n)| (d.to_string(), n.to_string()));
                list = list.child(
                    div()
                        .id(SharedString::from(format!("goto-{path}")))
                        .flex()
                        .gap(px(6.))
                        .px(px(8.))
                        .py(px(3.))
                        .rounded(px(5.))
                        .hover(|s| s.bg(theme.selected))
                        .child(div().flex_none().child(name))
                        .child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(11.)).text_color(theme.secondary).child(dir))
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
            let this = cx.entity().downgrade();
            let theme2 = *theme;
            let open_path = current.clone();
            let tree = uniform_list("file-tree", rows.len(), move |range, _, _| {
                range
                    .map(|i| {
                        let (path, name, depth, dir) = rows[i].clone();
                        let this: WeakEntity<Inspector> = this.clone();
                        let on = open_path.as_deref() == Some(path.as_str());
                        div()
                            .id(i)
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .pl(px(10. + depth as f32 * 14.))
                            .pr(px(8.))
                            .text_size(px(12.))
                            .when(on, |d| d.bg(theme2.accent.opacity(0.14)))
                            .hover(|s| s.bg(theme2.selected))
                            .child(div().w(px(10.)).text_color(theme2.secondary).child(if dir { "›" } else { "" }))
                            .child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().when(dir, |d| d.font_weight(FontWeight::MEDIUM)).child(name))
                            .on_click(move |e, _, cx| {
                                // One click opens a preview; a double click keeps it.
                                let keep = e.click_count() >= 2;
                                let _ = this.update(cx, |this, cx| if dir { this.toggle_dir(&path, cx) } else { this.open_tab(&path, None, !keep, cx) });
                            })
                    })
                    .collect()
            })
            .track_scroll(&self.tree_scroll);
            column = column.child(div().when(has_files, |d| d.h(px(220.)).flex_none()).when(!has_files, |d| d.flex_1()).min_h_0().child(tree.size_full()));
        }
        if !has_files {
            return column.into_any_element();
        }
        let mut tabs = div().id("file-tabs").flex().gap(px(4.)).px(px(8.)).py(px(6.)).border_t_1().border_b_1().border_color(theme.separator).overflow_x_scroll();
        let count = state.files.len();
        for (i, f) in state.files.iter().enumerate() {
            let on = i == state.current;
            let name = f.path.rsplit('/').next().unwrap_or(&f.path).to_string();
            let (this, folder, path) = (cx.entity().downgrade(), folder.clone(), f.path.clone());
            let preview = f.preview;
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("file-tab-{i}")))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(px(10.))
                    .text_size(px(11.))
                    .bg(if on { theme.accent.opacity(0.16) } else { theme.selected })
                    .text_color(if on { theme.text } else { theme.secondary })
                    .when(preview, |d| d.italic())
                    .child(name)
                    .child(div().id(SharedString::from(format!("close-{i}"))).px(px(2.)).text_color(theme.secondary).child("×").on_click(cx.listener(move |this, _, _, cx| {
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
                .flex()
                .items_center()
                .px(px(10.))
                .py(px(4.))
                .text_size(px(11.))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(div().min_w_0().text_ellipsis().overflow_hidden().text_color(theme.secondary).child(dir))
                .child(div().flex_none().child(name))
                .child(div().flex_1())
                .when_some(file.lines().map(|l| l.len()), |d, n| d.child(div().flex_none().text_color(theme.secondary).child(if n == 1 { "1 line".to_string() } else { format!("{n} lines") }))),
        );
        let notice = |text: String| div().px(px(10.)).py(px(4.)).text_size(px(11.)).text_color(theme.secondary).border_b_1().border_color(theme.separator).child(text);
        if file.cut_lines {
            column = column.child(notice(format!("Lines over {} characters are cut", "5,000")));
        }
        if let Some(n) = file.note.clone() {
            column = column.child(notice(n));
        }
        let full = if file.path.starts_with('/') { file.path.clone() } else { format!("{}/{}", folder.trim_end_matches('/'), file.path) };
        let unavailable = |title: &str, detail: Option<String>, open: bool| {
            let full = full.clone();
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .p(px(20.))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(title.to_string()))
                .when_some(detail, |d, t| d.child(div().text_size(px(12.)).text_center().text_color(theme.secondary).child(t)))
                .when(open, |d| d.child(Button::new("open-default").small().label("Open in Default App").on_click(move |_, _, _| open_default(&full))))
                .into_any_element()
        };
        let body = match &file.content {
            FileContent::Missing => unavailable("File Not Found", Some("It may have been moved or deleted.".into()), false),
            FileContent::Unreadable(why) => unavailable("Can't Read This File", Some(why.clone()), false),
            FileContent::Folder => unavailable("Folder", None, true),
            FileContent::Binary(size) => unavailable("Binary File", Some(size_text(*size)), true),
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
                let gutter = digits as f32 * 7.5 + 14.;
                let row = move |i: usize, wrap: bool| {
                    let on = picked.is_some_and(|(a, b)| i + 1 >= a && i + 1 <= b.max(a));
                    div()
                        .when(!wrap, |d| d.h(px(17.)))
                        .when(wrap, |d| d.w_full())
                        .flex()
                        .font_family(MONO)
                        .text_size(px(11.5))
                        .line_height(px(17.))
                        .when(!wrap, |d| d.whitespace_nowrap())
                        .when(on, |d| d.bg(theme3.warning.opacity(0.18)))
                        .child(div().flex_none().w(px(gutter)).pr(px(8.)).text_right().text_color(theme3.secondary.opacity(0.7)).child(format!("{}", i + 1)))
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
    /// The thread's browser: its tabs, the page's address, and the place the page is laid
    /// over (GPUI keeps it empty; the web view follows its bounds every frame).
    /// The browser's thread and project, for the panel's buttons.
    fn browser_thread(&self, cx: &App) -> Option<(String, String)> {
        self.thread.as_ref().map(|t| {
            let t = t.read(cx);
            (t.id().to_string(), t.meta.project_id.clone())
        })
    }

    /// The address field: a URL, a host, or words to search for, in the current tab or a new one.
    fn go_to(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some((thread, project)) = self.browser_thread(cx) else { return };
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let url = crate::browser::url_from(text).unwrap_or_else(|| format!("https://duckduckgo.com/?q={}", text.split_whitespace().collect::<Vec<_>>().join("+")));
        if crate::browser::Browsers::with_tab(&thread, |t| t.web.load(&url)).is_none() {
            crate::browser::Browsers::open_for_user(&thread, Some(&project), Some(&url));
        }
        cx.notify();
    }

    /// Pick an element on the page: what you click goes into the composer, described so the
    /// agent can find it again.
    fn pick_button(&self, thread: &str, cx: &mut Context<Self>) -> Button {
        let thread = thread.to_string();
        Button::new("browser-pick").xsmall().ghost().label("⌖").tooltip("Pick an element for your message (Esc cancels)").on_click(cx.listener(move |this, _, _, cx| this.pick(&thread, cx)))
    }

    pub fn pick(&mut self, thread: &str, cx: &mut Context<Self>) {
        let Some(reply) = crate::browser::Browsers::pick(thread) else { return };
        let ws = self.workspace.clone();
        cx.spawn(async move |_, cx| {
            if let Ok(Ok(json)) = reply.recv_async().await {
                if let Some(text) = crate::browser::Browsers::picked_text(&json) {
                    if std::env::var("YH_DATA_DIR").is_ok() {
                        eprintln!("PICKED {text}");
                    }
                    let _ = ws.update(cx, |_, cx| cx.emit(crate::state::workspace::WorkspaceEvent::AppendToComposer(text)));
                }
            }
        })
        .detach();
    }

    fn find_dev_servers(&mut self, cx: &mut Context<Self>) {
        let task = cx.background_executor().spawn(async { yh_core::dev_servers::list() });
        cx.spawn(async move |this, cx| {
            let found = task.await;
            let _ = this.update(cx, |this, cx| {
                this.dev_servers = Some(found);
                cx.notify();
            });
        })
        .detach();
    }

    fn browser(&mut self, theme: &Theme, window: &mut Window, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some((thread, project)) = self.browser_thread(cx) else { return empty("No thread open.", theme) };
        // In its own window: the panel points there.
        if crate::ui::browser_window::is_open(&thread) {
            crate::browser::Browsers::show(None);
            let (t1, t2) = (thread.clone(), thread.clone());
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .px(px(20.))
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Open in Its Own Window"))
                .child(div().text_size(px(12.)).text_color(theme.secondary).text_center().child("This thread's browser is in a window of its own. Claude keeps using it there."))
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(Button::new("browser-show-window").small().outline().label("Show Window").on_click(move |_, _, cx| crate::ui::browser_window::activate(&t1, cx)))
                        .child(Button::new("browser-bring-back").small().label("Move Back Here").on_click(cx.listener(move |_, _, _, cx| {
                            crate::ui::browser_window::close(&t2, cx);
                            cx.notify();
                        }))),
                )
                .into_any_element();
        }
        let tabs: Vec<(u32, String, bool, bool)> = crate::browser::Browsers::with(|b| {
            b.sessions
                .get(&thread)
                .map(|s| {
                    let current = s.selected_tab().map(|t| t.id);
                    s.tabs.iter().map(|t| (t.id, if t.title().is_empty() { t.short_url() } else { t.title() }, Some(t.id) == current, t.web.is_loading())).collect()
                })
                .unwrap_or_default()
        });
        if let Some(import) = &self.cookie_import {
            crate::browser::Browsers::show(None);
            return self.cookie_panel(import, theme, cx);
        }
        let import_button = Button::new("import-sign-ins").xsmall().ghost().label("Import Sign-ins…").on_click(cx.listener(|this, _, _, cx| {
            this.cookie_import = Some(CookieImport::Sources(yh_core::cookies::sources()));
            cx.notify();
        }));
        if tabs.is_empty() {
            crate::browser::Browsers::show(None);
            if self.dev_servers.is_none() {
                self.dev_servers = Some(vec![]);
                self.find_dev_servers(cx);
            }
            let servers = self.dev_servers.clone().unwrap_or_default();
            let restorable = crate::browser::Browsers::with(|b| b.sessions.get(&thread).map(|s| s.restorable.len()).unwrap_or(0));
            let mut list = div().flex().flex_col().gap(px(4.)).w_full().px(px(16.));
            if !servers.is_empty() {
                list = list.child(div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).child("Running on this Mac"));
            }
            for server in servers {
                let url = server.url();
                list = list.child(
                    Button::new(SharedString::from(format!("dev-{}", server.port))).small().outline().w_full().label(format!("localhost:{} · {}", server.port, server.command)).on_click(cx.listener(move |this, _, _, cx| this.go_to(&url, cx))),
                );
            }
            return div()
                .size_full()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(div().px(px(8.)).pt(px(8.)).child(Input::new(&self.address).small()))
                .child(div().flex_1().child(empty("No pages open. Type an address, or wait for Claude to open one: it shows here, and you can watch what it does.", theme)))
                .when(restorable > 0, |d| {
                    let (thread, project) = (thread.clone(), project.clone());
                    d.child(div().flex().justify_center().child(
                        Button::new("browser-reopen").small().outline().label(if restorable == 1 { "Reopen the Page Closed While Idle".to_string() } else { format!("Reopen {restorable} Pages Closed While Idle") }).on_click(cx.listener(move |_, _, _, cx| {
                            crate::browser::Browsers::reopen(&thread, Some(&project));
                            cx.notify();
                        })),
                    ))
                })
                .child(list)
                .child(
                    div()
                        .flex()
                        .justify_center()
                        .gap(px(6.))
                        .pb(px(16.))
                        .child(Button::new("dev-refresh").xsmall().ghost().label("Look for Dev Servers").on_click(cx.listener(|this, _, _, cx| this.find_dev_servers(cx))))
                        .child(import_button),
                )
                .into_any_element();
        }
        let (url, can_back, can_forward) = crate::browser::Browsers::with_tab(&thread, |t| (t.web.url().unwrap_or_default(), t.web.can_go_back(), t.web.can_go_forward())).unwrap_or_default();
        // The field shows the page's address, except while you're typing in it.
        if !gpui_kit::Focusable::focus_handle(self.address.read(cx), cx).is_focused(window) && self.address.read(cx).value() != url.as_str() {
            let shown = url.clone();
            self.address.update(cx, |a, cx| a.set_value(shown, window, cx));
        }
        let mut strip = div().id("browser-tabs").flex().items_center().gap(px(4.)).px(px(8.)).py(px(6.)).overflow_x_scroll();
        for (id, title, on, loading) in tabs {
            let (t, t2) = (thread.clone(), thread.clone());
            strip = strip.child(
                div()
                    .id(SharedString::from(format!("browser-tab-{id}")))
                    .group("tab")
                    .flex_none()
                    .max_w(px(170.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .pl(px(8.))
                    .pr(px(4.))
                    .py(px(2.))
                    .rounded(px(10.))
                    .text_size(px(11.))
                    .bg(if on { theme.accent.opacity(0.16) } else { theme.selected })
                    .text_color(if on { theme.text } else { theme.secondary })
                    .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(if loading { format!("◌ {title}") } else { title }))
                    .child(
                        div()
                            .id(SharedString::from(format!("browser-tab-close-{id}")))
                            .flex_none()
                            .px(px(3.))
                            .rounded(px(6.))
                            .text_color(theme.secondary)
                            .hover(|s| s.bg(theme.separator))
                            .child("×")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                crate::browser::Browsers::close_tab_now(&t2, id);
                                cx.stop_propagation();
                                cx.notify();
                            })),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        crate::browser::Browsers::select_tab(&t, id);
                        cx.notify();
                    })),
            );
        }
        let new_tab = {
            let (thread, project) = (thread.clone(), project.clone());
            Button::new("browser-new-tab").xsmall().ghost().label("+").tooltip("New Tab").on_click(cx.listener(move |this, _, window, cx| {
                crate::browser::Browsers::open_for_user(&thread, Some(&project), None);
                this.address.update(cx, |a, cx| {
                    a.set_value("", window, cx);
                    a.focus(window, cx);
                });
                cx.notify();
            }))
        };
        let nav = |id: &'static str, label: &'static str, tip: &'static str, enabled: bool, act: fn(&crate::webview::WebView)| {
            let thread = thread.clone();
            Button::new(id).xsmall().ghost().label(label).tooltip(tip).disabled(!enabled).on_click(cx.listener(move |_, _, _, cx| {
                crate::browser::Browsers::with_tab(&thread, |t| act(&t.web));
                cx.notify();
            }))
        };
        let menu = {
            let (thread, project, url) = (thread.clone(), project.clone(), url.clone());
            let page = cx.entity().downgrade();
            let ws = self.workspace.clone();
            Button::new("browser-more").xsmall().ghost().label("⋯").dropdown_menu(move |menu, _, _| {
                let (u1, u2) = (url.clone(), url.clone());
                let (p1, p2, p3, p4, p5) = (page.clone(), page.clone(), page.clone(), page.clone(), page.clone());
                let (project, thread) = (project.clone(), thread.clone());
                let (ws, pop_thread, pop_project) = (ws.clone(), thread.clone(), project.clone());
                menu.item(PopupMenuItem::new("Open in Window").on_click(move |_, _, cx| {
                    crate::ui::browser_window::open(ws.clone(), pop_thread.clone(), pop_project.clone(), cx);
                    let _ = p5.update(cx, |_, cx| cx.notify());
                }))
                .separator()
                .item(PopupMenuItem::new("Open in Default Browser").on_click(move |_, _, cx| cx.open_url(&u1)))
                    .item(PopupMenuItem::new("Copy Address").on_click(move |_, _, cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(u2.clone()))))
                    .separator()
                    .item(PopupMenuItem::new("Console").on_click(move |_, _, cx| {
                        let _ = p1.update(cx, |this, cx| {
                            this.browser_pane = if this.browser_pane == Some(BrowserPane::Console) { None } else { Some(BrowserPane::Console) };
                            cx.notify();
                        });
                    }))
                    .item(PopupMenuItem::new("Network").on_click(move |_, _, cx| {
                        let _ = p2.update(cx, |this, cx| {
                            this.browser_pane = if this.browser_pane == Some(BrowserPane::Network) { None } else { Some(BrowserPane::Network) };
                            cx.notify();
                        });
                    }))
                    .separator()
                    .item(PopupMenuItem::new("Import Sign-ins…").on_click(move |_, _, cx| {
                        let _ = p3.update(cx, |this, cx| {
                            this.cookie_import = Some(CookieImport::Sources(yh_core::cookies::sources()));
                            cx.notify();
                        });
                    }))
                    .item(PopupMenuItem::new("Clear Browsing Data").on_click(move |_, _, cx| {
                        // Sign-ins and storage go for the whole project; open pages reload signed out.
                        crate::webview::clear_store(&project);
                        crate::browser::Browsers::with_tab(&thread, |t| t.web.reload());
                        let _ = p4.update(cx, |_, cx| cx.notify());
                    }))
            })
        };
        let pane = self.browser_pane.map(|which| {
            let lines: Vec<(String, bool)> = crate::browser::Browsers::with_tab(&thread, |t| match which {
                BrowserPane::Console => t.console.iter().rev().take(200).map(|c| (format!("{} {}", c.level, c.text), c.level == "error")).collect(),
                BrowserPane::Network => t
                    .network
                    .iter()
                    .rev()
                    .take(200)
                    .map(|r| {
                        let status = r.failure.clone().or(r.status.map(|s| s.to_string())).unwrap_or_else(|| "…".into());
                        let failed = r.failure.is_some() || r.status.is_some_and(|s| s >= 400);
                        (format!("{status} {} {}", r.method, r.url), failed)
                    })
                    .collect(),
            })
            .unwrap_or_default();
            div()
                .h(px(180.))
                .flex_none()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(theme.separator)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(8.))
                        .py(px(3.))
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(if which == BrowserPane::Console { "Console" } else { "Network" })
                        .child(Button::new("pane-close").xsmall().ghost().label("×").on_click(cx.listener(|this, _, _, cx| {
                            this.browser_pane = None;
                            cx.notify();
                        }))),
                )
                .child(
                    div()
                        .id("browser-pane")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px(px(8.))
                        .font_family(MONO)
                        .text_size(px(10.5))
                        .when(lines.is_empty(), |d| d.text_color(theme.secondary).child("Nothing yet."))
                        .children(lines.into_iter().map(|(line, bad)| div().py(px(1.)).whitespace_nowrap().overflow_hidden().text_ellipsis().text_color(if bad { theme.error } else { theme.secondary }).child(line))),
                )
        });
        let place = thread.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().flex().items_center().child(div().flex_1().min_w_0().child(strip)).child(div().pr(px(6.)).child(new_tab)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .px(px(6.))
                    .pb(px(6.))
                    .child(nav("browser-back", "‹", "Back", can_back, |w| w.go_back()))
                    .child(nav("browser-forward", "›", "Forward", can_forward, |w| w.go_forward()))
                    .child(nav("browser-reload", "↻", "Reload", true, |w| w.reload()))
                    .child(div().flex_1().min_w_0().child(Input::new(&self.address).xsmall().font_family(MONO)))
                    .child(self.pick_button(&thread, cx))
                    .child(menu),
            )
            .child(
                div().flex_1().min_h_0().border_t_1().border_color(theme.separator).child(
                    gpui_kit::canvas(
                        move |bounds, _, _| {
                            let o = bounds.origin;
                            let s = bounds.size;
                            crate::browser::Browsers::show(Some((place.clone(), (f64::from(o.x), f64::from(o.y), f64::from(s.width), f64::from(s.height)))));
                        },
                        |_, _, _, _| {},
                    )
                    .size_full(),
                ),
            )
            .children(pane)
            .into_any_element()
    }

    /// The sign-ins panel: Safari and each Chromium profile; then the source's sites.
    fn cookie_panel(&self, import: &CookieImport, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let mut column = div().id("cookie-import").size_full().overflow_y_scroll().p(px(14.)).flex().flex_col().gap(px(8.)).child(
            div()
                .flex()
                .items_center()
                .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child("Import Sign-ins"))
                .child(Button::new("cookie-close").xsmall().ghost().label("Done").on_click(cx.listener(|this, _, _, cx| {
                    this.cookie_import = None;
                    cx.notify();
                }))),
        );
        match import {
            CookieImport::Sources(sources) => {
                column = column.child(div().text_size(px(12.)).text_color(theme.secondary).whitespace_normal().child(
                    "Copy the cookies of sites you're signed in to from another browser into this project's browser, so Claude can use them signed in. Nothing is read until you pick a browser, and macOS asks first.",
                ));
                for (i, source) in sources.iter().enumerate() {
                    let source = source.clone();
                    column = column.child(
                        Button::new(SharedString::from(format!("cookie-source-{i}"))).small().label(source.label()).on_click(cx.listener(move |this, _, _, cx| {
                            this.read_cookies(source.clone(), cx)
                        })),
                    );
                }
            }
            CookieImport::Reading(label) => column = column.child(div().text_color(theme.secondary).child(format!("Reading {label}…"))),
            CookieImport::Done(text) => column = column.child(div().whitespace_normal().child(text.clone())),
            CookieImport::Sites { source, sites, picked, .. } => {
                column = column.child(div().text_size(px(12.)).text_color(theme.secondary).child(format!("{source}: pick the sites to bring over.")));
                let count = picked.len();
                column = column.child(Button::new("cookie-import-go").small().primary().label(format!("Import {count} Site{}", if count == 1 { "" } else { "s" })).on_click(cx.listener(|this, _, _, cx| this.store_cookies(cx))));
                for (site, n) in sites.iter().take(400) {
                    let on = picked.contains(site);
                    let key = site.clone();
                    column = column.child(
                        div()
                            .id(SharedString::from(format!("site-{site}")))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(6.))
                            .py(px(3.))
                            .rounded(px(5.))
                            .hover(|d| d.bg(theme.selected))
                            .child(div().w(px(14.)).text_color(if on { theme.accent } else { theme.secondary }).child(if on { "☑" } else { "☐" }))
                            .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().child(site.clone()))
                            .child(div().text_size(px(11.)).text_color(theme.secondary).child(n.to_string()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(CookieImport::Sites { picked, .. }) = &mut this.cookie_import {
                                    if !picked.remove(&key) {
                                        picked.insert(key.clone());
                                    }
                                }
                                cx.notify();
                            })),
                    );
                }
            }
        }
        column.into_any_element()
    }

    fn read_cookies(&mut self, source: yh_core::cookies::Source, cx: &mut Context<Self>) {
        let label = source.label();
        self.cookie_import = Some(CookieImport::Reading(label.clone()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { yh_core::cookies::read(&source) }).await;
            let _ = this.update(cx, |this, cx| {
                this.cookie_import = Some(match result {
                    Ok(cookies) => {
                        let sites = yh_core::cookies::sites(&cookies);
                        CookieImport::Sites { source: label, cookies, sites, picked: HashSet::new() }
                    }
                    Err(e) => CookieImport::Done(e),
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn store_cookies(&mut self, cx: &mut Context<Self>) {
        let Some(CookieImport::Sites { cookies, picked, .. }) = &self.cookie_import else { return };
        let chosen: Vec<yh_core::cookies::Cookie> = cookies.iter().filter(|c| picked.contains(c.site())).cloned().collect();
        let Some(project) = self.thread.as_ref().map(|t| t.read(cx).meta.project_id.clone()) else { return };
        let sites = picked.len();
        let (tx, rx) = flume::bounded(1);
        crate::webview::store_cookies(&project, chosen, tx);
        cx.spawn(async move |this, cx| {
            let n = rx.recv_async().await.unwrap_or(0);
            let _ = this.update(cx, |this, cx| {
                this.cookie_import = Some(CookieImport::Done(format!("Imported {n} cookies for {sites} site{} into this project's browser. Pages opened from now on are signed in where those cookies are still good.", if sites == 1 { "" } else { "s" })));
                cx.notify();
            });
        })
        .detach();
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
        let b = Button::new(if compact { "split-stack-compact" } else { "split-stack" }).xsmall().ghost();
        let b = if compact { b.icon(IconName::GalleryVerticalEnd) } else { b.label("Split into a Stack") };
        Some(
            b.disabled(running)
                .tooltip("Ask Claude to split this thread's work into a stack of small pull requests, with gh stack")
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
        let Some((pulls, at)) = self.pulls.get(&id) else { return empty("Looking for this thread's pull requests…", theme) };
        if pulls.is_empty() {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .child(div().px(px(24.)).text_center().text_color(theme.secondary).child("No pull requests. When Claude opens, merges or comments on one in this thread, or one's linked here, it shows here and stays current."))
                .when_some(self.split_button(false, cx), |d, b| d.child(b))
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
        let mut column = div().id("pulls").size_full().overflow_y_scroll().p(px(12.)).flex().flex_col().gap(px(8.)).child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(11.))
                .text_color(theme.secondary)
                .child(div().flex_1().child(format!("{open_count} open · updated {}", if at.elapsed().as_secs() < 5 { "just now".to_string() } else { format!("{} s ago", at.elapsed().as_secs()) })))
                .when_some(self.split_button(true, cx), |d, b| d.child(b))
                .child(Button::new("pulls-refresh").xsmall().ghost().icon(IconName::RefreshCw).loading(self.pulls_loading).tooltip("Refresh").on_click(cx.listener(|this, _, _, cx| this.refresh_pulls(cx)))),
        );
        for p in open {
            column = column.child(pull_card(p, theme));
        }
        let header = |t: &str| div().pt(px(8.)).text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).child(t.to_string());
        if !done.is_empty() {
            column = column.child(header("MERGED AND CLOSED"));
            for p in done {
                column = column.child(pull_card(p, theme));
            }
        }
        if !mentioned.is_empty() {
            column = column.child(header("MENTIONED"));
            for p in mentioned {
                column = column.child(pull_card(p, theme));
            }
        }
        column.into_any_element()
    }

    fn side_chat(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some((chat, view, _)) = &self.side else { return empty("No thread open.", theme) };
        let (running, permission, empty_chat) = {
            let c = chat.read(cx);
            (c.running, c.permission().cloned(), c.items.is_empty())
        };
        let ws = self.workspace.clone();
        let parent = self.thread.as_ref().map(|t| t.read(cx).id().to_string()).unwrap_or_default();
        let mut column = div().size_full().flex().flex_col();
        if empty_chat {
            column = column.child(empty(
                "Ask something on the side. A copy of this thread's conversation answers: it can read files and run commands you approve, can't edit anything, and nothing said here reaches the thread.",
                theme,
            ));
        } else {
            column = column.child(div().flex_1().min_h_0().child(gpui_kit::AnyView::from(view.clone()).cached(gpui_kit::StyleRefinement::default().size_full())));
        }
        if let Some(p) = permission {
            let more = chat.read(cx).permissions.len().saturating_sub(1);
            column = column.child(div().px(px(10.)).pt(px(8.)).child(crate::ui::composer::permission_card(&p, chat, ws.downgrade(), more, theme)));
        }
        let (stop_ws, stop_chat, clear_ws) = (ws.clone(), chat.clone(), ws.clone());
        column
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .p(px(10.))
                    .border_t_1()
                    .border_color(theme.separator)
                    .child(div().flex_1().child(Input::new(&self.question).small()))
                    .when(running, |d| {
                        d.child(Button::new("side-stop").small().label("Stop").on_click(move |_, _, cx| stop_ws.update(cx, |ws, cx| ws.interrupt(&stop_chat, cx))))
                    })
                    .when(!running && !empty_chat, |d| {
                        d.child(Button::new("side-clear").small().ghost().label("Clear").tooltip("Start the side chat over").on_click(cx.listener(move |this, _, _, cx| {
                            clear_ws.update(cx, |ws, cx| ws.clear_side_chat(&parent, cx));
                            this.side = None;
                            this.ensure_side(cx);
                            cx.notify();
                        })))
                    }),
            )
            .into_any_element()
    }
}

/// A PR in the narrow sidebar: what it is, where it's going, what stands in its way.
fn pull_card(p: &crate::state::pulls::ThreadPull, theme: &Theme) -> impl IntoElement {
    let url = p.link.url.clone();
    let green: gpui_kit::Hsla = gpui_kit::rgb(0x34c759).into();
    let purple: gpui_kit::Hsla = gpui_kit::rgb(0xaf52de).into();
    let card = div()
        .id(SharedString::from(format!("pull-{}", p.link.id())))
        .p(px(10.))
        .rounded(px(8.))
        .bg(theme.card)
        .hover(|s| s.bg(theme.selected))
        .flex()
        .flex_col()
        .gap(px(4.))
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
    match &p.pull {
        Ok(pr) => {
            let (dot, chip): (gpui_kit::Hsla, Option<&str>) = match pr.state.as_str() {
                "merged" => (purple, Some("Merged")),
                "closed" => (theme.error, Some("Closed")),
                _ if pr.draft => (theme.secondary, Some("Draft")),
                _ if pr.auto_merge => (purple, Some("Auto-merge")),
                _ => (green, None),
            };
            let blocker = match pr.merge_status.as_deref().filter(|_| pr.state == "open") {
                Some("DIRTY") => Some("Conflicts"),
                Some("BEHIND") => Some("Behind base"),
                Some("BLOCKED") => Some("Blocked"),
                _ => None,
            };
            let review = match pr.review.as_str() {
                "approved" => Some(("Approved", green)),
                "changesRequested" => Some(("Changes requested", theme.error)),
                "required" => Some(("Review required", theme.secondary)),
                _ => None,
            };
            let checks = match pr.checks.as_str() {
                "passing" => Some(("Checks pass", green)),
                "failing" => Some(("Checks fail", theme.error)),
                "pending" => Some(("Checks running", theme.warning)),
                _ => None,
            };
            card = card
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .child(div().flex_none().w(px(8.)).h(px(8.)).rounded_full().bg(dot))
                        .child(div().flex_1().min_w_0().font_weight(FontWeight::MEDIUM).line_clamp(2).child(pr.title.clone()))
                        .when_some(chip, |d, c| d.child(div().flex_none().text_size(px(10.)).px(px(6.)).rounded(px(8.)).bg(dot.opacity(0.15)).text_color(dot).child(c))),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .text_size(px(11.))
                        .text_color(theme.secondary)
                        .child(format!("{} #{}", p.link.path, p.link.number))
                        .child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().font_family(MONO).child(format!("{} → {}", pr.head, pr.base))),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(8.))
                        .text_size(px(11.))
                        .when_some(blocker, |d, b| d.child(div().text_color(theme.warning).child(b)))
                        .when_some(review, |d, (t, c)| d.child(div().text_color(c).child(t)))
                        .when_some(checks, |d, (t, c)| d.child(div().text_color(c).child(t))),
                );
        }
        Err(e) => {
            card = card
                .child(div().font_weight(FontWeight::MEDIUM).child(format!("{} #{}", p.link.path, p.link.number)))
                .child(div().text_size(px(11.)).text_color(theme.secondary).line_clamp(2).child(format!("Couldn't look it up: {}", e.lines().next().unwrap_or(""))));
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

fn status_dot(status: &str, theme: &Theme) -> gpui_kit::Div {
    let color = match status {
        "running" => theme.accent,
        "failed" => theme.error,
        "stopped" | "killed" => theme.warning,
        _ => gpui_kit::rgb(0x34c759).into(),
    };
    div().flex_none().w(px(7.)).h(px(7.)).rounded_full().bg(color)
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

fn empty(text: &str, theme: &Theme) -> gpui_kit::AnyElement {
    div()
        .size_full()
        .p(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .child(div().w_full().text_center().whitespace_normal().text_color(theme.secondary).child(text.to_string()))
        .into_any_element()
}

impl Render for Inspector {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let panel = self.workspace.read(cx).panel;
        if panel == Panel::SideChat {
            self.ensure_side(cx);
        }
        if panel == Panel::PullRequests && self._pulls_timer.is_none() {
            self.ensure_pulls(cx);
        }
        let content = match panel {
            Panel::Browser => self.browser(&theme, window, cx),
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
