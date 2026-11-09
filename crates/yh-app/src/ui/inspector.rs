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
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

pub const WIDTH: f32 = 380.;

/// What Split into a Stack asks Claude.
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

/// A folder's files as a tree, read a folder at a time as it's opened (as Finder
/// does): each folder read so far, its entries folders first, by name. Everything on
/// disk shows; what git ignores is dimmed rather than hidden.
#[derive(Default)]
struct Tree {
    children: HashMap<String, Vec<(String, bool)>>,
}

/// A row of the tree: its path (relative), name, depth, whether it's a folder, and whether git
/// ignores it.
type TreeRow = (String, String, usize, bool, bool);

impl Tree {
    /// The rows shown with these folders open (only folders read so far open). `included` is
    /// what git sees, folders with a `/`: what isn't in it is ignored.
    fn rows(&self, open: &HashSet<String>, included: Option<&HashSet<String>>) -> Vec<TreeRow> {
        let ignored = |path: &str, dir: bool| included.is_some_and(|set| !set.contains(path) && !(dir && set.contains(&format!("{path}/"))));
        let mut rows = vec![];
        // Depth first, in order: push children reversed.
        let mut pending: Vec<(String, String, usize, bool)> = vec![];
        if let Some(top) = self.children.get("") {
            for (name, dir) in top.iter().rev() {
                pending.push((name.clone(), name.clone(), 0, *dir));
            }
        }
        while let Some((path, name, depth, dir)) = pending.pop() {
            if dir && open.contains(&path) {
                if let Some(kids) = self.children.get(&path) {
                    for (n, d) in kids.iter().rev() {
                        pending.push((format!("{path}/{n}"), n.clone(), depth + 1, *d));
                    }
                }
            }
            let dim = ignored(&path, dir);
            rows.push((path, name, depth, dir, dim));
        }
        rows
    }

    /// Forgets a folder that's gone, and what was read inside it.
    fn forget(&mut self, path: &str) {
        let prefix = format!("{path}/");
        self.children.retain(|k, _| k != path && !k.starts_with(&prefix));
    }
}

/// Folders of a tree read again, off the main thread: (folder, its entries, or None if gone),
/// and what git sees now when asked.
type Reread = (Vec<(String, Option<Vec<(String, bool)>>)>, Option<Option<HashSet<String>>>);

fn reread(root: &str, dirs: &[String], relist: bool) -> Reread {
    let full = |d: &str| if d.is_empty() { std::path::PathBuf::from(root) } else { std::path::Path::new(root).join(d) };
    let read = dirs.iter().map(|d| (d.clone(), full(d).is_dir().then(|| yh_core::files::list_dir(&full(d))))).collect();
    let included = relist.then(|| yh_core::files::not_ignored(std::path::Path::new(root)));
    (read, included)
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
    /// What each line starts inside (a comment or string from lines above), for colouring.
    carries: std::sync::Arc<Vec<yh_core::markdown::Carry>>,
    /// Its language, from its extension, for colouring.
    lang: String,
    /// The widest line: the list measures it to know how far it scrolls sideways.
    widest: usize,
}

impl OpenFile {
    fn lines(&self) -> Option<&std::sync::Arc<Vec<SharedString>>> {
        if let FileContent::Text(l) = &self.content { Some(l) } else { None }
    }

    /// Takes in what a read found, keeping its place: a reload when it changed on disk.
    fn take(&mut self, read: ViewerRead) {
        self.content = read.content;
        self.note = read.note;
        self.cut_lines = read.cut_lines;
        self.carries = read.carries;
        self.widest = read.widest;
        // A wrapped list measures each line: made again for the new lines, at the same place.
        if let (Some(old), FileContent::Text(lines)) = (self.wrapped.take(), &self.content) {
            let state = gpui_kit::ListState::new(lines.len(), gpui_kit::ListAlignment::Top, px(400.));
            let top = old.logical_scroll_top();
            state.scroll_to(gpui_kit::ListOffset { item_ix: top.item_ix.min(lines.len().saturating_sub(1)), offset_in_item: top.offset_in_item });
            self.wrapped = Some(state);
        }
    }
}

/// What reading a file for the viewer found.
struct ViewerRead {
    content: FileContent,
    note: Option<String>,
    cut_lines: bool,
    carries: std::sync::Arc<Vec<yh_core::markdown::Carry>>,
    widest: usize,
}

fn extension(path: &str) -> String {
    path.rsplit('/').next().and_then(|n| n.rsplit_once('.')).map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// `read_for_viewer`, with what each line starts inside worked out too.
fn read_file(full: &str) -> ViewerRead {
    let (content, note, cut_lines) = read_for_viewer(full);
    let (carries, widest) = match &content {
        FileContent::Text(lines) => (yh_core::markdown::line_carries(lines, &extension(full)), widest_line(lines)),
        _ => (vec![], 0),
    };
    ViewerRead { content, note, cut_lines, carries: std::sync::Arc::new(carries), widest }
}

/// The line that's drawn widest in a monospaced font, a tab as four.
fn widest_line(lines: &[SharedString]) -> usize {
    let width = |l: &SharedString| l.chars().map(|c| if c == '\t' { 4 } else { 1 }).sum::<usize>();
    lines.iter().enumerate().max_by_key(|(_, l)| width(l)).map_or(0, |(i, _)| i)
}

/// Lines longer than this are cut.
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

/// One line of code, coloured by `yh_core::markdown::highlight_line` from what it starts inside
/// (only lines on screen are), with Find's matches on it marked (`current` the one Find is on).
fn code_line(text: &SharedString, lang: &str, carry: yh_core::markdown::Carry, found: &[std::ops::Range<usize>], current: Option<usize>, theme: &Theme) -> gpui_kit::StyledText {
    use yh_core::markdown::Span;
    let styled = yh_core::markdown::highlight_line(text, lang, carry).0;
    let colours: Vec<(std::ops::Range<usize>, gpui_kit::Hsla)> = styled
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
            Some((r, color))
        })
        .collect();
    let style = |color: Option<gpui_kit::Hsla>, background: Option<gpui_kit::Hsla>| gpui_kit::HighlightStyle { color, background_color: background, ..Default::default() };
    if found.is_empty() {
        return gpui_kit::StyledText::new(text.clone()).with_highlights(colours.into_iter().map(|(r, c)| (r, style(Some(c), None))));
    }
    // GPUI takes runs in order, none overlapping: cut the text at every colour's and match's
    // edges, and give each piece its colour and background.
    let mut edges: Vec<usize> = vec![0, text.len()];
    for (r, _) in &colours {
        edges.extend([r.start, r.end]);
    }
    for r in found {
        edges.extend([r.start, r.end]);
    }
    edges.retain(|e| *e <= text.len() && text.is_char_boundary(*e));
    edges.sort_unstable();
    edges.dedup();
    let mut runs = vec![];
    for w in edges.windows(2) {
        let piece = w[0]..w[1];
        let color = colours.iter().find(|(r, _)| r.start <= piece.start && piece.end <= r.end).map(|(_, c)| *c);
        let hit = found.iter().position(|r| r.start <= piece.start && piece.end <= r.end);
        let background = hit.map(|i| if Some(i) == current { theme.warning.opacity(0.55) } else { theme.warning.opacity(0.25) });
        if color.is_some() || background.is_some() {
            runs.push((piece, style(color, background)));
        }
    }
    gpui_kit::StyledText::new(text.clone()).with_highlights(runs)
}

/// The longest a batch of a file's lines took to make, for `YH_FILES_PROBE` (µs).
static LINES_WORST_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// One folder's Files tab: what's open in the tree, the files in tabs.
#[derive(Default)]
struct Folder {
    tree: Option<Tree>,
    rows: Rc<Vec<TreeRow>>,
    open_dirs: HashSet<String>,
    /// What git doesn't ignore (folders with a `/`), when it's a repo: the rest is dimmed.
    included: Option<HashSet<String>>,
    files: Vec<OpenFile>,
    current: usize,
    loading: bool,
    /// Hide Folder: only the open file shows.
    hides_tree: bool,
    /// Watching the folder for changes, while it's the open thread's (with the task that
    /// takes in what changed). Gone for the others, which are `stale` until shown again.
    watch: Option<(crate::state::file_watch::FolderWatch, gpui_kit::Task<()>)>,
    stale: bool,
}

impl Folder {
    fn rebuild_rows(&mut self) {
        if let Some(tree) = &self.tree {
            self.rows = Rc::new(tree.rows(&self.open_dirs, self.included.as_ref()));
        }
    }
}

pub struct Inspector {
    workspace: Entity<Workspace>,
    thread: Option<Entity<ThreadState>>,
    seen_subagents: u64,
    /// Background sub-agents running in the thread when last drawn: the Sub-agents tab's badge.
    seen_agents: usize,
    /// The end of what each command the shown sub-agent started printed, by task ID: read off
    /// the main thread every two seconds while one runs and the Sub-agents tab shows.
    agent_outputs: HashMap<String, String>,
    agent_ticker: Option<gpui_kit::Task<()>>,
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
    pulls: HashMap<String, (crate::state::pulls::ThreadPulls, std::time::Instant)>,
    /// Threads whose PRs are being looked up now.
    pulls_syncing: HashSet<String>,
    /// A thread whose PRs were looked up before its project's first scan landed: looked up
    /// again once it does, for its branches' PRs and unsubmitted stacks.
    pulls_await_scan: Option<(String, String)>,
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
    /// Find in the open file (⌘F with the viewer focused): what's typed, the matches as
    /// (line, bytes in it), in order, the one it's on, and the file they're for.
    viewer_focus: gpui_kit::FocusHandle,
    file_find: Entity<InputState>,
    file_find_open: bool,
    file_hits: Rc<Vec<(usize, std::ops::Range<usize>)>>,
    file_hit: Option<usize>,
    file_hits_for: Option<String>,
    /// `YH_FILES_FIND=<words>`: typed in Find once a file opens (it needs the window).
    file_find_pending: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl Inspector {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let goto = cx.new(|cx| InputState::new(window, cx).placeholder("Go to File"));
        let file_find = cx.new(|cx| InputState::new(window, cx).placeholder("Find in file"));
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
            cx.subscribe(&file_find, |this, _, e: &InputEvent, cx| match e {
                InputEvent::Change => this.find_in_file(cx),
                InputEvent::PressEnter { shift, .. } => this.step_in_file(!*shift, cx),
                _ => {}
            }),
        ];
        let mut inspector = Inspector {
            viewer_focus: cx.focus_handle(),
            file_find,
            file_find_open: false,
            file_hits: Rc::new(vec![]),
            file_hit: None,
            file_hits_for: None,
            file_find_pending: None,
            workspace,
            thread: None,
            seen_subagents: 0,
            seen_agents: 0,
            agent_outputs: HashMap::new(),
            agent_ticker: None,
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
            pulls_syncing: HashSet::new(),
            pulls_await_scan: None,
            _pulls_timer: None,
            was_running: false,
            _subscriptions: subscriptions,
        };
        inspector.follow_selection(cx);
        Self::watch_pull_activity(cx);
        // `YH_SEARCH_PROBE=<words>`: how long until the full-text index (caught up in the
        // background a few seconds after launch) finds them, and what it finds.
        if let (Ok(words), Some(db)) = (std::env::var("YH_SEARCH_PROBE"), inspector.workspace.read(cx).library.db.clone()) {
            cx.background_executor()
                .spawn(async move {
                    let started = std::time::Instant::now();
                    let mut hits = vec![];
                    while started.elapsed().as_secs() < 60 {
                        hits = db.search(&words, 20);
                        if !hits.is_empty() {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(250));
                    }
                    eprintln!("SEARCH {words:?}: {} found after {:.1} s", hits.len(), started.elapsed().as_secs_f64());
                    for h in hits.iter().take(5) {
                        eprintln!("SEARCH hit {} #{}: {}", h.thread, h.seq, h.snippet);
                    }
                })
                .detach();
        }
        if std::env::var("YH_FILES_PROBE").is_ok() {
            inspector.files_probe(cx);
        }
        inspector
    }

    /// Watches the open thread: its sub-agents' changes redraw the sidebar, nothing else of it.
    fn follow_selection(&mut self, cx: &mut Context<Self>) {
        if let Some((thread, project)) = self.pulls_await_scan.clone() {
            // Once the scan is in and the first lookup is back (a second one now would be dropped).
            if self.workspace.read(cx).scans.contains_key(&project) && !self.pulls_syncing.contains(&thread) {
                self.pulls_await_scan = None;
                if let Some(t) = self.workspace.read(cx).thread(&thread) {
                    self.refresh_pulls_of(t, cx);
                }
            }
        }
        let selected = self.workspace.read(cx).selected_thread();
        if selected.as_ref().map(|t| t.entity_id()) != self.thread.as_ref().map(|t| t.entity_id()) {
            self.thread = selected.clone();
            self.prompt_open = false;
            self.seen_subagents = selected.as_ref().map_or(0, |t| t.read(cx).subagents_version);
            self.seen_agents = selected.as_ref().map_or(0, |t| t.read(cx).jobs.agents_running());
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
                    // The tab's badge follows how many run, whichever tab shows.
                    let agents = t.read(cx).jobs.agents_running();
                    if agents != this.seen_agents {
                        this.seen_agents = agents;
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

    /// The PRs tab looks its thread's PRs up when it shows, then every 15 seconds while it
    /// shows (and its project's repos again, every 30 s at most, for the PRs of its branches).
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
        if self.pulls.get(&id).is_none_or(|(_, at)| at.elapsed().as_secs() >= 10) {
            self.refresh_pulls(cx);
        }
        if self._pulls_timer.is_none() {
            self._pulls_timer = Some(cx.spawn(async move |this, cx| loop {
                cx.background_executor().timer(std::time::Duration::from_secs(15)).await;
                let Ok(showing) = this.update(cx, |this, cx| this.workspace.read(cx).panel == Panel::PullRequests && this.workspace.read(cx).shows_inspector) else { break };
                if showing {
                    let _ = this.update(cx, |this, cx| this.refresh_pulls(cx));
                }
            }));
        }
    }

    fn refresh_pulls(&mut self, cx: &mut Context<Self>) {
        if let Some(thread) = self.thread.clone() {
            self.refresh_pulls_of(thread, cx);
        }
    }

    /// Looks a thread's PRs up again, aside: those its tool calls named, the open ones of the
    /// branches it wrote to (from its project's scan), and those its record keeps. PRs found
    /// through its branches are saved with it, so they stay after merging.
    fn refresh_pulls_of(&mut self, thread: Entity<ThreadState>, cx: &mut Context<Self>) {
        let (db, id, folder, project, branches, saved) = {
            let ws = self.workspace.read(cx);
            let Some(db) = ws.library.db.clone() else { return };
            let t = thread.read(cx);
            let id = t.id().to_string();
            let branches = ws
                .scans
                .get(&t.meta.project_id)
                .map(|scan| {
                    let folders = yh_core::workspace::ThreadFolders {
                        id: t.id(),
                        footprint: &t.meta.footprint,
                        written: &t.meta.written_footprint,
                        working_directory: t.meta.working_directory.as_deref(),
                        last_active: t.meta.updated_at,
                    };
                    crate::state::pulls::branch_pulls(scan, &folders)
                })
                .unwrap_or_default();
            let strings = |key: &str| -> Vec<String> {
                ws.library.thread_field(&id, key).and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())).unwrap_or_default()
            };
            let saved = (strings("workedPullRequests"), strings("pullRequests"));
            (db, id.clone(), ws.folder_of(t), t.meta.project_id.clone(), branches, saved)
        };
        if !self.pulls_syncing.insert(id.clone()) {
            return;
        }
        if !self.workspace.read(cx).scans.contains_key(&project) {
            self.pulls_await_scan = Some((id.clone(), project.clone()));
        }
        // Its project's repos and their open PRs, read again now and then: where its branches are.
        self.workspace.update(cx, |ws, cx| ws.scan_project(&project, cx));
        let path = std::env::var("YH_PATH").ok();
        let new_links: Vec<String> = branches.pulls.iter().map(|p| p.link.url.clone()).filter(|u| !saved.0.contains(u)).collect();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let key = id.clone();
            let local = branches.local.clone();
            let found = cx
                .background_executor()
                .spawn(async move {
                    let links = crate::state::pulls::links(&db, &id, &folder, path.as_deref(), &branches.pulls, (&saved.0, &saved.1));
                    crate::state::pulls::look_up(links, path, &branches.open)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.pulls_syncing.remove(&key);
                if !new_links.is_empty() {
                    this.workspace.read(cx).library.update_thread(&key, |r| {
                        for field in ["workedPullRequests", "pullRequests"] {
                            let mut list: Vec<serde_json::Value> = r.get(field).and_then(|v| v.as_array().cloned()).unwrap_or_default();
                            for url in &new_links {
                                if !list.iter().any(|v| v.as_str() == Some(url.as_str())) {
                                    list.push(url.clone().into());
                                }
                            }
                            r.insert(field.into(), serde_json::Value::Array(list));
                        }
                    });
                }
                this.pulls.insert(key, (crate::state::pulls::ThreadPulls { pulls: found, local }, std::time::Instant::now()));
                // The project's first scan may have landed meanwhile: look again with it.
                if this.pulls_await_scan.is_some() {
                    this.follow_selection(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Claude touched a thread's PRs (`gh pr merge`, a push): look again 3 seconds later, and
    /// 12 after that, for checks and merges that take a little while.
    fn watch_pull_activity(cx: &mut Context<Self>) {
        let feed = crate::state::pulls::activity_feed();
        cx.spawn(async move |this, cx| {
            while let Ok(thread_id) = feed.recv_async().await {
                let later = this.clone();
                cx.spawn(async move |cx| {
                    for delay in [3, 12] {
                        cx.background_executor().timer(std::time::Duration::from_secs(delay)).await;
                        let looked = later.update(cx, |this, cx| {
                            let Some(t) = this.workspace.read(cx).thread(&thread_id) else { return false };
                            if std::env::var("YH_PR_PANEL_LOG").is_ok() {
                                eprintln!("PRS looking again at {thread_id} ({delay} s on)");
                            }
                            this.refresh_pulls_of(t, cx);
                            true
                        });
                        if !looked.unwrap_or(false) {
                            break;
                        }
                    }
                })
                .detach();
            }
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

    /// The open file's line count in the Files tab, once read (the perf probe).
    pub fn open_file_lines(&self, cx: &App) -> Option<usize> {
        let folder = self.folder(cx)?;
        let f = self.folders.get(&folder).and_then(|s| s.files.get(s.current))?;
        f.lines().map(|l| l.len())
    }

    fn folder(&self, cx: &App) -> Option<String> {
        let t = self.thread.as_ref()?;
        Some(self.workspace.read(cx).folder_of(t.read(cx)))
    }

    /// The Files tab's tree for the thread's folder, read off the main thread, and watched:
    /// files made, removed or renamed show in it, and an open file that changes reloads. Only
    /// the open thread's folder is watched; another's catches up when it shows again.
    fn ensure_tree(&mut self, cx: &mut Context<Self>) {
        if self.workspace.read(cx).panel != Panel::Files {
            return;
        }
        let Some(folder) = self.folder(cx) else { return };
        for (other, state) in self.folders.iter_mut() {
            if *other != folder && state.watch.take().is_some() {
                state.stale = true;
            }
        }
        let state = self.folders.entry(folder.clone()).or_default();
        if state.watch.is_none() {
            state.watch = start_watch(&folder, cx);
        }
        if state.stale {
            // Back to a folder that wasn't watched meanwhile: everything read, read again.
            state.stale = false;
            let dirs: Vec<String> = state.tree.as_ref().map(|t| t.children.keys().cloned().collect()).unwrap_or_default();
            let files: Vec<String> = state.files.iter().map(|f| f.path.clone()).collect();
            self.reread(folder.clone(), dirs, true, cx);
            self.reload_files(&folder, files, cx);
            return;
        }
        if state.tree.is_some() || state.loading {
            return;
        }
        state.loading = true;
        // The top, and the folders left open (a link opened inside them).
        let mut dirs: Vec<String> = vec![String::new()];
        dirs.extend(state.open_dirs.iter().cloned());
        self.reread(folder, dirs, true, cx);
    }

    /// Reads folders of the tree again aside (and asks git what it ignores, with `relist`),
    /// then shows them, all at once.
    fn reread(&mut self, folder: String, dirs: Vec<String>, relist: bool, cx: &mut Context<Self>) {
        if dirs.is_empty() && !relist {
            return;
        }
        let root = folder.clone();
        cx.spawn(async move |this, cx| {
            let (read, included) = cx.background_executor().spawn(async move { reread(&root, &dirs, relist) }).await;
            let _ = this.update(cx, |this, cx| {
                let state = this.folders.entry(folder).or_default();
                let tree = state.tree.get_or_insert_with(Tree::default);
                let mut changed = vec![];
                for (dir, entries) in read {
                    match entries {
                        Some(e) => {
                            if tree.children.get(&dir) != Some(&e) {
                                changed.push(dir.clone());
                                tree.children.insert(dir, e);
                            }
                        }
                        None => {
                            changed.push(dir.clone());
                            tree.forget(&dir);
                            state.open_dirs.remove(&dir);
                        }
                    }
                }
                let included_changed = included.as_ref().is_some_and(|i| *i != state.included);
                if let Some(i) = included {
                    state.included = i;
                }
                state.loading = false;
                if changed.is_empty() && !included_changed && !state.rows.is_empty() {
                    return;
                }
                if std::env::var("YH_FILES_LOG").is_ok() {
                    eprintln!("FILES tree read {changed:?}{}", if included_changed { ", what git ignores again" } else { "" });
                }
                state.rebuild_rows();
                cx.notify();
            });
        })
        .detach();
    }

    /// Paths that changed on disk in the watched folder: folders read so far that hold them
    /// are read again, and open files among them reload, keeping their place.
    fn changed_on_disk(&mut self, folder: &str, paths: Vec<String>, cx: &mut Context<Self>) {
        let Some(state) = self.folders.get(folder) else { return };
        let Some(tree) = &state.tree else { return };
        let mut dirs: Vec<String> = vec![];
        for p in &paths {
            for d in [crate::state::file_watch::parent_of(p), p.clone()] {
                if tree.children.contains_key(&d) && !dirs.contains(&d) {
                    dirs.push(d);
                }
            }
        }
        let files: Vec<String> = state.files.iter().filter(|f| paths.contains(&f.path)).map(|f| f.path.clone()).collect();
        // A file made, removed or renamed may change what git ignores; one saved doesn't.
        let relist = !dirs.is_empty();
        self.reread(folder.to_string(), dirs, relist, cx);
        self.reload_files(folder, files, cx);
    }

    /// Reads open files again, each keeping its place in the viewer.
    fn reload_files(&mut self, folder: &str, files: Vec<String>, cx: &mut Context<Self>) {
        for path in files {
            let full = if path.starts_with('/') { path.clone() } else { format!("{}/{path}", folder.trim_end_matches('/')) };
            let folder = folder.to_string();
            cx.spawn(async move |this, cx| {
                let read = cx.background_executor().spawn(async move { read_file(&full) }).await;
                let _ = this.update(cx, |this, cx| {
                    let Some(f) = this.folders.get_mut(&folder).and_then(|s| s.files.iter_mut().find(|f| f.path == path)) else { return };
                    if std::env::var("YH_FILES_LOG").is_ok() {
                        let n = if let FileContent::Text(l) = &read.content { l.len() } else { 0 };
                        let top = f.scroll.0.borrow().base_handle.offset().y;
                        eprintln!("FILES reloaded {path} ({n} lines), scrolled at {:.0}", f32::from(top));
                    }
                    f.take(read);
                    // Find's matches were for the old text.
                    if this.file_hits_for.as_deref() == Some(path.as_str()) {
                        this.file_hits_for = None;
                        this.find_in_file(cx);
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn toggle_dir(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(folder) = self.folder(cx) else { return };
        let Some(state) = self.folders.get_mut(&folder) else { return };
        if !state.open_dirs.remove(path) {
            state.open_dirs.insert(path.to_string());
            // Read first, then open: a folder never shows empty.
            if !state.tree.as_ref().is_some_and(|t| t.children.contains_key(path)) {
                self.reread(folder, vec![path.to_string()], false, cx);
                return;
            }
        }
        state.rebuild_rows();
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
            let ViewerRead { content, note, cut_lines, carries, widest } = cx.background_executor().spawn(async move { read_file(&full) }).await;
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
                let lang = extension(&relative);
                let file = OpenFile { path: relative, content, note, cut_lines, scroll, wrapped: None, lines_picked: lines, preview, carries, lang, widest };
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
                static FIND_ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                if let Ok(words) = std::env::var("YH_FILES_FIND") {
                    if !FIND_ONCE.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        this.file_find_pending = Some(words);
                    }
                }
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

    /// ⌘F with the viewer focused: Find in the open file.
    fn show_file_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.file_find_open = true;
        self.file_find.update(cx, |i, cx| {
            i.focus(window, cx);
            i.select_all(window, cx);
        });
        self.find_in_file(cx);
        cx.notify();
    }

    fn close_file_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.file_find_open = false;
        self.file_hits = Rc::new(vec![]);
        self.file_hit = None;
        self.file_hits_for = None;
        window.focus(&self.viewer_focus, cx);
        cx.notify();
    }

    /// The open file's lines matching what's typed (letters in any case), found aside; Find
    /// then goes to the first at or below where you are.
    fn find_in_file(&mut self, cx: &mut Context<Self>) {
        let query = self.file_find.read(cx).value().to_string();
        let Some(folder) = self.folder(cx) else { return };
        let Some(f) = self.folders.get(&folder).and_then(|s| s.files.get(s.current)) else { return };
        let (Some(lines), path) = (f.lines().cloned(), f.path.clone()) else { return };
        if query.is_empty() || !self.file_find_open {
            self.file_hits = Rc::new(vec![]);
            self.file_hit = None;
            self.file_hits_for = Some(path);
            cx.notify();
            return;
        }
        // The line at the top of the viewer: each is `CODE_LINE` tall unwrapped.
        let from = Some(match &f.wrapped {
            Some(w) if self.wraps => w.logical_scroll_top().item_ix,
            _ => (-f32::from(f.scroll.0.borrow().base_handle.offset().y) / text::CODE_LINE).max(0.) as usize,
        });
        cx.spawn(async move |this, cx| {
            let q = query.clone();
            let hits = cx
                .background_executor()
                .spawn(async move {
                    // Byte offsets stay put with ASCII case folded only.
                    let needle = q.to_ascii_lowercase();
                    let mut hits = vec![];
                    for (i, line) in lines.iter().enumerate() {
                        let hay = line.to_ascii_lowercase();
                        let mut at = 0;
                        while let Some(p) = hay[at..].find(&needle) {
                            hits.push((i, at + p..at + p + needle.len()));
                            at += p + needle.len().max(1);
                            if hits.len() >= 10_000 || at >= hay.len() {
                                break;
                            }
                        }
                        if hits.len() >= 10_000 {
                            break;
                        }
                    }
                    hits
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.file_find.read(cx).value() != query.as_str() {
                    return;
                }
                let first = from.map_or(0, |top| hits.iter().position(|(l, _)| *l >= top).unwrap_or(0));
                this.file_hit = (!hits.is_empty()).then_some(first);
                this.file_hits = Rc::new(hits);
                this.file_hits_for = Some(path);
                if std::env::var("YH_FILES_LOG").is_ok() {
                    eprintln!("FILES find {query:?}: {} found", this.file_hits.len());
                }
                this.reveal_hit(cx);
            });
        })
        .detach();
    }

    /// ↩ the next match, ⇧↩ the one before (⌘G, ⇧⌘G too).
    fn step_in_file(&mut self, forward: bool, cx: &mut Context<Self>) {
        let n = self.file_hits.len();
        if n == 0 {
            return;
        }
        self.file_hit = Some(match self.file_hit {
            Some(i) if forward => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
            None => 0,
        });
        self.reveal_hit(cx);
    }

    fn reveal_hit(&mut self, cx: &mut Context<Self>) {
        let Some((line, _)) = self.file_hit.and_then(|i| self.file_hits.get(i)).cloned() else {
            cx.notify();
            return;
        };
        if let Some(f) = self.folder(cx).and_then(|folder| self.folders.get(&folder)).and_then(|s| s.files.get(s.current)) {
            f.scroll.scroll_to_item(line, ScrollStrategy::Center);
            if let Some(w) = &f.wrapped {
                w.scroll_to(gpui_kit::ListOffset { item_ix: line.saturating_sub(3), offset_in_item: px(0.) });
            }
        }
        cx.notify();
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
    /// (task id, output file) of the commands a sub-agent started that still need reading:
    /// running ones, and ended ones not read yet.
    fn agent_command_files(&self, agent_call: &str, cx: &App) -> Vec<(String, String)> {
        let Some(thread) = self.thread.as_ref() else { return vec![] };
        let t = thread.read(cx);
        let folder = t.jobs.folder.clone().or_else(|| t.session_id.as_ref().map(|s| crate::state::jobs::output_folder(&self.workspace.read(cx).folder_of(t), s)));
        t.jobs
            .owned_by(agent_call)
            .into_iter()
            .filter(|j| j.status == crate::state::jobs::JobStatus::Running || !self.agent_outputs.contains_key(&j.id))
            .filter_map(|j| Some((j.id.clone(), j.output_file.clone().or_else(|| folder.as_ref().map(|f| format!("{f}/{}.output", j.id)))?)))
            .collect()
    }

    /// Reads the shown sub-agent's commands' output every two seconds while there's any to
    /// read and the tab shows; stops by itself after.
    fn follow_agent_commands(&mut self, agent_call: String, cx: &mut Context<Self>) {
        if self.agent_ticker.is_some() || self.agent_command_files(&agent_call, cx).is_empty() {
            return;
        }
        self.agent_ticker = Some(cx.spawn(async move |this, cx| loop {
            let files = this
                .update(cx, |this, cx| {
                    let shown = this.workspace.read(cx).panel == Panel::Subagents
                        && this.thread.as_ref().is_some_and(|t| t.read(cx).inspected_subagent.as_deref() == Some(agent_call.as_str()) || t.read(cx).jobs.owned_by(&agent_call).iter().any(|j| j.status == crate::state::jobs::JobStatus::Running));
                    if shown { this.agent_command_files(&agent_call, cx) } else { vec![] }
                })
                .unwrap_or_default();
            if files.is_empty() {
                let _ = this.update(cx, |this, _| this.agent_ticker = None);
                break;
            }
            let read = cx
                .background_executor()
                .spawn(async move {
                    files
                        .into_iter()
                        .map(|(id, path)| {
                            let text = crate::state::jobs::tail(&path, 8192).map(|t| {
                                let lines: Vec<&str> = t.lines().collect();
                                lines[lines.len().saturating_sub(8)..].join("\n")
                            });
                            (id, text.unwrap_or_default())
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let mut changed = false;
                for (id, text) in read {
                    if this.agent_outputs.get(&id) != Some(&text) {
                        this.agent_outputs.insert(id, text);
                        changed = true;
                    }
                }
                if changed {
                    cx.notify();
                }
            });
            cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
        }));
    }

    /// The commands a sub-agent started (in the background, or taking a while): each with how
    /// it stands and the end of its output.
    fn agent_commands(&self, agent_call: &str, theme: &Theme, cx: &App) -> Option<gpui_kit::AnyElement> {
        let thread = self.thread.as_ref()?.read(cx);
        let owned = thread.jobs.owned_by(agent_call);
        if owned.is_empty() {
            return None;
        }
        let mut list = div().flex().flex_col().gap(px(space::S)).child(
            div().text_size(px(text::CAPTION)).font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).child(if owned.len() == 1 { "Its command".to_string() } else { format!("Its commands · {}", owned.len()) }),
        );
        for job in owned {
            let running = job.status == crate::state::jobs::JobStatus::Running;
            let (label, failed) = if running {
                (if job.background { "In background".to_string() } else { "Running".to_string() }, false)
            } else {
                job.status_label()
            };
            let output = self.agent_outputs.get(&job.id).filter(|o| !o.is_empty()).cloned();
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::XS))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S))
                            .child(Icon::new(if job.watch { IconName::Eye } else { IconName::SquareTerminal }).size(px(13.)).text_color(theme.secondary))
                            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(if job.description.is_empty() { "Command".to_string() } else { job.description.clone() }))
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(text::CAPTION))
                                    .text_color(if failed { theme.error } else if running { theme.accent } else { theme.secondary })
                                    .child(label),
                            ),
                    )
                    .when_some(output, |d, text| {
                        d.child(
                            div()
                                .ml(px(21.))
                                .font_family(MONO)
                                .text_size(px(text::CAPTION))
                                .line_height(px(15.))
                                .bg(theme.code)
                                .rounded(px(radius::MEDIUM))
                                .px(px(8.))
                                .py(px(6.))
                                .text_color(theme.secondary)
                                .whitespace_normal()
                                .child(text),
                        )
                    }),
            );
        }
        Some(list.into_any_element())
    }

    /// The tabs: a segmented control of icons, each named in its tooltip and in the panel's
    /// header under it.
    fn tabs(&self, panel: Panel, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        // The chosen segment stands up from the track, as in Settings.
        let raised = if theme.canvas.l < 0.5 { theme.bubble } else { theme.canvas };
        let mut track = div().flex_1().flex().items_center().gap(px(2.)).p(px(2.)).rounded(px(radius::MEDIUM + 1.)).bg(theme.selected);
        // Sub-agents still working in the background: a count on their tab.
        let agents = self.thread.as_ref().map_or(0, |t| t.read(cx).jobs.agents_running());
        for p in Panel::ALL {
            let ws = self.workspace.clone();
            let on = p == panel;
            let (icon, tip): (Icon, SharedString) = match p {
                Panel::PullRequests => (crate::ui::icons::pull_request(), "Pull Requests".into()),
                Panel::SideChat => (crate::ui::icons::message_square(), "Side Chat".into()),
                Panel::Subagents if agents > 0 => (
                    crate::ui::icons::bot(),
                    format!("Sub-agents: {}", self.thread.as_ref().and_then(|t| t.read(cx).jobs.summary()).unwrap_or_default()).into(),
                ),
                Panel::Subagents => (crate::ui::icons::bot(), "Sub-agents".into()),
                Panel::Files => (crate::ui::icons::files(), "Files".into()),
                Panel::Browser => (Icon::new(IconName::Globe), "Browser".into()),
            };
            let badge = (p == Panel::Subagents && agents > 0).then(|| {
                div()
                    .absolute()
                    .top(px(1.))
                    .right(px(3.))
                    .min_w(px(14.))
                    .h(px(14.))
                    .px(px(3.))
                    .rounded_full()
                    .bg(theme.accent)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(text::CAPTION))
                    .line_height(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(gpui_kit::white())
                    .child(agents.to_string())
            });
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
                    .relative()
                    .child(icon.size(px(15.)).text_color(if on { theme.accent } else { theme.secondary }))
                    .children(badge)
                    .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
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

    fn subagents(&mut self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(thread) = self.thread.clone() else { return empty("No thread open.", theme) };
        // Runs in pages that aren't held are listed from what was saved, read once, aside.
        if thread.read(cx).needs_saved_runs() {
            let (ws, t) = (self.workspace.clone(), thread.clone());
            cx.defer(move |cx| ws.update(cx, |ws, cx| ws.ensure_subagent_runs(&t, cx)));
        }
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
        // Running in the background now (this `claude`'s, not a stale saved status).
        let live: std::collections::HashSet<String> = thread.read(cx).jobs.agents.iter().filter_map(|j| j.tool_id.clone()).collect();
        let selected = picked
            .and_then(|id| runs.iter().find(|(r, _)| *r == id).cloned())
            .or_else(|| runs.iter().rev().find(|(_, e)| e.borrow().sub.as_ref().is_some_and(|s| s.status == "running")).cloned())
            .unwrap_or_else(|| runs[runs.len() - 1].clone());
        // Listed from its summary: its steps and report are read now, aside.
        if thread.read(cx).wants_run_detail(&selected.0) {
            let (ws, t, run) = (self.workspace.clone(), thread.clone(), selected.0.clone());
            cx.defer(move |cx| ws.update(cx, |ws, cx| ws.ensure_run_detail(&t, &run, cx)));
        }
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
                        .when_some(
                            if live.contains(id) {
                                Some("In background".to_string())
                            } else {
                                b.data.tool_input.as_deref().and_then(|i| serde_json::from_str::<serde_json::Value>(i).ok()).and_then(|v| v["subagent_type"].as_str().map(str::to_string))
                            },
                            |d, kind| {
                                let color = if live.contains(id) { theme.accent } else { theme.secondary };
                                d.child(div().flex_none().max_w(px(120.)).truncate().text_size(px(text::CAPTION)).text_color(color).child(kind))
                            },
                        )
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
        // What the CLI counted when the log holds fewer (steps it didn't stream, or older ones).
        let logged = log.iter().filter(|s| s.tool.is_some() && s.tool.as_deref() != Some("SubagentHandback")).count();
        let tools = sub.as_ref().and_then(|s| s.tool_uses).map_or(logged, |n| (n as usize).max(logged));
        // While it works: what it's doing now and how long it's gone (since it was last started
        // or continued), as of the last word from it, which redraws this.
        let working_now = (status == "running").then(|| {
            let since = thread.read(cx).jobs.agents.iter().find(|j| j.tool_id.as_deref() == Some(selected.0.as_str())).map(|j| j.started.elapsed().as_secs());
            let since = since.unwrap_or_else(|| (yh_core::library::now_ms() / 1000.0 - sub.as_ref().map_or(0.0, |s| s.started_at)).max(0.0) as u64);
            let doing = sub.as_ref().and_then(|s| s.activity.clone()).unwrap_or_else(|| if logged == 0 { "Starting…".into() } else { "Working…".into() });
            format!("{doing} · {}", crate::state::jobs::clock(since))
        });
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
                .tooltip("Stop this sub-agent and the commands it started. The thread carries on.")
                .on_click(move |_, _, cx| {
                    ws.update(cx, |ws, cx| ws.stop_job(&t, &task, cx));
                })
        });
        let commands = self.agent_commands(&selected.0, theme, cx);
        self.follow_agent_commands(selected.0.clone(), cx);
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
                                    .child(match tools {
                                        0 if status == "running" => status_text(&status).to_string(),
                                        n => format!("{} · {n} tool{}", status_text(&status), if n == 1 { "" } else { "s" }),
                                    })
                                    .when_some(
                                        {
                                            let kind: Vec<String> = agent_type.into_iter().chain(input["run_in_background"].as_bool().filter(|b| *b).map(|_| "in the background".to_string())).collect();
                                            (!kind.is_empty()).then(|| kind.join(" · "))
                                        },
                                        |d, t| d.child(div().child(format!("· {t}"))),
                                    ),
                            )
                            .when_some(working_now, |d, now| {
                                d.child(div().text_size(px(text::FOOTNOTE)).text_color(theme.accent).overflow_hidden().whitespace_nowrap().text_ellipsis().child(now))
                            }),
                    )
                    .when_some(stop, |d, b| d.child(b)),
            );
        if let Some(commands) = commands {
            detail = detail.child(commands);
        }
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
        // Its steps as the transcript reads them: what it said as Markdown, each tool call by its
        // icon, verb and the argument that matters, its output under it, all selectable.
        for step in shown {
            detail = detail.child(match &step.tool {
                None => div().text_size(px(text::BODY)).line_height(px(text::BODY_LINE)).child(crate::ui::rows::markdown_view(&yh_core::markdown::parse(&step.text), theme)).into_any_element(),
                Some(name) => {
                    let input: serde_json::Value = serde_json::from_str(&step.text).unwrap_or_default();
                    let p = crate::ui::steps::present(name, &input, step.result.as_deref());
                    let running = step.result.is_none() && status == "running";
                    let (icon, color) = if step.is_error {
                        (IconName::CircleX, theme.error)
                    } else if running {
                        (IconName::LoaderCircle, theme.accent)
                    } else {
                        (p.icon, theme.secondary)
                    };
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
                                .child(div().flex_none().child(Icon::new(icon).size(px(13.)).text_color(color)))
                                .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(p.verb.clone()))
                                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_family(MONO).text_size(px(text::CAPTION)).text_color(theme.secondary).child(if p.detail.is_empty() { gist(name, &step.text) } else { p.detail.clone() })),
                        )
                        .when_some(step.result.as_ref().map(|r| first_lines(r, 4)).filter(|r| !r.trim().is_empty()), |d, r| {
                            d.child(
                                div()
                                    .ml(px(19.))
                                    .px(px(space::S))
                                    .py(px(space::XS + 1.))
                                    .rounded(px(radius::SMALL))
                                    .bg(theme.code)
                                    .font_family(MONO)
                                    .text_size(px(text::CAPTION))
                                    .line_height(px(15.))
                                    .text_color(if step.is_error { theme.error } else { theme.secondary })
                                    .child(crate::ui::rows::selectable(r, theme)),
                            )
                        })
                        .into_any_element()
                }
            });
        }
        if status == "running" {
            // What it's doing now: its last tool call still out, as its card in the thread says.
            let activity = log.iter().rev().find(|s| s.tool.is_some()).filter(|s| s.result.is_none()).map(|s| {
                let input: serde_json::Value = serde_json::from_str(&s.text).unwrap_or_default();
                let p = crate::ui::steps::present(s.tool.as_deref().unwrap_or(""), &input, None);
                if p.detail.is_empty() { p.verb } else { format!("{} {}", p.verb, p.detail) }
            });
            detail = detail.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::XS + 2.))
                    .text_size(px(text::FOOTNOTE))
                    .text_color(theme.secondary)
                    .child(Icon::new(IconName::LoaderCircle).size(px(13.)).text_color(theme.accent))
                    .child(div().flex_1().min_w_0().truncate().child(activity.map(|a| format!("{a}…")).unwrap_or_else(|| "Working…".into()))),
            );
        } else if let Some(report) = report.filter(|r| !r.trim().is_empty()) {
            detail = detail.child(
                div()
                    .p(px(space::M))
                    .rounded(px(radius::LARGE))
                    .border_1()
                    .border_color(theme.accent.opacity(0.25))
                    .bg(theme.accent_soft)
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
                    .child(div().text_size(px(text::BODY)).line_height(px(text::BODY_LINE)).child(crate::ui::rows::markdown_view(&yh_core::markdown::parse(&report), theme))),
            );
        }
        // The detail scrolls with a scroll bar over its right edge, as the transcript does.
        column
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(detail)
                    .child(gpui_kit::base::Scrollbar::vertical(&self.sub_scroll).mode(crate::platform::scroll_bar_mode())),
            )
            .into_any_element()
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
            let root = folder.clone();
            let tree = uniform_list("file-tree", rows.len(), move |range, _, _| {
                range
                    .map(|i| {
                        let (path, name, depth, dir, ignored) = rows[i].clone();
                        let this: WeakEntity<Inspector> = this.clone();
                        let on = open_path.as_deref() == Some(path.as_str());
                        let expanded = dir && open_dirs.contains(&path);
                        // What git ignores recedes, icon and all.
                        let icon = if dir {
                            Icon::new(if expanded { IconName::FolderOpen } else { IconName::FolderClosed }).text_color(if ignored { theme2.tertiary.opacity(0.7) } else { theme2.accent.opacity(0.85) })
                        } else {
                            file_icon(&name).text_color(if ignored { theme2.tertiary.opacity(0.7) } else { theme2.tertiary })
                        };
                        let (menu_this, menu_root, menu_path) = (this.clone(), root.clone(), path.clone());
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
                            .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().when(ignored, |d| d.text_color(theme2.tertiary)).child(name))
                            .on_click(move |e, _, cx| {
                                // One click opens a preview; a double click keeps it.
                                let keep = e.click_count() >= 2;
                                let _ = this.update(cx, |this, cx| if dir { this.toggle_dir(&path, cx) } else { this.open_tab(&path, None, !keep, cx) });
                            })
                            // Open in Tab, Open in Default App, Show in Finder, Copy Path…
                            .context_menu(move |menu, _, _| {
                                let menu = if dir {
                                    menu
                                } else {
                                    let (t, p) = (menu_this.clone(), menu_path.clone());
                                    menu.item(PopupMenuItem::new("Open in Tab").on_click(move |_, _, cx| {
                                        let _ = t.update(cx, |this, cx| this.open_tab(&p, None, false, cx));
                                    }))
                                };
                                file_menu(menu, &menu_root, &menu_path)
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
                let (carries, lang) = (file.carries.clone(), file.lang.clone());
                // Find's matches, when they're for this file.
                let hits = if self.file_find_open && self.file_hits_for.as_deref() == Some(file.path.as_str()) { self.file_hits.clone() } else { Rc::new(vec![]) };
                let hit = self.file_hit;
                let widest = file.widest;
                // Each line takes part in the window's text selection, after everything else
                // in reading order, under an id that's this file's.
                let file_key = {
                    use std::hash::{Hash, Hasher};
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    file.path.hash(&mut hasher);
                    hasher.finish()
                };
                let row = move |i: usize, wrap: bool| {
                    let on = picked.is_some_and(|(a, b)| i + 1 >= a && i + 1 <= b.max(a));
                    let first = hits.partition_point(|(l, _)| *l < i);
                    let found: Vec<std::ops::Range<usize>> = hits[first..].iter().take_while(|(l, _)| *l == i).map(|(_, r)| r.clone()).collect();
                    let current = hit.and_then(|h| h.checked_sub(first)).filter(|c| *c < found.len());
                    let carry = carries.get(i).copied().unwrap_or_default();
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
                        .child(div().when(!wrap, |d| d.flex_none()).when(wrap, |d| d.flex_1().min_w_0().overflow_hidden().whitespace_normal()).child(crate::ui::selectable::SelText::new(
                            gpui_kit::ElementId::NamedInteger("file-line".into(), file_key ^ i as u64),
                            lines[i].clone(),
                            code_line(&lines[i], &lang, carry, &found, current, &theme3),
                            (1 << 62) + i as u64,
                            theme3.accent.opacity(0.28),
                        )))
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
                            .with_horizontal_sizing_behavior(gpui_kit::ListHorizontalSizingBehavior::Unconstrained)
                            .with_width_from_item(Some(widest))
                            .track_scroll(&file.scroll)
                            .size_full(),
                        )
                        .into_any_element()
                }
            }
        };
        let is_text = file.lines().is_some();
        let bar = (self.file_find_open && is_text).then(|| self.file_find_bar(theme, cx));
        // ⌘F finds in the file when it's the one you're in (its bar too); the thread's own
        // Find otherwise.
        column
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .track_focus(&self.viewer_focus)
                    .key_context("FileViewer")
                    .when(is_text, |d| {
                        d.on_action(cx.listener(|this, _: &crate::ui::root::Find, window, cx| this.show_file_find(window, cx)))
                            .on_action(cx.listener(|this, _: &crate::ui::root::FindNext, _, cx| this.step_in_file(true, cx)))
                            .on_action(cx.listener(|this, _: &crate::ui::root::FindPrevious, _, cx| this.step_in_file(false, cx)))
                    })
                    .children(bar)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .on_mouse_down(gpui_kit::MouseButton::Left, cx.listener(|this, _, window, cx| window.focus(&this.viewer_focus, cx)))
                            .child(body),
                    ),
            )
            .into_any_element()
    }

    /// Find in the open file: what's typed, how many it found and which, ↑ ↓, Done.
    fn file_find_bar(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let n = self.file_hits.len();
        let typed = !self.file_find.read(cx).value().is_empty();
        let count = match (typed, self.file_hit) {
            (false, _) => String::new(),
            (true, Some(i)) if n > 0 => format!("{} of {n}{}", i + 1, if n >= 10_000 { "+" } else { "" }),
            _ => "Not found".into(),
        };
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(space::XS))
            .px(px(space::M))
            .py(px(space::XS))
            .border_b_1()
            .border_color(theme.separator)
            .on_key_down(cx.listener(|this, e: &gpui_kit::KeyDownEvent, window, cx| {
                if e.keystroke.key == "escape" {
                    this.close_file_find(window, cx);
                }
            }))
            .child(div().flex_1().min_w_0().child(Input::new(&self.file_find).small().prefix(Icon::new(IconName::Search).size(px(13.)).text_color(theme.tertiary))))
            .child(div().flex_none().text_size(px(text::CAPTION)).text_color(if count == "Not found" { theme.error } else { theme.secondary }).child(count))
            .child(tool_button("file-find-previous", Icon::new(IconName::ChevronUp), "Previous (⇧↩)", theme).disabled(n == 0).on_click(cx.listener(|this, _, _, cx| this.step_in_file(false, cx))))
            .child(tool_button("file-find-next", Icon::new(IconName::ChevronDown), "Next (↩)", theme).disabled(n == 0).on_click(cx.listener(|this, _, _, cx| this.step_in_file(true, cx))))
            .child(Button::new("file-find-done").small().ghost().label("Done").on_click(cx.listener(|this, _, window, cx| this.close_file_find(window, cx))))
            .into_any_element()
    }
}

/// Starts watching a folder for the Files tab; what changes goes to `changed_on_disk`.
fn start_watch(folder: &str, cx: &mut Context<Inspector>) -> Option<(crate::state::file_watch::FolderWatch, gpui_kit::Task<()>)> {
    let root = std::fs::canonicalize(folder).ok()?;
    let (watch, changes) = crate::state::file_watch::FolderWatch::start(&root)?;
    let folder = folder.to_string();
    let task = cx.spawn(async move |this, cx| {
        while let Ok(batch) = changes.recv_async().await {
            let paths: Vec<String> = batch.iter().filter_map(|p| crate::state::file_watch::relative(&root, p)).collect();
            if paths.is_empty() {
                continue;
            }
            if this.update(cx, |this, cx| this.changed_on_disk(&folder, paths, cx)).is_err() {
                break;
            }
        }
    });
    Some((watch, task))
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
        use crate::state::pulls::ThreadPull;
        let Some(thread) = &self.thread else { return empty("No thread open.", theme) };
        let id = thread.read(cx).id().to_string();
        let column = div().size_full().flex().flex_col();
        let Some((found, at)) = self.pulls.get(&id) else {
            return column.child(panel_header("Pull Requests", None, theme)).child(div().flex_1().min_h_0().child(empty("Looking for this thread's pull requests…", theme))).into_any_element();
        };
        let pulls = &found.pulls;
        let refresh = tool_button("pulls-refresh", Icon::new(IconName::RefreshCw), "Check the PRs now", theme)
            .loading(self.pulls_syncing.contains(&id))
            .on_click(cx.listener(|this, _, _, cx| this.refresh_pulls(cx)));
        if pulls.is_empty() && found.local.is_empty() {
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
                            "PRs show up here once this thread works on a branch that has one, or a PR link comes up in it, such as Claude running gh pr create.",
                            theme,
                        ))
                        .when_some(self.split_button(false, cx), |d, b| d.child(b)),
                )
                .into_any_element();
        }
        let worked: Vec<&ThreadPull> = pulls.iter().filter(|p| p.worked).collect();
        let known: Vec<&yh_core::forge::PullRequest> = worked.iter().filter_map(|p| p.pull.as_ref().ok()).collect();
        // A GitHub stack still going keeps its merged layers, to show how far along it is.
        let live: HashSet<String> = known.iter().filter(|p| p.is_open()).filter_map(|p| p.stack.map(|s| format!("{}#{}", p.link.path.to_lowercase(), s.number))).collect();
        let in_live = |p: &yh_core::forge::PullRequest| p.stack.is_some_and(|s| live.contains(&format!("{}#{}", p.link.path.to_lowercase(), s.number)));
        let open: Vec<yh_core::forge::PullRequest> = known.iter().filter(|p| p.is_open() || in_live(p)).map(|p| (*p).clone()).collect();
        let mut done: Vec<&ThreadPull> = worked.iter().copied().filter(|t| t.pull.as_ref().is_ok_and(|p| !p.is_open() && !in_live(p))).collect();
        let when = |p: &ThreadPull| p.pull.as_ref().ok().and_then(|p| p.merged_at.clone().or(p.updated_at.clone())).unwrap_or_default();
        done.sort_by_key(|p| std::cmp::Reverse(when(p)));
        let unknown: Vec<&ThreadPull> = worked.iter().copied().filter(|t| t.pull.is_err()).collect();
        let mentioned: Vec<&ThreadPull> = pulls.iter().filter(|p| !p.worked).collect();
        let open_count = open.iter().filter(|p| p.is_open()).count();
        let ago = at.elapsed().as_secs();
        let subtitle = format!(
            "{} · updated {}",
            match open_count {
                0 => "None open".to_string(),
                1 => "1 open".to_string(),
                n => format!("{n} open"),
            },
            if ago < 5 { "just now".to_string() } else if ago < 60 { format!("{ago} s ago") } else { format!("{} min ago", ago / 60) }
        );
        let header = panel_header("Pull Requests", Some(subtitle), theme).when_some(self.split_button(true, cx), |d, b| d.child(b)).child(refresh);
        let mut list = div().id("pulls").flex_1().min_h_0().overflow_y_scroll().px(px(space::M)).pb(px(space::M)).flex().flex_col().gap(px(space::S + 2.));
        // Open ones as stacks (a single PR is a stack of one), bottom of each stack first.
        for stack in yh_core::stacks::PullRequestStack::build(&open) {
            list = list.child(stack_card(&stack, theme));
        }
        for (repo, local) in &found.local {
            list = list.child(local_stack_card(repo, local, theme));
        }
        let section = |t: &str| div().pt(px(space::XS)).px(px(space::XS)).text_size(px(text::CAPTION)).font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).child(t.to_string());
        let rows_card = |items: &[&ThreadPull]| {
            let mut card = div().flex().flex_col().rounded(px(radius::LARGE)).overflow_hidden().border_1().border_color(theme.separator).bg(theme.card);
            for (i, p) in items.iter().enumerate() {
                card = card.child(pull_row(p, None, i > 0, theme));
            }
            card
        };
        if !done.is_empty() {
            list = list.child(section("Merged and Closed")).child(rows_card(&done));
        }
        for p in &unknown {
            list = list.child(rows_card(&[*p]));
        }
        if !mentioned.is_empty() {
            list = list.child(section("Mentioned")).child(rows_card(&mentioned));
        }
        if std::env::var("YH_PR_PANEL_LOG").is_ok() {
            let names = |v: Vec<String>| v.join(", ");
            let shown = format!(
                "open [{}] local [{}] done [{}] mentioned [{}]",
                names(open.iter().map(|p| format!("#{}{}", p.number(), p.stack.map(|s| format!(" (stack {} {}/{})", s.number, s.position, s.size)).unwrap_or_default())).collect()),
                names(found.local.iter().map(|(r, s)| format!("{r}: {}", s.layers.iter().map(|l| l.branch.clone()).collect::<Vec<_>>().join(" ← "))).collect()),
                names(done.iter().map(|p| format!("#{}", p.link.number)).collect()),
                names(mentioned.iter().map(|p| format!("#{}", p.link.number)).collect()),
            );
            static LAST: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
            let mut last = LAST.lock().unwrap();
            if *last != shown {
                eprintln!("PRS panel {shown}");
                *last = shown;
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
                                tool_button("side-stop", icons::circle_stop(), "Stop", theme).on_click(move |_, _, cx| stop_ws.update(cx, |ws, cx| ws.stop(&stop_chat, &Default::default(), cx))),
                            )
                        }),
                ),
            )
            .into_any_element()
    }
}

fn purple() -> gpui_kit::Hsla {
    gpui_kit::rgb(0xaf52de).into()
}

/// A PR's state as a colour: open green, draft grey, merged or merging purple, closed red.
fn pull_tint(pr: &yh_core::forge::PullRequest, theme: &Theme) -> gpui_kit::Hsla {
    match pr.state.as_str() {
        "merged" => purple(),
        "closed" => theme.error,
        _ if pr.draft => theme.secondary,
        _ if pr.auto_merge || pr.in_merge_queue => purple(),
        _ => theme.success,
    }
}

/// Lane spacing and the dot's size and place in a stack's tree.
const LANE: f32 = 18.;
const DOT: f32 = 16.;
const DOT_Y: f32 = 17.;

/// A row's slice of a stack's tree: the lines through it, and its PR's dot (with its layer)
/// in its lane, level with the title. Rows stack up into one unbroken drawing.
fn graph_cell(row: &yh_core::stacks::GraphRow, lanes: usize, theme: &Theme) -> gpui_kit::AnyElement {
    use yh_core::stacks::Segment;
    let segments: Vec<Segment> = row.segments.iter().copied().collect();
    let line = theme.secondary.opacity(0.35);
    let tint = pull_tint(&row.pull_request, theme);
    let layer = row.pull_request.stack.map(|s| s.position as usize).unwrap_or(row.layer);
    let lane = row.lane;
    let width = (lanes.max(1) - 1) as f32 * LANE + DOT;
    div()
        .relative()
        .flex_none()
        .w(px(width))
        .child(
            gpui_kit::canvas(
                |_, _, _| {},
                move |b, _, window, _| {
                    let x = |l: usize| f32::from(b.origin.x) + l as f32 * LANE + DOT / 2.;
                    let top = f32::from(b.origin.y);
                    let (dot, bottom) = (top + DOT_Y, top + f32::from(b.size.height));
                    let mut seg = |points: &[(f32, f32)]| {
                        let mut p = gpui_kit::PathBuilder::stroke(px(2.));
                        p.move_to(gpui_kit::point(px(points[0].0), px(points[0].1)));
                        for (x, y) in &points[1..] {
                            p.line_to(gpui_kit::point(px(*x), px(*y)));
                        }
                        if let Ok(p) = p.build() {
                            window.paint_path(p, line);
                        }
                    };
                    for s in &segments {
                        match *s {
                            Segment::Through(l) => seg(&[(x(l), top), (x(l), bottom)]),
                            Segment::FromAbove(l) => seg(&[(x(l), top), (x(l), dot)]),
                            Segment::ToBelow(l) => seg(&[(x(l), dot), (x(l), bottom)]),
                            // Down the parent's lane, round the corner, across into the dot.
                            Segment::Branch { from, to } => {
                                let r = 8f32.min((x(to) - x(from)) / 2.);
                                seg(&[(x(from), top), (x(from), dot - r), (x(from) + r * 0.3, dot - r * 0.3), (x(from) + r, dot), (x(to), dot)]);
                            }
                        }
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
        .child(
            div()
                .absolute()
                .top(px(DOT_Y - DOT / 2.))
                .left(px(lane as f32 * LANE))
                .size(px(DOT))
                .rounded_full()
                .border(px(1.5))
                .border_color(tint.opacity(0.8))
                .bg(theme.card)
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(9.))
                .font_weight(FontWeight::BOLD)
                .text_color(tint)
                .child(layer.to_string()),
        )
        .into_any_element()
}

/// A PR in the narrow sidebar, as a row of a card: what it is, where it's going, what stands
/// in its way. In a stack the tree runs down its left and the dot carries the state; on its
/// own, a small icon does.
fn pull_row(p: &crate::state::pulls::ThreadPull, graph: Option<(&yh_core::stacks::GraphRow, usize)>, divider: bool, theme: &Theme) -> gpui_kit::AnyElement {
    let url = p.link.url.clone();
    let mut row = div()
        .id(SharedString::from(format!("pull-{}", p.link.id())))
        .flex()
        .gap(px(space::S))
        .px(px(space::M))
        .when(graph.is_none(), |d| d.py(px(space::S)))
        .when(divider, |d| d.border_t_1().border_color(theme.separator))
        .cursor_pointer()
        .hover(|s| s.bg(theme.hover))
        .on_click(move |_, _, cx| cx.open_url(&url));
    // Open in Browser, Copy Link, Copy Branch Name.
    let (u1, u2) = (p.link.url.clone(), p.link.url.clone());
    let head = p.pull.as_ref().ok().map(|pr| pr.head.clone());
    let menu = move |menu: gpui_kit::component::menu::PopupMenu, _: &mut Window, _: &mut gpui_kit::Context<gpui_kit::component::menu::PopupMenu>| {
        let (u1, u2) = (u1.clone(), u2.clone());
        let menu = menu
            .item(PopupMenuItem::new("Open in Browser").icon(IconName::ExternalLink).on_click(move |_, _, cx| cx.open_url(&u1)))
            .item(PopupMenuItem::new("Copy Link").icon(IconName::Copy).on_click(move |_, _, cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(u2.clone()))));
        match head.clone() {
            Some(h) => menu.item(PopupMenuItem::new("Copy Branch Name").on_click(move |_, _, cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(h.clone())))),
            None => menu,
        }
    };
    let fact = |icon: Icon, label: &str, color: gpui_kit::Hsla| div().flex().items_center().gap(px(3.)).text_color(color).child(icon.size(px(11.)).text_color(color)).child(label.to_string());
    let Ok(pr) = &p.pull else {
        let why = p.pull.as_ref().err().map(|e| e.lines().next().unwrap_or("").to_string()).unwrap_or_default();
        return row
            .child(div().flex_none().pt(px(2.)).child(icons::pull_request().size(px(14.)).text_color(theme.tertiary)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().font_weight(FontWeight::MEDIUM).child(format!("{} #{}", p.link.path, p.link.number)))
                    .child(div().text_size(px(text::CAPTION)).text_color(theme.secondary).line_clamp(2).text_ellipsis().child(format!("Couldn't look it up: {why}"))),
            )
            .context_menu(menu)
            .into_any_element();
    };
    let tint = pull_tint(pr, theme);
    let (icon, chip): (Icon, Option<&str>) = match pr.state.as_str() {
        "merged" => (icons::merged(), Some("Merged")),
        "closed" => (icons::pull_request_closed(), Some("Closed")),
        _ if pr.draft => (icons::pull_request_draft(), Some("Draft")),
        _ if pr.in_merge_queue => (icons::pull_request(), Some("In merge queue")),
        _ if pr.auto_merge => (icons::pull_request(), Some("Auto-merge")),
        _ => (icons::pull_request(), None),
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
    let finished = pr.state != "open";
    let details = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        // In a stack the tree runs the row's whole height, so the text keeps the space.
        .when(graph.is_some(), |d| d.py(px(space::S)))
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(space::S))
                .child(div().flex_1().min_w_0().font_weight(FontWeight::MEDIUM).when(finished, |d| d.text_color(theme.secondary)).line_clamp(2).text_ellipsis().child(pr.title.clone()))
                .when_some(chip, |d, c| {
                    d.child(div().flex_none().mt(px(1.)).px(px(space::XS + 2.)).rounded(px(radius::SMALL)).bg(tint.opacity(0.14)).text_size(px(text::CAPTION)).font_weight(FontWeight::MEDIUM).text_color(tint).child(c))
                }),
        )
        .child(
            div()
                .flex()
                .gap(px(space::XS + 2.))
                .text_size(px(text::CAPTION))
                .text_color(theme.secondary)
                .child(div().flex_none().child(format!("{} #{}", p.link.path.rsplit('/').next().unwrap_or(&p.link.path), p.link.number)))
                .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_family(MONO).text_color(theme.tertiary).child(format!("{} → {}", pr.head, pr.base))),
        )
        .when(!finished && (blocker.is_some() || review.is_some() || checks.is_some()), |d| {
            d.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(space::S + 2.))
                    .text_size(px(text::CAPTION))
                    .when_some(blocker, |d, b| d.child(fact(Icon::new(IconName::TriangleAlert), b, theme.warning)))
                    .when_some(review, |d, (i, t, c)| d.child(fact(i, t, c)))
                    .when_some(checks, |d, (i, t, c)| d.child(fact(i, t, c))),
            )
        });
    row = match graph {
        Some((g, lanes)) => row.child(graph_cell(g, lanes, theme)),
        None => row.child(div().flex_none().pt(px(2.)).child(icon.size(px(14.)).text_color(tint))),
    };
    row.child(details).context_menu(menu).into_any_element()
}

/// One stack (or a single PR) in a card, drawn as a tree: bottom of the stack first, a line
/// of PRs in one lane, each fork opening a lane to its right.
fn stack_card(stack: &yh_core::stacks::PullRequestStack, theme: &Theme) -> gpui_kit::AnyElement {
    use yh_core::stacks::{Lanes, StackGraph};
    let graph = StackGraph::layout(stack, Lanes::Compact);
    let is_stack = stack.is_stacked() || stack.number().is_some();
    let mut card = div().flex().flex_col().rounded(px(radius::LARGE)).overflow_hidden().border_1().border_color(theme.separator).bg(theme.card).when(is_stack, |d| d.pb(px(3.)));
    if is_stack {
        let all = stack.all();
        let open = all.iter().filter(|p| p.is_open()).count();
        let title = match stack.number() {
            Some(n) => format!("Stack #{n}"),
            None => "Stack".to_string(),
        };
        let size = stack.pull_request.stack.map(|s| s.size as usize).unwrap_or(all.len());
        let detail = if open == all.len() { format!("{size} PRs") } else { format!("{open} of {size} open") };
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap(px(space::XS + 2.))
                .px(px(space::M))
                .pt(px(space::S + 1.))
                .pb(px(2.))
                .text_size(px(text::CAPTION))
                .child(icons::layers().size(px(12.)).text_color(theme.secondary))
                .child(div().font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).child(title))
                .child(div().text_color(theme.tertiary).child(format!("· {detail} · into {}", stack.pull_request.base))),
        );
    }
    let lanes = graph.lane_count;
    for row in &graph.rows {
        let p = crate::state::pulls::ThreadPull { link: row.pull_request.link.clone(), worked: true, pull: Ok(row.pull_request.clone()) };
        card = card.child(pull_row(&p, is_stack.then_some((row, lanes)), false, theme));
    }
    card.into_any_element()
}

/// A stack made with `gh stack` that isn't on GitHub yet: its branches, bottom first, each
/// waiting for `gh stack submit`.
fn local_stack_card(repo: &str, stack: &yh_core::github::LocalStack, theme: &Theme) -> gpui_kit::AnyElement {
    let mut card = div().flex().flex_col().pb(px(3.)).rounded(px(radius::LARGE)).overflow_hidden().border_1().border_dashed().border_color(theme.separator).bg(theme.card);
    card = card.child(
        div()
            .flex()
            .items_center()
            .gap(px(space::XS + 2.))
            .px(px(space::M))
            .pt(px(space::S + 1.))
            .pb(px(2.))
            .text_size(px(text::CAPTION))
            .child(icons::layers().size(px(12.)).text_color(theme.secondary))
            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).child("Stack, not submitted"))
            .child(div().min_w_0().truncate().text_color(theme.tertiary).child(format!("· {repo} · into {}", stack.trunk))),
    );
    let n = stack.layers.len();
    for (i, layer) in stack.layers.iter().enumerate() {
        let submitted = layer.pull_request_number;
        card = card.child(
            div()
                .relative()
                .flex()
                .items_center()
                .gap(px(space::S))
                .px(px(space::M))
                .h(px(30.))
                // The line up the stack, through every dot.
                .child(
                    div()
                        .relative()
                        .flex_none()
                        .w(px(DOT))
                        .h_full()
                        .when(n > 1, |d| {
                            d.child(div().absolute().left(px(DOT / 2. - 1.)).w(px(2.)).top(px(if i == 0 { 15. } else { 0. })).bottom(px(if i + 1 == n { 15. } else { 0. })).bg(theme.secondary.opacity(0.35)))
                        })
                        .child(
                            div()
                                .absolute()
                                .top(px(7.))
                                .left_0()
                                .size(px(DOT))
                                .rounded_full()
                                .border(px(1.5))
                                .border_dashed()
                                .border_color(theme.secondary)
                                .bg(theme.card)
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(text::CAPTION))
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme.secondary)
                                .child((i + 1).to_string()),
                        ),
                )
                .child(div().flex_1().min_w_0().truncate().font_family(MONO).text_size(px(text::CAPTION)).child(layer.branch.clone()))
                .child(div().flex_none().px(px(space::XS + 2.)).rounded(px(radius::SMALL)).bg(theme.selected).text_size(px(text::CAPTION)).text_color(theme.secondary).child(match submitted {
                    Some(n) => format!("#{n}"),
                    None => "Not submitted".to_string(),
                })),
        );
    }
    card.into_any_element()
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let panel = self.workspace.read(cx).panel;
        if panel == Panel::SideChat {
            self.ensure_side(cx);
        }
        if let Some(words) = self.file_find_pending.take() {
            self.file_find.update(cx, |i, cx| i.set_value(words, window, cx));
            self.show_file_find(window, cx);
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
