//! A thread's browser in a window of its own (the Swift app's pop-out): the same pages, moved
//! from over the right sidebar to over this window, which the sidebar then points to. Closing
//! the window puts them back.

use crate::state::workspace::Workspace;
use crate::theme::{Theme, MONO, UI};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::{div, prelude::*, px, AnyWindowHandle, App, Context, Entity, FontWeight, SharedString, Subscription, WeakEntity, Window};
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// Each popped-out thread's window and view.
    static WINDOWS: RefCell<HashMap<String, (AnyWindowHandle, WeakEntity<BrowserWindow>)>> = RefCell::new(HashMap::new());
}

pub struct BrowserWindow {
    workspace: Entity<Workspace>,
    thread: String,
    project: String,
    address: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
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
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("Address, or words to search for"));
        let subscriptions = vec![cx.subscribe_in(&address, window, |this, input, event: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = event {
                let text = input.read(cx).value().to_string();
                this.go_to(&text, cx);
            }
        })];
        BrowserWindow { workspace, thread, project, address, _subscriptions: subscriptions }
    }

    fn go_to(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let url = crate::browser::url_from(text).unwrap_or_else(|| format!("https://duckduckgo.com/?q={}", text.split_whitespace().collect::<Vec<_>>().join("+")));
        if crate::browser::Browsers::with_tab(&self.thread, |t| t.web.load(&url)).is_none() {
            crate::browser::Browsers::open_for_user(&self.thread, Some(&self.project), Some(&url));
        }
        cx.notify();
    }
}

impl Render for BrowserWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let thread = self.thread.clone();
        let tabs: Vec<(u32, String, bool, bool)> = crate::browser::Browsers::with(|b| {
            b.sessions
                .get(&thread)
                .map(|s| {
                    let current = s.selected_tab().map(|t| t.id);
                    s.tabs.iter().map(|t| (t.id, if t.title().is_empty() { t.short_url() } else { t.title() }, Some(t.id) == current, t.web.is_loading())).collect()
                })
                .unwrap_or_default()
        });
        let (url, can_back, can_forward) = crate::browser::Browsers::with_tab(&thread, |t| (t.web.url().unwrap_or_default(), t.web.can_go_back(), t.web.can_go_forward())).unwrap_or_default();
        if !gpui_kit::Focusable::focus_handle(self.address.read(cx), cx).is_focused(window) && self.address.read(cx).value() != url.as_str() {
            let shown = url.clone();
            self.address.update(cx, |a, cx| a.set_value(shown, window, cx));
        }
        let mut strip = div().id("pop-tabs").flex_1().min_w_0().flex().items_center().gap(px(4.)).overflow_x_scroll();
        for (id, title, on, loading) in tabs.iter().cloned() {
            let (t, t2) = (thread.clone(), thread.clone());
            strip = strip.child(
                div()
                    .id(SharedString::from(format!("pop-tab-{id}")))
                    .flex_none()
                    .max_w(px(200.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .pl(px(8.))
                    .pr(px(4.))
                    .py(px(2.))
                    .rounded(px(10.))
                    .text_size(px(11.))
                    .bg(if on { theme.accent.opacity(0.16) } else { theme.selected })
                    .text_color(if on { theme.text } else { theme.secondary })
                    .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(if loading { format!("◌ {title}") } else { title }))
                    .child(
                        div()
                            .id(SharedString::from(format!("pop-tab-close-{id}")))
                            .flex_none()
                            .px(px(3.))
                            .rounded(px(6.))
                            .text_color(theme.secondary)
                            .hover(|s| s.bg(theme.separator))
                            .child("×")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                crate::browser::Browsers::close_tab_now(&t2, id);
                                cx.stop_propagation();
                                cx.notify();
                            })),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        crate::browser::Browsers::select_tab(&t, id);
                        cx.notify();
                    })),
            );
        }
        let nav = |id: &'static str, label: &'static str, tip: &'static str, enabled: bool, act: fn(&crate::webview::WebView)| {
            let thread = thread.clone();
            Button::new(id).xsmall().ghost().label(label).tooltip(tip).disabled(!enabled).on_click(cx.listener(move |_, _, _, cx| {
                crate::browser::Browsers::with_tab(&thread, |t| act(&t.web));
                cx.notify();
            }))
        };
        let new_tab = {
            let (thread, project) = (thread.clone(), self.project.clone());
            Button::new("pop-new-tab").xsmall().ghost().label("+").tooltip("New Tab").on_click(cx.listener(move |this, _, window, cx| {
                crate::browser::Browsers::open_for_user(&thread, Some(&project), None);
                this.address.update(cx, |a, cx| {
                    a.set_value("", window, cx);
                    a.focus(window, cx);
                });
                cx.notify();
            }))
        };
        let back_to_sidebar = {
            let (thread, ws) = (thread.clone(), self.workspace.clone());
            Button::new("pop-back").xsmall().ghost().label("Move to Sidebar").tooltip("Show this browser in the right sidebar again").on_click(move |_, _, cx| {
                close(&thread, cx);
                ws.update(cx, |ws, cx| ws.show_panel(crate::state::workspace::Panel::Browser, cx));
            })
        };
        let place = thread.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .font_family(UI)
            .text_size(px(13.))
            .text_color(theme.text)
            .bg(theme.canvas)
            .child(div().flex().items_center().gap(px(4.)).px(px(8.)).pt(px(6.)).child(strip).child(new_tab))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .px(px(6.))
                    .py(px(6.))
                    .child(nav("pop-back-nav", "‹", "Back", can_back, |w| w.go_back()))
                    .child(nav("pop-forward", "›", "Forward", can_forward, |w| w.go_forward()))
                    .child(nav("pop-reload", "↻", "Reload", !tabs.is_empty(), |w| w.reload()))
                    .child(div().flex_1().min_w_0().child(Input::new(&self.address).xsmall().font_family(MONO)))
                    .child(back_to_sidebar),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .border_t_1()
                    .border_color(theme.separator)
                    .when(tabs.is_empty(), |d| {
                        d.child(
                            div()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(theme.secondary)
                                .font_weight(FontWeight::MEDIUM)
                                .child("No pages open. Type an address, or wait for Claude to open one."),
                        )
                    })
                    .child(
                        // GPUI keeps this empty; the page is laid over it, following its bounds.
                        gpui_kit::canvas(
                            move |bounds, _, _| {
                                let (o, s) = (bounds.origin, bounds.size);
                                crate::browser::Browsers::show_popped(&place, (f64::from(o.x), f64::from(o.y), f64::from(s.width), f64::from(s.height)));
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    ),
            )
    }
}
