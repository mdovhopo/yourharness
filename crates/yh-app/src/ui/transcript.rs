//! A thread's transcript: GPUI's `list`, bottom-aligned so it follows the end. It redraws when
//! its thread changes; a row is measured again only when the thread splices it.

use crate::probe;
use crate::state::thread::ThreadState;
use crate::theme::Theme;
use crate::ui::rows;
use gpui_kit::{div, list, prelude::*, px, Context, Entity, Subscription, Window};

pub struct TranscriptView {
    pub thread: Entity<ThreadState>,
    _observe: Subscription,
}

impl TranscriptView {
    pub fn new(thread: Entity<ThreadState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&thread, |_, _, cx| cx.notify());
        TranscriptView { thread, _observe: observe }
    }
}

impl Render for TranscriptView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let t = self.thread.read(cx);
        if !t.loaded {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.secondary)
                .child("Loading…")
                .into_any_element();
        }
        let thread = self.thread.clone();
        let mut root = div().size_full().relative();
        if probe::enabled() {
            root = root.child(probe::tracker("transcript".into()));
        }
        root.child(list(t.list.clone(), move |ix, window, cx| rows::render_item(&thread, ix, window, cx)).size_full().pb(px(16.)))
            .into_any_element()
    }
}
