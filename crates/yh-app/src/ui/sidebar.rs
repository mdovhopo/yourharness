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

/// The rows' grid: every row is 28 pt; a project's chevron, then its icon, then its name, and
/// each thread's status under the project's icon and its title under the name.
const ROW_HEIGHT: f32 = 28.;
const ROW_INSET: f32 = 6.;
const CHEVRON: f32 = 12.;
const ICON: f32 = 14.;
const GAP: f32 = 6.;
/// A limit meter's bar: thick enough to read at a glance.
const BAR: f32 = 6.;
/// Where titles start, from the row's edge.
const TITLE_X: f32 = ROW_INSET + CHEVRON + GAP + ICON + GAP;

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
        1 => "Claude is waiting on a background command".into(),
        n => format!("Claude is waiting on {n} background commands").into(),
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

/// How far a thread an agent started sits in from the one that started it: one status column.
const INDENT: f32 = ICON + GAP;

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
        let mut by_project: HashMap<&str, Vec<&Entity<ThreadState>>> = HashMap::new();
        for t in &ws.threads {
            by_project.entry(t.read(cx).meta.project_id.as_str()).or_default().push(t);
        }
        let mut projects: Vec<&ProjectMeta> = ws.projects.iter().collect();
        projects.sort_by_key(|p| Some(&p.id) != home.as_ref());
        let mut rows = vec![];
        for project in projects {
            rows.push(SidebarRow::Project(project.clone(), Some(&project.id) == home.as_ref()));
            if self.collapsed.contains(&project.id) {
                continue;
            }
            let threads = by_project.get(project.id.as_str()).cloned().unwrap_or_default();
            let (mut live, mut settled): (Vec<_>, Vec<_>) = threads.into_iter().partition(|t| t.read(cx).meta.settled_at.is_none());
            live.sort_by(|a, b| crate::state::workspace::sidebar_order(&a.read(cx).meta, &b.read(cx).meta));
            settled.sort_by(|a, b| b.read(cx).meta.settled_at.unwrap_or(0.).total_cmp(&a.read(cx).meta.settled_at.unwrap_or(0.)));
            let all = self.expanded.contains(&project.id);
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
            if tops > SHOWN_PER_PROJECT {
                rows.push(SidebarRow::More(project.id.clone(), tops - SHOWN_PER_PROJECT, all));
            }
            if !settled.is_empty() {
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

    /// New Thread and Search: rows like the threads under them, the shortcut quiet at the end.
    fn top_buttons(&self, theme: &Theme) -> impl IntoElement {
        let action = |id: &'static str, icon: Icon, label: &'static str, key: &'static str, strong: bool, tip: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(GAP))
                .h(px(ROW_HEIGHT))
                .px(px(ROW_INSET))
                .rounded(px(radius::MEDIUM))
                .hover(|s| s.bg(theme.hover))
                .child(div().flex_none().w(px(CHEVRON + GAP + ICON)).flex().justify_end().child(icon.size(px(ICON)).text_color(if strong { theme.accent } else { theme.secondary })))
                .child(div().flex_1().when(!strong, |d| d.text_color(theme.secondary)).child(label))
                .child(div().flex_none().text_size(px(text::CAPTION)).text_color(theme.tertiary).child(key))
                .role(gpui_kit::Role::Button)
                .aria_label(label)
                .aria_description(tip)
                .aria_keyshortcuts(key)
                .tooltip(tooltip(tip))
        };
        div()
            .flex()
            .flex_col()
            .px(px(8.))
            .pb(px(space::S))
            .child(action("new-thread", icons::square_pen(), "New Thread", "⌘N", true, "New Thread (⌘N)").on_click(|_, window, cx| {
                window.dispatch_action(Box::new(crate::ui::root::NewThread), cx)
            }))
            .child(
                action("search", Icon::new(IconName::Search), "Search", "⌘K", false, "Search threads, what was said in them, memories and tasks, and jump there (⌘K)")
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::ui::root::OpenSpotlight), cx)),
            )
    }

    fn project_row(&self, project: &ProjectMeta, home: bool, first: bool, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let ws = self.workspace.read(cx);
        let collapsed = self.collapsed.contains(&project.id);
        let threads: Vec<&Entity<ThreadState>> = ws.threads.iter().filter(|t| t.read(cx).meta.project_id == project.id && t.read(cx).meta.settled_at.is_none()).collect();
        let activity = threads.iter().any(|t| {
            let t = t.read(cx);
            t.run.running || t.unread || t.waiting_for_you() || t.run.jobs.agents_running() > 0
        });
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
            .mt(px(if first { 0. } else { space::M }))
            .px(px(ROW_INSET))
            .h(px(ROW_HEIGHT))
            .rounded(px(radius::MEDIUM))
            .text_size(px(text::FOOTNOTE))
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
                    .when(collapsed && activity, |d| d.child(div().flex_none().w(px(6.)).h(px(6.)).rounded_full().bg(theme.accent)))
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
                    .w(px(22.))
                    .h(px(22.))
                    .rounded(px(radius::SMALL))
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(|s| s.bg(theme.selected))
                    .child(icons::brain().size(px(ICON)).text_color(if memory_open { theme.accent } else { theme.secondary }))
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
                    .w(px(22.))
                    .h(px(22.))
                    .rounded(px(radius::SMALL))
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(|s| s.bg(theme.selected))
                    .child(Icon::new(IconName::Plus).size(px(ICON)).text_color(theme.secondary))
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
            (icons::hand().size(px(13.)).text_color(theme.warning).into_any_element(), "A thread it started is waiting for you")
        } else if waiting {
            (icons::hand().size(px(13.)).text_color(theme.warning).into_any_element(), "Waiting for your approval")
        } else if running {
            (Icon::new(IconName::LoaderCircle).size(px(13.)).text_color(theme.accent).into_any_element(), "Working…")
        } else if jobs.is_some() {
            (icons::hourglass().size(px(12.)).text_color(fg).into_any_element(), "")
        } else if unread {
            (div().w(px(7.)).h(px(7.)).rounded_full().bg(theme.accent).into_any_element(), "New reply")
        } else {
            // Idle: a small grey dot.
            (div().w(px(6.)).h(px(6.)).rounded_full().bg(theme.tertiary.opacity(0.6)).into_any_element(), "")
        };
        let status_tip: SharedString = agents.clone().or_else(|| jobs.clone()).unwrap_or_else(|| status_tip.into());
        let group: SharedString = format!("thread-row-{id}").into();
        let ws_click = self.workspace.clone();
        let click_id = id.clone();
        let this = cx.entity().downgrade();
        let age = yh_core::clock::short_age(t.meta.updated_at / 1000.0, now);
        // Hover covers the end of the title with the row's own colour, so nothing moves.
        let cover = theme.sidebar;
        let actions = {
            let (ws1, ws2, i1, i2) = (self.workspace.clone(), self.workspace.clone(), id.clone(), id.clone());
            let m = menu.clone();
            let button = |name: &str, icon: Icon, tip: &'static str| {
                div()
                    .id(SharedString::from(format!("{name}-{id}")))
                    .w(px(22.))
                    .h(px(22.))
                    .rounded(px(radius::SMALL))
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(|s| s.bg(theme.selected))
                    .child(icon.size(px(13.)).text_color(fg))
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
                                .icon(Icon::new(IconName::Ellipsis).size(px(13.)).text_color(fg))
                                .tooltip("More")
                                .dropdown_menu(move |menu, _, cx| m.build(menu, cx)),
                        )
                        .child(div().w(px(2.))),
                )
        };
        // VoiceOver: the title, then what it's doing.
        let spoken = {
            let mut parts = vec![t.title.clone()];
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
            .pl(px(ROW_INSET + nest.depth as f32 * INDENT))
            .pr(px(ROW_INSET))
            .rounded(px(radius::MEDIUM))
            .overflow_hidden()
            .when(is_selected, |d| d.bg(theme.selected))
            .when(!is_selected, |d| d.hover(|s| s.bg(theme.hover)))
            .child(if nest.workers > 0 {
                let (this, key) = (cx.entity().downgrade(), id.clone());
                let tip = workers_summary(&nest);
                div()
                    .id(SharedString::from(format!("fold-{id}")))
                    .flex_none()
                    .w(px(CHEVRON))
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(if nest.folded { IconName::ChevronRight } else { IconName::ChevronDown }).size(px(10.)).text_color(theme.tertiary))
                    .tooltip(tooltip(tip))
                    .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        let _ = this.update(cx, |s, cx| {
                            Self::toggle(&mut s.folded_workers, &key);
                            cx.notify();
                        });
                    })
                    .into_any_element()
            } else {
                div().flex_none().w(px(CHEVRON)).into_any_element()
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
                    // A placeholder until the generated title arrives.
                    .when(generating, |d| d.opacity(0.55))
                    .child(t.title.clone()),
            )
            .when(pinned, |d| d.child(icons::pin().size(px(11.)).text_color(theme.tertiary).rotate(gpui_kit::radians(0.7))))
            // Folded: how many it started, where they'd be.
            .when(nest.folded, |d| d.child(div().flex_none().text_size(px(text::CAPTION)).text_color(theme.tertiary).child(format!("+{}", nest.workers))))
            .child(div().flex_none().text_size(px(text::CAPTION)).text_color(if is_selected { theme.secondary } else { theme.tertiary }).child(age))
            .child(actions)
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
        let title = yh_core::limits::title(name);
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
            .id(SharedString::from(format!("meter-{name}")))
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
            .text_size(px(text::CAPTION))
            .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).text_color(theme.text).child(plan))
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
            for (name, window) in &meters {
                bars = bars.child(Self::meter(name, window.as_ref(), updated, now, theme));
            }
        }
        let (awake_ws, settings_ws, refresh_ws) = (self.workspace.clone(), self.workspace.clone(), self.workspace.clone());
        let awake = ws.keep_awake.reason.is_some();
        let manual = ws.keep_awake.has_manual();
        let awake_label = ws.keep_awake.describe();
        // No box: a hairline above, the meters, then a row of quiet controls.
        div()
            .flex_none()
            .px(px(space::L))
            .pt(px(space::M))
            .pb(px(space::S))
            .flex()
            .flex_col()
            .gap(px(space::S))
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
        .h(px(24.))
        .pl(px(TITLE_X))
        .pr(px(space::S))
        .text_size(px(text::FOOTNOTE))
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
                        .h(px(24.))
                        .pl(px(ROW_INSET + CHEVRON + GAP))
                        .pr(px(space::S))
                        .rounded(px(radius::MEDIUM))
                        .text_size(px(text::FOOTNOTE))
                        .text_color(theme.secondary)
                        .hover(|s| s.bg(theme.hover))
                        .child(div().w(px(ICON)).flex().justify_center().child(Icon::new(IconName::CircleCheck).size(px(12.)).text_color(theme.tertiary)))
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
                    .mt(px(space::M))
                    .h(px(ROW_HEIGHT))
                    .pl(px(ROW_INSET + CHEVRON + GAP))
                    .rounded(px(radius::MEDIUM))
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
            // Room for the traffic lights.
            .child(div().h(px(44.)).flex_none())
            .child(self.top_buttons(&theme))
            .child(column)
            .child(self.footer(&theme, cx))
    }
}
