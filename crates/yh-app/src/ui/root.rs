//! The window: sidebar, the open thread's transcript, composer. It follows the workspace: a
//! thread opened makes or brings back its transcript (six stay made for going back and forth,
//! the rest let go of their rows) and moves the composer to it.

use crate::state::workspace::{Workspace, WorkspaceEvent};
use crate::theme::{Theme, UI};
use crate::ui::composer::Composer;
use crate::ui::find::FindBar;
use crate::ui::inspector::Inspector;
use crate::ui::memory::MemoryPage;
use crate::ui::spotlight::Spotlight;
use crate::ui::sidebar::Sidebar;
use crate::ui::transcript::TranscriptView;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::Sizable;
use gpui_kit::{
    actions, div, prelude::*, px, AnyView, App, Context, Entity, FocusHandle, Focusable, FontWeight, StyleRefinement,
    Subscription, Window,
};
use yh_core::claude::Decision;

actions!(
    harness,
    [
        Stop, Allow, Deny, Quit, NewThread, Find, FindNext, FindPrevious, OpenSettings, OpenSpotlight, ToggleInspector, OpenMemory, ToggleUsage,
        PreviousThread, NextThread, PreviousMessage, NextMessage, TopOfThread, EndOfThread, AddProject, ResumeSession, ExportState, ImportState,
        CheckForUpdates, UndoThreadAction, ToggleSidebar
    ]
);

/// Transcripts kept made for going back and forth.
const KEPT_TRANSCRIPTS: usize = 6;

/// The sidebar's width, as dragged (`sidebarWidth`), and its limits.
const SIDEBAR_DEFAULT: f32 = 260.;
const SIDEBAR_MIN: f32 = 200.;
const SIDEBAR_MAX: f32 = 440.;

pub struct RootView {
    /// The left sidebar: hidden with ⌃⌘S (`sidebarHidden`), and its width while resizing.
    sidebar_hidden: bool,
    sidebar_width: f32,
    resizing_sidebar: bool,
    save_frame: Option<gpui_kit::Task<()>>,
    pub workspace: Entity<Workspace>,
    pub sidebar: Entity<Sidebar>,
    pub composer: Entity<Composer>,
    pub find: Entity<FindBar>,
    pub spotlight: Entity<Spotlight>,
    pub inspector: Entity<Inspector>,
    pub mcp: Entity<crate::ui::mcp::McpSheet>,
    deck: Vec<(String, Entity<TranscriptView>)>,
    /// The Memory page when one is open (or was last: it's kept for going back to).
    memory: Option<Entity<MemoryPage>>,
    /// The open project's page (PRs and repos), kept while it's the same project.
    project: Option<Entity<crate::ui::project::ProjectPage>>,
    /// Usage, made the first time it's opened.
    usage: Option<Entity<crate::ui::usage::UsagePage>>,
    /// The open thread's tasks from memory, for its header: (title, state, steps done, steps).
    tasks: Vec<(String, String, usize, usize)>,
    tasks_for: Option<(String, u64)>,
    focus: FocusHandle,
    /// "Working 0:12" over the transcript's bottom: outside it, so its tick doesn't redraw it.
    working: Entity<crate::ui::working::WorkingTimer>,
    _subscriptions: Vec<Subscription>,
}

impl RootView {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let updater = cx.new(crate::state::updater::Updater::new);
        let sidebar = cx.new(|cx| Sidebar::new(workspace.clone(), updater.clone(), window, cx));
        let composer = cx.new(|cx| Composer::new(workspace.clone(), window, cx));
        let find = cx.new(|cx| FindBar::new(workspace.clone(), window, cx));
        let spotlight = cx.new(|cx| Spotlight::new(workspace.clone(), window, cx));
        let inspector = cx.new(|cx| Inspector::new(workspace.clone(), window, cx));
        let mcp = cx.new(|cx| crate::ui::mcp::McpSheet::new(workspace.clone(), window, cx));
        cx.set_global(crate::ui::settings::SettingsLinks { mcp: mcp.clone(), updater: updater.clone() });
        let links = inspector.downgrade();
        cx.set_global(crate::ui::inspector::FileLinks(std::rc::Rc::new(move |path, lines, cx| {
            let _ = links.update(cx, |i, cx| i.open_file(path, lines, cx));
        })));
        let subscriptions = vec![
            // The window's place and size, saved a moment after it stops changing, for next launch.
            cx.observe_window_bounds(window, |this, window, cx| {
                if std::env::var("YH_PERF").is_ok() || window.is_fullscreen() {
                    return;
                }
                let b = window.bounds();
                let frame = serde_json::json!([f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)]);
                this.save_frame = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
                    let _ = this.update(cx, |this, cx| this.workspace.read(cx).library.set_setting("windowFrame", &frame));
                }));
            }),
            cx.subscribe_in(&workspace, window, |this, _, event, window, cx| match event {
                WorkspaceEvent::Selected(id) => this.show(id, window, cx),
                WorkspaceEvent::Removed(id) => {
                    this.deck.retain(|(t, _)| t != id);
                    cx.notify();
                }
                WorkspaceEvent::PageOpened => this.open_page(window, cx),
                WorkspaceEvent::OpenMcp => this.mcp.update(cx, |m, cx| m.show(cx)),
            }),
            cx.observe(&workspace, |this, _, cx| {
                this.refresh_tasks(cx);
                cx.notify();
            }),
        ];
        let (sidebar_hidden, sidebar_width) = {
            let lib = &workspace.read(cx).library;
            (
                lib.setting("sidebarHidden").and_then(|v| v.as_bool()).unwrap_or(false),
                lib.setting("sidebarWidth").and_then(|v| v.as_f64()).map_or(SIDEBAR_DEFAULT, |w| (w as f32).clamp(SIDEBAR_MIN, SIDEBAR_MAX)),
            )
        };
        let working = cx.new(|cx| crate::ui::working::WorkingTimer::new(workspace.clone(), cx));
        RootView { sidebar_hidden, sidebar_width, resizing_sidebar: false, save_frame: None, workspace, sidebar, composer, find, spotlight, inspector, mcp, deck: vec![], memory: None, project: None, usage: None, tasks: vec![], tasks_for: None, focus: cx.focus_handle(), working, _subscriptions: subscriptions }
    }

    fn show(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.workspace.read(cx).thread(id) else { return };
        if let Some(i) = self.deck.iter().position(|(t, _)| t == id) {
            let slot = self.deck.remove(i);
            self.deck.push(slot);
        } else {
            thread.update(cx, |t, _| t.in_deck = true);
            let ws = self.workspace.clone();
            let view = cx.new(|cx| TranscriptView::new(thread.clone(), ws, cx));
            self.deck.push((id.to_string(), view));
            while self.deck.len() > KEPT_TRANSCRIPTS {
                let (old, _) = self.deck.remove(0);
                let ws = self.workspace.clone();
                if let Some(t) = ws.read(cx).thread(&old) {
                    ws.update(cx, |ws, cx| ws.release(&t, cx));
                }
            }
        }
        self.composer.update(cx, |c, cx| c.switch_to(id, window, cx));
        cx.notify();
    }

    fn open_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(crate::state::workspace::Page::Project(project)) = self.workspace.read(cx).page.clone() {
            if self.project.as_ref().is_none_or(|p| p.read(cx).project != project) {
                let ws = self.workspace.clone();
                self.project = Some(cx.new(|cx| crate::ui::project::ProjectPage::new(ws, project, window, cx)));
            } else if let Some(p) = &self.project {
                p.update(cx, |p, cx| p.refresh(false, cx));
            }
            cx.notify();
            return;
        }
        let Some(crate::state::workspace::Page::Memory(project)) = self.workspace.read(cx).page.clone() else {
            if self.workspace.read(cx).page == Some(crate::state::workspace::Page::Usage) && self.usage.is_none() {
                let ws = self.workspace.clone();
                self.usage = Some(cx.new(|cx| crate::ui::usage::UsagePage::new(ws, window, cx)));
            }
            cx.notify();
            return;
        };
        if self.memory.as_ref().is_none_or(|m| m.read(cx).project != project) {
            let ws = self.workspace.clone();
            self.memory = Some(cx.new(|cx| MemoryPage::new(ws, project, window, cx)));
        }
        cx.notify();
    }

    /// The open thread's tasks, read off the main thread when the thread or memory changes.
    fn refresh_tasks(&mut self, cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        let key = ws.selected.clone().map(|id| (id, ws.memory_revision));
        if key == self.tasks_for {
            return;
        }
        self.tasks_for = key.clone();
        let (Some((id, _)), Some(store)) = (key, ws.memory_store()) else {
            self.tasks.clear();
            return;
        };
        cx.spawn(async move |this, cx| {
            let tasks = cx
                .background_executor()
                .spawn(async move {
                    store
                        .tasks_of_thread(&id)
                        .into_iter()
                        .filter(|m| m.status == "active" && m.task_state.as_deref() != Some("done"))
                        .map(|m| (m.title.clone(), m.task_state.clone().unwrap_or_else(|| "active".into()), m.steps_done(), m.steps.len()))
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.tasks != tasks {
                    this.tasks = tasks;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn memory_page(&self, cx: &App) -> Option<Entity<MemoryPage>> {
        let Some(crate::state::workspace::Page::Memory(project)) = self.workspace.read(cx).page.clone() else { return None };
        self.memory.clone().filter(|m| m.read(cx).project == project)
    }

    pub fn transcript_view(&self, cx: &App) -> Option<Entity<TranscriptView>> {
        let id = self.workspace.read(cx).selected.clone()?;
        self.deck.iter().find(|(t, _)| *t == id).map(|(_, v)| v.clone())
    }

    fn on_stop(&mut self, _: &Stop, _: &mut Window, cx: &mut Context<Self>) {
        let ws = self.workspace.clone();
        if let Some(t) = ws.read(cx).selected_thread() {
            if t.read(cx).running {
                ws.update(cx, |ws, cx| ws.interrupt(&t, cx));
            }
        }
    }

    fn respond(&mut self, decision: Decision, cx: &mut Context<Self>) {
        let ws = self.workspace.clone();
        if let Some(t) = ws.read(cx).selected_thread() {
            ws.update(cx, |ws, cx| ws.respond(&t, decision, cx));
        }
    }

    fn end_sidebar_resize(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.resizing_sidebar) || self.sidebar_width == SIDEBAR_DEFAULT {
            self.workspace.read(cx).library.set_setting("sidebarWidth", &serde_json::Value::from(self.sidebar_width as f64));
            cx.notify();
        }
    }

    fn on_toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.dispatch_action(Box::new(ToggleSidebar), cx);
    }

    /// The thread above or below the open one in the sidebar.
    fn adjacent_thread(&mut self, step: isize, cx: &mut Context<Self>) {
        let order = self.sidebar.read(cx).thread_order(cx);
        let current = self.workspace.read(cx).selected.clone();
        let i = current.and_then(|c| order.iter().position(|t| *t == c));
        let next = match i {
            Some(i) => (i as isize + step).clamp(0, order.len() as isize - 1) as usize,
            None => 0,
        };
        if let Some(id) = order.get(next).cloned() {
            self.workspace.update(cx, |ws, cx| ws.select(&id, cx));
        }
    }

    /// Jumps between your messages in the open thread: -1 the one above, 1 the one below,
    /// i64::MIN the top, i64::MAX the end.
    fn jump(&mut self, to: i64, cx: &mut Context<Self>) {
        let Some(thread) = self.workspace.read(cx).selected_thread() else { return };
        if to == i64::MIN && thread.read(cx).before > 0 {
            // Earlier pages first: the top is the thread's start.
            let ws = self.workspace.clone();
            ws.update(cx, |ws, cx| ws.load_earlier(&thread, cx));
        }
        thread.update(cx, |t, cx| {
            let count = t.items.len();
            if count == 0 {
                return;
            }
            let top = t.list.logical_scroll_top().item_ix;
            let users: Vec<usize> = t.items.iter().enumerate().filter(|(_, i)| i.ty == crate::state::thread::ItemType::User).map(|(n, _)| n).collect();
            let target = match to {
                i64::MIN => 0,
                i64::MAX => count - 1,
                -1 => users.iter().rev().find(|&&n| n < top).copied().unwrap_or(0),
                _ => users.iter().find(|&&n| n > top).copied().unwrap_or(count - 1),
            };
            if to == i64::MAX {
                t.list.scroll_to_end();
            } else {
                t.list.scroll_to(gpui_kit::ListOffset { item_ix: target, offset_in_item: gpui_kit::px(0.) });
            }
            cx.notify();
        });
    }

    fn on_new_thread(&mut self, _: &NewThread, _: &mut Window, cx: &mut Context<Self>) {
        let ws = self.workspace.clone();
        let project = ws
            .read(cx)
            .selected_thread()
            .map(|t| t.read(cx).meta.project_id.clone())
            .or_else(|| ws.read(cx).projects.first().map(|p| p.id.clone()));
        if let Some(project) = project {
            ws.update(cx, |ws, cx| ws.new_thread(&project, cx));
        }
    }
}

impl Focusable for RootView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let project_page = match &self.workspace.read(cx).page {
            Some(crate::state::workspace::Page::Project(p)) => self.project.clone().filter(|e| e.read(cx).project == *p),
            _ => None,
        };
        let page: Option<AnyView> = project_page.map(AnyView::from).or_else(|| self.memory_page(cx).map(AnyView::from)).or_else(|| {
            (self.workspace.read(cx).page == Some(crate::state::workspace::Page::Usage)).then(|| self.usage.clone().map(AnyView::from)).flatten()
        });
        let selected = if page.is_some() { None } else { self.workspace.read(cx).selected_thread() };
        let title = selected.as_ref().map(|t| t.read(cx).title.clone()).unwrap_or_else(|| "YourHarness".into());
        let transcript = self.transcript_view(cx);
        // The browser's page shows only over its panel; anything else hides it.
        let ws = self.workspace.read(cx);
        if !(selected.is_some() && ws.shows_inspector && ws.panel == crate::state::workspace::Panel::Browser) {
            crate::browser::Browsers::show(None);
        }
        let tasks = if selected.is_some() { self.tasks.clone() } else { vec![] };
        div()
            .id("root")
            .key_context("Harness")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_stop))
            .on_action(cx.listener(|this, _: &Allow, _, cx| this.respond(Decision::allow(), cx)))
            .on_action(cx.listener(|this, _: &Deny, _, cx| this.respond(Decision::deny(), cx)))
            .on_action(cx.listener(Self::on_new_thread))
            .on_action(cx.listener(|this, _: &OpenSpotlight, window, cx| this.spotlight.update(cx, |s, cx| s.show(window, cx))))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| crate::ui::settings::open(this.workspace.clone(), window, cx)))
            .on_action(cx.listener(|this, _: &Find, window, cx| this.find.update(cx, |f, cx| f.show(window, cx))))
            .on_action(cx.listener(|this, _: &ToggleInspector, _, cx| {
                this.workspace.update(cx, |ws, cx| {
                    let on = !ws.shows_inspector;
                    ws.set_shows_inspector(on, cx)
                })
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.sidebar_hidden = !this.sidebar_hidden;
                this.workspace.read(cx).library.set_setting("sidebarHidden", &serde_json::Value::Bool(this.sidebar_hidden));
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, e: &gpui_kit::MouseMoveEvent, _, cx| {
                if this.resizing_sidebar {
                    if e.dragging() {
                        this.sidebar_width = f32::from(e.position.x).clamp(SIDEBAR_MIN, SIDEBAR_MAX);
                        cx.notify();
                    } else {
                        this.end_sidebar_resize(cx);
                    }
                }
            }))
            .on_mouse_up(gpui_kit::MouseButton::Left, cx.listener(|this, _, _, cx| this.end_sidebar_resize(cx)))
            .on_action(cx.listener(|this, _: &UndoThreadAction, _, cx| this.workspace.update(cx, |ws, cx| ws.undo(cx))))
            .on_action(cx.listener(|this, _: &PreviousThread, _, cx| this.adjacent_thread(-1, cx)))
            .on_action(cx.listener(|this, _: &NextThread, _, cx| this.adjacent_thread(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousMessage, _, cx| this.jump(-1, cx)))
            .on_action(cx.listener(|this, _: &NextMessage, _, cx| this.jump(1, cx)))
            .on_action(cx.listener(|this, _: &TopOfThread, _, cx| this.jump(i64::MIN, cx)))
            .on_action(cx.listener(|this, _: &EndOfThread, _, cx| this.jump(i64::MAX, cx)))
            .on_action(cx.listener(|this, _: &AddProject, _, cx| this.sidebar.update(cx, |s, cx| s.add_project(cx))))
            .on_action(cx.listener(|this, _: &ResumeSession, window, cx| this.spotlight.update(cx, |s, cx| s.show(window, cx))))
            .on_action(cx.listener(|this, _: &ExportState, _, cx| crate::ui::state_transfer::export(this.workspace.clone(), cx)))
            .on_action(cx.listener(|this, _: &ImportState, window, cx| crate::ui::state_transfer::import(this.workspace.clone(), window, cx)))
            .on_action(cx.listener(|_, _: &CheckForUpdates, _, cx| {
                if let Some(links) = cx.try_global::<crate::ui::settings::SettingsLinks>() {
                    let u = links.updater.clone();
                    u.update(cx, |u, cx| u.check(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleUsage, _, cx| this.workspace.update(cx, |ws, cx| ws.toggle_usage(cx))))
            .on_action(cx.listener(|this, _: &OpenMemory, _, cx| {
                let ws = this.workspace.clone();
                let project = ws
                    .read(cx)
                    .selected_thread()
                    .map(|t| t.read(cx).meta.project_id.clone())
                    .or_else(|| ws.read(cx).projects.first().map(|p| p.id.clone()));
                if let Some(project) = project {
                    ws.update(cx, |ws, cx| ws.open_memory(&project, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &FindNext, _, cx| this.find.update(cx, |f, cx| f.step(true, cx))))
            .on_action(cx.listener(|this, _: &FindPrevious, _, cx| this.find.update(cx, |f, cx| f.step(false, cx))))
            .size_full()
            .relative()
            .flex()
            .font_family(UI)
            .text_size(px(13.))
            .line_height(px(19.5))
            .text_color(theme.text)
            .bg(theme.canvas)
            .when(!self.sidebar_hidden, |d| {
                let width = self.sidebar_width;
                d.child(AnyView::from(self.sidebar.clone()).cached(StyleRefinement::default().h_full().w(px(width)).flex_none())).child(
                    // The sidebar's edge: drag to resize, double-click for the default width.
                    div()
                        .id("sidebar-resize")
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(width - 3.))
                        .w(px(6.))
                        .cursor_col_resize()
                        .on_mouse_down(gpui_kit::MouseButton::Left, cx.listener(|this, e: &gpui_kit::MouseDownEvent, _, cx| {
                            if e.click_count == 2 {
                                this.sidebar_width = SIDEBAR_DEFAULT;
                                this.end_sidebar_resize(cx);
                            } else {
                                this.resizing_sidebar = true;
                            }
                            cx.stop_propagation();
                        })),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .when_some(page, |d, page| d.child(div().flex_1().min_h_0().child(page)))
                    .when(selected.is_some(), |d| {
                        d.child(
                            div()
                                .h(px(44.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .px(px(20.))
                                .when(self.sidebar_hidden, |d| d.pl(px(84.)).child(
                                    gpui_kit::component::button::Button::new("show-sidebar").small().ghost().label("☰").tooltip("Show Sidebar (⌃⌘S)").on_click(cx.listener(|this, _, window, cx| this.on_toggle_sidebar(window, cx))),
                                ))
                                .border_b_1()
                                .border_color(theme.separator)
                                .child(div().flex_1().min_w_0().font_weight(FontWeight::SEMIBOLD).overflow_hidden().whitespace_nowrap().text_ellipsis().child(title))
                                .children(tasks.into_iter().take(2).map(|(title, state, done, steps)| {
                                    // A task the agent tracks for this thread, from memory.
                                    div()
                                        .flex_none()
                                        .max_w(px(280.))
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .px(px(8.))
                                        .py(px(2.))
                                        .rounded(px(10.))
                                        .bg(theme.card)
                                        .text_size(px(11.))
                                        .text_color(theme.secondary)
                                        .child(div().w(px(6.)).h(px(6.)).rounded_full().bg(if state == "blocked" { theme.warning } else { theme.accent }))
                                        .child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().child(title))
                                        .when(steps > 0, |d| d.child(div().flex_none().child(format!("{done}/{steps}"))))
                                })),
                        )
                        .child(self.find.clone())
                        .child(match transcript {
                            // Cached: typing in the composer or a sidebar change doesn't redraw the
                            // transcript; its own thread changing does.
                            Some(view) => div()
                                .flex_1()
                                .min_h_0()
                                .relative()
                                .child(AnyView::from(view).cached(StyleRefinement::default().size_full()))
                                .child(div().absolute().bottom(px(6.)).right(px(28.)).child(self.working.clone()))
                                .into_any_element(),
                            None => div().flex_1().into_any_element(),
                        })
                        // Not cached: a cached view needs a fixed size, and the composer grows
                        // with its cards. Its render is a handful of elements.
                        .child(div().flex_none().w_full().child(self.composer.clone()))
                    })
                    .when(selected.is_none() && self.workspace.read(cx).page.is_none(), |d| {
                        d.child(
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(theme.secondary)
                                .child("Pick a thread, or start one with + next to a project."),
                        )
                    }),
            )
            .when(selected.is_some() && self.workspace.read(cx).shows_inspector, |d| {
                // Cached: it redraws when its tab's data changes, not with the transcript.
                d.child(AnyView::from(self.inspector.clone()).cached(StyleRefinement::default().h_full().w(px(crate::ui::inspector::WIDTH)).flex_none()))
            })
            .when_some(self.workspace.read(cx).undo_notice().map(str::to_string), |d, message| {
                // What the last thread action did, with a way back.
                let ws = self.workspace.clone();
                d.child(
                    div().absolute().bottom(px(16.)).left(px(16.)).child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .px(px(12.))
                            .py(px(8.))
                            .rounded(px(10.))
                            .bg(theme.canvas)
                            .border_1()
                            .border_color(theme.separator)
                            .shadow_lg()
                            .child(div().max_w(px(260.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(message))
                            .child(gpui_kit::component::button::Button::new("undo").small().label("Undo ⌘Z").on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.undo(cx)))),
                    ),
                )
            })
            .child(self.spotlight.clone())
            .child(self.mcp.clone())
    }
}
