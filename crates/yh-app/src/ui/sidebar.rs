//! Projects with their threads: Home first, then as saved; in each, pinned threads, then the
//! newest; settled ones on a shelf of their own. Right-click a thread for Rename, Pin, Settle
//! and Delete; double-click renames. One flat column of rows, so a row can be scrolled to by
//! index. A cached view: it redraws when the workspace says something changed, not per delta.

use crate::probe;
use crate::state::thread::ThreadState;
use crate::state::workspace::Workspace;
use crate::theme::Theme;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::component::{Sizable, WindowExt};
use gpui_kit::{div, prelude::*, px, App, Context, Entity, FontWeight, ScrollHandle, SharedString, Subscription, Window};
use std::collections::{HashMap, HashSet};
use yh_core::library::ProjectMeta;

const SHOWN_PER_PROJECT: usize = 12;

pub struct Sidebar {
    workspace: Entity<Workspace>,
    /// A new version, when there's one: said at the bottom.
    updater: Entity<crate::state::updater::Updater>,
    _observe_updater: Subscription,
    /// Projects showing all their threads, and those with their Settled shelf open.
    expanded: HashSet<String>,
    shelves: HashSet<String>,
    pub scroll: ScrollHandle,
    renaming: Option<(String, Entity<InputState>, Subscription)>,
    _observe: Subscription,
}

enum SidebarRow {
    Project(ProjectMeta, bool),
    Thread(Entity<ThreadState>),
    More(String, usize, bool),
    Shelf(String, usize, bool),
    AddProject,
}

impl Sidebar {
    pub fn new(workspace: Entity<Workspace>, updater: Entity<crate::state::updater::Updater>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&workspace, |_, _, cx| cx.notify());
        let observe_updater = cx.observe(&updater, |_, _, cx| cx.notify());
        Sidebar {
            updater,
            _observe_updater: observe_updater,
            workspace,
            expanded: HashSet::new(),
            shelves: HashSet::new(),
            scroll: ScrollHandle::new(),
            renaming: None,
            _observe: observe,
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
            let threads = by_project.get(project.id.as_str()).cloned().unwrap_or_default();
            let (mut live, mut settled): (Vec<_>, Vec<_>) = threads.into_iter().partition(|t| t.read(cx).meta.settled_at.is_none());
            live.sort_by(|a, b| {
                let (a, b) = (&a.read(cx).meta, &b.read(cx).meta);
                b.pinned.cmp(&a.pinned).then(b.updated_at.total_cmp(&a.updated_at))
            });
            settled.sort_by(|a, b| b.read(cx).meta.settled_at.unwrap_or(0.).total_cmp(&a.read(cx).meta.settled_at.unwrap_or(0.)));
            let all = self.expanded.contains(&project.id);
            let shown = if all { live.len() } else { live.len().min(SHOWN_PER_PROJECT) };
            rows.extend(live[..shown].iter().map(|t| SidebarRow::Thread((*t).clone())));
            if live.len() > SHOWN_PER_PROJECT {
                rows.push(SidebarRow::More(project.id.clone(), live.len() - SHOWN_PER_PROJECT, all));
            }
            if !settled.is_empty() {
                let open = self.shelves.contains(&project.id);
                rows.push(SidebarRow::Shelf(project.id.clone(), settled.len(), open));
                if open {
                    rows.extend(settled.iter().map(|t| SidebarRow::Thread((*t).clone())));
                }
            }
        }
        rows.push(SidebarRow::AddProject);
        rows
    }

    /// The threads as the sidebar lists them, top to bottom: for Previous and Next Thread.
    pub fn thread_order(&self, cx: &App) -> Vec<String> {
        self.rows(cx).iter().filter_map(|r| if let SidebarRow::Thread(t) = r { Some(t.read(cx).id().to_string()) } else { None }).collect()
    }

    /// The thread's row, for scrolling it into view as a person would before clicking.
    pub fn row_index(&self, id: &str, cx: &App) -> Option<usize> {
        self.rows(cx).iter().position(|r| matches!(r, SidebarRow::Thread(t) if t.read(cx).id() == id))
    }

    pub fn more_index(&self, project_id: &str, cx: &App) -> Option<usize> {
        self.rows(cx).iter().position(|r| matches!(r, SidebarRow::More(p, _, false) if p == project_id))
    }

    fn start_rename(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.workspace.read(cx).thread(id) else { return };
        let title = thread.read(cx).title.clone();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        input.update(cx, |i, cx| {
            i.focus(window, cx);
            i.select_all(window, cx);
        });
        let id2 = id.to_string();
        let sub = cx.subscribe_in(&input, window, move |this, input, event: &InputEvent, _, cx| match event {
            InputEvent::PressEnter { .. } | InputEvent::Blur => {
                let title = input.read(cx).value().to_string();
                this.renaming = None;
                let id = id2.clone();
                this.workspace.update(cx, |ws, cx| ws.rename(&id, &title, cx));
                cx.notify();
            }
            _ => {}
        });
        self.renaming = Some((id.to_string(), input, sub));
        cx.notify();
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
        let ws = self.workspace.downgrade();
        cx.spawn(async move |_, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                if let Some(path) = paths.first() {
                    let path = path.to_string_lossy().to_string();
                    let _ = ws.update(cx, |ws, cx| ws.add_project(&path, cx));
                }
            }
        })
        .detach();
    }

    fn thread_row(&self, thread: &Entity<ThreadState>, selected: Option<&str>, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let t = thread.read(cx);
        let id = t.id().to_string();
        let is_selected = selected == Some(id.as_str());
        if let Some((rid, input, _)) = &self.renaming {
            if *rid == id {
                return div().px(px(4.)).py(px(2.)).child(Input::new(input)).into_any_element();
            }
        }
        let waiting = t.permission.is_some();
        let running = t.running && !waiting;
        let unread = t.unread && !is_selected;
        let pinned = t.meta.pinned;
        let settled = t.meta.settled_at.is_some();
        let tracking = probe::enabled();
        let ws = self.workspace.clone();
        let click_id = id.clone();
        let menu_id = id.clone();
        let this = cx.entity().downgrade();
        div()
            .id(SharedString::from(format!("thread-{id}")))
            .relative()
            .flex()
            .items_center()
            .gap(px(6.))
            .w_full()
            .h(px(28.))
            .px(px(8.))
            .rounded(px(6.))
            .when(is_selected, |d| d.bg(theme.accent).text_color(gpui_kit::white()))
            .when(!is_selected, |d| d.hover(|s| s.bg(theme.selected)))
            .when(settled && !is_selected, |d| d.text_color(theme.secondary))
            .when(pinned, |d| d.child(div().text_size(px(8.)).opacity(0.7).child("●")))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .when(unread, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .child(t.title.clone()),
            )
            .when(waiting, |d| {
                d.child(
                    div()
                        .text_size(px(10.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .px(px(6.))
                        .rounded(px(8.))
                        .bg(theme.warning)
                        .text_color(gpui_kit::white())
                        .child("waiting"),
                )
            })
            .when(running, |d| {
                d.child(div().w(px(7.)).h(px(7.)).rounded_full().bg(if is_selected { gpui_kit::white() } else { theme.accent }))
            })
            .when(unread && !running && !waiting, |d| d.child(div().w(px(7.)).h(px(7.)).rounded_full().bg(theme.accent)))
            .when(tracking, |d| d.child(probe::tracker(format!("thread:{id}"))))
            .on_click(move |event, window, cx| {
                let id = click_id.clone();
                if event.click_count() == 2 {
                    let _ = this.update(cx, |s, cx| s.start_rename(&id, window, cx));
                    return;
                }
                ws.update(cx, |ws, cx| ws.select(&id, cx));
            })
            .context_menu({
                let this = cx.entity().downgrade();
                let ws = self.workspace.downgrade();
                let session = thread.read(cx).session_id.clone();
                move |menu, _, _| {
                    let (a, b, c, d, e) = (this.clone(), ws.clone(), ws.clone(), ws.clone(), ws.clone());
                    let (i1, i2, i3, i4, i5, i6) = (menu_id.clone(), menu_id.clone(), menu_id.clone(), menu_id.clone(), menu_id.clone(), menu_id.clone());
                    let mut menu = menu
                        .item(PopupMenuItem::new("Rename…").on_click(move |_, window, cx| {
                            let _ = a.update(cx, |s, cx| s.start_rename(&i1, window, cx));
                        }))
                        .item(PopupMenuItem::new(if pinned { "Unpin" } else { "Pin" }).on_click(move |_, _, cx| {
                            let _ = b.update(cx, |ws, cx| ws.toggle_pin(&i2, cx));
                        }))
                        .item(PopupMenuItem::new(if settled { "Bring Back" } else { "Settle" }).on_click(move |_, _, cx| {
                            let _ = c.update(cx, |ws, cx| ws.toggle_settled(&i3, cx));
                        }))
                        .item(PopupMenuItem::new("Regenerate Title").on_click(move |_, _, cx| {
                            let _ = e.update(cx, |ws, cx| ws.regenerate_title(&i5, cx));
                        }))
                        .separator();
                    if let Some(s) = session.clone() {
                        menu = menu.item(PopupMenuItem::new("Copy Session ID").on_click(move |_, _, cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(s.clone()))));
                    }
                    menu.item(PopupMenuItem::new("Copy Thread ID").on_click(move |_, _, cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(i6.clone()))))
                        .separator()
                        // No question: Undo (and ⌘Z) brings it back.
                        .item(PopupMenuItem::new("Delete").on_click(move |_, _, cx| {
                            let _ = d.update(cx, |ws, cx| ws.delete(&i4, cx));
                        }))
                }
            })
            .into_any_element()
    }
}

impl Sidebar {
    /// Claude's 5-hour and weekly limits as it last reported them, and the way to Settings.
    /// A new version: download it, how far along, then Restart.
    fn update_row(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        use crate::state::updater::Phase;
        let phase = self.updater.read(cx).phase.clone();
        let (text, action): (String, Option<(&'static str, bool)>) = match &phase {
            Phase::Available(r) => (format!("YourHarness {} is out", r.version), Some(("Update", false))),
            Phase::Downloading(r, p) => (format!("Downloading {}… {:.0}%", r.version, p * 100.), None),
            Phase::Preparing(r) => (format!("Checking {}…", r.version), None),
            Phase::Ready(v) => (format!("{v} is ready"), Some(("Restart", true))),
            Phase::Failed(e, Some(_)) => (e.clone(), Some(("Retry", false))),
            _ => return None,
        };
        let updater = self.updater.clone();
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(12.))
                .py(px(6.))
                .border_t_1()
                .border_color(theme.separator)
                .text_size(px(11.))
                .child(div().flex_1().min_w_0().text_color(theme.secondary).line_clamp(2).child(text))
                .when_some(action, |d, (label, restart)| {
                    d.child(Button::new("update-action").xsmall().primary().label(label).on_click(move |_, _, cx| {
                        updater.update(cx, |u, cx| if restart { u.restart(cx) } else { u.download(cx) })
                    }))
                })
                .into_any_element(),
        )
    }

    fn footer(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let limits = self.workspace.read(cx).limits.clone();
        let mut bars = div().flex_1().flex().flex_col().gap(px(3.));
        let windows = limits.as_ref().and_then(|l| l["unifiedWindows"].as_object().cloned());
        match windows {
            Some(windows) if !windows.is_empty() => {
                for (name, w) in windows {
                    let used = w["utilization"].as_f64().unwrap_or(0.).clamp(0., 1.);
                    let label = match name.as_str() {
                        "five_hour" => "5h".to_string(),
                        "seven_day" => "Week".to_string(),
                        other => other.replace('_', " "),
                    };
                    bars = bars.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(10.))
                            .text_color(theme.secondary)
                            .child(div().w(px(30.)).child(label))
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(4.))
                                    .rounded(px(2.))
                                    .bg(theme.selected)
                                    .child(div().h_full().rounded(px(2.)).w(gpui_kit::relative(used as f32)).bg(if used > 0.8 { theme.warning } else { theme.accent })),
                            )
                            .child(div().w(px(28.)).child(format!("{}%", (used * 100.).round()))),
                    );
                }
            }
            _ => bars = bars.child(div().text_size(px(10.)).text_color(theme.secondary).child("Usage shows after a turn")),
        }
        let ws = self.workspace.clone();
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(8.))
            .border_t_1()
            .border_color(theme.separator)
            .child(bars)
            .child({
                // The cup: keep the Mac awake for a while, or let it sleep.
                let ws = self.workspace.clone();
                let awake = self.workspace.read(cx).keep_awake.reason.is_some();
                let manual = self.workspace.read(cx).keep_awake.has_manual();
                let label = self.workspace.read(cx).keep_awake.describe();
                Button::new("keep-awake")
                    .xsmall()
                    .ghost()
                    .label(if awake { "☕︎" } else { "☾" })
                    .tooltip(label)
                    .dropdown_menu(move |mut menu, _, _| {
                        for (title, hours) in [("Keep Awake for 1 Hour", Some(1u64)), ("Keep Awake for 2 Hours", Some(2)), ("Keep Awake for 4 Hours", Some(4)), ("Keep Awake Until I Stop It", None)] {
                            let ws = ws.clone();
                            menu = menu.item(PopupMenuItem::new(title).on_click(move |_, _, cx| {
                                ws.update(cx, |ws, cx| {
                                    ws.keep_awake.keep_awake(hours.map(|h| std::time::Duration::from_secs(h * 3600)));
                                    ws.schedule_keep_awake(cx);
                                })
                            }));
                        }
                        if manual {
                            let ws = ws.clone();
                            menu = menu.separator().item(PopupMenuItem::new("Let the Mac Sleep").on_click(move |_, _, cx| {
                                ws.update(cx, |ws, cx| {
                                    ws.keep_awake.stop_manual();
                                    ws.schedule_keep_awake(cx);
                                })
                            }));
                        }
                        menu
                    })
            })
            .child(
                div()
                    .id("settings")
                    .px(px(4.))
                    .rounded(px(4.))
                    .text_color(theme.secondary)
                    .hover(|s| s.bg(theme.selected))
                    .child("⚙︎")
                    .on_click(move |_, window, cx| crate::ui::settings::open(ws.clone(), window, cx)),
            )
    }
}

fn small_row(id: SharedString, label: String, theme: &Theme) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .relative()
        .px(px(8.))
        .py(px(3.))
        .text_size(px(11.))
        .text_color(theme.secondary)
        .rounded(px(5.))
        .hover(|s| s.bg(theme.selected))
        .child(label)
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        // With a page open in its place, no thread shows as selected.
        let selected = if self.workspace.read(cx).page.is_some() { None } else { self.workspace.read(cx).selected.clone() };
        let tracking = probe::enabled();
        let mut column = div()
            .id("sidebar-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px(px(8.))
            .pb(px(16.));
        for row in self.rows(cx) {
            let element = match row {
                SidebarRow::Project(project, home) => {
                    let ws = self.workspace.clone();
                    let pid = project.id.clone();
                    div()
                        .flex()
                        .items_center()
                        .px(px(6.))
                        .pt(px(10.))
                        .pb(px(2.))
                        .text_color(theme.secondary)
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child({
                            // The name opens the project's page: its PRs and repos, and its memory.
                            let (ws, pid) = (self.workspace.clone(), project.id.clone());
                            div()
                                .id(SharedString::from(format!("project-{}", project.id)))
                                .flex_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .rounded(px(4.))
                                .hover(|s| s.text_color(theme.text))
                                .child(if home { "Home".to_string() } else { project.name.clone() })
                                .on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.open_project(&pid, cx)))
                                .context_menu({
                                    let (ws, pid, path, this) = (self.workspace.downgrade(), project.id.clone(), project.path.clone(), cx.entity().downgrade());
                                    move |menu, _, _| {
                                        let (a, b, c, d) = (ws.clone(), ws.clone(), ws.clone(), this.clone());
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
                                        if !home {
                                            menu = menu.separator().item(PopupMenuItem::new("Remove Project…").on_click(move |_, window, cx| {
                                                let _ = d.update(cx, |s, cx| s.confirm_remove_project(&p4, window, cx));
                                            }));
                                        }
                                        menu
                                    }
                                })
                        })
                        .child(
                            div()
                                .id(SharedString::from(format!("new-{}", project.id)))
                                .w(px(20.))
                                .h(px(20.))
                                .rounded(px(5.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(15.))
                                .hover(|s| s.bg(theme.selected))
                                .child("+")
                                .on_click(move |_, _, cx| {
                                    let pid = pid.clone();
                                    ws.update(cx, |ws, cx| {
                                        ws.new_thread(&pid, cx);
                                    });
                                }),
                        )
                        .into_any_element()
                }
                SidebarRow::Thread(thread) => self.thread_row(&thread, selected.as_deref(), &theme, cx),
                SidebarRow::More(project, count, all) => {
                    let key = project.clone();
                    small_row(SharedString::from(format!("more-{project}")), if all { "Show less".into() } else { format!("Show {count} more") }, &theme)
                        .when(tracking, |d| d.child(probe::tracker(format!("more:{project}"))))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.expanded.remove(&key) {
                                this.expanded.insert(key.clone());
                            }
                            cx.notify();
                        }))
                        .into_any_element()
                }
                SidebarRow::Shelf(project, count, open) => {
                    let key = project.clone();
                    small_row(SharedString::from(format!("shelf-{project}")), if open { "Hide settled".into() } else { format!("Settled ({count})") }, &theme)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.shelves.remove(&key) {
                                this.shelves.insert(key.clone());
                            }
                            cx.notify();
                        }))
                        .into_any_element()
                }
                SidebarRow::AddProject => small_row("add-project".into(), "+ Add Project…".into(), &theme)
                    .mt(px(8.))
                    .on_click(cx.listener(|this, _, _, cx| this.add_project(cx)))
                    .into_any_element(),
            };
            column = column.child(element);
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.separator)
            .child(div().h(px(44.)).flex_none())
            .child(column)
            .children(self.update_row(&theme, cx))
            .child(self.footer(&theme, cx))
    }
}
