//! Projects with their threads: Home first, then as saved; in each, pinned threads, then the
//! newest; settled ones on a shelf of their own. A project folds away with its chevron
//! (`collapsedProjectIDs`, as 0.0.x saved it). Hovering a row shows Pin, Settle and a
//! "…" menu; right-click for the same; double-click renames (Return saves, Escape cancels).
//! ↑/↓ move between threads while the sidebar has focus. One flat column of rows, so a row
//! can be scrolled to by index. A cached view: it redraws when the workspace says something
//! changed, on hover, and once a minute for the ages and the limits' countdown, never per delta.

use crate::probe;
use crate::state::thread::ThreadState;
use crate::state::workspace::{Page, Workspace};
use crate::theme::{radius, space, text, Theme};
use crate::ui::icons;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::component::{Disableable, Icon, IconName, Sizable, WindowExt};
use gpui_kit::{
    div, prelude::*, px, App, Context, Entity, FocusHandle, FontWeight, Hsla, ScrollHandle, SharedString, Subscription, Task, Visibility,
    WeakEntity, Window,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use yh_core::library::ProjectMeta;

const SHOWN_PER_PROJECT: usize = 12;

/// The rows' grid: compact 24 pt rows for high concurrent-agent density without wasting an
/// empty left chevron gutter on every thread.
const ROW_HEIGHT: f32 = 24.;
const ROW_INSET: f32 = 6.;
const CHEVRON: f32 = 12.;
const ICON: f32 = 13.;
const GAP: f32 = 5.;
/// A limit meter's bar: slim and crisp at a glance.
const BAR: f32 = 4.5;
/// Where thread titles start, from the row's edge.
const TITLE_X: f32 = ROW_INSET + 2. + ICON + GAP;

/// Settings: projects folded away (0.0.x's key), shelves open, projects showing all.
const COLLAPSED: &str = "collapsedProjectIDs";
const SHELVES: &str = "expandedShelves";
const SHOW_ALL: &str = "sidebarShowAll";

/// An idle thread with sub-agents in the background still working: it isn't done, so it
/// shows as working, and its tooltip says why ("2 sub-agents running").
fn agents_marker(t: &ThreadState) -> Option<SharedString> {
    let agents = t.run.jobs.agents_running();
    if agents == 0 {
        return None;
    }
    // "1 command and 2 sub-agents running (1 command of theirs)": worded by who started what.
    t.run.jobs.summary().map(Into::into)
}

/// An idle thread with background jobs running: what its hourglass says.
fn jobs_marker(t: &ThreadState, busy: bool) -> Option<SharedString> {
    let count = t.run.jobs.background().count();
    if busy || count == 0 {
        return None;
    }
    let waiting = t.run.jobs.waiting();
    Some(match waiting {
        0 if count == 1 => "1 command running".into(),
        0 => format!("{count} commands running").into(),
        1 => format!("{} is waiting on a background command", yh_core::agents::Agent::from_id(t.meta.agent.as_deref()).name()).into(),
        n => format!("{} is waiting on {n} background commands", yh_core::agents::Agent::from_id(t.meta.agent.as_deref()).name()).into(),
    })
}

fn now() -> f64 {
    yh_core::library::now_ms() / 1000.0
}

pub(crate) fn tooltip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> gpui_kit::AnyView + 'static {
    let text: SharedString = text.into();
    move |window, cx| gpui_kit::component::tooltip::Tooltip::new(text.clone()).build(window, cx)
}

fn set_of(value: Option<Value>) -> HashSet<String> {
    value.and_then(|v| v.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())).unwrap_or_default()
}

/// Not drawn (and not clickable) until a `group_hover` shows it.
fn hidden<E: Styled>(mut e: E) -> E {
    e.style().visibility = Some(Visibility::Hidden);
    e
}

pub struct Sidebar {
    workspace: Entity<Workspace>,
    /// A new version, when there's one: said at the bottom.
    updater: Entity<crate::state::updater::Updater>,
    /// The release notes the banner and the version popover show.
    release_notes: Entity<crate::ui::updates::ReleaseNotesView>,
    /// Projects showing all their threads, with their Settled shelf open, and folded away.
    expanded: HashSet<String>,
    shelves: HashSet<String>,
    collapsed: HashSet<String>,
    /// Threads whose agent-started threads are folded away under them.
    folded_workers: HashSet<String>,
    /// The one thread row whose hover action buttons (and "…" dropdown menu) are mounted:
    /// keeps 40–120 unhovered rows from building hidden Popover/DropdownMenu trees on every render.
    hot_row: Option<String>,
    /// Show only working, waiting, unread, or currently selected threads across projects.
    filter_active: bool,
    pub scroll: ScrollHandle,
    renaming: Option<(String, Entity<InputState>, Subscription)>,
    pub focus: FocusHandle,
    /// Once a minute: the rows' ages and the limits' countdown.
    _tick: Task<()>,
    _subscriptions: Vec<Subscription>,
}

enum SidebarRow {
    Project(ProjectMeta, bool),
    Thread(Entity<ThreadState>, Nest),
    More(String, usize, bool),
    Shelf(String, usize, bool),
    AddProject,
}

/// Where a thread sits among the threads agents started (`start_thread`): how deep, and for
/// one that started others, how many sit under it and what they're doing.
#[derive(Clone, Copy, Default)]
struct Nest {
    depth: usize,
    workers: usize,
    working: usize,
    waiting: usize,
    folded: bool,
}

/// How far a thread an agent started sits in from the one that started it.
const INDENT: f32 = 14.;

/// What a thread's menu needs, for its context menu and its "…" button alike.
#[derive(Clone)]
struct ThreadMenu {
    this: WeakEntity<Sidebar>,
    ws: WeakEntity<Workspace>,
    id: String,
    session: Option<String>,
    pinned: bool,
    settled: bool,
    can_regenerate: bool,
}

impl ThreadMenu {
    fn build(&self, menu: PopupMenu, cx: &App) -> PopupMenu {
        let m = self.clone();
        let auto = m.ws.upgrade().is_some_and(|ws| ws.read(cx).auto_settles(&m.id));
        let generating = m.ws.upgrade().is_some_and(|ws| ws.read(cx).is_generating_title(&m.id));
        let mut menu = menu;
        if !m.settled {
            let (ws, id) = (m.ws.clone(), m.id.clone());
            menu = menu.item(PopupMenuItem::new(if m.pinned { "Unpin" } else { "Pin" }).on_click(move |_, _, cx| {
                let _ = ws.update(cx, |ws, cx| ws.toggle_pin(&id, cx));
            }));
        }
        let (this, id) = (m.this.clone(), m.id.clone());
        menu = menu.item(PopupMenuItem::new("Rename…").on_click(move |_, window, cx| {
            let _ = this.update(cx, |s, cx| s.start_rename(&id, window, cx));
        }));
        let (ws, id) = (m.ws.clone(), m.id.clone());
        menu = menu
            .item(
                PopupMenuItem::new(if generating { "Regenerating Title…" } else { "Regenerate Title" })
                    .disabled(generating || !m.can_regenerate)
                    .on_click(move |_, _, cx| {
                        let _ = ws.update(cx, |ws, cx| ws.regenerate_title(&id, cx));
                    }),
            )
            .separator();
        let (ws, id) = (m.ws.clone(), m.id.clone());
        menu = menu.item(PopupMenuItem::new(if m.settled { "Un-settle" } else { "Settle" }).on_click(move |_, _, cx| {
            let _ = ws.update(cx, |ws, cx| ws.toggle_settled(&id, cx));
        }));
        let (ws, id) = (m.ws.clone(), m.id.clone());
        menu = menu
            .item(PopupMenuItem::new("Settle Automatically When Idle").checked(auto).on_click(move |_, _, cx| {
                let _ = ws.update(cx, |ws, cx| ws.set_auto_settle(&id, !auto, cx));
            }))
            .separator();
        let session = m.session.clone();
        menu = menu.item(PopupMenuItem::new("Copy Session ID").disabled(session.is_none()).on_click(move |_, _, cx| {
            if let Some(s) = &session {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(s.clone()));
            }
        }));
        let id = m.id.clone();
        menu = menu
            .item(PopupMenuItem::new("Copy Thread ID").on_click(move |_, _, cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(id.clone()))))
            .separator();
        let (ws, id) = (m.ws.clone(), m.id.clone());
        // No question: Undo (and ⌘Z) brings it back.
        menu.item(PopupMenuItem::new("Delete Thread").on_click(move |_, _, cx| {
            let _ = ws.update(cx, |ws, cx| ws.delete(&id, cx));
        }))
    }
}

impl Sidebar {
    pub fn new(workspace: Entity<Workspace>, updater: Entity<crate::state::updater::Updater>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&workspace, |_, _, cx| cx.notify()),
            cx.observe(&updater, |_, _, cx| cx.notify()),
        ];
        let lib = &workspace.read(cx).library;
        let (collapsed, shelves, expanded) = (set_of(lib.setting(COLLAPSED)), set_of(lib.setting(SHELVES)), set_of(lib.setting(SHOW_ALL)));
        // The minute turning over: ages and countdowns move on, nothing in between.
        let tick = cx.spawn(async move |this, cx| loop {
            let wait = 60.0 - now().rem_euclid(60.0) + 0.05;
            cx.background_executor().timer(std::time::Duration::from_secs_f64(wait)).await;
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        });
        // Each project's repos and PRs, a moment after launch: the review badges.
        let ws = workspace.downgrade();
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(3)).await;
            let _ = ws.update(cx, |ws, cx| {
                let ids: Vec<String> = ws.projects.iter().map(|p| p.id.clone()).collect();
                for id in ids {
                    ws.scan_project(&id, cx);
                }
            });
        })
        .detach();
        Sidebar {
            release_notes: { let u = updater.clone(); cx.new(|cx| crate::ui::updates::ReleaseNotesView::new(u, cx)) },
            updater,
            workspace,
            expanded,
            shelves,
            collapsed,
            folded_workers: HashSet::new(),
            hot_row: None,
            filter_active: false,
            scroll: ScrollHandle::new(),
            renaming: None,
            focus: cx.focus_handle(),
            _tick: tick,
            _subscriptions: subscriptions,
        }
    }

    fn save_set(&self, key: &str, set: &HashSet<String>, cx: &App) {
        let mut list: Vec<&String> = set.iter().collect();
        list.sort();
        self.workspace.read(cx).library.set_setting(key, &Value::from(list.into_iter().map(|s| Value::from(s.as_str())).collect::<Vec<_>>()));
    }

    pub fn set_collapsed(&mut self, project: &str, on: bool, cx: &mut Context<Self>) {
        let changed = if on { self.collapsed.insert(project.to_string()) } else { self.collapsed.remove(project) };
        if changed {
            self.save_set(COLLAPSED, &self.collapsed, cx);
            cx.notify();
        }
    }

    fn toggle(set: &mut HashSet<String>, key: &str) {
        if !set.remove(key) {
            set.insert(key.to_string());
        }
    }

    fn rows(&self, cx: &App) -> Vec<SidebarRow> {
        let ws = self.workspace.read(cx);
        let home = ws.home_project();
        let selected = ws.selected.as_deref();
        let hide_auto = ws.hide_automation_threads();
        let mut by_project: HashMap<&str, Vec<&Entity<ThreadState>>> = HashMap::new();
        for t in &ws.threads {
            let m = &t.read(cx).meta;
            if hide_auto && m.automation_id.is_some() {
                continue;
            }
            by_project.entry(m.project_id.as_str()).or_default().push(t);
        }
        let mut projects: Vec<&ProjectMeta> = ws.projects.iter().collect();
        projects.sort_by_key(|p| Some(&p.id) != home.as_ref());
        let mut rows = vec![];
        for project in projects {
            let threads = by_project.get(project.id.as_str()).cloned().unwrap_or_default();
            let (mut live, mut settled): (Vec<_>, Vec<_>) = threads.into_iter().partition(|t| t.read(cx).meta.settled_at.is_none());
            if self.filter_active {
                let mut keep: HashSet<String> = HashSet::new();
                let parents: HashMap<String, Option<String>> = live.iter().map(|t| {
                    let m = &t.read(cx).meta;
                    (m.id.clone(), m.parent_id.clone())
                }).collect();
                for t in &live {
                    let st = t.read(cx);
                    let active = st.waiting_for_you()
                        || st.run.running
                        || st.run.jobs.any_running()
                        || (st.unread && selected != Some(st.id()))
                        || selected == Some(st.id());
                    if active {
                        let mut cur = Some(st.id().to_string());
                        while let Some(id) = cur {
                            if !keep.insert(id.clone()) {
                                break;
                            }
                            cur = parents.get(&id).cloned().flatten();
                        }
                    }
                }
                live.retain(|t| keep.contains(t.read(cx).id()));
                if live.is_empty() {
                    continue;
                }
            }
            rows.push(SidebarRow::Project(project.clone(), Some(&project.id) == home.as_ref()));
            if self.collapsed.contains(&project.id) && !self.filter_active {
                continue;
            }
            live.sort_by(|a, b| crate::state::workspace::sidebar_order(&a.read(cx).meta, &b.read(cx).meta));
            settled.sort_by(|a, b| b.read(cx).meta.settled_at.unwrap_or(0.).total_cmp(&a.read(cx).meta.settled_at.unwrap_or(0.)));
            let all = self.expanded.contains(&project.id) || self.filter_active;
            // Threads an agent started sit under the one that started it; the limit counts
            // only the threads at the top.
            let nested = self.nested(live, cx);
            let tops = nested.iter().filter(|(_, n)| n.depth == 0).count();
            let mut seen = 0;
            for (t, n) in nested {
                if n.depth == 0 {
                    seen += 1;
                }
                if !all && seen > SHOWN_PER_PROJECT {
                    break;
                }
                rows.push(SidebarRow::Thread(t.clone(), n));
            }
            if !self.filter_active && tops > SHOWN_PER_PROJECT {
                rows.push(SidebarRow::More(project.id.clone(), tops - SHOWN_PER_PROJECT, all));
            }
            if !self.filter_active && !settled.is_empty() {
                let open = self.shelves.contains(&project.id);
                rows.push(SidebarRow::Shelf(project.id.clone(), settled.len(), open));
                if open {
                    rows.extend(self.nested(settled, cx).into_iter().map(|(t, n)| SidebarRow::Thread(t.clone(), n)));
                }
            }
        }
        rows.push(SidebarRow::AddProject);
        rows
    }

    /// The threads in their order with the ones agents started under theirs, those under a
    /// folded one left out, and what each started thread's parent shows of them.
    fn nested<'a>(&self, threads: Vec<&'a Entity<ThreadState>>, cx: &App) -> Vec<(&'a Entity<ThreadState>, Nest)> {
        let order = crate::state::workspace::nest(threads, |t| {
            let m = &t.read(cx).meta;
            (m.id.clone(), m.parent_id.clone(), m.created_at)
        });
        let mut out: Vec<(&Entity<ThreadState>, Nest)> = vec![];
        // The open threads above the current one, by depth, to count each one's workers.
        let mut chain: Vec<usize> = vec![];
        let mut hidden_below: Option<usize> = None;
        for (t, depth) in order {
            chain.truncate(depth);
            let (working, waiting) = {
                let t = t.read(cx);
                let waiting = t.waiting_for_you();
                (t.run.running && !waiting, waiting)
            };
            for &i in &chain {
                let n = &mut out[i].1;
                n.workers += 1;
                n.working += working as usize;
                n.waiting += waiting as usize;
            }
            if hidden_below.is_some_and(|d| depth > d) {
                // Under a folded thread: counted, not listed.
                continue;
            }
            hidden_below = None;
            let folded = self.folded_workers.contains(t.read(cx).id());
            if folded {
                hidden_below = Some(depth);
            }
            chain.push(out.len());
            out.push((t, Nest { depth, folded, ..Default::default() }));
        }
        out
    }

    /// The threads as the sidebar lists them, top to bottom, folded projects skipped: for
    /// Previous and Next Thread, and ↑/↓.
    pub fn thread_order(&self, cx: &App) -> Vec<String> {
        self.rows(cx).iter().filter_map(|r| if let SidebarRow::Thread(t, _) = r { Some(t.read(cx).id().to_string()) } else { None }).collect()
    }

    /// The thread's row, for scrolling it into view as a person would before clicking.
    pub fn row_index(&self, id: &str, cx: &App) -> Option<usize> {
        self.rows(cx).iter().position(|r| matches!(r, SidebarRow::Thread(t, _) if t.read(cx).id() == id))
    }

    pub fn more_index(&self, project_id: &str, cx: &App) -> Option<usize> {
        self.rows(cx).iter().position(|r| matches!(r, SidebarRow::More(p, _, false) if p == project_id))
    }

    /// ↑/↓ with the sidebar focused: the thread above or below, as Previous/Next Thread.
    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let order = self.thread_order(cx);
        if order.is_empty() {
            return;
        }
        let current = self.workspace.read(cx).selected.clone();
        let next = match current.and_then(|c| order.iter().position(|t| *t == c)) {
            Some(i) => (i as isize + by).clamp(0, order.len() as isize - 1) as usize,
            None if by > 0 => 0,
            None => order.len() - 1,
        };
        let id = order[next].clone();
        if let Some(row) = self.row_index(&id, cx) {
            self.scroll.scroll_to_item(row);
        }
        self.workspace.update(cx, |ws, cx| ws.select(&id, cx));
    }

    pub fn start_rename(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.workspace.read(cx).thread(id) else { return };
        let title = thread.read(cx).title.clone();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        input.update(cx, |i, cx| {
            i.focus(window, cx);
            i.select_all(window, cx);
        });
        let id2 = id.to_string();
        let sub = cx.subscribe_in(&input, window, move |this, input, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } | InputEvent::Blur => {
                let title = input.read(cx).value().to_string();
                this.renaming = None;
                let id = id2.clone();
                this.workspace.update(cx, |ws, cx| ws.rename(&id, &title, cx));
                if matches!(event, InputEvent::PressEnter { .. }) {
                    window.focus(&this.focus, cx);
                }
                cx.notify();
            }
            _ => {}
        });
        self.renaming = Some((id.to_string(), input, sub));
        cx.notify();
    }

    /// Escape: the title stays as it was.
    fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    /// Removing a project can't be undone: it asks first.
    fn confirm_remove_project(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        let Some(project) = ws.project(id).cloned() else { return };
        let count = ws.threads.iter().filter(|t| t.read(cx).meta.project_id == id).count();
        let ws = self.workspace.downgrade();
        let id = id.to_string();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (ws, id) = (ws.clone(), id.clone());
            alert
                .title(format!("Remove “{}”?", project.name))
                .description(format!("Its {count} thread{} go with it. Its folder and Claude Code's own session files stay.", if count == 1 { "" } else { "s" }))
                .ok_text("Remove")
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    let _ = ws.update(cx, |ws, cx| ws.remove_project(&id, cx));
                    true
                })
        });
    }

    pub fn add_project(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add Project".into()),
        });
        crate::platform::set_open_panel_message("Choose a project folder. Claude runs with this folder as its working directory.");
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                if let Some(path) = paths.first() {
                    let path = path.to_string_lossy().to_string();
                    let _ = this.update(cx, |s, cx| s.add_project_at(&path, cx));
                }
            }
        })
        .detach();
    }

    /// A folder added (or one already there): a new thread in it.
    pub fn add_project_at(&mut self, path: &str, cx: &mut Context<Self>) {
        let id = self.workspace.update(cx, |ws, cx| ws.add_project(path, cx));
        self.new_thread(&id, cx);
    }

    /// Home's folder: where threads that aren't in a project start.
    fn change_home_folder(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Use Folder".into()) });
        let ws = self.workspace.downgrade();
        cx.spawn(async move |_, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                if let Some(path) = paths.first() {
                    let path = path.to_string_lossy().to_string();
                    let _ = ws.update(cx, |ws, cx| ws.set_home_folder(&path, cx));
                }
            }
        })
        .detach();
    }

    /// A new thread, its project unfolded first (`+`, ⌘N, Spotlight).
    pub fn new_thread(&mut self, project: &str, cx: &mut Context<Self>) {
        self.set_collapsed(project, false, cx);
        let pid = project.to_string();
        self.workspace.update(cx, |ws, cx| {
            ws.new_thread(&pid, cx);
        });
    }

    /// New Thread and Search in a single compact row, keeping vertical space for threads.
    fn top_buttons(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let mut waiting_ids = vec![];
        let mut unread_ids = vec![];
        let mut working_n = 0usize;
        {
            let ws = self.workspace.read(cx);
            let selected = ws.selected.as_deref();
            let hide_auto = ws.hide_automation_threads();
            for t in &ws.threads {
                let st = t.read(cx);
                if st.meta.settled_at.is_some() {
                    continue;
                }
                let hidden_auto = hide_auto && st.meta.automation_id.is_some();
                if st.waiting_for_you() {
                    waiting_ids.push(st.id().to_string());
                } else if !hidden_auto && (st.run.running || st.run.jobs.agents_running() > 0) {
                    working_n += 1;
                } else if !hidden_auto && st.unread && selected != Some(st.id()) {
                    unread_ids.push(st.id().to_string());
                }
            }
        }
        let show_triage = !waiting_ids.is_empty() || working_n > 0 || !unread_ids.is_empty() || self.filter_active;
        let ws_wait = self.workspace.clone();
        let ws_unread = self.workspace.clone();
        let waiting_count = waiting_ids.len();
        let unread_count = unread_ids.len();
        let filter_on = self.filter_active;
        div()
            .flex()
            .flex_col()
            .gap(px(5.))
            .px(px(8.))
            .pb(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .child(
                        div()
                            .id("new-thread")
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .h(px(27.))
                            .px(px(8.))
                            .rounded(px(radius::MEDIUM))
                            .bg(theme.card)
                            .border_1()
                            .border_color(theme.separator)
                            .hover(|s| s.bg(theme.hover))
                            .cursor_pointer()
                            .child(icons::square_pen().size(px(13.)).text_color(theme.accent))
                            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(text::FOOTNOTE)).font_weight(FontWeight::MEDIUM).child("New Thread"))
                            .child(div().flex_none().text_size(px(text::CAPTION)).text_color(theme.tertiary).child("⌘N"))
                            .role(gpui_kit::Role::Button)
                            .aria_label("New Thread")
                            .aria_description("New Thread (⌘N)")
                            .aria_keyshortcuts("⌘N")
                            .tooltip(tooltip("New Thread (⌘N)"))
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::ui::root::NewThread), cx)),
                    )
                    .child(
                        div()
                            .id("search")
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .h(px(27.))
                            .px(px(8.))
                            .rounded(px(radius::MEDIUM))
                            .bg(theme.hover)
                            .border_1()
                            .border_color(theme.separator)
                            .hover(|s| s.bg(theme.hover))
                            .cursor_pointer()
                            .child(Icon::new(IconName::Search).size(px(13.)).text_color(theme.secondary))
                            .child(div().text_size(px(text::CAPTION)).text_color(theme.tertiary).child("⌘K"))
                            .role(gpui_kit::Role::Button)
                            .aria_label("Search")
                            .aria_description("Search threads, what was said in them, memories and tasks, and jump there (⌘K)")
                            .aria_keyshortcuts("⌘K")
                            .tooltip(tooltip("Search threads, memories and tasks (⌘K)"))
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::ui::root::OpenSpotlight), cx)),
                    ),
            )
            .child({
                let ws_r = self.workspace.read(cx);
                let auto_open = ws_r.page == Some(Page::Automations);
                let paused_all = ws_r.automations_paused();
                let autos = ws_r.automations();
                let active_n = autos.iter().filter(|a| a.enabled).count();
                let any_error = !paused_all && autos.iter().any(|a| a.enabled && a.last_run().is_some_and(|r| r.effective_status() == yh_core::automations::RunStatus::Error));
                let ws_auto = self.workspace.clone();
                div()
                    .id("nav-automations")
                    .role(gpui_kit::Role::Button)
                    .aria_label("Automations")
                    .aria_keyshortcuts("⇧⌘A")
                    .flex()
                    .items_center()
                    .gap(px(GAP))
                    .h(px(ROW_HEIGHT))
                    .px(px(ROW_INSET + 2.))
                    .rounded(px(radius::MEDIUM))
                    .cursor_pointer()
                    .when(auto_open, |d| d.bg(theme.selected))
                    .when(!auto_open, |d| d.hover(|s| s.bg(theme.hover)))
                    .text_size(px(text::FOOTNOTE))
                    .text_color(if auto_open { theme.text } else { theme.secondary })
                    .child(icons::zap().size(px(ICON)).text_color(if paused_all { theme.warning } else if auto_open { theme.accent } else { theme.secondary }))
                    .child(div().flex_1().min_w_0().truncate().font_weight(if auto_open { FontWeight::MEDIUM } else { FontWeight::NORMAL }).child("Automations"))
                    .when(any_error, |d| d.child(div().flex_none().w(px(6.)).h(px(6.)).rounded_full().bg(theme.error)))
                    .when(paused_all, |d| d.child(div().flex_none().text_size(px(10.)).text_color(theme.warning).child("Paused")))
                    .when(!paused_all && active_n > 0, |d| d.child(div().flex_none().text_size(px(text::CAPTION)).text_color(theme.tertiary).child(active_n.to_string())))
                    .tooltip(tooltip("Scheduled & Mac-event automations (⇧⌘A)"))
                    .on_click(move |_, _, cx| ws_auto.update(cx, |ws, cx| ws.open_automations(None, cx)))
            })
            .when(show_triage, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(4.))
                        .when(waiting_count > 0, |d| {
                            d.child(
                                div()
                                    .id("triage-waiting")
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .h(px(20.))
                                    .px(px(6.))
                                    .rounded(px(radius::SMALL))
                                    .bg(theme.warning.opacity(0.15))
                                    .border_1()
                                    .border_color(theme.warning.opacity(0.32))
                                    .hover(|s| s.bg(theme.warning.opacity(0.24)))
                                    .cursor_pointer()
                                    .text_size(px(text::CAPTION))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.warning)
                                    .child(icons::hand().size(px(11.)).text_color(theme.warning))
                                    .child(format!("{waiting_count} waiting"))
                                    .tooltip(tooltip("Jump to the next thread waiting for your approval"))
                                    .on_click(move |_, _, cx| {
                                        ws_wait.update(cx, |ws, cx| {
                                            let next = ws.selected.as_deref()
                                                .and_then(|cur| waiting_ids.iter().position(|id| id == cur))
                                                .map(|i| &waiting_ids[(i + 1) % waiting_ids.len()])
                                                .or_else(|| waiting_ids.first());
                                            if let Some(id) = next.cloned() {
                                                ws.select(&id, cx);
                                            }
                                        });
                                    }),
                            )
                        })
                        .when(working_n > 0 || filter_on, |d| {
                            d.child(
                                div()
                                    .id("triage-working")
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .h(px(20.))
                                    .px(px(6.))
                                    .rounded(px(radius::SMALL))
                                    .bg(if filter_on { theme.accent } else { theme.accent_soft })
                                    .hover(|s| s.opacity(0.9))
                                    .cursor_pointer()
                                    .text_size(px(text::CAPTION))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if filter_on { gpui_kit::white() } else { theme.accent })
                                    .child(Icon::new(IconName::LoaderCircle).size(px(11.)).text_color(if filter_on { gpui_kit::white() } else { theme.accent }))
                                    .child(if filter_on {
                                        if working_n > 0 { format!("{working_n} active ×") } else { "Active only ×".into() }
                                    } else {
                                        format!("{working_n} active")
                                    })
                                    .tooltip(tooltip(if filter_on { "Showing only active, waiting and unread threads — click to show all" } else { "Filter sidebar to active, waiting and unread threads" }))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.filter_active = !this.filter_active;
                                        cx.notify();
                                    })),
                            )
                        })
                        .when(unread_count > 0, |d| {
                            d.child(
                                div()
                                    .id("triage-unread")
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .h(px(20.))
                                    .px(px(6.))
                                    .rounded(px(radius::SMALL))
                                    .bg(theme.card)
                                    .border_1()
                                    .border_color(theme.separator)
                                    .hover(|s| s.bg(theme.hover))
                                    .cursor_pointer()
                                    .text_size(px(text::CAPTION))
                                    .text_color(theme.secondary)
                                    .child(div().w(px(5.)).h(px(5.)).rounded_full().bg(theme.accent))
                                    .child(format!("{unread_count} new"))
                                    .tooltip(tooltip("Jump to the next thread with a new reply"))
                                    .on_click(move |_, _, cx| {
                                        ws_unread.update(cx, |ws, cx| {
                                            if let Some(id) = unread_ids.first().cloned() {
                                                ws.select(&id, cx);
                                            }
                                        });
                                    }),
                            )
                        }),
                )
            })
    }

    fn project_row(&self, project: &ProjectMeta, home: bool, first: bool, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let ws = self.workspace.read(cx);
        let collapsed = self.collapsed.contains(&project.id) && !self.filter_active;
        let hide_auto = ws.hide_automation_threads();
        let threads: Vec<&Entity<ThreadState>> = ws
            .threads
            .iter()
            .filter(|t| {
                let m = &t.read(cx).meta;
                m.project_id == project.id && m.settled_at.is_none() && !(hide_auto && m.automation_id.is_some())
            })
            .collect();
        let mut waiting_n = 0usize;
        let mut working_n = 0usize;
        let mut unread_n = 0usize;
        for t in &threads {
            let st = t.read(cx);
            if st.waiting_for_you() {
                waiting_n += 1;
            } else if st.run.running || st.run.jobs.agents_running() > 0 {
                working_n += 1;
            } else if st.unread {
                unread_n += 1;
            }
        }
        let activity = waiting_n > 0 || working_n > 0 || unread_n > 0;
        let count = threads.len();
        let selected = ws.page == Some(Page::Project(project.id.clone()));
        let memory_open = ws.page == Some(Page::Memory(project.id.clone()));
        let reviews = ws.scans.get(&project.id).map_or(0, |s| s.pulls.pull_requests.iter().filter(|p| p.review_requested_from_me).count());
        let group: SharedString = format!("project-row-{}", project.id).into();
        let pid = project.id.clone();
        let menu = {
            let (ws, pid, path, this) = (self.workspace.downgrade(), project.id.clone(), project.path.clone(), cx.entity().downgrade());
            move |menu: PopupMenu, _: &mut Window, _: &mut Context<PopupMenu>| {
                let (a, b, c, d, e) = (ws.clone(), ws.clone(), ws.clone(), this.clone(), this.clone());
                let (p1, p2, p3, p4, path) = (pid.clone(), pid.clone(), pid.clone(), pid.clone(), path.clone());
                let mut menu = menu.item(PopupMenuItem::new("New Thread").on_click(move |_, _, cx| {
                    let _ = a.update(cx, |ws, cx| {
                        ws.new_thread(&p1, cx);
                    });
                }));
                if !home {
                    menu = menu.item(PopupMenuItem::new("Repos & Pull Requests").on_click(move |_, _, cx| {
                        let _ = b.update(cx, |ws, cx| ws.open_project(&p2, cx));
                    }));
                }
                menu = menu
                    .item(PopupMenuItem::new("Memory").on_click(move |_, _, cx| {
                        let _ = c.update(cx, |ws, cx| ws.open_memory(&p3, cx));
                    }))
                    .item(PopupMenuItem::new("Show in Finder").on_click(move |_, _, _| {
                        let _ = std::process::Command::new("/usr/bin/open").arg(&path).spawn();
                    }));
                if home {
                    menu = menu.item(PopupMenuItem::new("Change Default Folder…").on_click(move |_, _, cx| {
                        let _ = e.update(cx, |s, cx| s.change_home_folder(cx));
                    }));
                } else {
                    menu = menu.separator().item(PopupMenuItem::new("Remove Project…").on_click(move |_, window, cx| {
                        let _ = d.update(cx, |s, cx| s.confirm_remove_project(&p4, window, cx));
                    }));
                }
                menu
            }
        };
        let name = project.name.clone();
        // A workspace of repos (a folder or shell holding them) is a stack, the rest a folder;
        // known once the project's been read.
        let stacked = self.workspace.read(cx).scans.get(&project.id).and_then(|w| w.layout.as_ref()).is_some_and(|l| matches!(l.kind, yh_core::workspace::LayoutKind::Folder | yh_core::workspace::LayoutKind::Shell));
        let icon = if home { icons::house() } else if stacked { icons::layers() } else { Icon::new(IconName::Folder) };
        let chevron_key = pid.clone();
        let (open_ws, open_id) = (self.workspace.clone(), pid.clone());
        let (mem_ws, mem_id) = (self.workspace.clone(), pid.clone());
        let (new_id, memory_tip) = (pid.clone(), format!("What agents remember in {name} (⇧⌘M)"));
        div()
            .id(SharedString::from(format!("project-{}", project.id)))
            .role(gpui_kit::Role::Heading)
            .aria_label(format!("{}{}", project.name, if collapsed { ", collapsed" } else { "" }))
            .group(group.clone())
            .flex()
            .items_center()
            .gap(px(GAP))
            .mt(px(if first { 2. } else { 10. }))
            .px(px(ROW_INSET))
            .h(px(ROW_HEIGHT))
            .rounded(px(radius::MEDIUM))
            .text_size(px(text::CAPTION))
            .text_color(theme.secondary)
            .when(selected, |d| d.bg(theme.selected))
            .when(!selected, |d| d.hover(|s| s.bg(theme.hover)))
            .child(
                div()
                    .id(SharedString::from(format!("chevron-{}", project.id)))
                    .role(gpui_kit::Role::Button)
                    .aria_label(if collapsed { format!("Show threads of {name}") } else { format!("Hide threads of {name}") })
                    .flex_none()
                    .w(px(CHEVRON))
                    .h(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown }).size(px(10.)).text_color(theme.tertiary))
                    .tooltip(tooltip(if collapsed { "Show threads" } else { "Hide threads" }))
                    .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let on = !this.collapsed.contains(&chevron_key);
                        this.set_collapsed(&chevron_key, on, cx);
                        cx.stop_propagation();
                    })),
            )
            .child(
                // The name opens the project's page: its PRs and repos (Home: its memory).
                div()
                    .id(SharedString::from(format!("project-name-{}", project.id)))
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(GAP))
                    .child(icon.size(px(ICON)).text_color(theme.secondary))
                    .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_weight(FontWeight::SEMIBOLD).child(name.clone()))
                    .when(waiting_n > 0, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(2.))
                                .px(px(4.))
                                .h(px(15.))
                                .rounded(px(radius::SMALL))
                                .bg(theme.warning.opacity(0.16))
                                .text_color(theme.warning)
                                .text_size(px(10.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(icons::hand().size(px(9.)).text_color(theme.warning))
                                .child(waiting_n.to_string()),
                        )
                    })
                    .when(working_n > 0, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(2.))
                                .px(px(4.))
                                .h(px(15.))
                                .rounded(px(radius::SMALL))
                                .bg(theme.accent_soft)
                                .text_color(theme.accent)
                                .text_size(px(10.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(Icon::new(IconName::LoaderCircle).size(px(9.)).text_color(theme.accent))
                                .child(working_n.to_string()),
                        )
                    })
                    .when(collapsed && waiting_n == 0 && working_n == 0 && activity, |d| d.child(div().flex_none().w(px(6.)).h(px(6.)).rounded_full().bg(theme.accent)))
                    .when(collapsed && !activity && count > 0, |d| d.child(div().flex_none().text_size(px(text::CAPTION)).text_color(theme.tertiary).child(count.to_string())))
                    .child(div().flex_1())
                    .when(reviews > 0, |d| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("reviews-{}", project.id)))
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(3.))
                                .px(px(5.))
                                .rounded(px(radius::SMALL))
                                .bg(theme.accent_soft)
                                .text_color(theme.accent)
                                .text_size(px(text::CAPTION))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(Icon::new(IconName::Eye).size(px(11.)).text_color(theme.accent))
                                .child(reviews.to_string())
                                .tooltip(tooltip(format!("{reviews} pull request{} waiting for your review", if reviews == 1 { "" } else { "s" }))),
                        )
                    })
                    .tooltip(tooltip(if home { format!("{}: its memory", project.path) } else { format!("Pull requests, repos and memory of {name}") }))
                    .on_click(move |_, _, cx| open_ws.update(cx, |ws, cx| ws.open_project(&open_id, cx))),
            )
            .child(
                // Memory: shown on hover, and while its page is open.
                div()
                    .id(SharedString::from(format!("memory-{}", project.id)))
                    .role(gpui_kit::Role::Button)
                    .aria_label(format!("Memory of {name}"))
                    .flex_none()
                    .w(px(20.))
                    .h(px(20.))
                    .rounded(px(radius::SMALL))
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(|s| s.bg(theme.selected))
                    .child(icons::brain().size(px(13.)).text_color(if memory_open { theme.accent } else { theme.secondary }))
                    .map(|d| if memory_open { d } else { hidden(d).group_hover(group.clone(), |s| visible(s)) })
                    .tooltip(tooltip(memory_tip))
                    .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        mem_ws.update(cx, |ws, cx| ws.open_memory(&mem_id, cx))
                    }),
            )
            .child(
                div()
                    .id(SharedString::from(format!("new-{}", project.id)))
                    .role(gpui_kit::Role::Button)
                    .aria_label(format!("New thread in {name}"))
                    .flex_none()
                    .w(px(20.))
                    .h(px(20.))
                    .rounded(px(radius::SMALL))
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(|s| s.bg(theme.selected))
                    .child(Icon::new(IconName::Plus).size(px(13.)).text_color(theme.secondary))
                    .tooltip(tooltip(format!("New thread in {name}")))
                    .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.new_thread(&new_id, cx);
                    })),
            )
            .context_menu(menu)
            .into_any_element()
    }

    fn thread_row(&self, thread: &Entity<ThreadState>, nest: Nest, selected: Option<&str>, theme: &Theme, now: f64, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let t = thread.read(cx);
        let id = t.id().to_string();
        let is_selected = selected == Some(id.as_str());
        if let Some((rid, input, _)) = &self.renaming {
            if *rid == id {
                return div()
                    .h(px(ROW_HEIGHT))
                    .pl(px(TITLE_X - 8.))
                    .pr(px(ROW_INSET))
                    .flex()
                    .items_center()
                    .capture_key_down(cx.listener(|this, e: &gpui_kit::KeyDownEvent, window, cx| {
                        if e.keystroke.key == "escape" {
                            this.cancel_rename(window, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(Input::new(input).small())
                    .into_any_element();
            }
        }
        let ws = self.workspace.read(cx);
        let waiting = t.waiting_for_you();
        // Sub-agents in the background after the turn: still working.
        let agents = (!t.run.running && !waiting).then(|| agents_marker(t)).flatten();
        let running = (t.run.running || agents.is_some()) && !waiting;
        let unread = t.unread && !is_selected;
        let pinned = t.meta.pinned;
        let settled = t.meta.settled_at.is_some();
        let generating = ws.is_generating_title(&id);
        // A workflow other than Default (Ultracode): it can start many agents.
        let workflow = Some(ws.workflow_of(t)).filter(|w| *w != yh_core::claude::workflow::DEFAULT).map(|w| yh_core::claude::workflow::mode(w).name);
        let tracking = probe::enabled();
        let jobs = jobs_marker(t, running || waiting);
        // Selection is a soft fill: text keeps its colours, as in Finder's sidebar.
        let fg: Hsla = theme.secondary;
        let menu = ThreadMenu {
            this: cx.entity().downgrade(),
            ws: self.workspace.downgrade(),
            id: id.clone(),
            session: t.run.session_id.clone(),
            pinned,
            settled,
            can_regenerate: !t.run.running && (t.run.session_id.is_some() || t.run.total.unwrap_or(0) > 0 || !t.items.is_empty()),
        };
        // Working, waiting on you, waiting on a job, unread, or nothing when idle: under the
        // project's icon. Still icons: the sidebar is cached, nothing in it spins.
        // Folded away, the threads it started still show: one waiting on you while this one
        // isn't following them (its turn over).
        let workers_need_you = nest.folded && nest.waiting > 0 && !t.run.running;
        let (status, status_tip): (gpui_kit::AnyElement, &str) = if workers_need_you && !waiting {
            (icons::hand().size(px(12.)).text_color(theme.warning).into_any_element(), "A thread it started is waiting for you")
        } else if waiting {
            (icons::hand().size(px(12.)).text_color(theme.warning).into_any_element(), "Waiting for your approval")
        } else if running {
            (Icon::new(IconName::LoaderCircle).size(px(12.)).text_color(theme.accent).into_any_element(), "Working…")
        } else if jobs.is_some() {
            (icons::hourglass().size(px(11.)).text_color(fg).into_any_element(), "")
        } else if unread {
            (div().w(px(6.5)).h(px(6.5)).rounded_full().bg(theme.accent).into_any_element(), "New reply")
        } else {
            // Idle: a subtle dot.
            (div().w(px(5.)).h(px(5.)).rounded_full().bg(theme.tertiary.opacity(0.45)).into_any_element(), "")
        };
        let status_tip: SharedString = agents.clone().or_else(|| jobs.clone()).unwrap_or_else(|| status_tip.into());
        let group: SharedString = format!("thread-row-{id}").into();
        let ws_click = self.workspace.clone();
        let click_id = id.clone();
        let this = cx.entity().downgrade();
        let age = yh_core::clock::short_age(t.meta.updated_at / 1000.0, now);
        // Hover covers the end of the title with the row's own colour, so nothing moves.
        // Only mount the action strip (with its DropdownMenu popover) on the last-hovered row.
        let cover = theme.sidebar;
        let is_hot = self.hot_row.as_deref() == Some(id.as_str());
        let actions = is_hot.then(|| {
            let (ws1, ws2, i1, i2) = (self.workspace.clone(), self.workspace.clone(), id.clone(), id.clone());
            let m = menu.clone();
            let button = |name: &str, icon: Icon, tip: &'static str| {
                div()
                    .id(SharedString::from(format!("{name}-{id}")))
                    .w(px(20.))
                    .h(px(20.))
                    .rounded(px(radius::SMALL))
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(|s| s.bg(theme.selected))
                    .child(icon.size(px(12.)).text_color(fg))
                    .tooltip(tooltip(tip))
            };
            hidden(div())
                .group_hover(group.clone(), |s| visible(s))
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .flex()
                .items_center()
                .rounded_r(px(radius::MEDIUM))
                .bg(cover)
                .child(
                    div()
                        .h_full()
                        .flex()
                        .items_center()
                        .gap(px(1.))
                        .pl(px(4.))
                        .rounded_r(px(radius::MEDIUM))
                        // The row's own fill over the sidebar, so the cover can't be told apart.
                        .bg(if is_selected { theme.selected } else { theme.hover })
                        .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .when(!settled, |d| {
                            d.child(
                                button("pin", if pinned { icons::pin_off() } else { icons::pin() }, if pinned { "Unpin" } else { "Pin to the top" })
                                    .on_click(move |_, _, cx| {
                                        cx.stop_propagation();
                                        ws1.update(cx, |ws, cx| ws.toggle_pin(&i1, cx))
                                    }),
                            )
                        })
                        .child(
                            button(
                                "settle",
                                Icon::new(if settled { IconName::Undo2 } else { IconName::CircleCheck }),
                                if settled { "Move back to active work" } else { "Settle: move finished work to the shelf and stop its session" },
                            )
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                ws2.update(cx, |ws, cx| ws.toggle_settled(&i2, cx))
                            }),
                        )
                        .child(
                            Button::new(SharedString::from(format!("more-{id}")))
                                .xsmall()
                                .ghost()
                                .icon(Icon::new(IconName::Ellipsis).size(px(12.)).text_color(fg))
                                .tooltip("More")
                                .dropdown_menu(move |menu, _, cx| m.build(menu, cx)),
                        )
                        .child(div().w(px(2.))),
                )
        });
        let hover_id = id.clone();
        // VoiceOver: the title, then what it's doing.
        let spoken = {
            let mut parts = vec![t.title.clone()];
            if let Some(name) = workflow {
                parts.push(format!("Workflow {name}"));
            }
            if generating {
                parts.push("Generating title".into());
            }
            if waiting {
                parts.push("Waiting for your approval".into());
            } else if let Some(agents) = &agents {
                parts.push(agents.to_string());
            } else if running {
                parts.push("Working".into());
            } else if unread {
                parts.push("New reply".into());
            }
            if pinned {
                parts.push("Pinned".into());
            }
            if settled {
                parts.push("Settled".into());
            }
            parts.join(", ")
        };
        let agent = ws.agent_of(t);
        let is_agy = agent == yh_core::agents::Agent::Antigravity;
        let show_provider = ws.agy_found || is_agy;
        let needs_attention = waiting || workers_need_you;
        div()
            .id(SharedString::from(format!("thread-{id}")))
            .role(gpui_kit::Role::ListItem)
            .aria_label(spoken)
            .group(group)
            .relative()
            .flex()
            .items_center()
            .gap(px(GAP))
            .w_full()
            .h(px(ROW_HEIGHT))
            .pl(px(ROW_INSET + 2. + nest.depth as f32 * INDENT))
            .pr(px(ROW_INSET))
            .rounded(px(radius::MEDIUM))
            .overflow_hidden()
            .text_size(px(text::FOOTNOTE))
            .when(is_selected, |d| d.bg(theme.selected))
            .when(!is_selected && needs_attention, |d| d.bg(theme.warning.opacity(0.09)).hover(|s| s.bg(theme.warning.opacity(0.15))))
            .when(!is_selected && !needs_attention, |d| d.hover(|s| s.bg(theme.hover)))
            .when(is_selected, |d| {
                d.child(div().absolute().left_0().top(px(4.)).bottom(px(4.)).w(px(2.5)).rounded_r(px(2.)).bg(theme.accent))
            })
            .when(nest.depth > 0, |d| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(ROW_INSET + 8. + (nest.depth - 1) as f32 * INDENT))
                        .w(px(1.))
                        .bg(theme.separator),
                )
            })
            .child(
                div()
                    .id(SharedString::from(format!("status-{id}")))
                    .flex_none()
                    .w(px(ICON))
                    .h(px(ICON))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(status)
                    .when(!status_tip.is_empty(), |d| d.tooltip(tooltip(status_tip.clone()))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .when(settled, |d| d.text_color(theme.secondary))
                    .when(unread, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .when(!unread && (is_selected || needs_attention), |d| d.font_weight(FontWeight::MEDIUM))
                    // A placeholder until the generated title arrives.
                    .when(generating, |d| d.opacity(0.55))
                    .child(t.title.clone()),
            )
            .when(pinned, |d| d.child(icons::pin().size(px(10.)).text_color(theme.tertiary).rotate(gpui_kit::radians(0.7))))
            .when(t.meta.automation_id.is_some(), |d| {
                d.child(div().id(SharedString::from(format!("auto-badge-{id}"))).flex_none().child(icons::zap().size(px(10.5)).text_color(theme.tertiary)).tooltip(tooltip("Started by an automation")))
            })
            .when_some(workflow, |d, name| {
                d.child(div().id(SharedString::from(format!("workflow-{id}"))).flex_none().child(crate::ui::icons::workflow().size(px(11.)).text_color(theme.accent)).tooltip(tooltip(SharedString::from(format!("Workflow: {name}")))))
            })
            // Threads this agent started: a compact clickable pill to fold/unfold without wasting left gutter on every row.
            .when(nest.workers > 0, |d| {
                let (this, key) = (cx.entity().downgrade(), id.clone());
                let tip = workers_summary(&nest);
                let (bg, fg_c) = if nest.waiting > 0 {
                    (theme.warning.opacity(0.16), theme.warning)
                } else if nest.working > 0 {
                    (theme.accent_soft, theme.accent)
                } else {
                    (theme.selected, theme.secondary)
                };
                d.child(
                    div()
                        .id(SharedString::from(format!("fold-{id}")))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .h(px(16.))
                        .px(px(4.))
                        .rounded(px(radius::SMALL))
                        .bg(bg)
                        .text_size(px(10.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(fg_c)
                        .cursor_pointer()
                        .child(if nest.folded { format!("+{}", nest.workers) } else { format!("{}", nest.workers) })
                        .child(Icon::new(if nest.folded { IconName::ChevronRight } else { IconName::ChevronDown }).size(px(9.)).text_color(fg_c))
                        .tooltip(tooltip(tip))
                        .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            let _ = this.update(cx, |s, cx| {
                                Self::toggle(&mut s.folded_workers, &key);
                                cx.notify();
                            });
                        }),
                )
            })
            .when(show_provider, |d| {
                let color = if is_agy { theme.teal() } else { theme.accent };
                let badge = if is_agy { "A" } else { "C" };
                let tip = if is_agy { "Provider: Antigravity" } else { "Provider: Claude" };
                d.child(
                    div()
                        .id(SharedString::from(format!("provider-{id}")))
                        .flex_none()
                        .w(px(14.))
                        .h(px(14.))
                        .rounded(px(3.5))
                        .bg(color.opacity(0.15))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(9.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color)
                        .child(badge)
                        .tooltip(tooltip(tip)),
                )
            })
            .child(div().flex_none().text_size(px(10.5)).text_color(if is_selected { theme.secondary } else { theme.tertiary }).child(age))
            .children(actions)
            .on_hover(cx.listener(move |this, is_hovered, _, cx| {
                if *is_hovered && this.hot_row.as_deref() != Some(hover_id.as_str()) {
                    this.hot_row = Some(hover_id.clone());
                    cx.notify();
                }
            }))
            .when(tracking, |d| d.child(probe::tracker(format!("thread:{id}"))))
            .on_click(move |event, window, cx| {
                let id = click_id.clone();
                if event.click_count() == 2 {
                    let _ = this.update(cx, |s, cx| s.start_rename(&id, window, cx));
                    return;
                }
                if let Some(s) = this.upgrade() {
                    let focus = s.read(cx).focus.clone();
                    window.focus(&focus, cx);
                }
                ws_click.update(cx, |ws, cx| ws.select(&id, cx));
            })
            .context_menu(move |m, _, cx| menu.build(m, cx))
            .into_any_element()
    }
}

/// "3 threads it started: 1 working, 1 waiting for you".
fn workers_summary(n: &Nest) -> String {
    let mut parts = vec![];
    if n.working > 0 {
        parts.push(format!("{} working", n.working));
    }
    if n.waiting > 0 {
        parts.push(format!("{} waiting for you", n.waiting));
    }
    let head = format!("{} thread{} it started", n.workers, if n.workers == 1 { "" } else { "s" });
    let head = if parts.is_empty() { head } else { format!("{head}: {}", parts.join(", ")) };
    format!("{head} · click to {}", if n.folded { "show them" } else { "fold them away" })
}

fn visible(mut s: gpui_kit::StyleRefinement) -> gpui_kit::StyleRefinement {
    s.visibility = Some(Visibility::Visible);
    s
}


/// "312 MB", "1.2 GB": this app's memory, as macOS's byte counts say it.
fn memory_label() -> String {
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V4, &mut info as *mut _ as *mut libc::rusage_info_t) } == 0;
    if !ok {
        return String::new();
    }
    let bytes = info.ri_phys_footprint as f64;
    if bytes >= 1e9 { format!("{:.1} GB", bytes / 1e9) } else { format!("{:.0} MB", bytes / 1e6) }
}

/// An account's 5-hour or weekly limit in the footer's compact rows: the bar and its percent.
const ACCOUNT_METER: f32 = 62.;

impl Sidebar {
    /// What there is to do about an update, above the limits (`ui::updates::banner`).
    fn update_row(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        crate::ui::updates::banner(&self.updater, &self.workspace, &self.release_notes, theme, cx)
    }

    /// `5h ━━━━──── 6%  2h 14m`: each limit used, and how long until it resets. The tooltip says
    /// when; a click opens Usage.
    fn meter(name: &str, window: Option<&yh_core::limits::Window>, updated: Option<f64>, now: f64, theme: &Theme) -> gpui_kit::AnyElement {
        Self::meter_for("", name, window, updated, now, theme)
    }

    /// A meter of an agent's (`agent`: "" for Claude, else its key, and "Gemini models · " in
    /// its tooltip's title via `limits` names).
    fn meter_for(agent: &str, name: &str, window: Option<&yh_core::limits::Window>, updated: Option<f64>, now: f64, theme: &Theme) -> gpui_kit::AnyElement {
        let title = if agent.is_empty() { yh_core::limits::title(name) } else { format!("{agent} · {}", yh_core::limits::title(name)) };
        let used = window.map(|w| yh_core::limits::utilization(w, now));
        let reset = window.is_some_and(|w| w.resets_at.is_some_and(|at| at <= now));
        let tint = match used.unwrap_or(0.) {
            u if u < 0.7 => theme.accent,
            u if u < 0.9 => theme.warning,
            _ => theme.error,
        };
        let reading = match window {
            None => format!("{title}: no reading yet. Refresh to check."),
            Some(_) if reset => format!("{title}: reset since the last reading. Refresh to update."),
            Some(w) => match w.resets_at {
                Some(at) => format!("{title}: resets {}, {}", yh_core::clock::relative(at, now), yh_core::clock::short_date_time(at)),
                None => title.clone(),
            },
        };
        let mut tip = reading;
        if name == "five_hour" {
            if let Some(t) = updated {
                tip.push_str(&format!("\nUpdated {}", yh_core::clock::relative(t, now)));
            }
        }
        tip.push_str("\nClick for usage (⇧⌘U)");
        let countdown = window.and_then(|w| w.resets_at).filter(|_| !reset).map(|at| yh_core::clock::countdown(at, now)).unwrap_or_default();
        div()
            .id(SharedString::from(format!("meter-{agent}{name}")))
            .flex()
            .items_center()
            .gap(px(space::S))
            .h(px(18.))
            .text_size(px(text::CAPTION))
            .child(div().w(px(38.)).flex_none().whitespace_nowrap().font_weight(FontWeight::MEDIUM).text_color(theme.secondary).child(yh_core::limits::short_label(name)))
            .child(
                div().flex_1().h(px(BAR)).rounded_full().bg(theme.selected).overflow_hidden().when_some(used, |d, u| {
                    d.child(div().h_full().rounded_full().min_w(px(BAR)).w(gpui_kit::relative(u as f32)).bg(tint))
                }),
            )
            .child(div().w(px(30.)).flex_none().flex().justify_end().text_color(theme.secondary).child(used.map(|u| format!("{}%", (u * 100.).round() as i64)).unwrap_or_else(|| "–".into())))
            .child(div().w(px(44.)).flex_none().flex().justify_end().text_color(theme.tertiary).whitespace_nowrap().child(countdown))
            .tooltip(tooltip(tip))
            .into_any_element()
    }

    /// One compact meter cell inside a 2-column pair (`5h 42%   3h 36m` over a full-width bar):
    /// keeps both utilization and reset countdown visible for both windows.
    fn compact_meter(agent: &str, name: &str, window: Option<&yh_core::limits::Window>, updated: Option<f64>, now: f64, theme: &Theme) -> gpui_kit::AnyElement {
        let title = if agent.is_empty() { yh_core::limits::title(name) } else { format!("{agent} · {}", yh_core::limits::title(name)) };
        let used = window.map(|w| yh_core::limits::utilization(w, now));
        let reset = window.is_some_and(|w| w.resets_at.is_some_and(|at| at <= now));
        let tint = match used.unwrap_or(0.) {
            u if u < 0.7 => theme.accent,
            u if u < 0.9 => theme.warning,
            _ => theme.error,
        };
        let reading = match window {
            None => format!("{title}: no reading yet. Refresh to check."),
            Some(_) if reset => format!("{title}: reset since the last reading. Refresh to update."),
            Some(w) => match w.resets_at {
                Some(at) => format!("{title}: resets {}, {}", yh_core::clock::relative(at, now), yh_core::clock::short_date_time(at)),
                None => title.clone(),
            },
        };
        let mut tip = reading;
        if name == "five_hour" {
            if let Some(t) = updated {
                tip.push_str(&format!("\nUpdated {}", yh_core::clock::relative(t, now)));
            }
        }
        tip.push_str("\nClick for usage (⇧⌘U)");
        let countdown = window.and_then(|w| w.resets_at).filter(|_| !reset).map(|at| yh_core::clock::countdown(at, now)).unwrap_or_default();
        div()
            .id(SharedString::from(format!("meter-compact-{agent}{name}")))
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .h(px(13.))
                    .text_size(px(10.))
                    .child(div().flex_none().font_weight(FontWeight::MEDIUM).text_color(theme.tertiary).child(yh_core::limits::short_label(name)))
                    .child(div().flex_none().font_weight(FontWeight::MEDIUM).text_color(theme.secondary).child(used.map(|u| format!("{}%", (u * 100.).round() as i64)).unwrap_or_else(|| "–".into())))
                    .child(div().flex_1())
                    .when(!countdown.is_empty(), |d| d.child(div().flex_none().text_color(theme.tertiary).whitespace_nowrap().child(countdown))),
            )
            .child(
                div().w_full().h(px(4.)).rounded_full().bg(theme.selected).overflow_hidden().when_some(used, |d, u| {
                    d.child(div().h_full().rounded_full().min_w(px(4.)).w(gpui_kit::relative(u as f32)).bg(tint))
                }),
            )
            .tooltip(tooltip(tip))
            .into_any_element()
    }

    /// A subscription's name over its meters: `Claude Max`, the account only in its tooltip.
    fn subscription_row(sub: Option<&yh_core::limits::Subscription>, theme: &Theme) -> gpui_kit::AnyElement {
        let plan = sub.map(|s| s.plan_name()).unwrap_or_else(|| "Claude".into());
        let tip = match sub {
            Some(s) => {
                let mut t = format!("Claude Code is signed in to {plan}");
                if let Some(email) = &s.email {
                    t.push_str(&format!(" as {email}"));
                }
                // Without an email, the organization is already its name.
                if let Some(org) = s.org.as_ref().filter(|_| s.email.is_some()) {
                    t.push_str(&format!("\n{org}"));
                }
                t
            }
            None => "Who Claude Code is signed in as isn't known yet".into(),
        };
        div()
            .id("subscription")
            .flex()
            .items_center()
            .gap(px(5.))
            .h(px(15.))
            .text_size(px(text::CAPTION))
            .child(div().w(px(5.)).h(px(5.)).rounded_full().bg(theme.accent))
            .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).text_color(theme.text).child(plan))
            .tooltip(tooltip(tip))
            .into_any_element()
    }

    /// `Antigravity · Google AI Pro` over its meters; the tooltip has every group's limits.
    fn agy_row(limits: &crate::state::limits::AgentLimits, theme: &Theme) -> gpui_kit::AnyElement {
        let now = now();
        let mut tip = match &limits.plan {
            Some(p) => format!("Antigravity is signed in to {p}"),
            None => "Antigravity".to_string(),
        };
        for g in &limits.groups {
            tip.push_str(&format!("\n\n{}", g.name));
            if let Some(models) = g.description.strip_prefix("Models within this group: ") {
                tip.push_str(&format!(" ({models})"));
            }
            for w in &g.windows {
                let used = (yh_core::limits::utilization(w, now) * 100.).round() as i64;
                let reset = w.resets_at.filter(|at| *at > now).map(|at| format!(" · resets {}", yh_core::clock::relative(at, now))).unwrap_or_default();
                tip.push_str(&format!("\n{}: {used}% used{reset}", yh_core::limits::title(&w.name)));
            }
        }
        if let Some(e) = &limits.error {
            tip.push_str(&format!("\n\nLast refresh: {e}"));
        }
        if let Some(t) = limits.updated_at {
            tip.push_str(&format!("\nUpdated {}", yh_core::clock::relative(t, now)));
        }
        div()
            .id("agy-subscription")
            .flex()
            .items_center()
            .gap(px(5.))
            .h(px(15.))
            .text_size(px(text::CAPTION))
            .child(div().w(px(5.)).h(px(5.)).rounded_full().bg(theme.teal()))
            .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).text_color(theme.text).child("Antigravity"))
            .when_some(limits.plan.clone(), |d, p| d.child(div().flex_1().min_w_0().truncate().text_color(theme.tertiary).child(p)))
            .tooltip(tooltip(tip))
            .into_any_element()
    }

    /// `5h  7d` over the accounts' columns.
    fn accounts_header(theme: &Theme) -> gpui_kit::AnyElement {
        let column = |label: &'static str| div().w(px(ACCOUNT_METER)).flex_none().flex().justify_end().child(label);
        div()
            .flex()
            .items_center()
            .gap(px(space::S))
            .h(px(14.))
            .text_size(px(text::CAPTION))
            .text_color(theme.tertiary)
            .child(div().flex_1())
            .child(column("5h"))
            .child(column("7d"))
            .into_any_element()
    }

    /// One account in a row: a dot on the active one, who it is, its 5-hour and weekly limits.
    /// A click switches to it; on the active one, it opens Usage.
    fn account_row(a: &crate::state::limits::AccountState, label: String, active: bool, now: f64, theme: &Theme, ws: &gpui_kit::Entity<Workspace>) -> gpui_kit::AnyElement {
        let windows = a.limits.as_ref().map(yh_core::limits::windows).unwrap_or_default();
        let window = |name: &str| windows.iter().find(|w| w.name == name).cloned();
        let mini = |w: Option<yh_core::limits::Window>| {
            let used = w.as_ref().map(|w| yh_core::limits::utilization(w, now));
            let tint = match used.unwrap_or(0.) {
                u if u < 0.7 => theme.accent,
                u if u < 0.9 => theme.warning,
                _ => theme.error,
            };
            div()
                .w(px(ACCOUNT_METER))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(div().flex_1().h(px(BAR)).rounded_full().bg(theme.selected).overflow_hidden().when_some(used, |d, u| d.child(div().h_full().rounded_full().min_w(px(BAR)).w(gpui_kit::relative(u as f32)).bg(tint))))
                .child(div().w(px(28.)).flex_none().flex().justify_end().child(used.map(|u| format!("{}%", (u * 100.).round() as i64)).unwrap_or_else(|| "–".into())))
        };
        let mut tip = match &a.subscription {
            Some(s) => format!("{} · {}", s.plan_name(), s.who().unwrap_or("signed in")),
            None if a.login.is_some() => format!("{label}: signing in, in your browser"),
            None if a.signed_out() => format!("{label}: signed out. Sign in from Settings → Agents"),
            None => label.clone(),
        };
        for w in &windows {
            if let Some(at) = w.resets_at.filter(|at| *at > now) {
                tip.push_str(&format!("\n{}: {}% · resets {}", yh_core::limits::title(&w.name), (yh_core::limits::utilization(w, now) * 100.).round() as i64, yh_core::clock::relative(at, now)));
            }
        }
        if let Some(e) = &a.error {
            tip.push_str(&format!("\nLast refresh: {e}"));
        }
        let signed_out = a.signed_out() && a.login.is_none();
        tip.push_str(if signed_out {
            "\nClick to sign in, in your browser"
        } else if active {
            "\nNew threads use this account; each thread keeps its own (in its header). Click for usage (⇧⌘U)"
        } else {
            "\nClick to use it for new threads; each thread keeps its own (in its header)"
        });
        let id = a.id().to_string();
        let ws = ws.clone();
        let quiet = a.signed_out() || a.login.is_some();
        div()
            .id(SharedString::from(format!("account-{id}")))
            .role(gpui_kit::Role::Button)
            .aria_label(format!("{label}{}", if active { ", for new threads" } else { "" }))
            .flex()
            .items_center()
            .gap(px(space::S))
            .h(px(18.))
            .mx(px(-4.))
            .px(px(4.))
            .rounded(px(radius::SMALL))
            .hover(|s| s.bg(theme.hover))
            .text_size(px(text::CAPTION))
            .text_color(theme.secondary)
            .child(div().flex_none().size(px(6.)).rounded_full().when(active, |d| d.bg(theme.accent)).when(!active, |d| d.border_1().border_color(theme.tertiary)))
            .child(div().flex_1().min_w_0().truncate().when(active, |d| d.font_weight(FontWeight::MEDIUM).text_color(theme.text)).child(label))
            .when(!quiet, |d| d.child(mini(window("five_hour"))).child(mini(window("seven_day"))))
            .when(quiet, |d| d.child(div().flex_none().text_color(theme.tertiary).child(if a.login.is_some() { "Signing in…" } else { "Signed out" })))
            .tooltip(tooltip(tip))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                ws.update(cx, |ws, cx| {
                    if signed_out {
                        ws.sign_in(&id, cx)
                    } else if active {
                        if ws.page != Some(Page::Usage) {
                            ws.toggle_usage(cx)
                        }
                    } else {
                        ws.switch_account(&id, cx)
                    }
                })
            })
            .into_any_element()
    }

    /// Usage limits, this app's memory, a refresh, Keep Awake and Settings.
    fn footer(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.read(cx);
        let now = now();
        let active = ws.active();
        let enabled: Vec<&crate::state::limits::AccountState> = ws.accounts.iter().filter(|a| a.account.enabled).collect();
        let meters = yh_core::limits::meters(active.limits.as_ref());
        let updated = active.limits_updated_at;
        let refreshing = ws.limits_refreshing();
        let errors: Vec<String> = enabled.iter().filter_map(|a| a.error.as_ref().map(|e| if enabled.len() > 1 { format!("{}: {e}", ws.account_label(a.id())) } else { e.clone() })).collect();
        let error = (!errors.is_empty()).then(|| errors.join("\n"));
        let refresh_tip = match &error {
            Some(e) => format!("Couldn't refresh: {e}"),
            None if refreshing => "Refreshing usage limits…".to_string(),
            None => {
                let mut t = format!("Refresh usage limits (sends Claude a tiny Haiku request{})", if enabled.len() > 1 { " on each account" } else { "" });
                if let Some(u) = updated {
                    t.push_str(&format!("\nUpdated {}", yh_core::clock::relative(u, now)));
                }
                t
            }
        };
        let usage_ws = self.workspace.clone();
        let mut bars = div().id("limits").flex().flex_col().gap(px(2.)).on_click(move |_, _, cx| {
            usage_ws.update(cx, |ws, cx| {
                if ws.page != Some(Page::Usage) {
                    ws.toggle_usage(cx)
                }
            })
        });
        if enabled.len() > 1 {
            // Several accounts: a row each, the one in use marked.
            bars = bars.child(Self::accounts_header(theme));
            for a in &enabled {
                bars = bars.child(Self::account_row(a, ws.account_label(a.id()), a.id() == active.id(), now, theme, &self.workspace));
            }
        } else {
            bars = bars.child(Self::subscription_row(active.subscription.as_ref(), theme));
            if meters.len() >= 2 {
                bars = bars.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(Self::compact_meter("", &meters[0].0, meters[0].1.as_ref(), updated, now, theme))
                        .child(Self::compact_meter("", &meters[1].0, meters[1].1.as_ref(), updated, now, theme)),
                );
                for (name, window) in meters.iter().skip(2) {
                    bars = bars.child(Self::meter(name, window.as_ref(), updated, now, theme));
                }
            } else {
                for (name, window) in &meters {
                    bars = bars.child(Self::meter(name, window.as_ref(), updated, now, theme));
                }
            }
        }
        // Antigravity's limits under Claude's, drawn the same: compact 2-column pair with resets.
        if ws.agy_found && (!ws.agy_limits.groups.is_empty() || ws.agy_limits.error.is_some()) {
            let main = ws.agy_limits.main_group();
            let agent = main.map_or("Antigravity", |g| g.name.as_str());
            let w0 = main.and_then(|g| g.windows.iter().find(|w| w.name == yh_core::limits::PRIMARY[0]));
            let w1 = yh_core::limits::PRIMARY.get(1).and_then(|n| main.and_then(|g| g.windows.iter().find(|w| w.name == *n)));
            bars = bars.child(div().h(px(4.)));
            bars = bars.child(Self::agy_row(&ws.agy_limits, theme));
            bars = bars.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(Self::compact_meter(agent, yh_core::limits::PRIMARY[0], w0, ws.agy_limits.updated_at, now, theme))
                    .when_some(yh_core::limits::PRIMARY.get(1), |d, n| {
                        d.child(Self::compact_meter(agent, n, w1, ws.agy_limits.updated_at, now, theme))
                    }),
            );
        }
        let (awake_ws, settings_ws, refresh_ws) = (self.workspace.clone(), self.workspace.clone(), self.workspace.clone());
        let awake = ws.keep_awake.reason.is_some();
        let manual = ws.keep_awake.has_manual();
        let awake_label = ws.keep_awake.describe();
        // No box: a hairline above, the meters, then a row of quiet controls.
        div()
            .flex_none()
            .px(px(12.))
            .pt(px(8.))
            .pb(px(6.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .border_t_1()
            .border_color(theme.separator)
            .children(self.update_row(theme, cx))
            .child(bars)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .mr(px(-6.))
                    .child(
                        div()
                            .id("app-memory")
                            .flex()
                            .items_center()
                            .gap(px(space::XS))
                            .text_size(px(text::CAPTION))
                            .text_color(theme.tertiary)
                            .child(Icon::new(IconName::MemoryStick).size(px(12.)).text_color(theme.tertiary))
                            .child(memory_label())
                            .tooltip(tooltip("This app's memory use")),
                    )
                    .child(crate::ui::updates::version_button(&self.updater, &self.workspace, &self.release_notes, theme, cx))
                    .child(div().flex_1())
                    .child(
                        Button::new("keep-awake")
                            .xsmall()
                            .ghost()
                            .icon(Icon::new(if awake { IconName::Sun } else { IconName::Moon }).size(px(ICON)).text_color(if awake { theme.accent } else { theme.secondary }))
                            .tooltip(awake_label)
                            .dropdown_menu(move |mut menu, _, _| {
                                // The cup: keep the Mac awake for a while, or let it sleep.
                                for (title, hours) in [("Keep Awake for 1 Hour", Some(1u64)), ("Keep Awake for 2 Hours", Some(2)), ("Keep Awake for 4 Hours", Some(4)), ("Keep Awake Until I Stop It", None)] {
                                    let ws = awake_ws.clone();
                                    menu = menu.item(PopupMenuItem::new(title).on_click(move |_, _, cx| {
                                        ws.update(cx, |ws, cx| {
                                            ws.keep_awake.keep_awake(hours.map(|h| std::time::Duration::from_secs(h * 3600)));
                                            ws.schedule_keep_awake(cx);
                                        })
                                    }));
                                }
                                if manual {
                                    let ws = awake_ws.clone();
                                    menu = menu.separator().item(PopupMenuItem::new("Let the Mac Sleep").on_click(move |_, _, cx| {
                                        ws.update(cx, |ws, cx| {
                                            ws.keep_awake.stop_manual();
                                            ws.schedule_keep_awake(cx);
                                        })
                                    }));
                                }
                                menu
                            }),
                    )
                    .child(
                        Button::new("refresh-limits")
                            .xsmall()
                            .ghost()
                            .disabled(refreshing)
                            .icon(
                                Icon::new(if error.is_some() {
                                    IconName::TriangleAlert
                                } else if refreshing {
                                    IconName::LoaderCircle
                                } else {
                                    IconName::RefreshCw
                                })
                                .size(px(ICON))
                                .text_color(if error.is_some() { theme.warning } else { theme.secondary }),
                            )
                            .tooltip(refresh_tip)
                            .on_click(move |_, _, cx| refresh_ws.update(cx, |ws, cx| ws.refresh_limits(cx))),
                    )
                    .child(
                        Button::new("settings")
                            .xsmall()
                            .ghost()
                            .icon(Icon::new(IconName::Settings).size(px(ICON)).text_color(theme.secondary))
                            .tooltip("Settings (⌘,)")
                            .on_click(move |_, window, cx| crate::ui::settings::open(settings_ws.clone(), window, cx)),
                    ),
            )
    }
}

/// "Show 12 more" and friends: a footnote under the titles, in a row of its own height.
fn small_row(id: SharedString, label: String, theme: &Theme) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .role(gpui_kit::Role::Button)
        .aria_label(label.clone())
        .relative()
        .flex()
        .items_center()
        .h(px(22.))
        .pl(px(TITLE_X))
        .pr(px(space::S))
        .text_size(px(text::CAPTION))
        .text_color(theme.secondary)
        .rounded(px(radius::MEDIUM))
        .hover(|s| s.bg(theme.hover))
        .child(label)
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        // With a page open in its place, no thread shows as selected.
        let selected = if self.workspace.read(cx).page.is_some() { None } else { self.workspace.read(cx).selected.clone() };
        let tracking = probe::enabled();
        let now = now();
        let mut column = div()
            .id("sidebar-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px(px(8.))
            .pb(px(16.));
        let mut first_project = true;
        for row in self.rows(cx) {
            let element = match row {
                SidebarRow::Project(project, home) => {
                    let first = std::mem::replace(&mut first_project, false);
                    self.project_row(&project, home, first, &theme, cx)
                }
                SidebarRow::Thread(thread, nest) => self.thread_row(&thread, nest, selected.as_deref(), &theme, now, cx),
                SidebarRow::More(project, count, all) => {
                    let key = project.clone();
                    small_row(SharedString::from(format!("more-{project}")), if all { "Show less".into() } else { format!("Show {count} more") }, &theme)
                        .when(tracking, |d| d.child(probe::tracker(format!("more:{project}"))))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            Self::toggle(&mut this.expanded, &key);
                            this.save_set(SHOW_ALL, &this.expanded, cx);
                            cx.notify();
                        }))
                        .into_any_element()
                }
                SidebarRow::Shelf(project, count, open) => {
                    let key = project.clone();
                    div()
                        .id(SharedString::from(format!("shelf-{project}")))
                        .role(gpui_kit::Role::Button)
                        .aria_label(format!("Settled, {count}, {}", if open { "shown" } else { "hidden" }))
                        .flex()
                        .items_center()
                        .gap(px(GAP))
                        .h(px(22.))
                        .pl(px(ROW_INSET + 2.))
                        .pr(px(space::S))
                        .rounded(px(radius::MEDIUM))
                        .text_size(px(text::CAPTION))
                        .text_color(theme.secondary)
                        .hover(|s| s.bg(theme.hover))
                        .child(div().w(px(ICON)).flex().justify_center().child(Icon::new(IconName::CircleCheck).size(px(11.)).text_color(theme.tertiary)))
                        .child("Settled")
                        .child(div().text_color(theme.tertiary).child(count.to_string()))
                        .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(10.)).text_color(theme.tertiary))
                        .tooltip(tooltip(if open { "Hide settled threads" } else { "Show settled threads" }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            Self::toggle(&mut this.shelves, &key);
                            this.save_set(SHELVES, &this.shelves, cx);
                            cx.notify();
                        }))
                        .into_any_element()
                }
                SidebarRow::AddProject => div()
                    .id("add-project")
                    .role(gpui_kit::Role::Button)
                    .aria_label("Add Project…")
                    .flex()
                    .items_center()
                    .gap(px(GAP))
                    .mt(px(8.))
                    .h(px(ROW_HEIGHT))
                    .pl(px(ROW_INSET + 2.))
                    .rounded(px(radius::MEDIUM))
                    .text_size(px(text::FOOTNOTE))
                    .text_color(theme.secondary)
                    .hover(|s| s.bg(theme.hover))
                    .child(icons::folder_plus().size(px(ICON)).text_color(theme.secondary))
                    .child("Add Project…")
                    .tooltip(tooltip("Add a folder to work in (⌘O)"))
                    .on_click(cx.listener(|this, _, _, cx| this.add_project(cx)))
                    .into_any_element(),
            };
            column = column.child(element);
        }
        div()
            .id("sidebar")
            .key_context("Sidebar")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e: &gpui_kit::KeyDownEvent, _, cx| {
                if this.renaming.is_some() || !e.keystroke.modifiers.is_subset_of(&gpui_kit::Modifiers::shift()) {
                    return;
                }
                match e.keystroke.key.as_str() {
                    "up" => this.step(-1, cx),
                    "down" => this.step(1, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.separator)
            // Room for the traffic lights, aligned with the 38 pt top bar.
            .child(div().h(px(38.)).flex_none())
            .child(self.top_buttons(&theme, cx))
            .child(column)
            .child(self.footer(&theme, cx))
    }
}
