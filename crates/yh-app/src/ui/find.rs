//! Find in thread (⌘F): rows whose text, tool input or result has what you type, among the rows
//! held; ↩ steps to older ones, ⇧↩ to newer, ⎋ closes. Past the oldest row held, the page before
//! is read and the next ↩ looks there too.

use crate::state::workspace::Workspace;
use crate::theme::Theme;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::Sizable;
use gpui_kit::{div, prelude::*, px, Context, Entity, Subscription, Window};

pub struct FindBar {
    workspace: Entity<Workspace>,
    pub input: Entity<InputState>,
    pub open: bool,
    hits: Vec<i64>,
    current: Option<usize>,
    _subscriptions: Vec<Subscription>,
}

impl FindBar {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in thread"));
        let subscriptions = vec![cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| match event {
            InputEvent::Change => this.search(cx),
            InputEvent::PressEnter { shift, .. } => this.step(!*shift, cx),
            _ => {}
        })];
        FindBar { workspace, input, open: false, hits: vec![], current: None, _subscriptions: subscriptions }
    }

    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.input.update(cx, |i, cx| {
            i.focus(window, cx);
            i.select_all(window, cx);
        });
        self.search(cx);
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        if let Some(t) = self.workspace.read(cx).selected_thread() {
            t.update(cx, |t, cx| {
                t.show_hit(None);
                cx.notify();
            });
        }
        cx.notify();
    }

    pub fn search(&mut self, cx: &mut Context<Self>) {
        let q = self.input.read(cx).value().trim().to_lowercase();
        let Some(thread) = self.workspace.read(cx).selected_thread() else { return };
        self.hits = if q.chars().count() < 2 { vec![] } else { thread.read(cx).find(&q) };
        self.current = self.hits.len().checked_sub(1);
        let hit = self.current.map(|i| self.hits[i]);
        thread.update(cx, |t, cx| {
            t.show_hit(hit);
            cx.notify();
        });
        cx.notify();
    }

    /// `back`: to the older match.
    pub fn step(&mut self, back: bool, cx: &mut Context<Self>) {
        let Some(thread) = self.workspace.read(cx).selected_thread() else { return };
        let next = match (self.current, back) {
            (Some(0), true) | (None, _) => {
                // Nothing older held: read the page before; the next ↩ finds there too.
                if back && thread.read(cx).before > 0 {
                    let ws = self.workspace.clone();
                    ws.update(cx, |ws, cx| ws.load_earlier(&thread, cx));
                }
                return;
            }
            (Some(i), true) => i - 1,
            (Some(i), false) => (i + 1).min(self.hits.len() - 1),
        };
        self.current = Some(next);
        let hit = self.hits[next];
        thread.update(cx, |t, cx| {
            t.show_hit(Some(hit));
            cx.notify();
        });
        cx.notify();
    }
}

impl Render for FindBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        if !self.open {
            return div();
        }
        let label = match self.current {
            Some(i) => format!("{} of {}", i + 1, self.hits.len()),
            None if self.input.read(cx).value().trim().chars().count() >= 2 => "No matches".into(),
            None => String::new(),
        };
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(20.))
            .py(px(6.))
            .border_b_1()
            .border_color(theme.separator)
            .bg(theme.card)
            .on_key_down(cx.listener(|this, e: &gpui_kit::KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    this.close(cx);
                }
            }))
            .child(div().w(px(320.)).child(Input::new(&self.input).small()))
            .child(div().text_size(px(12.)).text_color(theme.secondary).child(label))
    }
}
