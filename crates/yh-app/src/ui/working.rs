//! Over the bottom of the open thread's transcript: "Working 0:12", how long its turn has been
//! going, and the jump buttons (top, your previous message, your next one, the end). A view of
//! its own outside the transcript's, so its tick once a second and the buttons coming and
//! going as you scroll redraw this and the window's root, never the transcript list (a notify
//! marks the view and its ancestors to draw again, and the transcript is a cached sibling).

use crate::state::thread::{ItemType, ThreadState};
use crate::state::workspace::Workspace;
use crate::theme::Theme;
use gpui_kit::component::{Icon, IconName};
use gpui_kit::{div, prelude::*, px, Action, App, Context, Entity, FontWeight, Global, WeakEntity, Window};
use std::time::Duration;

/// Where the open transcript can go: (top, previous message, next message and the end).
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Jumps {
    pub top: bool,
    pub previous: bool,
    pub next: bool,
}

impl Jumps {
    /// From the thread's list as last laid out.
    pub fn of(t: &ThreadState) -> Jumps {
        let count = t.items.len();
        if !t.loaded || count == 0 {
            return Jumps::default();
        }
        let taller = t.list.max_offset_for_scrollbar().y > px(1.);
        let top = t.list.logical_scroll_top();
        // At the end, following it, the list has no top row of its own (it says the count):
        // there's somewhere to go up when it doesn't all fit, and your messages are up there.
        if top.item_ix >= count {
            let up = taller || t.before > 0;
            return Jumps { top: up, previous: up && t.items.iter().any(|i| i.ty == ItemType::User), next: false };
        }
        let at_top = top.item_ix == 0 && top.offset_in_item <= px(0.);
        let next = !t.list.is_following_tail()
            && match t.list.bounds_for_item(count - 1) {
                Some(last) => last.bottom() > t.list.viewport_bounds().bottom() + px(1.),
                None => top.item_ix < count - 1,
            };
        Jumps {
            top: !at_top || t.before > 0,
            previous: t.items[..top.item_ix.min(count)].iter().any(|i| i.ty == ItemType::User),
            next,
        }
    }
}

/// The thread's list scrolled: the buttons are looked at again (set by the timer view).
struct ScrollWatch(WeakEntity<WorkingTimer>);

impl Global for ScrollWatch {}

/// Called by each thread's list when it scrolls: the open one's buttons may come or go.
/// Deferred: the list calls it with its state borrowed, and the buttons read that state.
pub fn scrolled(cx: &mut App) {
    let Some(view) = cx.try_global::<ScrollWatch>().and_then(|w| w.0.upgrade()) else { return };
    cx.defer(move |cx| {
        view.update(cx, |this, cx| {
            if this.jumps(cx) != this.shown_jumps {
                cx.notify();
            }
        })
    });
}

pub struct WorkingTimer {
    workspace: Entity<Workspace>,
    /// What it last drew, so a tick that changes nothing draws nothing.
    shown: Option<String>,
    shown_jumps: Jumps,
}

impl WorkingTimer {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        cx.set_global(ScrollWatch(cx.entity().downgrade()));
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
        WorkingTimer { workspace, shown: None, shown_jumps: Jumps::default() }
    }

    /// "Working 0:12" while the open thread's turn goes on.
    fn text(&self, cx: &App) -> Option<String> {
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

    fn jumps(&self, cx: &App) -> Jumps {
        let ws = self.workspace.read(cx);
        if ws.page.is_some() {
            return Jumps::default();
        }
        ws.selected_thread().map(|t| Jumps::of(t.read(cx))).unwrap_or_default()
    }
}

/// A small round button floating over the transcript: solid, a hairline edge, a soft shadow.
fn jump_button(id: &'static str, name: IconName, tip: &'static str, action: Box<dyn Action>, theme: &Theme) -> impl IntoElement {
    div()
        .id(id)
        .w(px(30.))
        .h(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(theme.canvas)
        .border_1()
        .border_color(theme.separator)
        .shadow_sm()
        .cursor_pointer()
        .hover(|s| s.bg(theme.card))
        .child(Icon::new(name).size(px(15.)).text_color(theme.secondary))
        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx))
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
}

impl Render for WorkingTimer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        self.shown = self.text(cx);
        self.shown_jumps = self.jumps(cx);
        let j = self.shown_jumps;
        use crate::ui::root::{EndOfThread, NextMessage, PreviousMessage, TopOfThread};
        div()
            .flex()
            .flex_col()
            .items_end()
            .gap(px(8.))
            .when(j.top || j.previous || j.next, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .when(j.top, |d| d.child(jump_button("jump-top", IconName::ArrowUp, "The top of the thread (⇧⌘[)", Box::new(TopOfThread), &theme)))
                        .when(j.previous, |d| d.child(jump_button("jump-previous", IconName::ChevronUp, "Your previous message (⌘[)", Box::new(PreviousMessage), &theme)))
                        .when(j.next, |d| {
                            d.child(jump_button("jump-next", IconName::ChevronDown, "Your next message, or the end (⌘])", Box::new(NextMessage), &theme))
                                .child(jump_button("jump-end", IconName::ArrowDown, "The end of the thread (⇧⌘])", Box::new(EndOfThread), &theme))
                        }),
                )
            })
            .when_some(self.shown.clone(), |d, text| {
                d.child(
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
                        .child(div().font_weight(FontWeight::MEDIUM).child(text)),
                )
            })
    }
}
