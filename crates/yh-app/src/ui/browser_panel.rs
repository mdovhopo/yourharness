//! A thread's browser as you see it, in the right sidebar and in its own window: its
//! tabs, the address bar with Pick and Take Control, the card for a page's
//! alert, confirm or prompt, the place the page is laid over (GPUI keeps it empty; the web view
//! follows its bounds every frame), and the console and network under it. With nothing open:
//! the dev servers this thread started, and Import Sign-ins.

use crate::browser::Browsers;
use crate::state::workspace::{Workspace, WorkspaceEvent};
use crate::theme::{radius, space, text, Theme, MONO, UI};
use crate::ui::icons;
use crate::ui::inspector::{strip_tab, tab_close, tool_button};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{Disableable, Icon, IconName, Selectable, Sizable, WindowExt};
use gpui_kit::{div, prelude::*, px, App, Context, Entity, FontWeight, ScrollHandle, SharedString, Subscription, Task, WeakEntity, Window};
use std::cell::RefCell;
use std::collections::HashSet;
use std::time::Duration;

thread_local! {
    /// Every panel made, to draw again when a page changes.
    static PANELS: RefCell<Vec<WeakEntity<BrowserPanel>>> = const { RefCell::new(vec![]) };
}

/// A page changed (loaded, logged, a tab opened or closed): every panel draws again.
pub fn notify_all(cx: &mut App) {
    let panels: Vec<WeakEntity<BrowserPanel>> = PANELS.with(|p| {
        let mut p = p.borrow_mut();
        p.retain(|w| w.upgrade().is_some());
        p.clone()
    });
    for p in panels {
        let _ = p.update(cx, |_, cx| cx.notify());
    }
}

/// Import Sign-ins: pick a browser, then the sites whose cookies to copy.
enum CookieImport {
    Sources(Vec<yh_core::cookies::Source>),
    Reading(String),
    Sites { source: String, cookies: Vec<yh_core::cookies::Cookie>, sites: Vec<(String, usize)>, picked: HashSet<String> },
    Done(String),
    /// Reading failed; Safari's cookies need Full Disk Access, which System Settings gives.
    Failed { message: String, needs_access: bool },
}

#[derive(Clone, Copy, PartialEq)]
enum LogKind {
    Console,
    Network,
}

/// The page sizes the menu offers.
const PAGE_SIZES: [(&str, Option<(f64, f64)>); 5] = [
    ("Fit the Sidebar", None),
    ("Laptop (1512 × 982)", Some(crate::webview::LAPTOP)),
    ("Desktop (1920 × 1080)", Some(crate::webview::DESKTOP)),
    ("Tablet (834 × 1210)", Some(crate::webview::TABLET)),
    ("Phone (402 × 874)", Some(crate::webview::PHONE)),
];

/// What the panel shows of the open tab, read once a render.
struct Shown {
    url: String,
    can_back: bool,
    can_forward: bool,
    loading: bool,
    progress: f64,
    dialog: Option<(String, String, Option<String>, u64)>,
    viewport: Option<(f64, f64)>,
    scheme: Option<String>,
    tab: u32,
}

pub struct BrowserPanel {
    workspace: Entity<Workspace>,
    /// The thread and its project.
    thread: Option<(String, String)>,
    /// In the browser's own window rather than the right sidebar.
    in_window: bool,
    address: Entity<InputState>,
    /// What you type for a page's prompt, and which dialog it was filled for (tab, number).
    answer: Entity<InputState>,
    answer_for: Option<(u32, u64)>,
    filter: Entity<InputState>,
    logs_open: bool,
    log_kind: LogKind,
    errors_only: bool,
    /// Log rows opened to their whole text, by their number.
    open_rows: HashSet<u64>,
    log_scroll: ScrollHandle,
    /// How many rows the log showed last: new ones scroll it to the end.
    log_rows: usize,
    confirming_clear: bool,
    cookie_import: Option<CookieImport>,
    /// The dev servers this thread started, for the empty state: (thread, addresses).
    servers: Option<(String, Vec<String>)>,
    scanning: bool,
    _rescan: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl BrowserPanel {
    pub fn new(workspace: Entity<Workspace>, in_window: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("Address or search"));
        let answer = cx.new(|cx| InputState::new(window, cx));
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        let subscriptions = vec![
            cx.subscribe(&address, |this, input, e: &InputEvent, cx| {
                if let InputEvent::PressEnter { .. } = e {
                    let text = input.read(cx).value().to_string();
                    this.go_to(&text, cx);
                }
            }),
            cx.subscribe(&answer, |this, _, e: &InputEvent, cx| {
                if let InputEvent::PressEnter { .. } = e {
                    this.answer_dialog(true, cx);
                }
            }),
            cx.subscribe(&filter, |_, _, e: &InputEvent, cx| {
                if let InputEvent::Change = e {
                    cx.notify();
                }
            }),
        ];
        let me = cx.entity().downgrade();
        PANELS.with(|p| p.borrow_mut().push(me));
        BrowserPanel {
            workspace,
            thread: None,
            in_window,
            address,
            answer,
            answer_for: None,
            filter,
            // Open; `YH_BROWSER_PANE=console|network` picks the pane, `=none` folds it.
            logs_open: std::env::var("YH_BROWSER_PANE").as_deref() != Ok("none"),
            log_kind: if std::env::var("YH_BROWSER_PANE").as_deref() == Ok("network") { LogKind::Network } else { LogKind::Console },
            errors_only: std::env::var("YH_BROWSER_ERRORS_ONLY").as_deref() == Ok("1"),
            open_rows: HashSet::new(),
            log_scroll: ScrollHandle::new(),
            log_rows: 0,
            confirming_clear: std::env::var("YH_BROWSER_CONFIRM_CLEAR").as_deref() == Ok("1"),
            cookie_import: None,
            servers: None,
            scanning: false,
            _rescan: None,
            _subscriptions: subscriptions,
        }
    }

    /// Which thread's browser it shows (the sidebar follows the open thread).
    pub fn set_thread(&mut self, thread: Option<(String, String)>, cx: &mut Context<Self>) {
        if self.thread == thread {
            return;
        }
        // Another thread's browser: what was open over this one's goes.
        if self.thread.is_some() {
            self.confirming_clear = false;
            self.cookie_import = None;
            self.open_rows.clear();
        }
        self.thread = thread;
        self.test_hooks(cx);
    }

    /// Development: `YH_COOKIE_SITES=<Cookies.binarycookies>` opens Import Sign-ins at its sites
    /// step with a synthetic Safari-format file's cookies (never a real browser's), and
    /// `YH_COOKIE_IMPORT=all` then presses Select All and Import; `YH_COOKIE_FAILED=access` shows
    /// what reading Safari's cookies without Full Disk Access says.
    fn test_hooks(&mut self, cx: &mut Context<Self>) {
        static ONCE: std::sync::Once = std::sync::Once::new();
        if self.thread.is_none() || std::env::var("YH_DATA_DIR").is_err() {
            return;
        }
        let mut first = false;
        ONCE.call_once(|| first = true);
        if !first {
            return;
        }
        if std::env::var("YH_COOKIE_FAILED").as_deref() == Ok("access") {
            self.cookie_import = Some(CookieImport::Failed { message: yh_core::cookies::SAFARI_ACCESS.into(), needs_access: true });
        }
        if let Ok(file) = std::env::var("YH_COOKIE_SITES") {
            match std::fs::read(&file).map_err(|e| e.to_string()).and_then(|d| yh_core::cookies::parse_safari(&d)) {
                Ok(cookies) => {
                    let sites = yh_core::cookies::sites(&cookies);
                    eprintln!("COOKIES sites: {}", sites.iter().map(|(s, n)| format!("{s} ({n})")).collect::<Vec<_>>().join(", "));
                    self.cookie_import = Some(CookieImport::Sites { source: "Safari".into(), cookies, sites, picked: HashSet::new() });
                    if std::env::var("YH_COOKIE_IMPORT").as_deref() == Ok("all") {
                        self.select_all_sites(true);
                        self.store_cookies(cx);
                    }
                }
                Err(e) => eprintln!("COOKIES {e}"),
            }
        }
    }

    fn thread_id(&self) -> Option<&str> {
        self.thread.as_ref().map(|(t, _)| t.as_str())
    }

    /// The address field: a URL, a host, or words to search for, in the current tab or a new one.
    fn go_to(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some((thread, project)) = self.thread.clone() else { return };
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let url = crate::browser::url_from(text).unwrap_or_else(|| format!("https://duckduckgo.com/?q={}", text.split_whitespace().collect::<Vec<_>>().join("+")));
        if Browsers::with_tab(&thread, |t| t.load(&url)).is_none() {
            Browsers::open_for_user(&thread, Some(&project), Some(&url));
        }
        cx.notify();
    }

    /// Pick an element on the page: what you click goes into the composer, described so the
    /// agent can find it again, with a picture of it.
    pub fn pick(&mut self, cx: &mut Context<Self>) {
        let Some(thread) = self.thread_id().map(str::to_string) else { return };
        let Some(reply) = Browsers::pick(&thread) else { return };
        let ws = self.workspace.clone();
        cx.spawn(async move |_, cx| {
            let Ok(Ok(json)) = reply.recv_async().await else { return };
            let Some(text) = Browsers::picked_text(&json) else { return };
            let picture = match cx.update(|_| Browsers::picture_of_pick(&thread, &json)) {
                Some(rx) => rx.recv_async().await.ok().flatten(),
                None => None,
            };
            if std::env::var("YH_DATA_DIR").is_ok() {
                eprintln!("PICKED {text}");
                eprintln!("PICKED picture {}", picture.as_ref().map_or("none".to_string(), |p| format!("{} bytes", p.len())));
            }
            let _ = ws.update(cx, |_, cx| {
                cx.emit(WorkspaceEvent::AppendToComposer(text));
                if let Some(p) = picture {
                    cx.emit(WorkspaceEvent::AttachImages(vec![p]));
                }
            });
        })
        .detach();
    }

    /// The page's dialog answered from the card: OK (with what's typed, for a prompt) or Cancel.
    fn answer_dialog(&mut self, accept: bool, cx: &mut Context<Self>) {
        let Some(thread) = self.thread_id().map(str::to_string) else { return };
        let text = self.answer.read(cx).value().to_string();
        let answered = Browsers::with_tab(&thread, move |t| {
            let kind = t.dialog.take().map(|d| d.0);
            if kind.is_some() {
                t.web.answer_dialog(accept, Some(text));
            }
            kind
        })
        .flatten();
        if let Some(kind) = answered {
            if std::env::var("YH_DATA_DIR").is_ok() {
                eprintln!("BROWSER you {} the {kind}", if accept { "accepted" } else { "dismissed" });
            }
        }
        self.answer_for = None;
        cx.notify();
    }

    /// Looks for the dev servers this thread started: ports under its `claude`, and local
    /// addresses its commands printed. Never a scan of every port.
    fn scan_servers(&mut self, cx: &mut Context<Self>) {
        let Some(thread) = self.thread_id().map(str::to_string) else { return };
        if self.scanning {
            return;
        }
        let pid = self.workspace.read(cx).claude_pid(&thread);
        let mut printed: Vec<String> = vec![];
        if let Some(t) = self.workspace.read(cx).thread(&thread) {
            'rows: for item in t.read(cx).items.iter().rev() {
                for e in item.entries.iter().rev() {
                    let e = e.borrow();
                    if e.data.kind != yh_core::library::Kind::Tool || !matches!(e.data.tool_name.as_deref(), Some("Bash" | "BashOutput" | "TaskOutput")) {
                        continue;
                    }
                    if let Some(out) = &e.data.tool_result {
                        printed.extend(yh_core::dev_servers::printed_urls(out));
                    }
                    if printed.len() >= 6 {
                        break 'rows;
                    }
                }
            }
        }
        self.scanning = true;
        let task = cx.background_executor().spawn(async move { pid.map(yh_core::dev_servers::ports_under).unwrap_or_default() });
        cx.spawn(async move |this, cx| {
            let ports = task.await;
            let _ = this.update(cx, |this, cx| {
                this.scanning = false;
                let found = yh_core::dev_servers::found(&ports, &printed);
                if this.servers.as_ref() != Some(&(thread.clone(), found.clone())) {
                    if std::env::var("YH_DATA_DIR").is_ok() {
                        eprintln!("BROWSER servers started in this thread: {}", if found.is_empty() { "none".to_string() } else { found.join(", ") });
                    }
                    this.servers = Some((thread, found));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The page's place: over the right sidebar's, or the window's (None hides it).
    fn place_page(&self, bounds: Option<(f64, f64, f64, f64)>) {
        let Some(thread) = self.thread_id() else { return };
        if self.in_window {
            Browsers::show_popped(thread, bounds);
        } else {
            Browsers::show(bounds.map(|b| (thread.to_string(), b)));
        }
    }

    fn select_all_sites(&mut self, all: bool) {
        if let Some(CookieImport::Sites { sites, picked, .. }) = &mut self.cookie_import {
            if all {
                picked.extend(sites.iter().map(|(s, _)| s.clone()));
            } else {
                picked.clear();
            }
        }
    }

    fn read_cookies(&mut self, source: yh_core::cookies::Source, cx: &mut Context<Self>) {
        let label = source.label();
        self.cookie_import = Some(CookieImport::Reading(label.clone()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { yh_core::cookies::read(&source) }).await;
            let _ = this.update(cx, |this, cx| {
                this.cookie_import = Some(match result {
                    Ok(cookies) if cookies.is_empty() => CookieImport::Failed { message: format!("{label} has no cookies to import."), needs_access: false },
                    Ok(cookies) => {
                        let sites = yh_core::cookies::sites(&cookies);
                        CookieImport::Sites { source: label, cookies, sites, picked: HashSet::new() }
                    }
                    Err(e) => CookieImport::Failed { needs_access: e == yh_core::cookies::SAFARI_ACCESS, message: e },
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn store_cookies(&mut self, cx: &mut Context<Self>) {
        let Some(CookieImport::Sites { cookies, picked, .. }) = &self.cookie_import else { return };
        let chosen: Vec<yh_core::cookies::Cookie> = cookies.iter().filter(|c| picked.contains(c.site())).cloned().collect();
        let Some((thread, project)) = self.thread.clone() else { return };
        let sites = picked.len();
        let (tx, rx) = flume::bounded(1);
        crate::webview::store_cookies(&project, chosen, tx);
        // Cookie values aren't kept once they're in the store.
        self.cookie_import = Some(CookieImport::Reading("the cookies in".into()));
        cx.spawn(async move |this, cx| {
            let n = rx.recv_async().await.unwrap_or(0);
            let _ = this.update(cx, |this, cx| {
                // Open pages load again with them.
                let reloaded = Browsers::with(|b| {
                    b.sessions.get(&thread).map_or(0, |s| {
                        s.tabs.iter().for_each(|t| t.web.reload());
                        s.tabs.len()
                    })
                });
                if std::env::var("YH_DATA_DIR").is_ok() {
                    eprintln!("COOKIES imported {n} for {sites} sites, {reloaded} pages reloaded");
                }
                this.cookie_import = Some(CookieImport::Done(format!(
                    "Imported {n} cookie{} for {sites} site{} into this project's browser. Open pages reloaded with them; Clear Browsing Data in the browser's menu removes them again.",
                    if n == 1 { "" } else { "s" },
                    if sites == 1 { "" } else { "s" }
                )));
                cx.notify();
            });
        })
        .detach();
    }

    /// The sign-ins panel: Safari and each Chromium profile; then the source's sites.
    fn cookie_panel(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(import) = &self.cookie_import else { return div().into_any_element() };
        let header = panel_header("Import Sign-ins", theme).child(Button::new("cookie-close").small().ghost().label(if matches!(import, CookieImport::Done(_)) { "Done" } else { "Cancel" }).on_click(cx.listener(|this, _, _, cx| {
            this.cookie_import = None;
            cx.notify();
        })));
        let mut column = div().id("cookie-import").flex_1().min_h_0().overflow_y_scroll().px(px(space::M)).pb(px(space::L)).flex().flex_col().gap(px(space::S));
        let note = |line: String| div().px(px(space::XS)).text_size(px(text::FOOTNOTE)).line_height(px(17.)).text_color(theme.secondary).whitespace_normal().child(line);
        let card = |icon: Icon, line: String| {
            div()
                .flex()
                .items_start()
                .gap(px(space::S + 2.))
                .p(px(space::M))
                .rounded(px(radius::LARGE))
                .bg(theme.card)
                .border_1()
                .border_color(theme.separator)
                .child(div().pt(px(1.)).child(icon.size(px(15.)).text_color(theme.secondary)))
                .child(div().flex_1().min_w_0().text_size(px(text::FOOTNOTE)).line_height(px(17.)).whitespace_normal().child(line))
        };
        match import {
            CookieImport::Sources(sources) => {
                column = column.child(note(
                    "Copy the cookies of sites you're signed in to from another browser into this project's browser, so Claude can use them signed in. Nothing is read until you pick a browser, and macOS asks first: Chrome, Arc, Brave, Edge or Vivaldi once for the key they're encrypted with; Safari's need Full Disk Access.".into(),
                ));
                let mut group = div().mt(px(space::XS)).flex().flex_col().rounded(px(radius::LARGE)).border_1().border_color(theme.separator).bg(theme.card).overflow_hidden();
                for (i, source) in sources.iter().enumerate() {
                    let source = source.clone();
                    group = group.child(
                        div()
                            .id(SharedString::from(format!("cookie-source-{i}")))
                            .flex()
                            .items_center()
                            .gap(px(space::S + 2.))
                            .h(px(36.))
                            .px(px(space::M))
                            .cursor_pointer()
                            .when(i > 0, |d| d.border_t_1().border_color(theme.separator))
                            .hover(|s| s.bg(theme.hover))
                            .child(Icon::new(IconName::Globe).size(px(15.)).text_color(theme.secondary))
                            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(source.label()))
                            .child(Icon::new(IconName::ChevronRight).size(px(13.)).text_color(theme.tertiary))
                            .on_click(cx.listener(move |this, _, _, cx| this.read_cookies(source.clone(), cx))),
                    );
                }
                column = column.child(group);
            }
            CookieImport::Reading(label) => {
                column = column.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S))
                        .px(px(space::XS))
                        .text_size(px(text::FOOTNOTE))
                        .text_color(theme.secondary)
                        .child(Icon::new(IconName::LoaderCircle).size(px(13.)).text_color(theme.accent))
                        .child(format!("Reading {label}…")),
                )
            }
            CookieImport::Done(line) => column = column.child(card(icons::cookie(), line.clone())),
            CookieImport::Failed { message, needs_access } => {
                column = column.child(card(Icon::new(IconName::TriangleAlert), message.clone()));
                if *needs_access {
                    column = column.child(div().flex().child(Button::new("cookie-privacy").small().outline().label("Open Privacy Settings").on_click(|_, _, cx| {
                        cx.open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles");
                    })));
                }
            }
            CookieImport::Sites { source, sites, picked, .. } => {
                let count = picked.len();
                column = column.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S))
                        .child(div().flex_1().min_w_0().child(note(format!("{source}: pick the sites to bring over. Only those are copied; nothing leaves your Mac."))))
                        .child(
                            Button::new("cookie-import-go")
                                .small()
                                .primary()
                                .disabled(count == 0)
                                .label(if count == 0 { "Import".to_string() } else { format!("Import {count} Site{}", if count == 1 { "" } else { "s" }) })
                                .on_click(cx.listener(|this, _, _, cx| this.store_cookies(cx))),
                        ),
                );
                column = column.child(
                    div()
                        .flex()
                        .gap(px(space::XS))
                        .child(Button::new("cookie-all").small().ghost().label("Select All").on_click(cx.listener(|this, _, _, cx| {
                            this.select_all_sites(true);
                            cx.notify();
                        })))
                        .child(Button::new("cookie-none").small().ghost().label("Select None").disabled(count == 0).on_click(cx.listener(|this, _, _, cx| {
                            this.select_all_sites(false);
                            cx.notify();
                        }))),
                );
                let mut list = div().flex().flex_col().gap(px(1.));
                for (site, n) in sites.iter().take(400) {
                    let on = picked.contains(site);
                    let key = site.clone();
                    list = list.child(
                        div()
                            .id(SharedString::from(format!("site-{site}")))
                            .flex()
                            .items_center()
                            .gap(px(space::S + 2.))
                            .h(px(28.))
                            .px(px(space::S))
                            .rounded(px(radius::MEDIUM))
                            .cursor_pointer()
                            .hover(|d| d.bg(theme.hover))
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(14.))
                                    .h(px(14.))
                                    .rounded(px(4.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(on, |d| d.bg(theme.accent).child(Icon::new(IconName::Check).size(px(10.)).text_color(gpui_kit::white())))
                                    .when(!on, |d| d.border_1().border_color(theme.tertiary)),
                            )
                            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(site.clone()))
                            .child(div().flex_none().text_size(px(text::CAPTION)).text_color(theme.tertiary).child(n.to_string()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(CookieImport::Sites { picked, .. }) = &mut this.cookie_import {
                                    if !picked.remove(&key) {
                                        picked.insert(key.clone());
                                    }
                                }
                                cx.notify();
                            })),
                    );
                }
                column = column.child(list);
            }
        }
        div().size_full().flex().flex_col().child(header).child(column).into_any_element()
    }

    /// Nothing open: an address to type, and the dev servers this thread started.
    fn empty(&mut self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let thread = self.thread_id().unwrap_or_default().to_string();
        if self.servers.as_ref().is_none_or(|(t, _)| *t != thread) {
            self.servers = Some((thread.clone(), vec![]));
            self.scan_servers(cx);
        }
        // A server started in the background takes a moment to listen: look again now and then.
        if self._rescan.is_none() {
            self._rescan = Some(cx.spawn(async move |this, cx| loop {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                if this.update(cx, |this, cx| this.scan_servers(cx)).is_err() {
                    break;
                }
            }));
        }
        let servers = self.servers.as_ref().map(|(_, s)| s.clone()).unwrap_or_default();
        let mut list = div().flex().flex_col().gap(px(2.)).w_full().max_w(px(300.)).px(px(space::M));
        if !servers.is_empty() {
            list = list.child(div().px(px(space::XS)).pb(px(space::XS)).text_size(px(text::FOOTNOTE)).font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).child("Started in This Thread"));
        }
        for (i, url) in servers.into_iter().enumerate() {
            let short = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).unwrap_or(&url).trim_end_matches('/').to_string();
            list = list.child(
                div()
                    .id(SharedString::from(format!("dev-{i}")))
                    .flex()
                    .items_center()
                    .gap(px(space::S))
                    .h(px(30.))
                    .px(px(space::S))
                    .rounded(px(radius::MEDIUM))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.hover))
                    .child(Icon::new(IconName::SquareTerminal).size(px(14.)).text_color(theme.secondary))
                    .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_family(MONO).text_size(px(text::CODE)).child(short))
                    .on_click(cx.listener(move |this, _, _, cx| this.go_to(&url, cx))),
            );
        }
        let import_button = Button::new("import-sign-ins").small().ghost().icon(icons::cookie().size(px(13.)).text_color(theme.secondary)).label("Import Sign-ins…").on_click(cx.listener(|this, _, _, cx| {
            this.cookie_import = Some(CookieImport::Sources(yh_core::cookies::sources()));
            cx.notify();
        }));
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(space::S))
            .child(div().px(px(space::M)).pt(px(space::M)).child(Input::new(&self.address).small().prefix(Icon::new(IconName::Search).size(px(13.)).text_color(theme.tertiary))))
            .child(
                div().flex_1().min_h_0().flex().flex_col().items_center().justify_center().gap(px(space::L)).child(empty_state(
                    Icon::new(IconName::Globe),
                    "No Pages Open",
                    "Claude opens pages here to test what it builds, and you watch each step. You can type an address above too.",
                    theme,
                ))
                .child(list),
            )
            .child(div().flex().justify_center().pb(px(space::L)).child(import_button))
            .into_any_element()
    }

    fn read_shown(thread: &str) -> Option<Shown> {
        Browsers::with_tab(thread, |t| Shown {
            url: t.web.url().unwrap_or_default(),
            can_back: t.web.can_go_back(),
            can_forward: t.web.can_go_forward(),
            loading: t.web.is_loading(),
            progress: t.web.progress(),
            dialog: t.dialog.clone().map(|(k, m, d)| (k, m, d, t.dialog_id)),
            viewport: t.web.viewport(),
            scheme: t.color_scheme.clone(),
            tab: t.id,
        })
    }

    /// The More menu: the window, the tab, page size and appearance, the inspector, sign-ins.
    fn menu(&self, shown: &Shown, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let (thread, project) = self.thread.clone().unwrap_or_default();
        let url = shown.url.clone();
        let (viewport, scheme, tab) = (shown.viewport, shown.scheme.clone(), shown.tab);
        let page = cx.entity().downgrade();
        let ws = self.workspace.clone();
        let in_window = self.in_window;
        tool_button("browser-more", Icon::new(IconName::Ellipsis), "More", theme)
            .dropdown_menu(move |mut menu, _, _| {
                if !in_window {
                    let (ws, t, p, page) = (ws.clone(), thread.clone(), project.clone(), page.clone());
                    menu = menu
                        .item(PopupMenuItem::new("Open in Window").icon(icons::app_window()).on_click(move |_, _, cx| {
                            crate::ui::browser_window::open(ws.clone(), t.clone(), p.clone(), cx);
                            let _ = page.update(cx, |_, cx| cx.notify());
                        }))
                        .separator();
                }
                let (t1, p1, t2) = (thread.clone(), project.clone(), thread.clone());
                let full = Browsers::with(|b| b.sessions.get(&thread).is_some_and(|s| s.tabs.len() >= crate::browser::MAX_TABS));
                menu = menu
                    .item(PopupMenuItem::new("New Tab").icon(IconName::Plus).disabled(full).on_click(move |_, _, _| Browsers::open_for_user(&t1, Some(&p1), None)))
                    .item(PopupMenuItem::new("Close Tab").icon(icons::x()).on_click(move |_, _, _| Browsers::close_tab_now(&t2, tab)))
                    .separator()
                    .label("Page Size");
                for (label, size) in PAGE_SIZES {
                    let t = thread.clone();
                    menu = menu.item(PopupMenuItem::new(label).checked(viewport == size).on_click(move |_, _, cx| {
                        Browsers::with_tab(&t, |tab| tab.web.set_viewport(size));
                        Browsers::with(|b| b.place());
                        cx.refresh_windows();
                    }));
                }
                if viewport.is_some() && !PAGE_SIZES.iter().any(|(_, s)| *s == viewport) {
                    menu = menu.item(PopupMenuItem::new("Set by Claude").checked(true));
                }
                menu = menu.separator().label("Page Appearance");
                for (label, value) in [("Same as the App", "auto"), ("Light", "light"), ("Dark", "dark")] {
                    let t = thread.clone();
                    let on = scheme.as_deref().unwrap_or("auto") == value;
                    menu = menu.item(PopupMenuItem::new(label).checked(on).on_click(move |_, _, _| {
                        Browsers::with_tab(&t, |tab| tab.set_color_scheme(value));
                    }));
                }
                let (u1, u2, t3) = (url.clone(), url.clone(), thread.clone());
                let (p2, p3) = (page.clone(), page.clone());
                menu.separator()
                    .item(PopupMenuItem::new("Open in Default Browser").icon(IconName::ExternalLink).disabled(u1.is_empty()).on_click(move |_, _, cx| cx.open_url(&u1)))
                    .item(PopupMenuItem::new("Copy Address").icon(IconName::Copy).disabled(u2.is_empty()).on_click(move |_, _, cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(u2.clone()))))
                    .item(PopupMenuItem::new("Show Web Inspector").icon(IconName::Inspector).on_click(move |_, window, cx| {
                        if Browsers::with_tab(&t3, |t| t.web.show_inspector()) != Some(true) {
                            window.open_alert_dialog(cx, |a, _, _| {
                                a.title("Open the Web Inspector from Safari").description("In Safari, turn on Settings → Advanced → Show features for web developers, then choose Develop → this Mac → YourHarness → the page.").ok_text("OK")
                            });
                        }
                    }))
                    .separator()
                    .item(PopupMenuItem::new("Import Sign-ins…").icon(icons::cookie()).on_click(move |_, _, cx| {
                        let _ = p2.update(cx, |this, cx| {
                            this.cookie_import = Some(CookieImport::Sources(yh_core::cookies::sources()));
                            cx.notify();
                        });
                    }))
                    .item(PopupMenuItem::new("Clear Browsing Data…").icon(IconName::Delete).on_click(move |_, _, cx| {
                        let _ = p3.update(cx, |this, cx| {
                            this.confirming_clear = true;
                            cx.notify();
                        });
                    }))
            })
            // A menu is drawn by the window, under the page laid over it: the page steps aside.
            .on_open_change(|open, _, cx| {
                Browsers::set_covered(*open);
                cx.refresh_windows();
            })
            .into_any_element()
    }

    /// The card a page's alert, confirm or prompt asks you with.
    fn dialog_card(&mut self, dialog: &(String, String, Option<String>, u64), tab: u32, theme: &Theme, window: &mut Window, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let (kind, message, default, number) = dialog;
        if self.answer_for != Some((tab, *number)) {
            self.answer_for = Some((tab, *number));
            let default = default.clone().unwrap_or_default();
            self.answer.update(cx, |a, cx| a.set_value(default, window, cx));
        }
        let prompt = kind == "prompt";
        notice_card(theme)
            .flex_col()
            .items_stretch()
            .gap(px(space::S))
            .child(div().text_size(px(text::CAPTION)).text_color(theme.secondary).child("The page says"))
            .child(div().whitespace_normal().text_size(px(text::BODY)).line_height(px(19.)).child(message.clone()))
            .when(prompt, |d| d.child(Input::new(&self.answer).small()))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(space::S))
                    .when(kind != "alert", |d| d.child(Button::new("dialog-cancel").small().outline().label("Cancel").on_click(cx.listener(|this, _, _, cx| this.answer_dialog(false, cx)))))
                    .child(Button::new("dialog-ok").small().primary().label("OK").on_click(cx.listener(|this, _, _, cx| this.answer_dialog(true, cx)))),
            )
            .into_any_element()
    }

    /// The console and network under the page.
    fn logs(&mut self, thread: &str, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let filter = self.filter.read(cx).value().to_lowercase();
        let (kind, errors_only, open) = (self.log_kind, self.errors_only, self.logs_open);
        struct Row {
            id: u64,
            bad: bool,
            warn: bool,
            level: String,
            head: String,
            more: String,
            detail: Option<String>,
            side: Option<String>,
        }
        let (console_errors, failed, rows) = Browsers::with_tab(thread, |t| {
            let errors = t.console.iter().filter(|c| c.level == "error").count();
            let failed = t.network.iter().filter(|r| r.failure.is_some() || r.status.is_some_and(|s| s >= 400)).count();
            let rows: Vec<Row> = if !open {
                vec![]
            } else if kind == LogKind::Console {
                let skip = t.console.len().saturating_sub(500);
                t.console[skip..]
                    .iter()
                    .filter(|c| !errors_only || c.level == "error" || c.level == "warn")
                    .filter(|c| filter.is_empty() || c.text.to_lowercase().contains(&filter))
                    .map(|c| Row { id: c.id, bad: c.level == "error", warn: c.level == "warn", level: c.level.clone(), head: c.text.lines().next().unwrap_or("").to_string(), more: c.text.clone(), detail: None, side: None })
                    .collect()
            } else {
                let skip = t.network.len().saturating_sub(500);
                t.network[skip..]
                    .iter()
                    .filter(|r| !errors_only || r.failure.is_some() || r.status.is_some_and(|s| s >= 400))
                    .filter(|r| filter.is_empty() || r.url.to_lowercase().contains(&filter))
                    .map(|r| {
                        let bad = r.failure.is_some() || r.status.is_some_and(|s| s >= 400);
                        let status = r.status.map(|s| s.to_string()).unwrap_or_else(|| if r.failure.is_some() { "failed".into() } else { "–".into() });
                        let path = r.url.split_once("://").map(|(_, rest)| rest.to_string()).unwrap_or_else(|| r.url.clone());
                        Row {
                            id: r.id,
                            bad,
                            warn: false,
                            level: r.method.clone(),
                            head: format!("{status:<4} {path}"),
                            more: r.url.clone(),
                            detail: r.failure.clone(),
                            side: Some(format!("{}{}", r.kind, r.ms.map(|ms| format!(" {} ms", ms as i64)).unwrap_or_default())),
                        }
                    })
                    .collect()
            };
            (errors, failed, rows)
        })
        .unwrap_or_default();
        if rows.len() > self.log_rows {
            self.log_scroll.scroll_to_bottom();
        }
        self.log_rows = rows.len();
        let segment = |id: &'static str, label: String, on: bool, which: LogKind| {
            div()
                .id(id)
                .h(px(22.))
                .px(px(space::S))
                .flex()
                .items_center()
                .rounded(px(radius::SMALL))
                .text_size(px(text::FOOTNOTE))
                .cursor_pointer()
                .when(on, |d| d.bg(theme.selected).text_color(theme.text).font_weight(FontWeight::MEDIUM))
                .when(!on, |d| d.text_color(theme.secondary).hover(|s| s.bg(theme.hover)))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.log_kind = which;
                    this.logs_open = true;
                    this.log_rows = 0;
                    cx.notify();
                }))
        };
        let count = |name: &str, n: usize| if n > 0 { format!("{name} ({n} ⚠)") } else { name.to_string() };
        let header = div()
            .flex_none()
            .h(px(32.))
            .flex()
            .items_center()
            .gap(px(space::XS))
            .px(px(space::S))
            .child(
                tool_button("logs-toggle", Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }), if open { "Hide the console and network" } else { "Show the console and network" }, theme).on_click(cx.listener(|this, _, _, cx| {
                    this.logs_open = !this.logs_open;
                    cx.notify();
                })),
            )
            .child(segment("logs-console", count("Console", console_errors), kind == LogKind::Console, LogKind::Console))
            .child(segment("logs-network", count("Network", failed), kind == LogKind::Network, LogKind::Network))
            .child(div().flex_1())
            .when(open, |d| {
                d.child(
                    Button::new("logs-errors")
                        .small()
                        .ghost()
                        .icon(Icon::new(IconName::TriangleAlert).size(px(14.)).text_color(if errors_only { theme.warning } else { theme.secondary }))
                        .selected(errors_only)
                        .tooltip(if kind == LogKind::Console { "Errors Only: errors and warnings only" } else { "Errors Only: failed requests only" })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.errors_only = !this.errors_only;
                            cx.notify();
                        })),
                )
                .child(tool_button("logs-clear", Icon::new(IconName::Delete), "Clear the console and network logs", theme).on_click(cx.listener(|this, _, _, cx| {
                    if let Some(t) = this.thread_id() {
                        Browsers::with_tab(t, |t| t.clear_logs());
                    }
                    this.open_rows.clear();
                    cx.notify();
                })))
            });
        let empty_line = if kind == LogKind::Console { "No console messages" } else { "No requests" };
        let list = open.then(|| {
            let mut list = div().id("browser-log").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.log_scroll).pb(px(space::XS)).font_family(MONO).text_size(px(text::CAPTION)).line_height(px(16.));
            if rows.is_empty() {
                list = list.child(div().pt(px(space::L)).w_full().text_center().font_family(UI).text_size(px(text::FOOTNOTE)).text_color(theme.tertiary).child(empty_line));
            }
            for row in rows {
                let expanded = self.open_rows.contains(&row.id);
                let id = row.id;
                let ink = if row.bad { theme.error } else { theme.text };
                let lead: gpui_kit::AnyElement = if kind == LogKind::Console {
                    let (icon, tint) = match row.level.as_str() {
                        "error" => (IconName::CircleX, theme.error),
                        "warn" => (IconName::TriangleAlert, theme.warning),
                        "info" => (IconName::Info, theme.tertiary),
                        "debug" => (IconName::Info, theme.tertiary),
                        _ => (IconName::ChevronRight, theme.tertiary),
                    };
                    div().flex_none().w(px(14.)).pt(px(2.)).child(Icon::new(icon).size(px(11.)).text_color(tint)).into_any_element()
                } else {
                    div().flex_none().w(px(38.)).font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).child(row.level.clone()).into_any_element()
                };
                list = list.child(
                    div()
                        .id(SharedString::from(format!("log-{id}")))
                        .flex()
                        .items_start()
                        .gap(px(space::XS + 2.))
                        .px(px(space::M))
                        .py(px(2.))
                        .cursor_pointer()
                        .when(row.bad, |d| d.bg(theme.error.opacity(0.06)))
                        .when(row.warn, |d| d.bg(theme.warning.opacity(0.07)))
                        .child(lead)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(ink)
                                .when(!expanded, |d| d.overflow_hidden().whitespace_nowrap().text_ellipsis().child(row.head.clone()))
                                .when(expanded, |d| {
                                    d.whitespace_normal().child(row.more.clone()).when_some(row.detail.clone(), |d, f| d.child(div().text_color(theme.error).child(f)))
                                }),
                        )
                        .when_some(row.side.clone(), |d, s| d.child(div().flex_none().text_color(theme.tertiary).child(s)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.open_rows.remove(&id) {
                                this.open_rows.insert(id);
                            }
                            cx.notify();
                        })),
                );
            }
            div().flex_1().min_h_0().flex().flex_col().child(div().px(px(space::M)).pb(px(space::XS)).child(Input::new(&self.filter).small())).child(list)
        });
        div().flex_none().when(open, |d| d.h(px(220.))).flex().flex_col().border_t_1().border_color(theme.separator).child(header).children(list).into_any_element()
    }
}

impl Render for BrowserPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let base = div().size_full().flex().flex_col().font_family(UI).text_size(px(text::BODY)).text_color(theme.text);
        let Some((thread, project)) = self.thread.clone() else {
            return base.into_any_element();
        };
        // In its own window: the sidebar points there.
        if !self.in_window && crate::ui::browser_window::is_open(&thread) {
            Browsers::show(None);
            let (t1, t2) = (thread.clone(), thread.clone());
            return base
                .pb(px(space::XL * 2.))
                .items_center()
                .justify_center()
                .gap(px(space::L))
                .child(empty_state(icons::app_window(), "In Its Own Window", "This thread's browser is open in a separate window. Claude keeps using it there.", &theme))
                .child(
                    div()
                        .flex()
                        .gap(px(space::S))
                        .child(Button::new("browser-show-window").small().outline().label("Show Window").on_click(move |_, _, cx| crate::ui::browser_window::activate(&t1, cx)))
                        .child(Button::new("browser-bring-back").small().primary().label("Bring It Back").on_click(cx.listener(move |_, _, _, cx| {
                            crate::ui::browser_window::close(&t2, cx);
                            cx.notify();
                        }))),
                )
                .into_any_element();
        }
        // Last session's tabs, or ones closed for being idle, reopen when the panel shows them.
        if Browsers::restore(&thread, Some(&project)) {
            cx.notify();
        }
        if self.cookie_import.is_some() {
            self.place_page(None);
            return base.child(self.cookie_panel(&theme, cx)).into_any_element();
        }
        let tabs: Vec<(u32, String, bool, bool)> = Browsers::with(|b| {
            b.sessions
                .get(&thread)
                .map(|s| {
                    let current = s.selected_tab().map(|t| t.id);
                    s.tabs.iter().map(|t| (t.id, if t.title().is_empty() { t.short_url() } else { t.title() }, Some(t.id) == current, t.web.is_loading())).collect()
                })
                .unwrap_or_default()
        });
        let shown = Self::read_shown(&thread);
        let (Some(shown), false) = (shown, tabs.is_empty()) else {
            self.place_page(None);
            return base.child(self.empty(&theme, cx)).into_any_element();
        };
        self._rescan = None;
        let control = Browsers::has_control(&thread);
        // The field shows the page's address, except while you're typing in it.
        if !gpui_kit::Focusable::focus_handle(self.address.read(cx), cx).is_focused(window) && self.address.read(cx).value() != shown.url.as_str() {
            let url = shown.url.clone();
            self.address.update(cx, |a, cx| a.set_value(url, window, cx));
        }
        let prefix = if self.in_window { "pop" } else { "browser" };
        let mut strip = div().id(SharedString::from(format!("{prefix}-tabs"))).flex_1().min_w_0().flex().items_center().gap(px(2.)).overflow_x_scroll();
        for (id, title, on, loading) in tabs.iter().cloned() {
            let (t, t2) = (thread.clone(), thread.clone());
            let icon = if loading { Icon::new(IconName::LoaderCircle) } else { Icon::new(IconName::Globe) };
            strip = strip.child(
                strip_tab(SharedString::from(format!("{prefix}-tab-{id}")), Some(icon), title, on, false, if self.in_window { 220. } else { 170. }, &theme)
                    .child(tab_close(SharedString::from(format!("{prefix}-tab-close-{id}")), &theme).on_click(cx.listener(move |_, _, _, cx| {
                        Browsers::close_tab_now(&t2, id);
                        cx.stop_propagation();
                        cx.notify();
                    })))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        Browsers::select_tab(&t, id);
                        cx.notify();
                    })),
            );
        }
        let new_tab = {
            let (thread, project) = (thread.clone(), project.clone());
            tool_button(SharedString::from(format!("{prefix}-new-tab")), Icon::new(IconName::Plus), "New Tab", &theme).disabled(tabs.len() >= crate::browser::MAX_TABS).on_click(cx.listener(move |this, _, window, cx| {
                Browsers::open_for_user(&thread, Some(&project), None);
                this.address.update(cx, |a, cx| {
                    a.set_value("", window, cx);
                    a.focus(window, cx);
                });
                cx.notify();
            }))
        };
        let nav = |id: &'static str, icon: Icon, tip: &'static str, enabled: bool, act: fn(&crate::webview::WebView)| {
            let thread = thread.clone();
            tool_button(id, icon, tip, &theme).disabled(!enabled).on_click(cx.listener(move |_, _, _, cx| {
                Browsers::with_tab(&thread, |t| act(&t.web));
                cx.notify();
            }))
        };
        let back = nav("browser-back", Icon::new(IconName::ArrowLeft), "Back", shown.can_back, |w| w.go_back());
        let forward = nav("browser-forward", Icon::new(IconName::ArrowRight), "Forward", shown.can_forward, |w| w.go_forward());
        let reload = if shown.loading {
            nav("browser-stop", icons::x(), "Stop loading", true, |w| w.stop_loading())
        } else {
            nav("browser-reload", Icon::new(IconName::RotateCw), "Reload", true, |w| w.reload())
        };
        let pick = tool_button("browser-pick", icons::mouse_pointer_click(), "Pick an element to put in your message, with a picture of it (Esc cancels)", &theme).on_click(cx.listener(|this, _, _, cx| this.pick(cx)));
        let take = {
            let thread = thread.clone();
            Button::new("browser-control")
                .small()
                .ghost()
                .icon(icons::hand().size(px(14.)).text_color(if control { theme.warning } else { theme.secondary }))
                .selected(control)
                .tooltip(if control { "Give Back Control: let Claude act in the browser again" } else { "Take Control: stop Claude from acting in the browser while you use it. It can still look." })
                .on_click(cx.listener(move |_, _, _, cx| {
                    Browsers::set_control(&thread, !control);
                    cx.notify();
                }))
        };
        let last = if self.in_window {
            let (thread, ws) = (thread.clone(), self.workspace.clone());
            Some(tool_button("pop-back", Icon::new(IconName::PanelRight), "Move to Sidebar: show this browser in the right sidebar again", &theme).on_click(move |_, _, cx| {
                crate::ui::browser_window::close(&thread, cx);
                ws.update(cx, |ws, cx| ws.show_panel(crate::state::workspace::Panel::Browser, cx));
            }))
        } else {
            None
        };
        let menu = self.menu(&shown, &theme, cx);
        let progress = shown.loading.then(|| div().h(px(2.)).mx(px(space::S)).child(div().h_full().rounded(px(1.)).bg(theme.accent).w(gpui_kit::relative(shown.progress.clamp(0.05, 1.) as f32))));
        let banner = control.then(|| {
            let thread = thread.clone();
            notice_card(&theme)
                .items_center()
                .gap(px(space::S))
                .child(icons::hand().size(px(14.)).text_color(theme.warning))
                .child(div().flex_1().min_w_0().whitespace_normal().text_size(px(text::FOOTNOTE)).child("You have control. Claude can look but not act."))
                .child(Button::new("browser-give-back").small().outline().label("Give Back").on_click(cx.listener(move |_, _, _, cx| {
                    Browsers::set_control(&thread, false);
                    cx.notify();
                })))
        });
        let dialog = match shown.dialog.clone() {
            Some(d) => Some(self.dialog_card(&d, shown.tab, &theme, window, cx)),
            None => None,
        };
        let confirm = self.confirming_clear.then(|| {
            let project = project.clone();
            let thread = thread.clone();
            notice_card(&theme)
                .flex_col()
                .items_stretch()
                .gap(px(space::S))
                .child(div().text_size(px(text::HEADLINE)).font_weight(FontWeight::SEMIBOLD).child("Clear this project's browsing data?"))
                .child(div().whitespace_normal().text_size(px(text::FOOTNOTE)).line_height(px(17.)).text_color(theme.secondary).child("Cookies, logins and site storage for this project's browser are removed. Other projects and Safari keep theirs."))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(space::S))
                        .child(Button::new("clear-cancel").small().outline().label("Cancel").on_click(cx.listener(|this, _, _, cx| {
                            this.confirming_clear = false;
                            cx.notify();
                        })))
                        .child(Button::new("clear-confirm").small().danger().label("Clear").on_click(cx.listener(move |this, _, _, cx| {
                            // Sign-ins and storage go for the whole project; open pages reload signed out.
                            crate::webview::clear_store(&project);
                            Browsers::with(|b| b.sessions.get(&thread).into_iter().flat_map(|s| s.tabs.iter()).for_each(|t| t.web.reload()));
                            if std::env::var("YH_DATA_DIR").is_ok() {
                                eprintln!("BROWSER cleared browsing data");
                            }
                            this.confirming_clear = false;
                            cx.notify();
                        }))),
                )
        });
        // Answers the page's dialog as the card's buttons do, for test runs
        // (`YH_BROWSER_DIALOG_ANSWER=ok|cancel`).
        if shown.dialog.is_some() {
            if let Ok(answer) = std::env::var("YH_BROWSER_DIALOG_ANSWER") {
                if std::env::var("YH_DATA_DIR").is_ok() {
                    let this = cx.entity().downgrade();
                    cx.defer(move |cx| {
                        let _ = this.update(cx, |this, cx| this.answer_dialog(answer == "ok", cx));
                    });
                }
            }
        }
        let place = self.thread.clone().map(|(t, _)| t).unwrap_or_default();
        let in_window = self.in_window;
        let logs = self.logs(&thread, &theme, cx);
        base.child(div().flex().items_center().gap(px(space::XS)).px(px(space::S)).pt(px(space::XS + 1.)).child(strip).child(new_tab))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .px(px(space::S))
                    .py(px(space::XS + 1.))
                    .child(back)
                    .child(forward)
                    .child(reload)
                    .child(div().flex_1().min_w_0().px(px(2.)).child(Input::new(&self.address).small().text_size(px(text::FOOTNOTE))))
                    .child(pick)
                    .child(take)
                    .child(menu)
                    .children(last),
            )
            .children(progress)
            .children(banner)
            .children(dialog)
            .children(confirm)
            .child(
                div().flex_1().min_h_0().border_t_1().border_color(theme.separator).child(
                    gpui_kit::canvas(
                        move |bounds, _, _| {
                            let (o, s) = (bounds.origin, bounds.size);
                            let b = (f64::from(o.x), f64::from(o.y), f64::from(s.width), f64::from(s.height));
                            if in_window {
                                Browsers::show_popped(&place, Some(b));
                            } else {
                                Browsers::show(Some((place.clone(), b)));
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .size_full(),
                ),
            )
            .child(logs)
            .into_any_element()
    }
}

/// A card over the page's top: a banner, the page's question, a confirmation.
fn notice_card(theme: &Theme) -> gpui_kit::Div {
    div().mx(px(space::S)).mb(px(space::S)).flex().px(px(space::M)).py(px(space::S + 2.)).rounded(px(radius::LARGE)).bg(theme.card).border_1().border_color(theme.separator)
}

/// A panel's header, as the thread's: its title, its actions after.
fn panel_header(title: &str, _theme: &Theme) -> gpui_kit::Div {
    div()
        .flex_none()
        .h(px(40.))
        .flex()
        .items_center()
        .gap(px(2.))
        .pl(px(space::L))
        .pr(px(space::S + 2.))
        .child(div().flex_1().min_w_0().text_size(px(text::HEADLINE)).font_weight(FontWeight::SEMIBOLD).child(title.to_string()))
}

/// What shows with nothing in it yet: its icon, what it is, and what will show here.
fn empty_state(icon: Icon, title: &str, detail: &str, theme: &Theme) -> gpui_kit::Div {
    div()
        .w_full()
        .px(px(space::XL + 4.))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(space::S))
        .child(div().mb(px(2.)).w(px(36.)).h(px(36.)).rounded(px(radius::LARGE - 2.)).bg(theme.selected).flex().items_center().justify_center().child(icon.size(px(16.)).text_color(theme.secondary)))
        .child(div().text_size(px(text::HEADLINE)).font_weight(FontWeight::SEMIBOLD).child(title.to_string()))
        .child(div().w_full().text_center().whitespace_normal().text_size(px(text::FOOTNOTE)).line_height(px(17.)).text_color(theme.secondary).child(detail.to_string()))
}
