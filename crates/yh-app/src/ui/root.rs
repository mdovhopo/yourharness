//! The window: sidebar, the open thread's transcript, composer. It follows the workspace: a
//! thread opened makes or brings back its transcript (six stay made for going back and forth,
//! the rest let go of their rows) and moves the composer to it.

use crate::state::workspace::{Workspace, WorkspaceEvent};
use crate::theme::{Theme, UI};
use crate::ui::composer::Composer;
use crate::ui::find::FindBar;
use crate::ui::sidebar::Sidebar;
use crate::ui::transcript::TranscriptView;
use gpui_kit::{
    actions, div, prelude::*, px, AnyView, App, Context, Entity, FocusHandle, Focusable, FontWeight, StyleRefinement,
    Subscription, Window,
};
use yh_core::claude::Decision;

actions!(harness, [Stop, Allow, Deny, Quit, NewThread, Find, FindNext, FindPrevious, OpenSettings]);

/// Transcripts kept made for going back and forth.
const KEPT_TRANSCRIPTS: usize = 6;

pub struct RootView {
    pub workspace: Entity<Workspace>,
    pub sidebar: Entity<Sidebar>,
    pub composer: Entity<Composer>,
    pub find: Entity<FindBar>,
    deck: Vec<(String, Entity<TranscriptView>)>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl RootView {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sidebar = cx.new(|cx| Sidebar::new(workspace.clone(), window, cx));
        let composer = cx.new(|cx| Composer::new(workspace.clone(), window, cx));
        let find = cx.new(|cx| FindBar::new(workspace.clone(), window, cx));
        let subscriptions = vec![
            cx.subscribe_in(&workspace, window, |this, _, event, window, cx| match event {
                WorkspaceEvent::Selected(id) => this.show(id, window, cx),
                WorkspaceEvent::Removed(id) => {
                    this.deck.retain(|(t, _)| t != id);
                    cx.notify();
                }
            }),
            cx.observe(&workspace, |_, _, cx| cx.notify()),
        ];
        RootView { workspace, sidebar, composer, find, deck: vec![], focus: cx.focus_handle(), _subscriptions: subscriptions }
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
        let selected = self.workspace.read(cx).selected_thread();
        let title = selected.as_ref().map(|t| t.read(cx).title.clone()).unwrap_or_else(|| "YourHarness".into());
        let transcript = self.transcript_view(cx);
        div()
            .id("root")
            .key_context("Harness")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_stop))
            .on_action(cx.listener(|this, _: &Allow, _, cx| this.respond(Decision::allow(), cx)))
            .on_action(cx.listener(|this, _: &Deny, _, cx| this.respond(Decision::deny(), cx)))
            .on_action(cx.listener(Self::on_new_thread))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| crate::ui::settings::open(this.workspace.clone(), window, cx)))
            .on_action(cx.listener(|this, _: &Find, window, cx| this.find.update(cx, |f, cx| f.show(window, cx))))
            .on_action(cx.listener(|this, _: &FindNext, _, cx| this.find.update(cx, |f, cx| f.step(true, cx))))
            .on_action(cx.listener(|this, _: &FindPrevious, _, cx| this.find.update(cx, |f, cx| f.step(false, cx))))
            .size_full()
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
                    .child(
                        div()
                            .h(px(44.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .px(px(20.))
                            .border_b_1()
                            .border_color(theme.separator)
                            .child(div().font_weight(FontWeight::SEMIBOLD).overflow_hidden().whitespace_nowrap().child(title)),
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
                        None => div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(theme.secondary)
                            .child("Pick a thread, or start one with + next to a project.")
                            .into_any_element(),
                    })
                    .when(selected.is_some(), |d| {
                        // Not cached: a cached view needs a fixed size, and the composer grows
                        // with its cards. Its render is a handful of elements.
                        d.child(div().flex_none().w_full().child(self.composer.clone()))
                    }),
            )
    }
}
