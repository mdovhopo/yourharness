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
use gpui_kit::{
    actions, div, prelude::*, px, AnyView, App, Context, Entity, FocusHandle, Focusable, FontWeight, StyleRefinement,
    Subscription, Window,
};
use yh_core::claude::Decision;

actions!(harness, [Stop, Allow, Deny, Quit, NewThread, Find, FindNext, FindPrevious, OpenSettings, OpenSpotlight, ToggleInspector, OpenMemory]);

/// Transcripts kept made for going back and forth.
const KEPT_TRANSCRIPTS: usize = 6;

pub struct RootView {
    pub workspace: Entity<Workspace>,
    pub sidebar: Entity<Sidebar>,
    pub composer: Entity<Composer>,
    pub find: Entity<FindBar>,
    pub spotlight: Entity<Spotlight>,
    pub inspector: Entity<Inspector>,
    deck: Vec<(String, Entity<TranscriptView>)>,
    /// The Memory page when one is open (or was last: it's kept for going back to).
    memory: Option<Entity<MemoryPage>>,
    /// The open thread's tasks from memory, for its header: (title, state, steps done, steps).
    tasks: Vec<(String, String, usize, usize)>,
    tasks_for: Option<(String, u64)>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl RootView {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sidebar = cx.new(|cx| Sidebar::new(workspace.clone(), window, cx));
        let composer = cx.new(|cx| Composer::new(workspace.clone(), window, cx));
        let find = cx.new(|cx| FindBar::new(workspace.clone(), window, cx));
        let spotlight = cx.new(|cx| Spotlight::new(workspace.clone(), window, cx));
        let inspector = cx.new(|cx| Inspector::new(workspace.clone(), window, cx));
        let links = inspector.downgrade();
        cx.set_global(crate::ui::inspector::FileLinks(std::rc::Rc::new(move |path, lines, cx| {
            let _ = links.update(cx, |i, cx| i.open_file(path, lines, cx));
        })));
        let subscriptions = vec![
            cx.subscribe_in(&workspace, window, |this, _, event, window, cx| match event {
                WorkspaceEvent::Selected(id) => this.show(id, window, cx),
                WorkspaceEvent::Removed(id) => {
                    this.deck.retain(|(t, _)| t != id);
                    cx.notify();
                }
                WorkspaceEvent::PageOpened => this.open_page(window, cx),
            }),
            cx.observe(&workspace, |this, _, cx| {
                this.refresh_tasks(cx);
                cx.notify();
            }),
        ];
        RootView { workspace, sidebar, composer, find, spotlight, inspector, deck: vec![], memory: None, tasks: vec![], tasks_for: None, focus: cx.focus_handle(), _subscriptions: subscriptions }
    }

    fn show(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.workspace.read(cx).thread(id) else { return };
        if let Some(i) = self.deck.iter().position(|(t, _)| t == id) {
            let slot = self.deck.remove(i);
            self.deck.push(slot);
        } else {
            thread.update(cx, |t, _| t.in_deck = true);
            let view = cx.new(|cx| TranscriptView::new(thread.clone(), cx));
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
        let Some(project) = self.workspace.read(cx).page.clone() else { return };
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
        let project = self.workspace.read(cx).page.clone()?;
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
        let page = self.memory_page(cx);
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
            .child(AnyView::from(self.sidebar.clone()).cached(StyleRefinement::default().h_full().w(px(260.)).flex_none()))
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
                                .into_any_element(),
                            None => div().flex_1().into_any_element(),
                        })
                        // Not cached: a cached view needs a fixed size, and the composer grows
                        // with its cards. Its render is a handful of elements.
                        .child(div().flex_none().w_full().child(self.composer.clone()))
                    })
                    .when(selected.is_none() && self.memory_page(cx).is_none(), |d| {
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
            .child(self.spotlight.clone())
    }
}
