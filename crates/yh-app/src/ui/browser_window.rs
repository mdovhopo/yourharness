//! A thread's browser in a window of its own (the Swift app's pop-out): the same pages, moved
//! from over the right sidebar to over this window, which the sidebar then points to. Closing
//! the window puts them back.

use crate::state::workspace::Workspace;
use crate::theme::Theme;
use crate::ui::browser_panel::BrowserPanel;
use gpui_kit::{div, prelude::*, px, AnyWindowHandle, App, Context, Entity, WeakEntity, Window};
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// Each popped-out thread's window and view.
    static WINDOWS: RefCell<HashMap<String, (AnyWindowHandle, WeakEntity<BrowserWindow>)>> = RefCell::new(HashMap::new());
}

pub struct BrowserWindow {
    /// The same panel as the right sidebar's, drawing the page over this window.
    panel: Entity<BrowserPanel>,
}

/// Whether the thread's browser is in its own window.
pub fn is_open(thread: &str) -> bool {
    WINDOWS.with(|w| w.borrow().contains_key(thread))
}

/// Brings the thread's browser window to the front (not in a background test run).
pub fn activate(thread: &str, cx: &mut App) {
    let Some((handle, _)) = WINDOWS.with(|w| w.borrow().get(thread).cloned()) else { return };
    if std::env::var("YH_BACKGROUND").as_deref() == Ok("1") {
        return;
    }
    let _ = handle.update(cx, |_, window, _| window.activate_window());
}

/// A page changed (loaded, a tab opened or closed): the open windows draw again.
pub fn notify_all(cx: &mut App) {
    let views: Vec<WeakEntity<BrowserWindow>> = WINDOWS.with(|w| w.borrow().values().map(|(_, v)| v.clone()).collect());
    for v in views {
        let _ = v.update(cx, |_, cx| cx.notify());
    }
}

/// Opens the thread's browser in its own window, or brings that window forward.
pub fn open(workspace: Entity<Workspace>, thread: String, project: String, cx: &mut App) {
    use gpui_kit::{size, Bounds, TitlebarOptions, WindowBounds, WindowOptions};
    if is_open(&thread) {
        return activate(&thread, cx);
    }
    let title = workspace.read(cx).thread(&thread).map(|t| t.read(cx).title.clone()).unwrap_or_else(|| "Browser".into());
    let background = std::env::var("YH_BACKGROUND").as_deref() == Ok("1");
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1100.), px(780.)), cx))),
        titlebar: Some(TitlebarOptions { title: Some(format!("Browser · {title}").into()), ..Default::default() }),
        focus: !background,
        window_min_size: Some(size(px(420.), px(320.))),
        ..Default::default()
    };
    let (t2, p2) = (thread.clone(), project.clone());
    let opened = gpui_kit::open_window(options, cx, move |window, cx| {
        if background {
            crate::platform::order_back(window);
        }
        crate::theme::apply_appearance(window.appearance(), window, cx);
        cx.new(|cx| BrowserWindow::new(workspace, t2, p2, window, cx))
    });
    let Ok((handle, view)) = opened else { return eprintln!("BROWSER couldn't open a window") };
    WINDOWS.with(|w| w.borrow_mut().insert(thread.clone(), (handle, view.downgrade())));
    let _ = handle.update(cx, |_, window, cx| {
        crate::browser::Browsers::pop_out(&thread, crate::platform::ns_view(window));
        let closing = thread.clone();
        window.on_window_should_close(cx, move |_, cx| {
            // The pages go back over the right sidebar before this window's view goes.
            crate::browser::Browsers::bring_back(&closing);
            WINDOWS.with(|w| w.borrow_mut().remove(&closing));
            cx.refresh_windows();
            true
        });
    });
    if std::env::var("YH_DATA_DIR").is_ok() {
        eprintln!("BROWSER window opened for {thread}");
    }
    cx.refresh_windows();
}

/// Closes the thread's browser window, its pages going back to the right sidebar.
pub fn close(thread: &str, cx: &mut App) {
    let Some((handle, _)) = WINDOWS.with(|w| w.borrow_mut().remove(thread)) else { return };
    crate::browser::Browsers::bring_back(thread);
    let _ = handle.update(cx, |_, window, _| window.remove_window());
    cx.refresh_windows();
}

impl BrowserWindow {
    fn new(workspace: Entity<Workspace>, thread: String, project: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let panel = cx.new(|cx| {
            let mut p = BrowserPanel::new(workspace, true, window, cx);
            p.set_thread(Some((thread, project)), cx);
            p
        });
        BrowserWindow { panel }
    }
}

impl Render for BrowserWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        div().size_full().bg(theme.canvas).child(self.panel.clone())
    }
}
