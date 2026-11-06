//! The right sidebar (⌥⌘0), for the open thread: its sub-agents, followed step by step, and
//! its folder's files (a tree, Go to File, files opened in tabs, read-only). A cached view of
//! its own: it redraws when its tab's data changes, not with every streamed delta.

use crate::state::thread::{SubStep, ThreadState};
use crate::state::workspace::{Panel, Workspace};
use crate::ui::transcript::TranscriptView;
use yh_core::claude::Decision;
use crate::theme::{Theme, MONO};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::Sizable;
use gpui_kit::{
    div, prelude::*, px, uniform_list, App, Context, Entity, FontWeight, ScrollHandle, ScrollStrategy, SharedString, Subscription,
    UniformListScrollHandle, WeakEntity, Window,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

pub const WIDTH: f32 = 380.;

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

struct OpenFile {
    path: String,
    lines: Rc<Vec<SharedString>>,
    note: Option<String>,
    scroll: UniformListScrollHandle,
    /// Lines to show picked out, from a link (1-based).
    lines_picked: Option<(usize, usize)>,
}

/// One folder's Files tab: what's open in the tree, the files in tabs.
#[derive(Default)]
struct Folder {
    tree: Option<Rc<Tree>>,
    rows: Rc<Vec<(String, String, usize, bool)>>,
    open_dirs: HashSet<String>,
    files: Vec<OpenFile>,
    current: usize,
    loading: bool,
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
    side_state: (bool, bool),
    _subscriptions: Vec<Subscription>,
}

impl Inspector {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let goto = cx.new(|cx| InputState::new(window, cx).placeholder("Go to File"));
        let question = cx.new(|cx| InputState::new(window, cx).placeholder("Ask on the side…"));
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
            side_state: (false, false),
            pulls: HashMap::new(),
            pulls_loading: false,
            _pulls_timer: None,
            was_running: false,
            _subscriptions: subscriptions,
        };
        inspector.follow_selection(cx);
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
        let view = cx.new(|cx| TranscriptView::new(chat.clone(), cx));
        // Its running state and permission requests change the controls under it.
        let watch = cx.observe(&chat, |this, chat, cx| {
            let state = (chat.read(cx).running, chat.read(cx).permission.is_some());
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
        let Some(folder) = self.folder(cx) else { return };
        let relative = path.strip_prefix(&format!("{}/", folder.trim_end_matches('/'))).unwrap_or(path).to_string();
        self.workspace.update(cx, |ws, cx| ws.show_panel(Panel::Files, cx));
        let state = self.folders.entry(folder.clone()).or_default();
        if let Some(i) = state.files.iter().position(|f| f.path == relative) {
            state.current = i;
            if let Some((start, _)) = lines {
                state.files[i].lines_picked = lines;
                state.files[i].scroll.scroll_to_item(start.saturating_sub(4), ScrollStrategy::Top);
            }
            cx.notify();
            return;
        }
        let full = if relative.starts_with('/') { relative.clone() } else { format!("{}/{relative}", folder.trim_end_matches('/')) };
        cx.spawn(async move |this, cx| {
            let (text, note) = cx
                .background_executor()
                .spawn(async move {
                    use std::io::Read;
                    let Ok(file) = std::fs::File::open(&full) else { return (vec![], Some("Can't open this file.".to_string())) };
                    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
                    let mut bytes = vec![];
                    let _ = file.take(MAX_FILE).read_to_end(&mut bytes);
                    if bytes.iter().take(8000).any(|b| *b == 0) {
                        return (vec![], Some("A binary file: nothing to show.".to_string()));
                    }
                    let note = (size > MAX_FILE).then(|| format!("Showing the first {} MB of {} MB.", MAX_FILE >> 20, size >> 20));
                    let text = String::from_utf8_lossy(&bytes);
                    // A last newline ends the last line; it doesn't start another.
                    let text = text.strip_suffix('\n').unwrap_or(&text);
                    let lines: Vec<SharedString> = text.split('\n').map(|l| SharedString::from(l.replace('\t', "    "))).collect();
                    (lines, note)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let state = this.folders.entry(folder).or_default();
                let scroll = UniformListScrollHandle::new();
                if let Some((start, _)) = lines {
                    scroll.scroll_to_item(start.saturating_sub(4), ScrollStrategy::Top);
                }
                state.files.push(OpenFile { path: relative, lines: Rc::new(text), note, scroll, lines_picked: lines });
                state.current = state.files.len() - 1;
                cx.notify();
            });
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
            .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(description))
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

    fn files(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(folder) = self.folder(cx) else { return empty("No thread open.", theme) };
        let state = self.folders.get(&folder);
        let query = !self.goto.read(cx).value().trim().is_empty();
        let mut column = div().size_full().flex().flex_col().child(div().p(px(10.)).child(Input::new(&self.goto).small().cleanable(true)));
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
        let Some(state) = state.filter(|s| s.tree.is_some()) else { return column.child(empty("Reading the folder…", theme)).into_any_element() };
        let rows = state.rows.clone();
        let this = cx.entity().downgrade();
        let theme2 = *theme;
        let has_file = !state.files.is_empty();
        let tree = uniform_list("file-tree", rows.len(), move |range, _, _| {
            range
                .map(|i| {
                    let (path, name, depth, dir) = rows[i].clone();
                    let this: WeakEntity<Inspector> = this.clone();
                    div()
                        .id(i)
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .pl(px(10. + depth as f32 * 14.))
                        .pr(px(8.))
                        .text_size(px(12.))
                        .hover(|s| s.bg(theme2.selected))
                        .child(div().w(px(10.)).text_color(theme2.secondary).child(if dir { "›" } else { "" }))
                        .child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().when(dir, |d| d.font_weight(FontWeight::MEDIUM)).child(name))
                        .on_click(move |_, _, cx| {
                            let _ = this.update(cx, |this, cx| if dir { this.toggle_dir(&path, cx) } else { this.open_file(&path, None, cx) });
                        })
                })
                .collect()
        })
        .track_scroll(&self.tree_scroll);
        column = column.child(div().when(has_file, |d| d.h(px(220.)).flex_none()).when(!has_file, |d| d.flex_1()).min_h_0().child(tree.size_full()));
        if has_file {
            let mut tabs = div().id("file-tabs").flex().gap(px(4.)).px(px(8.)).py(px(6.)).border_t_1().border_b_1().border_color(theme.separator).overflow_x_scroll();
            for (i, f) in state.files.iter().enumerate() {
                let on = i == state.current;
                let name = f.path.rsplit('/').next().unwrap_or(&f.path).to_string();
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
                        .child(name)
                        .child(div().id(SharedString::from(format!("close-{i}"))).px(px(2.)).text_color(theme.secondary).child("×").on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.close_file(i, cx)
                        })))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(folder) = this.folder(cx) {
                                if let Some(s) = this.folders.get_mut(&folder) {
                                    s.current = i;
                                }
                            }
                            cx.notify();
                        })),
                );
            }
            column = column.child(tabs);
            let file = &state.files[state.current];
            column = column.child(
                div()
                    .px(px(10.))
                    .py(px(4.))
                    .text_size(px(11.))
                    .font_family(MONO)
                    .text_color(theme.secondary)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(format!("{} · {} lines", file.path, file.lines.len()))
                    .when_some(file.note.clone(), |d, n| d.child(div().text_color(theme.warning).child(n))),
            );
            let lines = file.lines.clone();
            let picked = file.lines_picked;
            let digits = lines.len().to_string().len();
            let theme3 = *theme;
            column = column.child(
                div().flex_1().min_h_0().child(
                    uniform_list("file-lines", lines.len(), move |range, _, _| {
                        range
                            .map(|i| {
                                let on = picked.is_some_and(|(a, b)| i + 1 >= a && i + 1 <= b.max(a));
                                div()
                                    .h(px(17.))
                                    .flex()
                                    .font_family(MONO)
                                    .text_size(px(11.5))
                                    .whitespace_nowrap()
                                    .when(on, |d| d.bg(theme3.warning.opacity(0.18)))
                                    .child(div().flex_none().w(px(digits as f32 * 7.5 + 14.)).pr(px(8.)).text_right().text_color(theme3.secondary.opacity(0.7)).child(format!("{}", i + 1)))
                                    .child(div().flex_none().child(lines[i].clone()))
                            })
                            .collect()
                    })
                    .track_scroll(&file.scroll)
                    .size_full(),
                ),
            );
        }
        column.into_any_element()
    }
}

impl Inspector {
    /// The thread's browser: its tabs, the page's address, and the place the page is laid
    /// over (GPUI keeps it empty; the web view follows its bounds every frame).
    fn browser(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(thread) = self.thread.as_ref().map(|t| t.read(cx).id().to_string()) else { return empty("No thread open.", theme) };
        let tabs: Vec<(u32, String, bool, bool)> = crate::browser::Browsers::with(|b| {
            b.sessions
                .get(&thread)
                .map(|s| {
                    let current = s.selected_tab().map(|t| t.id);
                    s.tabs.iter().map(|t| (t.id, if t.title().is_empty() { t.short_url() } else { t.title() }, Some(t.id) == current, t.web.is_loading())).collect()
                })
                .unwrap_or_default()
        });
        if tabs.is_empty() {
            crate::browser::Browsers::show(None);
            return empty("No pages open. When Claude opens one (mcp__browser__open), it shows here, and you can watch what it does.", theme);
        }
        let address = crate::browser::Browsers::with(|b| b.sessions.get(&thread).and_then(|s| s.selected_tab().map(|t| t.short_url()))).unwrap_or_default();
        let mut strip = div().id("browser-tabs").flex().gap(px(4.)).px(px(8.)).py(px(6.)).overflow_x_scroll();
        for (id, title, on, loading) in tabs {
            let t = thread.clone();
            strip = strip.child(
                div()
                    .id(SharedString::from(format!("browser-tab-{id}")))
                    .flex_none()
                    .max_w(px(160.))
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(px(10.))
                    .text_size(px(11.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .bg(if on { theme.accent.opacity(0.16) } else { theme.selected })
                    .text_color(if on { theme.text } else { theme.secondary })
                    .child(if loading { format!("◌ {title}") } else { title })
                    .on_click(cx.listener(move |_, _, _, cx| {
                        crate::browser::Browsers::select_tab(&t, id);
                        cx.notify();
                    })),
            );
        }
        let place = thread.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(strip)
            .child(
                div()
                    .mx(px(8.))
                    .mb(px(6.))
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(6.))
                    .bg(theme.card)
                    .text_size(px(11.))
                    .font_family(MONO)
                    .text_color(theme.secondary)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(address),
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
            .into_any_element()
    }

    fn pull_requests(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(thread) = &self.thread else { return empty("No thread open.", theme) };
        let id = thread.read(cx).id().to_string();
        let Some((pulls, at)) = self.pulls.get(&id) else { return empty("Looking for this thread's pull requests…", theme) };
        if pulls.is_empty() {
            return empty("No pull requests. When Claude opens, merges or comments on one in this thread, or one's linked here, it shows here and stays current.", theme);
        }
        let rank = |p: &crate::state::pulls::ThreadPull| match p.pull.as_ref().map(|p| p.state.as_str()) {
            Ok("open") => 0,
            Ok("merged") => 1,
            Ok(_) => 2,
            Err(_) => 3,
        };
        let mut worked: Vec<&crate::state::pulls::ThreadPull> = pulls.iter().filter(|p| p.worked).collect();
        worked.sort_by_key(|p| rank(p));
        let mentioned: Vec<&crate::state::pulls::ThreadPull> = pulls.iter().filter(|p| !p.worked).collect();
        let open = pulls.iter().filter(|p| p.worked && p.pull.as_ref().is_ok_and(|p| p.state == "open")).count();
        let mut column = div().id("pulls").size_full().overflow_y_scroll().p(px(12.)).flex().flex_col().gap(px(8.)).child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(11.))
                .text_color(theme.secondary)
                .child(div().flex_1().child(format!("{open} open · updated {}", if at.elapsed().as_secs() < 5 { "just now".to_string() } else { format!("{} s ago", at.elapsed().as_secs()) })))
                .child(Button::new("pulls-refresh").xsmall().ghost().label(if self.pulls_loading { "Looking…" } else { "Refresh" }).on_click(cx.listener(|this, _, _, cx| this.refresh_pulls(cx)))),
        );
        for p in worked {
            column = column.child(pull_card(p, theme));
        }
        if !mentioned.is_empty() {
            column = column.child(div().pt(px(8.)).text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).child("MENTIONED"));
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
            (c.running, c.permission.clone(), c.items.is_empty())
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
            let (allow, deny, c1, c2) = (ws.clone(), ws.clone(), chat.clone(), chat.clone());
            column = column.child(
                div()
                    .m(px(10.))
                    .p(px(10.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme.warning.opacity(0.5))
                    .bg(theme.warning.opacity(0.08))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("Allow {}?", p.tool)))
                    .child(div().font_family(MONO).text_size(px(11.)).text_color(theme.secondary).line_clamp(4).child(gist(&p.tool, &p.input)))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(6.))
                            .child(Button::new("side-deny").small().label("Deny").on_click(move |_, _, cx| deny.update(cx, |ws, cx| ws.respond(&c2, Decision::deny(), cx))))
                            .child(Button::new("side-allow").small().primary().label("Allow").on_click(move |_, _, cx| allow.update(cx, |ws, cx| ws.respond(&c1, Decision::allow(), cx)))),
                    ),
            );
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
    let mut card = div()
        .id(SharedString::from(format!("pull-{}", p.link.id())))
        .p(px(10.))
        .rounded(px(8.))
        .bg(theme.card)
        .hover(|s| s.bg(theme.selected))
        .flex()
        .flex_col()
        .gap(px(4.))
        .on_click(move |_, _, cx| cx.open_url(&url));
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
