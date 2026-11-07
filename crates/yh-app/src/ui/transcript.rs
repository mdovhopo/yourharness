//! A thread's transcript: GPUI's `list`, bottom-aligned so it follows the end. It redraws when
//! its thread changes; a row is measured again only when the thread splices it.

use crate::probe;
use crate::state::thread::ThreadState;
use crate::state::workspace::Workspace;
use crate::theme::Theme;
use crate::ui::rows;
use gpui_kit::{div, list, prelude::*, px, Context, Entity, Subscription, Window};
use std::sync::Arc;
use yh_core::links::{Linker, Repo};

pub struct TranscriptView {
    pub thread: Entity<ThreadState>,
    workspace: Entity<Workspace>,
    /// The thread was running when last looked: its files are read again when it stops.
    was_running: bool,
    _observe: Subscription,
}

impl TranscriptView {
    pub fn new(thread: Entity<ThreadState>, workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&thread, |this, thread, cx| {
            let running = thread.read(cx).running;
            if this.was_running && !running {
                this.load_links(cx);
            }
            this.was_running = running;
            cx.notify()
        });
        let was_running = thread.read(cx).running;
        let mut view = TranscriptView { thread, workspace, was_running, _observe: observe };
        view.load_links(cx);
        view
    }

    /// Reads what the thread's replies link against, off the main thread: its folder's files
    /// (a turn may have made some) and its repo.
    fn load_links(&mut self, cx: &mut Context<Self>) {
        let folder = self.workspace.read(cx).folder_of(self.thread.read(cx));
        let thread = self.thread.downgrade();
        cx.spawn(async move |_, cx| {
            let linker = cx
                .background_executor()
                .spawn(async move {
                    let path = std::path::Path::new(&folder);
                    let files = yh_core::files::list(path);
                    let repo = yh_core::forge::origin(path).map(|(host, path, gitlab)| Repo { host, path, gitlab });
                    Arc::new(Linker::new(&folder, repo, &files))
                })
                .await;
            let _ = thread.update(cx, |t, cx| {
                if t.linker.as_ref().map(|l| l.revision) != Some(linker.revision) {
                    t.linker = Some(linker);
                    cx.notify();
                }
            });
        })
        .detach();
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
