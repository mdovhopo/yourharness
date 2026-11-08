//! The agents' browser: each thread's tabs (WKWebViews over the window, see `webview.rs`), and
//! `mcp__browser__*`, the tools that drive them. Names, schemas and replies are the Swift
//! app's (legacy/YourHarness/Browser). Tabs live on the main thread (`Browsers`); a tool call
//! runs on a thread of its own and sends the main thread jobs, waiting on their replies, so
//! waiting for a page never holds up the window.

#![allow(deprecated)]

use crate::webview::{WebEvent, WebView};
use gpui_kit::App;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use yh_core::mcp::{schema, tool, ToolResult, ToolServer};

pub type Job = Box<dyn FnOnce(&mut Browsers, &mut App) + Send>;

static JOBS: std::sync::OnceLock<flume::Sender<Job>> = std::sync::OnceLock::new();

thread_local! {
    static BROWSERS: RefCell<Browsers> = RefCell::new(Browsers::default());
}

/// Big tool results get moved to a file by Claude Code, and the refs with them.
const OUTLINE_BUDGET: usize = 18_000;
pub const MAX_TABS: usize = 8;

#[derive(Clone)]
pub struct Console {
    pub id: u64,
    pub level: String,
    pub text: String,
}

impl Console {
    fn is_error(&self) -> bool {
        self.level == "error"
    }
}

#[derive(Clone)]
pub struct Request {
    pub id: u64,
    pub method: String,
    pub url: String,
    pub status: Option<i64>,
    pub failure: Option<String>,
    pub kind: String,
    pub ms: Option<f64>,
}

impl Request {
    fn failed(&self) -> bool {
        self.failure.is_some() || self.status.is_some_and(|s| s >= 400)
    }
}

pub struct Tab {
    pub id: u32,
    pub web: WebView,
    pub console: Vec<Console>,
    pub network: Vec<Request>,
    next_log: u64,
    reported_console: u64,
    reported_network: u64,
    console_read: u64,
    network_read: u64,
    last_url: Option<String>,
    last_title: Option<String>,
    nav_error: Option<String>,
    pub navigations: u64,
    /// An alert, confirm or prompt the page waits on: (kind, message, default text).
    pub dialog: Option<(String, String, Option<String>)>,
    /// Goes up with each dialog, so the card asking you starts each one afresh.
    pub dialog_id: u64,
    /// The page's color scheme when set (`resize`, the panel's menu), else it follows the app.
    pub color_scheme: Option<String>,
    /// Reopened from the last session and not used since: the agent's next `open` takes it over.
    pub restored: bool,
    /// Refs go on counting from here on the next page, so a stale one can't name a new element.
    next_ref: u64,
}

impl Tab {
    fn new(id: u32, web: WebView) -> Tab {
        Tab {
            id,
            web,
            console: vec![],
            network: vec![],
            next_log: 0,
            reported_console: 0,
            reported_network: 0,
            console_read: 0,
            network_read: 0,
            last_url: None,
            last_title: None,
            nav_error: None,
            navigations: 0,
            dialog: None,
            dialog_id: 0,
            color_scheme: None,
            restored: false,
            next_ref: 1,
        }
    }

    /// Goes somewhere: a reopened tab is then in use.
    pub fn load(&mut self, url: &str) {
        self.restored = false;
        self.web.load(url);
    }

    /// Light, dark, or `auto` (the app's), for the page's `prefers-color-scheme`.
    pub fn set_color_scheme(&mut self, scheme: &str) {
        self.web.set_color_scheme(scheme);
        self.color_scheme = (scheme == "light" || scheme == "dark").then(|| scheme.to_string());
    }

    /// Refs from before are stale: the page's helpers start numbering again above them.
    fn reset_refs(&self) {
        let (tx, _rx) = flume::bounded(1);
        self.web.call(&format!("__yh.reset({})", self.next_ref), false, tx);
    }

    /// Empties the console and network logs (the panel's Clear).
    pub fn clear_logs(&mut self) {
        self.console.clear();
        self.network.clear();
    }

    /// What the agent is doing, over the page for a moment (the Swift app's `show`).
    fn show_activity(&self, label: &str, rect: Option<(f64, f64, f64, f64)>, cx: &mut App) {
        let seq = self.web.show_activity(label, rect);
        if std::env::var("YH_BROWSER_ACTIVITY_LOG").as_deref() == Ok("1") {
            let at = rect.map(|(x, y, w, h)| format!(" at {x:.0},{y:.0} {w:.0}×{h:.0}")).unwrap_or_default();
            eprintln!("BROWSER activity {label}{at} shown={}", self.web.shows_activity());
            if let Ok(png) = std::env::var("YH_BROWSER_ACTIVITY_SHOT") {
                eprintln!("BROWSER activity drawn {}", self.web.overlay_png(&png));
            }
        }
        let key = self.web.key();
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_millis(1400)).await;
            let _ = cx.update(|_| Browsers::with(|b| b.tab_mut(key).map(|t| t.web.clear_activity(seq))));
        })
        .detach();
    }

    pub fn short_url(&self) -> String {
        let url = self.web.url().unwrap_or_else(|| "about:blank".into());
        let url = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).unwrap_or(&url).to_string();
        clip(url.trim_end_matches('/'), 120)
    }

    pub fn title(&self) -> String {
        self.web.title().unwrap_or_default()
    }

    fn log(&mut self, v: &Value) {
        self.next_log += 1;
        let id = self.next_log;
        match v["kind"].as_str() {
            Some("console") => {
                self.console.push(Console { id, level: v["level"].as_str().unwrap_or("log").into(), text: v["text"].as_str().unwrap_or("").into() });
                if self.console.len() > 1000 {
                    self.console.drain(..200);
                }
            }
            Some("request") => {
                self.network.push(Request {
                    id,
                    method: v["method"].as_str().unwrap_or("GET").into(),
                    url: v["url"].as_str().unwrap_or("").into(),
                    status: v["status"].as_i64(),
                    failure: v["failure"].as_str().map(str::to_string),
                    kind: v["type"].as_str().unwrap_or("other").into(),
                    ms: v["ms"].as_f64(),
                });
                if self.network.len() > 2000 {
                    self.network.drain(..400);
                }
            }
            _ => {}
        }
    }
}

#[derive(Default)]
pub struct Session {
    pub tabs: Vec<Tab>,
    pub selected: Option<u32>,
    next_id: u32,
    /// When the agent or you last used it, or it was last on screen.
    last_active: Option<Instant>,
    /// Where its tabs were at the last quit, or when they closed for being idle: reopened when
    /// the browser is next used (the agent's call, or the panel showing).
    pub restorable: Vec<String>,
    /// You took over: the agent's actions fail until you give it back; it can still look.
    pub user_has_control: bool,
}

impl Session {
    pub fn selected_tab(&self) -> Option<&Tab> {
        self.selected.and_then(|id| self.tabs.iter().find(|t| t.id == id)).or(self.tabs.last())
    }

    /// What's saved with the thread: the open tabs' addresses, or what still waits to reopen.
    fn saved_urls(&self) -> Vec<String> {
        if self.tabs.is_empty() {
            return self.restorable.clone();
        }
        self.tabs.iter().filter_map(|t| t.web.url()).filter(|u| !u.starts_with("about:")).collect()
    }
}

#[derive(Default)]
pub struct Browsers {
    /// GPUI's view: web views are laid over it.
    host: usize,
    pub sessions: HashMap<String, Session>,
    /// Where the browser panel is, for which thread: its tabs show there.
    shown: Option<(String, (f64, f64, f64, f64))>,
    /// Shows the thread's browser to the user (the right sidebar on Browser).
    reveal: Option<Box<dyn Fn(&str, &mut App)>>,
    /// Tells the panel something changed (a page loaded, a tab opened).
    changed: Option<Box<dyn Fn(&mut App)>>,
    /// Threads whose browser is in a window of its own: that window's view, and where in it the
    /// page goes (None until it's laid out). The right sidebar then points there.
    popped: HashMap<String, (usize, Option<(f64, f64, f64, f64)>)>,
    /// Where threads are saved: each one's tabs go in its record (`browserTabs`, as the Swift
    /// app keeps them).
    library: Option<std::sync::Arc<yh_core::library::Library>>,
    /// Threads whose tabs changed since they were last saved.
    unsaved: std::collections::HashSet<String>,
    /// When the panel was last told the logs grew: at most a few times a second.
    logs_told: Option<Instant>,
    /// A menu is open over the page (the window draws it under the web view): pages step aside.
    covered: bool,
}

impl Browsers {
    pub fn with<T>(f: impl FnOnce(&mut Browsers) -> T) -> T {
        BROWSERS.with(|b| f(&mut b.borrow_mut()))
    }

    /// The thread's browser, made on first use with the tabs saved with the thread waiting to
    /// reopen.
    fn session(&mut self, thread: &str) -> &mut Session {
        if !self.sessions.contains_key(thread) {
            let saved: Vec<String> = self
                .library
                .as_ref()
                .and_then(|l| l.thread_record(thread))
                .and_then(|r| r.get("browserTabs").and_then(Value::as_array).map(|a| a.iter().filter_map(|u| u.as_str().map(str::to_string)).collect()))
                .unwrap_or_default();
            self.sessions.insert(thread.to_string(), Session { restorable: saved, ..Default::default() });
        }
        self.sessions.get_mut(thread).expect("made above")
    }

    /// Reopens what waits to (last session's tabs, or ones closed for being idle), once, on
    /// first use; true if it did.
    pub fn restore_if_needed(&mut self, thread: &str, project: Option<&str>) -> bool {
        let host = self.host;
        let s = self.session(thread);
        if !s.tabs.is_empty() || s.restorable.is_empty() || host == 0 {
            return false;
        }
        let urls: Vec<String> = std::mem::take(&mut s.restorable).into_iter().take(MAX_TABS).collect();
        for url in urls {
            let id = self.open_tab(thread, project.filter(|p| p.len() == 36), &url);
            if let Some(t) = self.sessions.get_mut(thread).and_then(|s| s.tabs.iter_mut().find(|t| t.id == id)) {
                t.restored = true;
            }
        }
        if std::env::var("YH_DATA_DIR").is_ok() {
            eprintln!("BROWSER reopened {} tabs for {thread}", self.sessions.get(thread).map_or(0, |s| s.tabs.len()));
        }
        true
    }

    /// The panel showing the thread's browser: last session's tabs reopen (the Swift app's
    /// `restoreIfNeeded` when the panel appears).
    pub fn restore(thread: &str, project: Option<&str>) -> bool {
        Browsers::with(|b| b.restore_if_needed(thread, project))
    }

    /// The tabs of threads that changed into their records.
    fn save_unsaved(&mut self) {
        let Some(library) = self.library.clone() else { return };
        for thread in std::mem::take(&mut self.unsaved) {
            let Some(urls) = self.sessions.get(&thread).map(Session::saved_urls) else { continue };
            library.update_thread(&thread, |r| {
                if urls.is_empty() {
                    r.remove("browserTabs");
                } else {
                    r.insert("browserTabs".into(), json!(urls));
                }
            });
        }
    }

    /// Everything not saved yet, now (at quit).
    pub fn save_now() {
        Browsers::with(|b| b.save_unsaved());
    }

    /// Take Control, or Give Back: refs the agent had are stale either way.
    pub fn set_control(thread: &str, on: bool) {
        Browsers::with(|b| {
            let s = b.session(thread);
            if s.user_has_control == on {
                return;
            }
            s.user_has_control = on;
            for t in &s.tabs {
                t.reset_refs();
            }
            if std::env::var("YH_DATA_DIR").is_ok() {
                eprintln!("BROWSER control {}", if on { "taken" } else { "given back" });
            }
        });
    }

    pub fn has_control(thread: &str) -> bool {
        Browsers::with(|b| b.sessions.get(thread).is_some_and(|s| s.user_has_control))
    }

    /// The tab a page opens (`window.open`, `target=_blank`, a sign-in popup), in its opener's
    /// browser, made with the configuration WebKit gives; None past the most tabs.
    fn popup(opener: usize, configuration: cocoa::base::id, url: Option<String>) -> Option<cocoa::base::id> {
        BROWSERS.with(|b| {
            // Never inside another change to the browsers (WebKit asks from the run loop).
            let mut b = b.try_borrow_mut().ok()?;
            let (thread, from) = b.sessions.iter().find_map(|(thread, s)| s.tabs.iter().find(|t| t.web.key() == opener).map(|t| (thread.clone(), t.id)))?;
            if b.sessions.get(&thread).is_some_and(|s| s.tabs.len() >= MAX_TABS) || b.host == 0 {
                return None;
            }
            let web = WebView::with_configuration(b.host as cocoa::base::id, configuration);
            let view = web.key() as cocoa::base::id;
            let s = b.session(&thread);
            s.next_id += 1;
            let id = s.next_id;
            let mut tab = Tab::new(id, web);
            tab.log(&json!({"kind": "console", "level": "info", "text": format!("Opened by tab {from} ({})", url.as_deref().unwrap_or("a new window"))}));
            s.tabs.push(tab);
            s.selected = Some(id);
            s.last_active = Some(Instant::now());
            b.unsaved.insert(thread.clone());
            b.place();
            if std::env::var("YH_DATA_DIR").is_ok() {
                eprintln!("BROWSER tab {id} opened by tab {from}: {}", url.as_deref().unwrap_or("a new window"));
            }
            Some(view)
        })
    }

    fn tab_mut(&mut self, view: usize) -> Option<&mut Tab> {
        self.sessions.values_mut().flat_map(|s| s.tabs.iter_mut()).find(|t| t.web.key() == view)
    }

    /// Lays the shown thread's current tab over the panel, and a popped-out thread's over its
    /// own window; every other tab hidden. A tab moves to the view it shows in.
    pub fn place(&self) {
        for (thread, session) in &self.sessions {
            let current = session.selected_tab().map(|t| t.id);
            let (host, here) = match self.popped.get(thread) {
                Some((host, bounds)) => (*host, *bounds),
                None => (self.host, self.shown.as_ref().filter(|(t, _)| t == thread).map(|(_, b)| *b)),
            };
            let here = here.filter(|_| !self.covered);
            for tab in &session.tabs {
                if host != 0 {
                    tab.web.move_to(host as cocoa::base::id);
                }
                tab.web.set_frame(if Some(tab.id) == current { here } else { None });
            }
        }
    }

    /// The thread's browser moves to a window of its own, whose view is `host`.
    pub fn pop_out(thread: &str, host: cocoa::base::id) {
        Browsers::with(|b| {
            b.popped.insert(thread.to_string(), (host as usize, None));
            b.touch(thread);
            b.place();
        });
    }

    /// Where the page goes in the thread's own window (its bounds there), each frame it draws;
    /// None hides it (Import Sign-ins over it).
    pub fn show_popped(thread: &str, bounds: Option<(f64, f64, f64, f64)>) {
        Browsers::with(|b| {
            let Some(entry) = b.popped.get_mut(thread) else { return };
            if entry.1 != bounds {
                entry.1 = bounds;
                b.touch(thread);
                b.place();
            }
        });
    }

    /// A menu opened over the page, or closed: the pages step aside meanwhile.
    pub fn set_covered(on: bool) {
        Browsers::with(|b| {
            if b.covered != on {
                b.covered = on;
                b.place();
            }
        });
    }

    /// Back in the right sidebar: its window closed.
    pub fn bring_back(thread: &str) {
        Browsers::with(|b| {
            if b.popped.remove(thread).is_some() {
                b.place();
            }
        });
    }

    pub fn is_popped(thread: &str) -> bool {
        Browsers::with(|b| b.popped.contains_key(thread))
    }

    fn touch(&mut self, thread: &str) {
        if let Some(s) = self.sessions.get_mut(thread) {
            s.last_active = Some(Instant::now());
        }
    }

    /// Pages nobody has used or looked at for 30 minutes (`YH_BROWSER_IDLE_SECS`) are closed, as
    /// the Swift app does: each tab is a web content process. Their addresses stay, to open again.
    pub fn close_idle(&mut self) {
        let limit = Duration::from_secs(std::env::var("YH_BROWSER_IDLE_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(30 * 60));
        let shown = self.shown.as_ref().map(|(t, _)| t.clone());
        let mut closed = 0;
        for (thread, s) in self.sessions.iter_mut() {
            let idle = s.last_active.is_none_or(|t| t.elapsed() > limit);
            // On screen in its own window counts as looked at.
            if s.tabs.is_empty() || !idle || shown.as_deref() == Some(thread.as_str()) || self.popped.contains_key(thread) {
                continue;
            }
            s.restorable = s.tabs.iter().filter_map(|t| t.web.url()).filter(|u| u != "about:blank").collect();
            closed += s.tabs.len();
            s.tabs.clear();
            s.selected = None;
        }
        if closed > 0 {
            if std::env::var("YH_DATA_DIR").is_ok() {
                eprintln!("BROWSER closed {closed} idle pages");
            }
            self.place();
        }
    }

    /// The panel's place for a thread's browser, or None when it isn't showing.
    pub fn show(thread: Option<(String, (f64, f64, f64, f64))>) {
        Browsers::with(|b| {
            if let Some((t, _)) = &thread {
                b.touch(&t.clone());
            }
            if b.shown != thread {
                b.shown = thread;
                b.place();
            }
        });
    }

    fn open_tab(&mut self, thread: &str, project: Option<&str>, url: &str) -> u32 {
        let host = self.host as cocoa::base::id;
        let session = self.session(thread);
        session.next_id += 1;
        let id = session.next_id;
        let web = WebView::new(host, project);
        if let Some(u) = url_from(url) {
            web.load(&u);
        }
        session.tabs.push(Tab::new(id, web));
        session.selected = Some(id);
        session.last_active = Some(Instant::now());
        session.restorable.clear();
        self.unsaved.insert(thread.to_string());
        self.place();
        id
    }

    /// A tab you open from the panel (an address, or a blank page with None).
    pub fn open_for_user(thread: &str, project: Option<&str>, url: Option<&str>) {
        Browsers::with(|b| {
            b.open_tab(thread, project, url.unwrap_or(""));
        });
    }

    /// Does something with a thread's current tab, from the panel's buttons.
    pub fn with_tab<T>(thread: &str, f: impl FnOnce(&mut Tab) -> T) -> Option<T> {
        Browsers::with(|b| {
            let s = b.sessions.get_mut(thread)?;
            let id = s.selected_tab()?.id;
            s.tabs.iter_mut().find(|t| t.id == id).map(f)
        })
    }

    /// The element picker in the thread's current tab: the reply comes when you click something
    /// (its description, selector, HTML and the page's address as JSON) or press Escape (null).
    pub fn pick(thread: &str) -> Option<flume::Receiver<Result<String, String>>> {
        let (tx, rx) = flume::bounded(1);
        Browsers::with_tab(thread, |t| t.web.call("const info = await __yh.pick(); return info && { ...info, url: location.href };", false, tx))?;
        Some(rx)
    }

    /// A picture of what was picked (its box in CSS pixels, a little around it), as a JPEG for
    /// the message (the Swift app's `PickedElement.image`); None without a tab or a box.
    pub fn picture_of_pick(thread: &str, json: &str) -> Option<flume::Receiver<Option<Vec<u8>>>> {
        let v: Value = serde_json::from_str(json).ok()?;
        let r: Vec<f64> = v["rect"].as_array()?.iter().filter_map(Value::as_f64).collect();
        if r.len() != 4 || r[2] < 1. || r[3] < 1. {
            return None;
        }
        let rect = (r[0] - 6., r[1] - 6., r[2] + 12., r[3] + 12.);
        static PICKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = PICKS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("yh-pick-{}-{n}.jpg", std::process::id())).to_string_lossy().into_owned();
        let (tx, rx) = flume::bounded(1);
        let (done, out) = flume::bounded(1);
        let p = path.clone();
        Browsers::with_tab(thread, move |t| t.web.capture(Some(rect), 1200., p, tx))?;
        std::thread::spawn(move || {
            let bytes = rx.recv_timeout(Duration::from_secs(10)).ok().and_then(Result::ok).and_then(|_| std::fs::read(&path).ok());
            let _ = std::fs::remove_file(&path);
            let _ = done.send(bytes);
        });
        Some(out)
    }

    /// What a picked element reads like in your message: enough for the agent to find it again
    /// (the Swift app's `PickedElement.promptText`).
    pub fn picked_text(json: &str) -> Option<String> {
        let v: Value = serde_json::from_str(json).ok()?;
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        v.is_object().then(|| format!("[Picked in the browser at {}: {}, selector `{}`]\n```html\n{}\n```", s("url"), s("describe"), s("selector"), s("html")))
    }

    pub fn close_tab_now(thread: &str, id: u32) {
        Browsers::with(|b| b.close_tab(thread, id));
    }

    pub fn select_tab(thread: &str, id: u32) {
        Browsers::with(|b| {
            if let Some(s) = b.sessions.get_mut(thread) {
                s.selected = Some(id);
            }
            b.place();
        });
    }

    pub fn close_tab(&mut self, thread: &str, id: u32) {
        if let Some(s) = self.sessions.get_mut(thread) {
            s.tabs.retain(|t| t.id != id);
            if s.selected == Some(id) {
                s.selected = s.tabs.last().map(|t| t.id);
            }
            self.unsaved.insert(thread.to_string());
        }
        self.place();
    }

    /// Opens a page in a new tab of the thread's browser and shows it in the right sidebar: a
    /// `localhost` link in a reply, which is the app Claude is building.
    pub fn open_page(thread: &str, project: Option<&str>, url: &str, cx: &mut App) {
        let reveal = Browsers::with(|b| {
            if b.host != 0 && b.sessions.get(thread).is_none_or(|s| s.tabs.len() < MAX_TABS) {
                b.open_tab(thread, project.filter(|p| p.len() == 36), url);
            }
            (b.reveal.take(), b.changed.take())
        });
        if let Some(f) = &reveal.0 {
            f(thread, cx);
        }
        if let Some(f) = &reveal.1 {
            f(cx);
        }
        Browsers::with(|b| {
            b.reveal = reveal.0;
            b.changed = reveal.1;
        });
    }

    /// A thread's browser goes with it.
    pub fn close_thread(thread: &str) {
        Browsers::with(|b| {
            b.sessions.remove(thread);
            b.popped.remove(thread);
        });
    }
}

/// Starts the browser: the window's view to lay pages over, where threads are saved (their tabs
/// go with them), and the loops that run tool calls' jobs and take in what pages report.
pub fn start(
    host: cocoa::base::id,
    library: Option<std::sync::Arc<yh_core::library::Library>>,
    reveal: impl Fn(&str, &mut App) + 'static,
    changed: impl Fn(&mut App) + 'static,
    cx: &mut App,
) {
    Browsers::with(|b| {
        b.host = host as usize;
        b.library = library;
        b.reveal = Some(Box::new(reveal));
        b.changed = Some(Box::new(changed));
    });
    crate::webview::set_popups(Browsers::popup);
    let (jobs, rx) = flume::unbounded::<Job>();
    let _ = JOBS.set(jobs);
    // Each second, threads whose tabs went somewhere are saved (redirects come in bursts); idle
    // pages close, looked at every five minutes (every two seconds in a test with a short limit).
    #[allow(clippy::infinite_loop)]
    cx.spawn(async move |cx| {
        let every = if std::env::var("YH_BROWSER_IDLE_SECS").is_ok() { 2 } else { 300 };
        let mut ticks = 0u64;
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            ticks += 1;
            cx.update(|_| {
                Browsers::with(|b| {
                    if !b.unsaved.is_empty() {
                        b.save_unsaved();
                    }
                    if ticks % every == 0 {
                        b.close_idle();
                    }
                })
            });
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        while let Ok(job) = rx.recv_async().await {
            let _ = cx.update(|cx| BROWSERS.with(|b| job(&mut b.borrow_mut(), cx)));
        }
    })
    .detach();
    let (events, rx) = flume::unbounded::<WebEvent>();
    crate::webview::set_events(events);
    cx.spawn(async move |cx| {
        while let Ok(event) = rx.recv_async().await {
            let _ = cx.update(|cx| {
                let notify = BROWSERS.with(|b| take_event(&mut b.borrow_mut(), event));
                if notify {
                    let changed = BROWSERS.with(|b| b.borrow_mut().changed.take());
                    if let Some(f) = changed {
                        f(cx);
                        BROWSERS.with(|b| b.borrow_mut().changed = Some(f));
                    }
                }
            });
        }
    })
    .detach();
}

/// What a page reported, taken in; whether the panel should draw again.
fn take_event(b: &mut Browsers, event: WebEvent) -> bool {
    let view = match &event {
        WebEvent::Log { view, .. }
        | WebEvent::UploadAsked { view }
        | WebEvent::Started { view }
        | WebEvent::Committed { view }
        | WebEvent::Finished { view }
        | WebEvent::Failed { view, .. }
        | WebEvent::Closed { view }
        | WebEvent::Crashed { view }
        | WebEvent::Dialog { view, .. } => *view,
    };
    let Some(thread) = b.sessions.iter().find(|(_, s)| s.tabs.iter().any(|t| t.web.key() == view)).map(|(t, _)| t.clone()) else { return false };
    if let WebEvent::Closed { .. } = event {
        // `window.close()` from a page a script opened: its tab goes.
        let id = b.tab_mut(view).map(|t| t.id).unwrap_or(0);
        if std::env::var("YH_DATA_DIR").is_ok() {
            eprintln!("BROWSER tab {id} closed itself");
        }
        b.close_tab(&thread, id);
        return true;
    }
    let mut navigated = false;
    let Some(tab) = b.tab_mut(view) else { return false };
    let notify = match event {
        WebEvent::Log { json, .. } => {
            if let Ok(v) = serde_json::from_str::<Value>(&json) {
                tab.log(&v);
            }
            // The log strip shows counts: told a few times a second at most.
            let due = b.logs_told.is_none_or(|t| t.elapsed() > Duration::from_millis(250));
            if due {
                b.logs_told = Some(Instant::now());
            }
            due
        }
        WebEvent::Started { .. } => {
            tab.navigations += 1;
            tab.nav_error = None;
            true
        }
        WebEvent::Committed { .. } => {
            // A new document: its refs go on from the last page's.
            tab.reset_refs();
            navigated = true;
            true
        }
        WebEvent::Finished { .. } => {
            navigated = true;
            true
        }
        WebEvent::Failed { error, url, .. } => {
            tab.log(&json!({ "kind": "request", "method": "GET", "url": url.unwrap_or_default(), "type": "document", "failure": error }));
            tab.log(&json!({ "kind": "console", "level": "error", "text": format!("Navigation failed: {error}") }));
            tab.nav_error = Some(error);
            true
        }
        WebEvent::Crashed { .. } => {
            tab.log(&json!({ "kind": "console", "level": "error", "text": "The page crashed (its web content process ended). Reloading." }));
            if std::env::var("YH_DATA_DIR").is_ok() {
                eprintln!("BROWSER tab {} crashed, reloading", tab.id);
            }
            tab.web.reload();
            true
        }
        WebEvent::Dialog { kind, message, default, .. } => {
            tab.dialog = Some((kind, message, default));
            tab.dialog_id += 1;
            true
        }
        WebEvent::UploadAsked { .. } | WebEvent::Closed { .. } => false,
    };
    if navigated {
        b.unsaved.insert(thread);
    }
    notify
}

/// As typed in a URL field: a URL, a host, `localhost:3000`, or words to search for.
pub fn url_from(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Some((scheme, _)) = text.split_once(':') {
        if ["http", "https", "file", "about", "data"].contains(&scheme.to_lowercase().as_str()) {
            return Some(text.to_string());
        }
    }
    let local = ["localhost", "127.0.0.1", "0.0.0.0", "[::1]"].iter().any(|p| text.starts_with(p));
    if !text.contains(' ') && (text.contains('.') || text.contains(':') || local) {
        return Some(format!("{}{text}", if local { "http://" } else { "https://" }));
    }
    let q: String = text.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    Some(format!("https://duckduckgo.com/?q={q}"))
}

fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() > limit {
        format!("{}…", text.chars().take(limit - 1).collect::<String>())
    } else {
        text.to_string()
    }
}

fn one_line(text: &str, limit: usize) -> String {
    clip(&text.lines().collect::<Vec<_>>().join(" "), limit)
}

/// A script's exception as the agent should read it (the Swift app's `BrowserError(script:)`).
fn script_error(message: &str) -> String {
    let message = message.strip_prefix("Error: ").unwrap_or(message);
    if let Some(r) = message.strip_prefix("stale:") {
        format!("Ref {r} is from an older snapshot: the page changed. Take a fresh snapshot.")
    } else if message.contains("__yh") {
        "The page isn't ready for that yet (it's loading, or it's not a web page).".into()
    } else {
        message.to_string()
    }
}

fn quoted(text: &str, limit: usize) -> String {
    format!("\"{}\"", one_line(text, limit))
}

fn count(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

// MARK: The tools

pub struct BrowserTools {
    pub thread: String,
    /// The project's ID: its browser data store (cookies, storage) is the project's.
    pub project: Option<String>,
    /// A side chat looks at its thread's browser; it can't act.
    pub read_only: bool,
}

type Fail = String;

const READ_ONLY_TOOLS: &[&str] = &["status", "snapshot", "screenshot", "wait_for", "console", "network"];

impl BrowserTools {
    /// Runs `f` on the main thread with the browsers and waits for what it returns.
    fn main<T: Send + 'static>(&self, f: impl FnOnce(&mut Browsers, &mut App) -> T + Send + 'static) -> Result<T, Fail> {
        let (tx, rx) = flume::bounded(1);
        let thread = self.thread.clone();
        // The agent's first call reopens last session's tabs; a side chat only looks.
        let restore = (!self.read_only).then(|| self.project.clone());
        let job: Job = Box::new(move |b, cx| {
            if let Some(project) = restore {
                b.restore_if_needed(&thread, project.as_deref());
            }
            b.touch(&thread);
            let _ = tx.send(f(b, cx));
        });
        JOBS.get().ok_or("The browser isn't ready.")?.send(job).map_err(|_| "The browser is gone.".to_string())?;
        rx.recv_timeout(Duration::from_secs(70)).map_err(|_| "The browser didn't answer.".to_string())
    }

    /// The tab a call means: `tab`, else the current one.
    fn tab_id(&self, a: &Value) -> Result<u32, Fail> {
        let thread = self.thread.clone();
        let asked = a["tab"].as_u64().map(|t| t as u32);
        self.main(move |b, _| {
            let Some(s) = b.sessions.get(&thread).filter(|s| !s.tabs.is_empty()) else { return Err("No page is open. Use open with a URL first.".to_string()) };
            match asked {
                Some(id) if s.tabs.iter().any(|t| t.id == id) => Ok(id),
                Some(id) => Err(format!("There's no tab {id}. Open tabs: {}.", s.tabs.iter().map(|t| t.id.to_string()).collect::<Vec<_>>().join(", "))),
                None => Ok(s.selected_tab().map(|t| t.id).unwrap_or(0)),
            }
        })?
    }

    /// Does something with a tab on the main thread.
    fn with_tab<T: Send + 'static>(&self, id: u32, f: impl FnOnce(&mut Tab) -> T + Send + 'static) -> Result<T, Fail> {
        let thread = self.thread.clone();
        self.main(move |b, _| {
            b.sessions.get_mut(&thread).and_then(|s| s.tabs.iter_mut().find(|t| t.id == id)).map(f).ok_or_else(|| "That tab is gone.".to_string())
        })?
    }

    /// Runs a function body in the helpers' world (or the page's) and parses what it returned.
    fn script(&self, id: u32, body: String, in_page: bool, timeout: Duration) -> Result<Value, Fail> {
        let (tx, rx) = flume::bounded(1);
        self.with_tab(id, move |t| t.web.call(&body, in_page, tx))?;
        let text = rx.recv_timeout(timeout).map_err(|_| "The page didn't answer in time.".to_string())?.map_err(|e| script_error(&e))?;
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    /// The user took control: the agent can look, not act.
    fn check_control(&self) -> Result<(), Fail> {
        let thread = self.thread.clone();
        if self.main(move |b, _| b.sessions.get(&thread).is_some_and(|s| s.user_has_control))? {
            return Err("The user has taken control of the browser. You can still look (status, snapshot, screenshot, console, network); wait for them to give it back before acting.".into());
        }
        Ok(())
    }

    /// Where the agent acts, shown over the page for a moment.
    fn show(&self, id: u32, label: &str, rect: Option<(f64, f64, f64, f64)>) {
        let label = label.to_string();
        let thread = self.thread.clone();
        let _ = self.main(move |b, cx| {
            if let Some(t) = b.sessions.get(&thread).and_then(|s| s.tabs.iter().find(|t| t.id == id)) {
                t.show_activity(&label, rect, cx);
            }
        });
    }

    fn helper(&self, id: u32, body: &str) -> Result<Value, Fail> {
        self.script(id, body.to_string(), false, Duration::from_secs(65))
    }

    fn is_loading(&self, id: u32) -> bool {
        self.with_tab(id, |t| t.web.is_loading()).unwrap_or(false)
    }

    fn wait_for_load(&self, id: u32, timeout: Duration) -> bool {
        let start = Instant::now();
        while self.is_loading(id) {
            if start.elapsed() > timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        true
    }

    /// After an action: if it started a navigation, wait for it; then two frames, so what
    /// it changed is on screen for a screenshot.
    fn settle(&self, id: u32, timeout: Duration) {
        let before = self.with_tab(id, |t| t.navigations).unwrap_or(0);
        std::thread::sleep(Duration::from_millis(150));
        if self.with_tab(id, |t| t.navigations).unwrap_or(0) != before || self.is_loading(id) {
            self.wait_for_load(id, timeout);
        }
        let _ = self.script(id, "await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))".into(), false, Duration::from_secs(2));
    }

    fn wait_for_navigation(&self, id: u32, a: &Value) -> Result<(), Fail> {
        if a["wait_until"].as_str() == Some("none") {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
        let timeout = a["timeout"].as_f64().unwrap_or(30.).clamp(1., 60.);
        let finished = self.wait_for_load(id, Duration::from_secs_f64(timeout));
        if let Some(error) = self.with_tab(id, |t| t.nav_error.take())? {
            return Err(format!("Couldn't load {}: {error}", self.with_tab(id, |t| t.short_url())?));
        }
        if !finished {
            self.with_tab(id, |t| t.log(&json!({"kind": "console", "level": "info", "text": "Still loading after the timeout."})))?;
        }
        self.settle(id, Duration::from_secs(2));
        Ok(())
    }

    /// Acting while the page waits on a dialog would hang: say so instead.
    fn check_dialog(&self, id: u32) -> Result<(), Fail> {
        match self.with_tab(id, |t| t.dialog.clone())? {
            Some((kind, message, _)) => Err(format!("The page is waiting on a {kind}: {}. Answer it with dialog first.", quoted(&message, 200))),
            None => Ok(()),
        }
    }

    fn has_target(a: &Value) -> bool {
        a["ref"].is_string() || a["selector"].is_string() || a["label"].is_string()
    }

    fn target(a: &Value) -> Value {
        let mut t = serde_json::Map::new();
        if let Some(r) = a["ref"].as_str() {
            t.insert("ref".into(), r.into());
        }
        if let Some(s) = a["selector"].as_str() {
            t.insert("selector".into(), s.into());
        }
        if let Some(l) = a["label"].as_str() {
            t.insert("text".into(), l.into());
        }
        Value::Object(t)
    }

    /// Where an element is: its point to click, its box, how it's described, what covers it.
    fn locate(&self, id: u32, a: &Value) -> Result<(f64, f64, (f64, f64, f64, f64), String, Option<String>), Fail> {
        if !Self::has_target(a) {
            return Err("Say which element: a ref from the latest snapshot, a CSS selector, or its text.".into());
        }
        let info = self.helper(id, &format!("const t = {}; return __yh.point(t)", Self::target(a)))?;
        let r: Vec<f64> = info["rect"].as_array().map(|v| v.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
        let rect = if r.len() == 4 { (r[0], r[1], r[2], r[3]) } else { (0., 0., 0., 0.) };
        Ok((
            info["x"].as_f64().unwrap_or(rect.0 + rect.2 / 2.),
            info["y"].as_f64().unwrap_or(rect.1 + rect.3 / 2.),
            rect,
            info["describe"].as_str().unwrap_or("the element").to_string(),
            info["blockedBy"].as_str().map(str::to_string),
        ))
    }

    /// New console errors and failed requests since the agent last heard about them.
    fn news(&self, id: u32) -> Option<String> {
        if self.read_only {
            return None;
        }
        self.with_tab(id, |t| {
            let errors: Vec<Console> = t.console.iter().filter(|c| c.id > t.reported_console && c.is_error()).cloned().collect();
            let failed: Vec<Request> = t.network.iter().filter(|r| r.id > t.reported_network && r.failed()).cloned().collect();
            t.reported_console = t.reported_console.max(t.console.last().map_or(0, |c| c.id));
            t.reported_network = t.reported_network.max(t.network.last().map_or(0, |r| r.id));
            if errors.is_empty() && failed.is_empty() {
                return None;
            }
            let mut parts = vec![];
            if !errors.is_empty() {
                parts.push(count(errors.len(), "console error"));
            }
            if !failed.is_empty() {
                parts.push(count(failed.len(), "failed request"));
            }
            let mut lines = vec![format!("New in the page: {} (see console, network).", parts.join(", "))];
            for e in errors.iter().take(2) {
                lines.push(format!("  {}", one_line(&e.text, 240)));
            }
            for r in failed.iter().take(2) {
                lines.push(format!("  {} {} → {}", r.method, one_line(&r.url, 160), r.status.map(|s| s.to_string()).or(r.failure.clone()).unwrap_or_else(|| "failed".into())));
            }
            Some(lines.join("\n"))
        })
        .ok()
        .flatten()
    }

    /// What every action says: what it did, where the page is if that changed, and what
    /// went wrong in the page since the agent last heard.
    fn result(&self, id: u32, line: &str, show_page: bool) -> ToolResult {
        let page = self
            .with_tab(id, move |t| {
                t.restored = false;
                let (url, title) = (t.web.url(), t.title());
                let changed = show_page || t.last_url != url || t.last_title.as_deref() != Some(title.as_str());
                t.last_url = url;
                t.last_title = Some(title.clone());
                changed.then(|| {
                    format!(
                        "Page: {}{}{}",
                        t.short_url(),
                        if title.is_empty() { String::new() } else { format!(" — {}", quoted(&title, 80)) },
                        if t.web.is_loading() { " (still loading)" } else { "" }
                    )
                })
            })
            .ok()
            .flatten();
        let mut lines = vec![format!("{line}.")];
        lines.extend(page);
        if let Ok(Some((kind, message, _))) = self.with_tab(id, |t| t.dialog.clone()) {
            lines.push(format!("The page opened a {kind}: {}. Answer it with dialog.", quoted(&message, 200)));
        }
        lines.extend(self.news(id));
        ToolResult::text(lines.join("\n"))
    }

    fn reveal(&self) {
        let thread = self.thread.clone();
        let _ = self.main(move |b, cx| {
            let reveal = b.reveal.take();
            if let Some(f) = &reveal {
                f(&thread, cx);
            }
            b.reveal = reveal;
        });
    }

    fn status(&self) -> Result<String, Fail> {
        let thread = self.thread.clone();
        self.main(move |b, _| {
            let Some(s) = b.sessions.get(&thread).filter(|s| !s.tabs.is_empty()) else { return "No pages are open. Use open with a URL.".to_string() };
            let current = s.selected_tab().map(|t| t.id);
            let mut lines = vec!["Tabs:".to_string()];
            for t in &s.tabs {
                let mut line = format!("{} {}  {}", if Some(t.id) == current { "*" } else { " " }, t.id, t.short_url());
                let title = t.title();
                if !title.is_empty() {
                    line += &format!(" — {}", quoted(&title, 80));
                }
                if t.web.is_loading() {
                    line += &format!(" (loading {}%)", (t.web.progress() * 100.) as i64);
                }
                if t.restored {
                    line += " (reopened from last session)";
                }
                lines.push(line);
                if let Some((kind, message, _)) = &t.dialog {
                    lines.push(format!("    waiting on a {kind}: {}", quoted(message, 120)));
                }
            }
            if let Some(t) = s.selected_tab() {
                let viewport = match t.web.viewport() {
                    Some((w, h)) => format!("{}×{}", w as i64, h as i64),
                    None => "fits the panel".into(),
                };
                lines.push(format!("Viewport: {viewport}{}.", t.color_scheme.as_ref().map(|c| format!(", {c}")).unwrap_or_default()));
                let errors = t.console.iter().filter(|c| c.is_error()).count();
                let failed = t.network.iter().filter(|r| r.failed()).count();
                lines.push(format!("Logged: {} ({}), {} ({failed} failed).", count(t.console.len(), "console message"), count(errors, "error"), count(t.network.len(), "request")));
            }
            lines.push(if s.user_has_control {
                "The user has taken control: you can look, but actions fail until they give it back.".into()
            } else {
                "You can act; the user watches in the app and may take over.".into()
            });
            lines.join("\n")
        })
    }

    fn snapshot(&self, id: u32, selector: Option<&str>) -> Result<String, Fail> {
        if self.is_loading(id) {
            self.wait_for_load(id, Duration::from_secs(10));
        }
        let start = self.with_tab(id, |t| t.next_ref)?;
        let mut options = json!({ "budget": OUTLINE_BUDGET, "keepRefs": self.read_only, "start": start });
        if let Some(s) = selector {
            options["selector"] = s.into();
        }
        let info = self.helper(id, &format!("const o = {options}; return __yh.snapshot(o)"))?;
        if !info.is_object() {
            return Err("Couldn't read the page.".into());
        }
        if let Some(next) = info["nextRef"].as_u64() {
            self.with_tab(id, move |t| t.next_ref = t.next_ref.max(next))?;
        }
        let thread = self.thread.clone();
        let control = self.main(move |b, _| b.sessions.get(&thread).is_some_and(|s| s.user_has_control))?;
        let dialog = self.with_tab(id, |t| t.dialog.clone())?;
        let ints = |k: &str| -> Vec<i64> { info[k].as_array().map(|a| a.iter().filter_map(|v| v.as_f64().map(|f| f as i64)).collect()).unwrap_or_default() };
        let (viewport, scroll, size) = (ints("viewport"), ints("scroll"), ints("size"));
        let short = self.with_tab(id, |t| t.short_url())?;
        let title = info["title"].as_str().filter(|t| !t.is_empty()).map(|t| format!(" — {}", quoted(t, 100))).unwrap_or_default();
        let mut lines = vec![format!("Tab {id}: {short}{title}")];
        if viewport.len() == 2 && scroll.len() == 2 && size.len() == 2 {
            lines.push(format!("Viewport {}×{}, scrolled to {},{} of {}×{}.", viewport[0], viewport[1], scroll[0], scroll[1], size[0], size[1]));
        }
        if let Some((kind, message, _)) = dialog {
            lines.push(format!("A {kind} is open: {}. Answer it with dialog.", quoted(&message, 200)));
        }
        if control {
            lines.push("The user has control: you can read, not act.".into());
        }
        lines.push(String::new());
        let outline = info["outline"].as_str().unwrap_or("");
        lines.push(if outline.is_empty() { "(The page shows nothing readable.)".into() } else { outline.to_string() });
        if let Some(dropped) = info["dropped"].as_i64().filter(|d| *d > 0) {
            lines.push(String::new());
            lines.push(format!("({dropped} lines left out to keep this short, text and off-screen parts first. Snapshot a part with selector, or scroll and snapshot again.)"));
        }
        if let Some(note) = self.news(id) {
            lines.push(String::new());
            lines.push(note);
        }
        Ok(lines.join("\n"))
    }

    /// A phone- or tablet-sized viewport (not fitting the panel) on a page that doesn't lay itself
    /// out for one: a real device would show it 980px wide, shrunk.
    fn mobile_note(&self, id: u32) -> Option<String> {
        let (viewport, url) = self.with_tab(id, |t| (t.web.viewport(), t.web.url())).ok()?;
        let (w, _) = viewport.filter(|(w, _)| *w < 1024.)?;
        url.filter(|u| u != "about:blank")?;
        let meta = self.helper(id, "return document.querySelector('meta[name=viewport]')?.content ?? ''").ok()?.as_str()?.to_string();
        (!meta.replace(' ', "").to_lowercase().contains("width=device-width")).then(|| {
            format!("This page has no <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">: on a real phone or tablet it's laid out 980px wide and shrunk to fit, so it won't look like this {}px layout there.", w as i64)
        })
    }

    fn screenshot(&self, id: u32, a: &Value, tool_use: Option<&str>) -> Result<ToolResult, Fail> {
        let rect = if Self::has_target(a) {
            let (_, _, r, _, _) = self.locate(id, a)?;
            Some((r.0 - 8., r.1 - 8., r.2 + 16., r.3 + 16.))
        } else {
            None
        };
        let (w, h) = self.with_tab(id, |t| t.web.size())?;
        // The whole page: the view grows to the page's height for the capture, then back. The
        // layout stays at the viewport's size meanwhile, so a `100vh` hero is one screen tall.
        let full = if rect.is_none() && a["full_page"].as_bool() == Some(true) {
            self.helper(id, "return Math.min(document.documentElement.scrollHeight, 12000)")?.as_f64().filter(|full| *full > h + 1.)
        } else {
            None
        };
        let mut scrolled = 0.;
        if let Some(full) = full {
            scrolled = self.helper(id, "const s = document.scrollingElement; const y = s.scrollTop; s.scrollTop = 0; return y")?.as_f64().unwrap_or(0.);
            self.with_tab(id, move |t| t.web.hold_full_height(Some(full)))?;
            std::thread::sleep(Duration::from_millis(120));
            self.settle(id, Duration::from_secs(1));
        }
        let rect = rect.or(full.map(|full| (0., 0., w, full)));
        let path = std::env::temp_dir().join(format!("yh-shot-{}-{id}.jpg", std::process::id())).to_string_lossy().into_owned();
        let (tx, rx) = flume::bounded(1);
        let p = path.clone();
        let shown = self.with_tab(id, move |t| {
            let shown = t.web.is_shown();
            t.web.capture(rect, 1280., p, tx);
            shown
        })?;
        let captured = rx.recv_timeout(Duration::from_secs(20));
        if full.is_some() {
            self.with_tab(id, |t| t.web.hold_full_height(None))?;
            let _ = self.helper(id, &format!("document.scrollingElement.scrollTop = {scrolled}"));
        }
        let (pw, ph) = captured.map_err(|_| if shown { "Couldn't capture the page." } else { "The browser isn't on screen: open it with open (show: true) first." }.to_string())??;
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&path);
        // A copy for the transcript, whose step shows it (`<thread>-browser-<tool use>.jpg`).
        if let Some(tool_use) = tool_use {
            let dir = yh_core::library::data_directory().join("attachments");
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join(format!("{}-browser-{tool_use}.jpg", self.thread)), &bytes);
        }
        let css_width = rect.map_or(w, |r| r.2);
        let scale = pw as f64 / css_width.max(1.);
        let mut text = format!("Screenshot of tab {id}, {} ({pw}×{ph}", self.with_tab(id, |t| t.short_url())?);
        text += &if (scale - 1.).abs() > 0.01 { format!(", {scale:.2}× CSS pixels") } else { ", 1 pixel = 1 CSS pixel".into() };
        text += &format!("; viewport {}×{}).", w as i64, h as i64);
        if let Some(note) = self.mobile_note(id) {
            text += &format!("\n{note}");
        }
        if let Some(note) = self.news(id) {
            text += &format!("\n{note}");
        }
        let mut result = ToolResult::text(text);
        result.images.push(("image/jpeg".into(), yh_core::attachments::base64(&bytes)));
        Ok(result)
    }

    fn perform(&self, name: &str, a: &Value, tool_use: Option<&str>) -> Result<ToolResult, Fail> {
        // While the user has control, the agent can only look.
        const ACTIONS: &[&str] = &["open", "navigate", "click", "hover", "type", "press", "select", "scroll", "drag", "evaluate", "close", "resize", "dialog", "upload"];
        if ACTIONS.contains(&name) {
            self.check_control()?;
        }
        match name {
            "status" => Ok(ToolResult::text(self.status()?)),
            "open" => {
                let address = a["url"].as_str().ok_or("Give a url to open.")?.to_string();
                let url = url_from(&address).ok_or("Give a url to open.")?;
                let thread = self.thread.clone();
                let project = self.project.clone().filter(|p| p.len() == 36);
                let new_tab = a["new_tab"].as_bool() == Some(true);
                let id = self.main(move |b, _| {
                    let current = b.sessions.get(&thread).and_then(|s| s.selected_tab().map(|t| t.id));
                    match current.filter(|_| !new_tab) {
                        Some(id) => {
                            let s = b.sessions.get_mut(&thread).unwrap();
                            if let Some(t) = s.tabs.iter_mut().find(|t| t.id == id) {
                                t.load(&url);
                            }
                            s.selected = Some(id);
                            Ok(id)
                        }
                        None => {
                            if b.sessions.get(&thread).is_some_and(|s| s.tabs.len() >= MAX_TABS) {
                                return Err(format!("{MAX_TABS} tabs are open, the most there can be. Close one first."));
                            }
                            Ok(b.open_tab(&thread, project.as_deref(), &url))
                        }
                    }
                })??;
                if a["show"].as_bool() != Some(false) {
                    self.reveal();
                }
                self.wait_for_navigation(id, a)?;
                let short = self.with_tab(id, |t| t.short_url())?;
                Ok(self.result(id, &format!("Opened {short} in tab {id}"), true))
            }
            "navigate" => {
                let id = self.tab_id(a)?;
                self.check_dialog(id)?;
                let before = self.with_tab(id, |t| t.web.url())?;
                let action = a["action"].as_str().map(str::to_string);
                if let Some(address) = a["url"].as_str() {
                    let url = url_from(address).ok_or("Give a url.")?;
                    self.with_tab(id, move |t| t.load(&url))?;
                } else {
                    let act = action.clone();
                    self.with_tab(id, move |t| match act.as_deref() {
                        Some("back") if t.web.can_go_back() => {
                            t.web.go_back();
                            Ok(())
                        }
                        Some("back") => Err("There's no page to go back to.".to_string()),
                        Some("forward") if t.web.can_go_forward() => {
                            t.web.go_forward();
                            Ok(())
                        }
                        Some("forward") => Err("There's no page to go forward to.".to_string()),
                        Some("reload") => {
                            t.web.reload();
                            Ok(())
                        }
                        _ => Err("Give a url, or an action: back, forward or reload.".to_string()),
                    })??;
                }
                self.wait_for_navigation(id, a)?;
                let short = self.with_tab(id, |t| t.short_url())?;
                let verb = match action.as_deref() {
                    Some("reload") => "Reloaded".to_string(),
                    Some(other) => format!("Went {other}"),
                    None => "Went".to_string(),
                };
                let changed = before != self.with_tab(id, |t| t.web.url())? || action.as_deref() == Some("reload");
                Ok(self.result(id, &format!("{verb} to {short}"), changed))
            }
            "snapshot" => {
                let id = self.tab_id(a)?;
                Ok(ToolResult::text(self.snapshot(id, a["selector"].as_str())?))
            }
            "screenshot" => {
                let id = self.tab_id(a)?;
                self.screenshot(id, a, tool_use)
            }
            "click" => {
                let id = self.tab_id(a)?;
                self.check_dialog(id)?;
                let right = a["button"].as_str() == Some("right");
                let count = a["count"].as_i64().unwrap_or(1).clamp(1, 3) as isize;
                let modifiers = modifier_flags(&a["modifiers"]);
                let described = if let (Some(x), Some(y), false) = (a["x"].as_f64(), a["y"].as_f64(), Self::has_target(a)) {
                    self.show(id, "Click", Some((x - 12., y - 12., 24., 24.)));
                    self.with_tab(id, move |t| t.web.click(x, y, right, count, modifiers))?;
                    format!("at {},{}", x as i64, y as i64)
                } else {
                    let (x, y, rect, describe, blocked) = self.locate(id, a)?;
                    if let Some(blocker) = &blocked {
                        if a["force"].as_bool() != Some(true) {
                            return Err(format!("{describe} is covered by {blocker} there. Close or scroll that away first, or pass force: true to click it with a script."));
                        }
                    }
                    self.show(id, "Click", Some(rect));
                    if blocked.is_some() {
                        self.helper(id, &format!("const t = {}; return __yh.click(t)", Self::target(a)))?;
                    } else {
                        self.with_tab(id, move |t| t.web.click(x, y, right, count, modifiers))?;
                    }
                    describe
                };
                self.settle(id, Duration::from_secs(10));
                let verb = if count == 2 { "Double-clicked" } else if right { "Right-clicked" } else { "Clicked" };
                Ok(self.result(id, &format!("{verb} {described}"), false))
            }
            "hover" => {
                let id = self.tab_id(a)?;
                let (x, y, rect, describe, _) = self.locate(id, a)?;
                self.show(id, "Hover", Some(rect));
                self.with_tab(id, move |t| t.web.hover(x, y))?;
                self.settle(id, Duration::from_secs(2));
                Ok(self.result(id, &format!("Hovered over {describe}"), false))
            }
            "type" => {
                let id = self.tab_id(a)?;
                self.check_dialog(id)?;
                let text = a["text"].as_str().ok_or("Give the text to type.")?.to_string();
                let clear = a["clear"].as_bool().unwrap_or(true);
                let mut described = "the focused element".to_string();
                if Self::has_target(a) {
                    let (_, _, rect, describe, _) = self.locate(id, a)?;
                    self.show(id, "Type", Some(rect));
                    described = describe;
                }
                let target = Self::target(a);
                if let Some(d) = self.helper(id, &format!("const t = {target}; return __yh.prepareTyping(t, {clear})"))?.as_str() {
                    described = d.to_string();
                }
                if clear && text.is_empty() {
                    self.helper(id, &format!("const t = {target}; __yh.clear(t)"))?;
                } else {
                    let t2 = text.clone();
                    self.with_tab(id, move |t| t.web.insert(&t2))?;
                }
                let submit = a["submit"].as_bool() == Some(true);
                if submit {
                    let key = Key::parse("Enter").unwrap();
                    self.with_tab(id, move |t| t.web.press(key.code, &key.characters, &key.plain, key.modifiers))?;
                }
                self.settle(id, Duration::from_secs(10));
                let secret = self.helper(id, &format!("const t = {target}; return __yh.isSecret(t)")).ok().and_then(|v| v.as_bool()).unwrap_or(false);
                let mut line = if secret { format!("Typed {} characters into {described}", text.chars().count()) } else { format!("Typed {} into {described}", quoted(&text, 60)) };
                if !secret {
                    if let Some(value) = self.helper(id, &format!("const t = {target}; return __yh.value(t)")).ok().and_then(|v| v.as_str().map(str::to_string)) {
                        if value != text && !value.is_empty() {
                            line += &format!("; it now holds {}", quoted(&value, 80));
                        }
                    }
                }
                if submit {
                    line += ", and pressed Enter";
                }
                Ok(self.result(id, &line, false))
            }
            "press" => {
                let id = self.tab_id(a)?;
                self.check_dialog(id)?;
                let spec = a["key"].as_str().unwrap_or("").to_string();
                let key = Key::parse(&spec).ok_or("Unknown key. Use names like Enter, Tab, Escape, Backspace, ArrowDown, PageDown, a, or combinations like Shift+Tab and Meta+A.")?;
                if Self::has_target(a) {
                    let rect = self.locate(id, a)?.2;
                    self.helper(id, &format!("const t = {}; __yh.prepareTyping(t, false)", Self::target(a)))?;
                    self.show(id, &format!("Press {spec}"), Some(rect));
                } else {
                    self.show(id, &format!("Press {spec}"), None);
                }
                let count = a["count"].as_i64().unwrap_or(1).clamp(1, 50);
                for _ in 0..count {
                    let k = key.clone();
                    self.with_tab(id, move |t| t.web.press(k.code, &k.characters, &k.plain, k.modifiers))?;
                    // ⌘A in a page, without the menu command that would need the keyboard focus.
                    if key.modifiers == COMMAND && key.plain == "a" {
                        self.helper(id, "__yh.selectAll()")?;
                    }
                }
                self.settle(id, Duration::from_secs(10));
                Ok(self.result(id, &format!("Pressed {spec}{}", if count > 1 { format!(" ×{count}") } else { String::new() }), false))
            }
            "select" => {
                let id = self.tab_id(a)?;
                // As the schema says, or one option as models sometimes write it.
                let values: Vec<String> = match a["values"].as_array() {
                    Some(v) => v.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
                    None => a["value"].as_str().map(|v| vec![v.to_string()]).unwrap_or_default(),
                };
                if values.is_empty() {
                    return Err("Give the option to select, as values.".into());
                }
                let (_, _, rect, describe, _) = self.locate(id, a)?;
                self.show(id, "Select", Some(rect));
                let picked = self.helper(id, &format!("const t = {}; const v = {}; return __yh.select(t, v)", Self::target(a), json!(values)))?;
                let picked: Vec<String> = picked.as_array().map(|v| v.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or(values);
                self.settle(id, Duration::from_secs(10));
                Ok(self.result(id, &format!("Selected {} in {describe}", picked.iter().map(|p| quoted(p, 40)).collect::<Vec<_>>().join(", ")), false))
            }
            "scroll" => {
                let id = self.tab_id(a)?;
                let mut o = serde_json::Map::new();
                if Self::has_target(a) {
                    o.insert("target".into(), Self::target(a));
                }
                for k in ["direction", "amount", "to"] {
                    if !a[k].is_null() {
                        o.insert(k.into(), a[k].clone());
                    }
                }
                let line = self.helper(id, &format!("const o = {}; return __yh.scroll(o)", Value::Object(o)))?;
                self.show(id, "Scroll", None);
                self.settle(id, Duration::from_secs(2));
                Ok(self.result(id, line.as_str().unwrap_or("Scrolled"), false))
            }
            "drag" => {
                let id = self.tab_id(a)?;
                let (from, to) = (a["from"].clone(), a["to"].clone());
                if from.is_null() || to.is_null() {
                    return Err("Give from and to, each a ref or selector.".into());
                }
                let (fx, fy, start, fd, _) = self.locate(id, &from)?;
                let (tx, ty, _, td, _) = self.locate(id, &to)?;
                self.show(id, "Drag", Some(start));
                // From a `draggable` element it's an HTML drag and drop, played as the page's drag
                // events: WebKit hands that to an AppKit drag session, which follows the real
                // mouse, so mouse events alone never drop anything.
                let html = self.script(id, format!("const from = [{fx}, {fy}], to = [{tx}, {ty}];\n{HTML_DRAG}"), true, Duration::from_secs(10))?;
                if html.is_object() {
                    self.with_tab(id, move |t| t.web.hover(tx, ty))?;
                    self.settle(id, Duration::from_secs(10));
                    let target = html["target"].as_str().unwrap_or("");
                    let line = if html["started"].as_bool() != Some(true) {
                        format!("The page cancelled the drag of {fd} at dragstart")
                    } else if html["dropped"].as_bool() == Some(true) {
                        format!("Dragged {fd} to {td} (HTML drag and drop; {target} took the drop)")
                    } else {
                        format!("Dragged {fd} to {td}, but nothing there accepts the drop (HTML drag and drop ended over {target}, which didn't cancel dragover)")
                    };
                    return Ok(self.result(id, &line, false));
                }
                // A mouse drag, a frame a step, as a hand moves it.
                self.with_tab(id, move |t| t.web.mouse_down(fx, fy))?;
                for step in 1..=12 {
                    let f = step as f64 / 12.;
                    let (x, y) = (fx + (tx - fx) * f, fy + (ty - fy) * f);
                    self.with_tab(id, move |t| t.web.mouse_dragged(x, y))?;
                    std::thread::sleep(Duration::from_millis(16));
                }
                self.with_tab(id, move |t| t.web.mouse_up(tx, ty))?;
                self.settle(id, Duration::from_secs(10));
                Ok(self.result(id, &format!("Dragged {fd} to {td}"), false))
            }
            "wait_for" => {
                let id = self.tab_id(a)?;
                let timeout = a["timeout"].as_f64().unwrap_or(10.).clamp(0.1, 60.);
                let started = Instant::now();
                if let Some(fragment) = a["url"].as_str() {
                    loop {
                        let url = self.with_tab(id, |t| t.web.url())?.unwrap_or_default();
                        if url.contains(fragment) {
                            break;
                        }
                        if started.elapsed().as_secs_f64() > timeout {
                            return Err(format!("Timed out after {} s waiting for the URL to contain {fragment}; it's {}.", timeout as i64, self.with_tab(id, |t| t.short_url())?));
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    self.wait_for_load(id, Duration::from_secs_f64((timeout - started.elapsed().as_secs_f64()).max(1.)));
                }
                let mut o = serde_json::Map::new();
                o.insert("timeout".into(), json!((timeout - started.elapsed().as_secs_f64()) * 1000.));
                for k in ["text", "selector", "gone"] {
                    if let Some(v) = a[k].as_str() {
                        o.insert(k.into(), v.into());
                    }
                }
                if o.len() > 1 {
                    self.script(id, format!("const o = {}; return await __yh.waitFor(o)", Value::Object(o)), false, Duration::from_secs_f64(timeout + 5.))?;
                }
                Ok(self.result(id, &format!("Found it after {:.1} s", started.elapsed().as_secs_f64()), false))
            }
            "console" => {
                let id = self.tab_id(a)?;
                let level = a["level"].as_str().unwrap_or("all").to_string();
                let all = a["all"].as_bool() == Some(true) || self.read_only;
                let limit = a["limit"].as_u64().unwrap_or(100).clamp(1, 500) as usize;
                let ro = self.read_only;
                Ok(ToolResult::text(self.with_tab(id, move |t| {
                    let mut entries: Vec<Console> = t.console.iter().filter(|c| all || c.id > t.console_read).cloned().collect();
                    match level.as_str() {
                        "error" => entries.retain(|c| c.is_error()),
                        "warning" => entries.retain(|c| c.is_error() || c.level == "warn"),
                        _ => {}
                    }
                    if !ro {
                        t.console_read = t.console.last().map_or(0, |c| c.id);
                    }
                    if entries.is_empty() {
                        return if all { "The console is empty.".into() } else { "Nothing new in the console since you last looked.".into() };
                    }
                    let skip = entries.len().saturating_sub(limit);
                    let mut lines: Vec<String> = entries[skip..].iter().map(|c| format!("[{}] {}", c.level, clip(&c.text, 2000))).collect();
                    if skip > 0 {
                        lines.insert(0, format!("({skip} older messages not shown)"));
                    }
                    lines.join("\n")
                })?))
            }
            "network" => {
                let id = self.tab_id(a)?;
                let all = a["all"].as_bool() == Some(true) || self.read_only;
                let limit = a["limit"].as_u64().unwrap_or(100).clamp(1, 500) as usize;
                let failed_only = a["failed_only"].as_bool() == Some(true);
                let filter = a["filter"].as_str().unwrap_or("").to_lowercase();
                let ro = self.read_only;
                Ok(ToolResult::text(self.with_tab(id, move |t| {
                    let mut entries: Vec<Request> = t.network.iter().filter(|r| all || r.id > t.network_read).cloned().collect();
                    if failed_only {
                        entries.retain(|r| r.failed());
                    }
                    if !filter.is_empty() {
                        entries.retain(|r| r.url.to_lowercase().contains(&filter));
                    }
                    if !ro {
                        t.network_read = t.network.last().map_or(0, |r| r.id);
                    }
                    if entries.is_empty() {
                        return if all { "No requests logged.".into() } else { "No new requests since you last looked.".into() };
                    }
                    let skip = entries.len().saturating_sub(limit);
                    let mut lines: Vec<String> = entries[skip..]
                        .iter()
                        .map(|r| {
                            let mut line = format!("{} {} → {} [{}]", r.method, clip(&r.url, 300), r.status.map(|s| s.to_string()).or(r.failure.clone()).unwrap_or_else(|| "…".into()), r.kind);
                            if let Some(ms) = r.ms {
                                line += &format!(" {} ms", ms as i64);
                            }
                            if let (Some(f), Some(_)) = (&r.failure, r.status) {
                                line += &format!(" {f}");
                            }
                            line
                        })
                        .collect();
                    if skip > 0 {
                        lines.insert(0, format!("({skip} older requests not shown)"));
                    }
                    lines.join("\n")
                })?))
            }
            "evaluate" => {
                let id = self.tab_id(a)?;
                self.check_dialog(id)?;
                let source = a["expression"].as_str().ok_or("Give the JavaScript to run, as expression.")?.to_string();
                self.show(id, "Run JavaScript", None);
                // An expression, or a function body with its own return: told apart in our world.
                let is_expression = self.script(id, format!("const s = {}; return __yh.isExpression(s)", json!(source)), false, Duration::from_secs(5)).ok().and_then(|v| v.as_bool()).unwrap_or(true);
                let body = if is_expression { format!("return (\n{source}\n);") } else { source };
                let wrapped = format!(
                    "const __yhValue = await (async () => {{ {body} }})();
const __yhSeen = new WeakSet();
return JSON.stringify(__yhValue, (key, v) => {{
  if (typeof Node !== 'undefined' && v instanceof Node) return v.nodeType === 1 ? v.outerHTML.slice(0, 500) : String(v.textContent).slice(0, 500);
  if (v instanceof Error) return String(v);
  if (typeof v === 'function') return 'ƒ ' + (v.name || 'anonymous');
  if (typeof v === 'bigint') return String(v);
  if (v && typeof v === 'object') {{ if (__yhSeen.has(v)) return '[circular]'; __yhSeen.add(v); }}
  return v;
}}) ?? 'undefined';"
                );
                let value = self.script(id, wrapped, true, Duration::from_secs(30))?;
                // The page's JSON text came back as a JSON string of it.
                let mut text = match value {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                let n = text.chars().count();
                if n > 20_000 {
                    text = format!("{}… ({} more characters; return less)", text.chars().take(20_000).collect::<String>(), n - 20_000);
                }
                if let Some(note) = self.news(id) {
                    text += &format!("\n\n{note}");
                }
                Ok(ToolResult::text(text))
            }
            "close" => {
                let id = self.tab_id(a)?;
                let thread = self.thread.clone();
                Ok(ToolResult::text(self.main(move |b, _| {
                    b.close_tab(&thread, id);
                    let s = b.sessions.get(&thread);
                    format!(
                        "Closed tab {id}. {}",
                        match s.and_then(|s| s.selected_tab()) {
                            Some(t) => format!("Tab {} is current.", t.id),
                            None => "No tabs are open.".into(),
                        }
                    )
                })?))
            }
            "resize" => {
                let id = self.tab_id(a)?;
                let mut viewport = self.with_tab(id, |t| t.web.viewport())?;
                match a["preset"].as_str() {
                    Some("laptop") => viewport = Some(crate::webview::LAPTOP),
                    Some("desktop") => viewport = Some(crate::webview::DESKTOP),
                    Some("tablet") => viewport = Some(crate::webview::TABLET),
                    Some("phone") => viewport = Some(crate::webview::PHONE),
                    Some("fit") => viewport = None,
                    _ => {}
                }
                let (cw, ch) = viewport.unwrap_or(crate::webview::LAPTOP);
                if let Some(w) = a["width"].as_f64() {
                    viewport = Some((w.clamp(200., 3840.), viewport.map_or(ch, |v| v.1)));
                }
                if let Some(h) = a["height"].as_f64() {
                    viewport = Some((viewport.map_or(cw, |v| v.0), h.clamp(200., 2400.)));
                }
                // As the schema says, or as models sometimes write it.
                let scheme = a["color_scheme"].as_str().or(a["colorScheme"].as_str()).map(str::to_string);
                self.with_tab(id, move |t| {
                    t.web.set_viewport(viewport);
                    if let Some(s) = scheme {
                        t.set_color_scheme(&s);
                    }
                })?;
                // Laid out with the panel's next frame.
                let thread = self.thread.clone();
                self.main(move |b, cx| {
                    let _ = thread;
                    b.place();
                    cx.refresh_windows();
                })?;
                self.settle(id, Duration::from_secs(2));
                let (w, h) = self.with_tab(id, |t| t.web.size())?;
                let scheme = self.with_tab(id, |t| t.color_scheme.clone())?;
                let mut line = format!("Viewport is {}×{}{}", w as i64, h as i64, scheme.map(|s| format!(", {s}")).unwrap_or_default());
                if let Some(note) = self.mobile_note(id) {
                    line += &format!(".\n{}", note.trim_end_matches('.'));
                }
                Ok(self.result(id, &line, false))
            }
            "dialog" => {
                let id = self.tab_id(a)?;
                let Some((kind, message, _)) = self.with_tab(id, |t| t.dialog.clone())? else { return Err("No dialog is open.".into()) };
                let accept = a["accept"].as_bool().unwrap_or(true);
                let text = a["text"].as_str().map(str::to_string);
                self.with_tab(id, move |t| {
                    t.dialog = None;
                    t.web.answer_dialog(accept, text)
                })?;
                self.settle(id, Duration::from_secs(10));
                Ok(self.result(id, &format!("{} the {kind} {}", if accept { "Accepted" } else { "Dismissed" }, quoted(&message, 80)), false))
            }
            "upload" => {
                let id = self.tab_id(a)?;
                let paths: Vec<String> = a["paths"].as_array().map(|p| p.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
                if paths.is_empty() {
                    return Err("Give absolute file paths, as paths.".into());
                }
                let home = std::env::var("HOME").unwrap_or_default();
                let paths: Vec<String> = paths.into_iter().map(|p| p.strip_prefix("~/").map(|r| format!("{home}/{r}")).unwrap_or(p)).collect();
                if let Some(missing) = paths.iter().find(|p| !std::path::Path::new(p).exists()) {
                    return Err(format!("No file at {missing}."));
                }
                let names: Vec<String> = paths.iter().map(|p| p.rsplit('/').next().unwrap_or(p).to_string()).collect();
                let (x, y, rect, describe, _) = self.locate(id, a)?;
                self.show(id, "Upload", Some(rect));
                self.with_tab(id, move |t| {
                    t.web.set_upload(Some(paths));
                    t.web.click(x, y, false, 1, 0);
                })?;
                self.settle(id, Duration::from_secs(10));
                if self.with_tab(id, |t| t.web.set_upload(None))? {
                    return Err(format!("Clicking {describe} didn't open a file chooser. Target the <input type=file> or the button that opens it."));
                }
                Ok(self.result(id, &format!("Uploaded {} with {describe}", names.join(", ")), false))
            }
            other => Err(format!("Unknown tool {other}.")),
        }
    }
}

/// Null unless the drag starts on a `draggable` element; then the events a browser fires for
/// it, in order, and whether the drop was taken (the last dragover was cancelled). Runs in the
/// page's world, so its handlers see this drag's dropEffect and effectAllowed; `from` and `to`
/// are defined before it. The Swift app's `BrowserTab.htmlDrag`.
const HTML_DRAG: &str = r#"
const [x0, y0] = from, [x1, y1] = to;
const source = document.elementFromPoint(x0, y0)?.closest('[draggable="true"]');
if (!source) return null;
const data = new DataTransfer();
// A DataTransfer made by script ignores these outside a real drag; here they work as in one.
let dropEffect = 'none', effectAllowed = 'uninitialized';
Object.defineProperty(data, 'dropEffect', { configurable: true, get: () => dropEffect,
    set: (value) => { if (['none', 'copy', 'link', 'move'].includes(value)) dropEffect = value; } });
Object.defineProperty(data, 'effectAllowed', { configurable: true, get: () => effectAllowed,
    set: (value) => { if (['none', 'copy', 'copyLink', 'copyMove', 'link', 'linkMove', 'move', 'all', 'uninitialized'].includes(value)) effectAllowed = value; } });
const init = (x, y, extra) => ({ bubbles: true, cancelable: true, composed: true, view: window,
    clientX: x, clientY: y, screenX: screenX + x, screenY: screenY + y, ...extra });
const fire = (target, type, x, y, extra = {}) => target.dispatchEvent(
    type.startsWith('pointer') ? new PointerEvent(type, init(x, y, { pointerId: 1, pointerType: 'mouse', isPrimary: true, ...extra }))
    : type.startsWith('mouse') ? new MouseEvent(type, init(x, y, extra))
    : new DragEvent(type, init(x, y, { dataTransfer: data, ...extra })));
// What a browser starts each dragenter and dragover with, from effectAllowed; the page may change it.
const effect = () => ({ none: 'none', copy: 'copy', copyLink: 'copy', copyMove: 'copy', all: 'copy', link: 'link', linkMove: 'link' })[data.effectAllowed] ?? 'move';
const describe = (el) => el.id ? '#' + el.id : el.localName + (el.classList.length ? '.' + [...el.classList].join('.') : '');
fire(source, 'pointerdown', x0, y0, { button: 0, buttons: 1 });
fire(source, 'mousedown', x0, y0, { button: 0, buttons: 1 });
if (!fire(source, 'dragstart', x0, y0)) return { started: false, dropped: false, target: describe(source) };
fire(source, 'pointercancel', x0, y0);
let over = null, accepted = false;
for (let step = 1; step <= 12; step++) {
    const x = x0 + (x1 - x0) * step / 12, y = y0 + (y1 - y0) * step / 12;
    fire(source, 'drag', x, y);
    const under = document.elementFromPoint(x, y) ?? document.documentElement;
    if (under !== over) {
        data.dropEffect = effect();
        fire(under, 'dragenter', x, y);
        if (over) fire(over, 'dragleave', x, y);
        over = under;
    }
    data.dropEffect = effect();
    accepted = !fire(under, 'dragover', x, y) && data.dropEffect !== 'none';
    await new Promise((done) => setTimeout(done, 16));
}
if (accepted) fire(over, 'drop', x1, y1);
// dragend says what happened: the drop's effect, or none.
if (!accepted) data.dropEffect = 'none';
fire(source, 'dragend', x1, y1);
return { started: true, dropped: accepted, target: describe(over) };
"#;

const COMMAND: usize = 1 << 20;

fn modifier_flags(v: &Value) -> usize {
    let mut flags = 0;
    for name in v.as_array().into_iter().flatten().filter_map(Value::as_str) {
        flags |= match name.to_lowercase().as_str() {
            "meta" | "cmd" | "command" => COMMAND,
            "shift" => 1 << 17,
            "alt" | "option" => 1 << 19,
            "control" | "ctrl" => 1 << 18,
            _ => 0,
        };
    }
    flags
}

/// A key as the agent names it (`Enter`, `Shift+Tab`, `Meta+A`, `a`), as AppKit sends it.
#[derive(Clone)]
struct Key {
    code: u16,
    characters: String,
    plain: String,
    modifiers: usize,
}

impl Key {
    fn parse(spec: &str) -> Option<Key> {
        let mut parts: Vec<String> = spec.split('+').map(str::to_string).collect();
        // "Meta++" is Meta and "+".
        if spec.ends_with("++") {
            parts.truncate(parts.len().saturating_sub(2));
            parts.push("+".into());
        }
        let name = parts.pop().filter(|n| !n.is_empty())?;
        let mut modifiers = 0;
        for p in &parts {
            modifiers |= match p.to_lowercase().as_str() {
                "meta" | "cmd" | "command" | "⌘" | "controlormeta" => COMMAND,
                "control" | "ctrl" | "⌃" => 1 << 18,
                "alt" | "option" | "opt" | "⌥" => 1 << 19,
                "shift" | "⇧" => 1 << 17,
                _ => return None,
            };
        }
        let named: &[(&str, u16, u32)] = &[
            ("enter", 36, 0x0D), ("return", 36, 0x0D), ("tab", 48, 0x09), ("space", 49, 0x20), (" ", 49, 0x20), ("backspace", 51, 0x7F),
            ("escape", 53, 0x1B), ("esc", 53, 0x1B), ("delete", 117, 0xF728), ("arrowup", 126, 0xF700), ("arrowdown", 125, 0xF701),
            ("arrowleft", 123, 0xF702), ("arrowright", 124, 0xF703), ("up", 126, 0xF700), ("down", 125, 0xF701), ("left", 123, 0xF702),
            ("right", 124, 0xF703), ("home", 115, 0xF729), ("end", 119, 0xF72B), ("pageup", 116, 0xF72C), ("pagedown", 121, 0xF72D),
        ];
        if let Some((_, code, scalar)) = named.iter().find(|(n, _, _)| *n == name.to_lowercase()) {
            let c = char::from_u32(*scalar).unwrap_or(' ').to_string();
            // Function keys (arrows, Home…) carry the function flag.
            let function = if *code >= 115 { 1 << 23 } else { 0 };
            return Some(Key { code: *code, characters: c.clone(), plain: c, modifiers: modifiers | function });
        }
        let codes: &[(&str, u16)] = &[
            ("a", 0), ("s", 1), ("d", 2), ("f", 3), ("h", 4), ("g", 5), ("z", 6), ("x", 7), ("c", 8), ("v", 9), ("b", 11), ("q", 12), ("w", 13),
            ("e", 14), ("r", 15), ("y", 16), ("t", 17), ("1", 18), ("2", 19), ("3", 20), ("4", 21), ("6", 22), ("5", 23), ("=", 24), ("9", 25),
            ("7", 26), ("-", 27), ("8", 28), ("0", 29), ("]", 30), ("o", 31), ("u", 32), ("[", 33), ("i", 34), ("p", 35), ("l", 37), ("j", 38),
            ("'", 39), ("k", 40), (";", 41), ("\\", 42), (",", 43), ("/", 44), ("n", 45), ("m", 46), (".", 47), ("`", 50), ("+", 24),
        ];
        if name.chars().count() != 1 {
            return None;
        }
        let lower = name.to_lowercase();
        let (_, code) = codes.iter().find(|(k, _)| *k == lower)?;
        let shifted = if modifiers & (1 << 17) != 0 { name.to_uppercase() } else { name.clone() };
        let characters = if modifiers & (1 << 18) != 0 {
            char::from_u32((lower.chars().next()? as u32).saturating_sub(96).max(1)).unwrap_or(' ').to_string()
        } else {
            shifted
        };
        Some(Key { code: *code, characters, plain: lower, modifiers })
    }
}

/// What the agent is told about the browser.
pub fn instructions() -> &'static str {
    "YourHarness has a real web browser built in, which you drive with the mcp__browser__ tools. The user sees it in the app's sidebar and watches what you do there; they can take control at any time. Use it to check web UI you build or change: start the dev server in the background (Bash with run_in_background), then `open` its URL. Take a `snapshot` to read the page and get refs for click, type and the rest, and a `screenshot` to see how it looks. Every result mentions new console errors and failed requests; `console` and `network` have the details. Use this browser rather than Playwright, Puppeteer, headless Chrome or curl for checking pages, unless the user asks for those."
}

impl ToolServer for BrowserTools {
    fn name(&self) -> &str {
        "browser"
    }

    fn tools(&self) -> Vec<Value> {
        let tab = json!({ "type": "integer", "description": "Which tab, from status. Defaults to the current one." });
        let r#ref = json!({ "type": "string", "description": "An element's ref from the latest snapshot, e.g. e12." });
        let selector = json!({ "type": "string", "description": "A CSS selector, instead of a ref." });
        let label = json!({ "type": "string", "description": "Visible text of a button or link to find, instead of a ref." });
        let wait_until = json!({ "type": "string", "enum": ["load", "none"], "description": "Wait for the page to load (default), or return right away." });
        let element = |extra: Value| -> Value {
            let mut m = json!({ "ref": r#ref, "selector": selector, "label": label, "tab": tab });
            if let (Some(m), Some(e)) = (m.as_object_mut(), extra.as_object()) {
                for (k, v) in e {
                    m.insert(k.clone(), v.clone());
                }
            }
            m
        };
        let all = vec![
            tool("status", "The browser's tabs (URL, title, loading), the viewport, whether a dialog is waiting, and whether the user has taken control.", schema(json!({}), &[]), true, false),
            tool("open", "Open a URL in the browser built into the app, which the user watches. Loads in the current tab unless new_tab is true. Use it for the app you're building: start its dev server in the background first, then open http://localhost:<port>.",
                schema(json!({
                    "url": { "type": "string", "description": "A URL, a host, or localhost:3000." },
                    "new_tab": { "type": "boolean", "description": "Open another tab instead of using the current one." },
                    "show": { "type": "boolean", "description": "Show the browser to the user (default true)." },
                    "wait_until": wait_until,
                }), &["url"]), false, false),
            tool("navigate", "Go to a URL in the current tab, or back, forward, or reload.",
                schema(json!({ "url": { "type": "string" }, "action": { "type": "string", "enum": ["back", "forward", "reload"] }, "wait_until": wait_until, "tab": tab }), &[]), false, false),
            tool("snapshot", "Read the page as an outline: landmarks, headings, text, and interactive elements with refs ([ref=e12]) to use in click, type and the rest. Take one before acting, and again after the page changes. Password values are hidden.",
                schema(json!({ "selector": { "type": "string", "description": "Only this part of the page, for a big page." }, "tab": tab }), &[]), true, false),
            tool("screenshot", "See the page: the visible viewport, the whole page, or one element. Use it to check layout, styling and images, which the outline can't show.",
                schema(element(json!({ "full_page": { "type": "boolean", "description": "The whole scrollable page." } })), &[]), true, false),
            tool("click", "Click an element with the real mouse: by ref, selector or label, or at x,y in CSS pixels. Waits for a navigation it starts.",
                schema(element(json!({
                    "x": { "type": "number" }, "y": { "type": "number" },
                    "button": { "type": "string", "enum": ["left", "right"] },
                    "count": { "type": "integer", "description": "2 for a double click." },
                    "modifiers": { "type": "array", "items": { "type": "string", "enum": ["Meta", "Shift", "Alt", "Control"] } },
                    "force": { "type": "boolean", "description": "Click with a script even if something covers it." },
                })), &[]), false, false),
            tool("type", "Type text into a field (by ref, selector or label), or into the focused element. Replaces what's there unless clear is false.",
                schema(element(json!({
                    "text": { "type": "string" },
                    "clear": { "type": "boolean", "description": "Replace the current value (default true)." },
                    "submit": { "type": "boolean", "description": "Press Enter after." },
                })), &["text"]), false, false),
            tool("press", "Press a key, optionally after focusing an element: Enter, Tab, Escape, Backspace, Delete, ArrowUp/Down/Left/Right, Home, End, PageUp, PageDown, a single character, or combinations like Shift+Tab or Meta+A.",
                schema(element(json!({ "key": { "type": "string" }, "count": { "type": "integer", "description": "Press it this many times." } })), &["key"]), false, false),
            tool("hover", "Move the mouse over an element, for hover menus and tooltips.", schema(element(json!({})), &[]), false, false),
            tool("select", "Pick options in a native <select> by value or label. For custom dropdowns, click instead.",
                schema(element(json!({ "values": { "type": "array", "items": { "type": "string" } } })), &["values"]), false, false),
            tool("scroll", "Scroll the page or the scrollable area around an element: a direction and amount in pixels, to top or bottom, or just an element into view.",
                schema(element(json!({
                    "direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
                    "amount": { "type": "number" },
                    "to": { "type": "string", "enum": ["top", "bottom"] },
                })), &[]), false, false),
            tool("drag", "Drag from one element to another with the mouse.",
                schema(json!({
                    "from": { "type": "object", "description": "{ref} or {selector}", "properties": { "ref": r#ref, "selector": selector } },
                    "to": { "type": "object", "description": "{ref} or {selector}", "properties": { "ref": r#ref, "selector": selector } },
                    "tab": tab,
                }), &["from", "to"]), false, false),
            tool("wait_for", "Wait until text appears, an element matching a selector is visible, text is gone, or the URL contains something. Fails at the timeout.",
                schema(json!({
                    "text": { "type": "string" }, "selector": { "type": "string" }, "gone": { "type": "string", "description": "Text that should disappear." },
                    "url": { "type": "string", "description": "Part of the URL to wait for." },
                    "timeout": { "type": "number", "description": "Seconds, default 10, at most 60." },
                    "tab": tab,
                }), &[]), true, false),
            tool("console", "Console messages and uncaught errors from the page, new since you last looked unless all is true.",
                schema(json!({ "level": { "type": "string", "enum": ["all", "warning", "error"] }, "all": { "type": "boolean" }, "limit": { "type": "integer" }, "tab": tab }), &[]), true, false),
            tool("network", "Requests the page made: method, URL, status, type and time, new since you last looked unless all is true.",
                schema(json!({
                    "failed_only": { "type": "boolean" }, "filter": { "type": "string", "description": "Only URLs containing this." },
                    "all": { "type": "boolean" }, "limit": { "type": "integer" }, "tab": tab,
                }), &[]), true, false),
            tool("evaluate", "Run JavaScript in the page and get the result as JSON. An expression, or a function body with return; await works.",
                schema(json!({ "expression": { "type": "string" }, "tab": tab }), &["expression"]), false, false),
            tool("resize", "Change the viewport: a preset (laptop 1512×982, the default; desktop 1920×1080; tablet 834×1210; phone 402×874; fit = the panel's size) or width and height, and the color scheme the page sees.",
                schema(json!({
                    "preset": { "type": "string", "enum": ["laptop", "desktop", "tablet", "phone", "fit"] },
                    "width": { "type": "number" }, "height": { "type": "number" },
                    "color_scheme": { "type": "string", "enum": ["light", "dark", "auto"] },
                    "tab": tab,
                }), &[]), false, false),
            tool("dialog", "Answer the alert, confirm or prompt the page is waiting on.",
                schema(json!({
                    "accept": { "type": "boolean", "description": "OK (default) or Cancel." },
                    "text": { "type": "string", "description": "For a prompt." },
                    "tab": tab,
                }), &[]), false, false),
            tool("upload", "Put files in a file input: give the input (or the button that opens the file chooser) and absolute paths.",
                schema(element(json!({ "paths": { "type": "array", "items": { "type": "string" } } })), &["paths"]), false, false),
            tool("close", "Close a tab.", schema(json!({ "tab": tab }), &[]), false, false),
        ];
        if self.read_only {
            all.into_iter().filter(|t| t["name"].as_str().is_some_and(|n| READ_ONLY_TOOLS.contains(&n))).collect()
        } else {
            all
        }
    }

    fn call(&self, name: &str, a: &Value, tool_use: Option<&str>) -> ToolResult {
        if self.read_only && !READ_ONLY_TOOLS.contains(&name) {
            return ToolResult::error("The side chat can look at the browser, not use it.");
        }
        self.perform(name, a, tool_use).unwrap_or_else(ToolResult::error)
    }
}

/// Development: `YH_BROWSER_PROBE=<steps.json>` runs browser tool calls on a thread's browser
/// at launch, as the agent makes them (on a thread aside, through `BrowserTools::call`), and
/// prints each result to stderr; screenshots go next to the file as `<steps>.<n>.jpg`. The
/// file is a list of `{"tool": "click", "args": {...}}`; `{"sleep": seconds}` waits. `== done`
/// at the end, then it quits with `YH_BROWSER_PROBE_QUIT=1`. scripts/browser-fidelity runs it.
pub fn run_probe(path: String, thread: String, project: Option<String>) {
    let steps: Vec<Value> = match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string())) {
        Ok(Value::Array(steps)) => steps,
        Ok(_) => return eprintln!("BROWSER PROBE {path}: not a list of steps"),
        Err(e) => return eprintln!("BROWSER PROBE {path}: {e}"),
    };
    let tools = BrowserTools { thread: thread.clone(), project, read_only: false };
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(1));
        let shots = yh_core::library::data_directory().join("attachments");
        for (index, step) in steps.iter().enumerate() {
            if let Some(seconds) = step["sleep"].as_f64() {
                std::thread::sleep(Duration::from_secs_f64(seconds));
                continue;
            }
            // `{"control": true}`: you take control (false gives it back), as the panel's button does.
            if let Some(on) = step["control"].as_bool() {
                let t = thread.clone();
                if let Some(jobs) = JOBS.get() {
                    let _ = jobs.send(Box::new(move |b, _| {
                        let s = b.session(&t);
                        s.user_has_control = on;
                        s.tabs.iter().for_each(Tab::reset_refs);
                    }));
                }
                eprintln!("== {index} control {on}");
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            // `{"crash": true}`: the current tab's web content process is ended, as a crash would.
            if step["crash"].as_bool() == Some(true) {
                let t = thread.clone();
                if let Some(jobs) = JOBS.get() {
                    let _ = jobs.send(Box::new(move |b, _| {
                        if let Some(tab) = b.sessions.get(&t).and_then(|s| s.selected_tab()) {
                            eprintln!("== {index} crash: {}", tab.web.end_content_process());
                        }
                    }));
                }
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            let Some(tool) = step["tool"].as_str() else { continue };
            let args = if step["args"].is_object() { step["args"].clone() } else { json!({}) };
            let started = Instant::now();
            let tool_use = format!("probe-{index}");
            let result = ToolServer::call(&tools, tool, &args, Some(&tool_use));
            let mut lines = vec![format!("== {index} {tool} {args} ({} ms){}", started.elapsed().as_millis(), if result.is_error { " ERROR" } else { "" })];
            if !result.images.is_empty() {
                // The copy the transcript's step would show, moved next to the steps.
                let from = shots.join(format!("{thread}-browser-{tool_use}.jpg"));
                let to = format!("{path}.{index}.jpg");
                let size = std::fs::metadata(&from).map(|m| m.len()).unwrap_or(0);
                let _ = std::fs::rename(&from, &to);
                lines.push(format!("[image {size} bytes → {to}]"));
            }
            lines.push(result.text);
            eprintln!("{}", lines.join("\n"));
        }
        // Where the page was drawn, so a run can tell it tested the place it meant to.
        let (tx, rx) = flume::bounded(1);
        let t = thread.clone();
        if let Some(jobs) = JOBS.get() {
            let _ = jobs.send(Box::new(move |b, _| {
                let _ = tx.send(b.sessions.get(&t).and_then(|s| s.selected_tab()).map(|tab| tab.web.report_place()));
            }));
        }
        let place = rx.recv_timeout(Duration::from_secs(5)).ok().flatten().unwrap_or_else(|| "no tab".into());
        eprintln!("== place {place}");
        eprintln!("== done");
        if std::env::var("YH_BROWSER_PROBE_QUIT").as_deref() == Ok("1") {
            if let Some(jobs) = JOBS.get() {
                let _ = jobs.send(Box::new(|_, cx| cx.quit()));
            }
        }
    });
}
