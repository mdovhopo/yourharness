//! ⌘K: threads by title, then what was said in them; a Claude Code session ID resumes that
//! session as a thread. ↑↓ choose, ↩ opens, ⎋ closes.

use crate::state::workspace::{session_id_in, Workspace};
use crate::theme::Theme;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::{div, prelude::*, px, Context, Entity, FontWeight, Subscription, Task, Window};

pub struct Spotlight {
    workspace: Entity<Workspace>,
    pub input: Entity<InputState>,
    pub open: bool,
    results: Vec<(String, String, String)>,
    index: usize,
    error: Option<String>,
    _search: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Spotlight {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search threads, or paste a Claude Code session ID"));
        let subscriptions = vec![cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::Change => this.query(window, cx),
            InputEvent::PressEnter { .. } => this.choose(this.index, cx),
            _ => {}
        })];
        Spotlight { workspace, input, open: false, results: vec![], index: 0, error: None, _search: None, _subscriptions: subscriptions }
    }

    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.error = None;
        self.input.update(cx, |i, cx| {
            i.focus(window, cx);
            i.select_all(window, cx);
        });
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
    }

    pub fn query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let q = self.input.read(cx).value().to_string();
        self.error = None;
        let task = self.workspace.update(cx, |ws, cx| ws.search(q, cx));
        self._search = Some(cx.spawn_in(window, async move |this, cx| {
            let results = task.await;
            let _ = this.update(cx, |this, cx| {
                this.results = results;
                this.index = 0;
                cx.notify();
            });
        }));
    }

    fn choose(&mut self, index: usize, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        if session_id_in(&text).is_some() {
            match self.workspace.update(cx, |ws, cx| ws.resume_session(&text, cx)) {
                Ok(()) => self.close(cx),
                Err(e) => {
                    self.error = Some(e);
                    cx.notify();
                }
            }
            return;
        }
        let Some((id, ..)) = self.results.get(index).cloned() else { return };
        self.workspace.update(cx, |ws, cx| ws.select(&id, cx));
        self.close(cx);
    }

    fn on_key(&mut self, e: &gpui_kit::KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let n = self.results.len().max(1);
        match e.keystroke.key.as_str() {
            "up" => self.index = (self.index + n - 1) % n,
            "down" => self.index = (self.index + 1) % n,
            "escape" => self.close(cx),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for Spotlight {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        if !self.open {
            return div();
        }
        let resuming = session_id_in(&self.input.read(cx).value()).is_some();
        let mut list = div().flex().flex_col().max_h(px(420.)).overflow_hidden();
        if resuming {
            list = list.child(div().px(px(12.)).py(px(8.)).text_color(theme.secondary).child("↩ Resume this Claude Code session as a thread"));
        }
        for (i, (_, title, snippet)) in self.results.iter().enumerate() {
            list = list.child(
                div()
                    .id(("spotlight", i))
                    .flex()
                    .flex_col()
                    .px(px(12.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .when(i == self.index, |d| d.bg(theme.selected))
                    .child(div().font_weight(FontWeight::MEDIUM).child(title.clone()))
                    .when(!snippet.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(theme.secondary).overflow_hidden().whitespace_nowrap().child(snippet.clone())))
                    .on_click(cx.listener(move |this, _, _, cx| this.choose(i, cx))),
            );
        }
        // Over the window, its top third, as Spotlight sits.
        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(80.))
            .bg(gpui_kit::black().opacity(0.15))
            .on_mouse_down(gpui_kit::MouseButton::Left, cx.listener(|this, _, _, cx| this.close(cx)))
            .child(
                div()
                    .w(px(560.))
                    .h_auto()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(10.))
                    .rounded(px(14.))
                    .bg(theme.canvas)
                    .border_1()
                    .border_color(theme.separator)
                    .shadow_lg()
                    .capture_key_down(cx.listener(Self::on_key))
                    .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(Input::new(&self.input))
                    .when_some(self.error.clone(), |d, e| d.child(div().px(px(12.)).text_color(theme.error).child(e)))
                    .child(list),
            )
    }
}
