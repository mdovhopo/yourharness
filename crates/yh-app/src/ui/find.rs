//! Find in thread (⌘F): rows whose text, tool input or result has what you type (from one
//! character, case and accents ignored), among the rows held, their matches marked in the rows
//! (the current row's orange, the others yellow); ↩ and ⌘G go to the next one down, ⇧↩ and ⇧⌘G
//! up, round from either end; ⎋ closes. Past the oldest row held, the page before is read and
//! the next step looks there too.

use crate::state::workspace::Workspace;
use crate::theme::{space, text, Theme};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable, Icon, IconName, Sizable};
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
        self.hits.clear();
        self.current = None;
        if let Some(t) = self.workspace.read(cx).selected_thread() {
            t.update(cx, |t, cx| {
                t.set_find_query(None);
                t.show_hit(None);
                cx.notify();
            });
        }
        cx.notify();
    }

    /// What's typed, matched without case or accents, from one character.
    fn query(&self, cx: &Context<Self>) -> String {
        yh_core::find::query(&self.input.read(cx).value())
    }

    /// New matches; stays on the row it was on if that still matches, else the newest.
    pub fn search(&mut self, cx: &mut Context<Self>) {
        let q = self.query(cx);
        let Some(thread) = self.workspace.read(cx).selected_thread() else { return };
        let was = self.current.and_then(|i| self.hits.get(i).copied());
        self.hits = if q.is_empty() { vec![] } else { thread.read(cx).find(&q) };
        self.current = was.and_then(|p| self.hits.iter().position(|h| *h == p)).or(self.hits.len().checked_sub(1));
        let hit = self.current.map(|i| self.hits[i]);
        thread.update(cx, |t, cx| {
            t.set_find_query(Some(q));
            t.show_hit(hit);
            cx.notify();
        });
        cx.notify();
    }

    /// Find Next and Previous have somewhere to go (the menu's items are on).
    pub fn has_hits(&self) -> bool {
        self.open && !self.hits.is_empty()
    }

    /// `newer` (↩, ⌘G): to the next match down; else (⇧↩, ⇧⌘G) up. Round from either end;
    /// past the oldest row held, the page before is read first and looked through too.
    pub fn step(&mut self, newer: bool, cx: &mut Context<Self>) {
        let Some(thread) = self.workspace.read(cx).selected_thread() else { return };
        // Rows came (an earlier page, a new reply) since the last look.
        let was = self.current.and_then(|i| self.hits.get(i).copied());
        let q = self.query(cx);
        if !q.is_empty() {
            self.hits = thread.read(cx).find(&q);
            self.current = was.and_then(|p| self.hits.iter().position(|h| *h == p)).or(self.hits.len().checked_sub(1));
        }
        if self.hits.is_empty() {
            return;
        }
        let last = self.hits.len() - 1;
        let next = match (self.current, newer) {
            (Some(0), false) if thread.read(cx).before > 0 => {
                // Nothing older held: read the page before; the next step finds there too.
                let ws = self.workspace.clone();
                ws.update(cx, |ws, cx| ws.load_earlier(&thread, cx));
                return;
            }
            (Some(0), false) | (None, false) => last,
            (Some(i), false) => i - 1,
            (Some(i), true) if i >= last => 0,
            (Some(i), true) => i + 1,
            (None, true) => 0,
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
            None if !self.input.read(cx).value().trim().is_empty() => "No matches".into(),
            None => String::new(),
        };
        let has_hits = !self.hits.is_empty();
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .h(px(40.))
            .px(px(space::XL - 4.))
            .border_b_1()
            .border_color(theme.separator)
            .on_key_down(cx.listener(|this, e: &gpui_kit::KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    this.close(cx);
                }
            }))
            .child(div().w(px(300.)).mr(px(space::S)).child(Input::new(&self.input).small().prefix(Icon::new(IconName::Search).size(px(13.)).text_color(theme.tertiary))))
            .child(
                Button::new("find-previous")
                    .small()
                    .ghost()
                    .icon(Icon::new(IconName::ChevronUp).size(px(14.)).text_color(theme.secondary))
                    .tooltip("Previous Match (⇧⌘G)")
                    .disabled(!has_hits)
                    .on_click(cx.listener(|this, _, _, cx| this.step(false, cx))),
            )
            .child(
                Button::new("find-next")
                    .small()
                    .ghost()
                    .icon(Icon::new(IconName::ChevronDown).size(px(14.)).text_color(theme.secondary))
                    .tooltip("Next Match (⌘G)")
                    .disabled(!has_hits)
                    .on_click(cx.listener(|this, _, _, cx| this.step(true, cx))),
            )
            .child(div().flex_1().pl(px(space::S)).text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(label))
            .child(
                Button::new("find-close")
                    .small()
                    .ghost()
                    .icon(crate::ui::icons::x().size(px(14.)).text_color(theme.secondary))
                    .tooltip("Close (⎋)")
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
            )
    }
}
