//! "Working 0:12": how long the open thread's turn has been going, over the bottom of its
//! transcript. A view of its own outside the transcript's, so its tick once a second redraws
//! this pill and the window's root, never the transcript list (a notify marks the view and its
//! ancestors to draw again, and the transcript is a cached sibling).

use crate::state::workspace::Workspace;
use crate::theme::Theme;
use gpui_kit::{div, prelude::*, px, Context, Entity, FontWeight, Window};
use std::time::Duration;

pub struct WorkingTimer {
    workspace: Entity<Workspace>,
    /// What it last drew, so a tick that changes nothing draws nothing.
    shown: Option<String>,
}

impl WorkingTimer {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let Ok(()) = this.update(cx, |this, cx| {
                    if this.text(cx) != this.shown {
                        cx.notify();
                    }
                }) else {
                    break;
                };
            }
        })
        .detach();
        WorkingTimer { workspace, shown: None }
    }

    /// "Working 0:12" while the open thread's turn goes on.
    fn text(&self, cx: &gpui_kit::App) -> Option<String> {
        let ws = self.workspace.read(cx);
        if ws.page.is_some() {
            return None;
        }
        let thread = ws.selected_thread()?;
        let t = thread.read(cx);
        if !t.running {
            return None;
        }
        let since = t.turn_started_at()?;
        Some(format!("Working {}", yh_core::clock::elapsed(yh_core::library::now_ms() / 1000.0 - since)))
    }
}

impl Render for WorkingTimer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        self.shown = self.text(cx);
        let Some(text) = self.shown.clone() else { return div().into_any_element() };
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .py(px(3.))
            .rounded(px(10.))
            .bg(theme.card)
            .border_1()
            .border_color(theme.separator)
            .text_size(px(11.5))
            .text_color(theme.secondary)
            .child(div().w(px(6.)).h(px(6.)).rounded_full().bg(theme.accent))
            .child(div().font_weight(FontWeight::MEDIUM).child(text))
            .into_any_element()
    }
}
