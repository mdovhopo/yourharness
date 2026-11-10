//! A thread's browser on the phone (protocol 7, `yh_server::browser`): the shown tab of each
//! browser a phone watches is captured as a JPEG, often while it changes or the phone acts on
//! it, rarely while it sits still; its tabs go with it. What the phone does (a tap, a scroll,
//! typing, an address) comes back here and is done as you'd do it in the panel.

use super::{url_from, Browsers, Key, MAX_TABS, COMMAND};
use gpui_kit::App;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use yh_server::browser::Hub;
use yh_server::wire::{BrowserDialog, BrowserInput, BrowserState, BrowserTab};

/// JPEG quality of a frame: text stays readable at a fraction of the bytes.
const QUALITY: f64 = 0.6;

/// How a browser's pictures are paced: one capture at a time, sooner after a change.
struct Pace {
    busy: bool,
    due: Instant,
    changed_at: Instant,
    state: Option<BrowserState>,
}

thread_local! {
    /// Whether each thread's focused element takes typing, as the last tap left it.
    static EDITING: RefCell<HashMap<String, bool>> = RefCell::new(HashMap::new());
}

/// Streams watched browsers to `hub` for as long as the app runs. `project_of` names a
/// thread's project, whose cookies its tabs use.
pub fn start(hub: Arc<Hub>, project_of: impl Fn(&str, &App) -> Option<String> + 'static, cx: &mut App) {
    let (done, results) = flume::unbounded::<(String, bool)>();
    let mut paces: HashMap<String, Pace> = HashMap::new();
    #[allow(clippy::infinite_loop)]
    cx.spawn(async move |cx| loop {
        cx.background_executor().timer(Duration::from_millis(40)).await;
        let watched = hub.watched();
        for (thread, changed) in results.try_iter() {
            if let Some(p) = paces.get_mut(&thread) {
                p.busy = false;
                if changed {
                    p.changed_at = Instant::now();
                }
            }
        }
        paces.retain(|t, _| watched.iter().any(|(w, _)| w == t));
        if watched.is_empty() {
            if Browsers::with(|b| !b.remote.is_empty()) {
                Browsers::with(|b| b.remote.clear());
            }
            continue;
        }
        cx.update(|cx| {
            Browsers::with(|b| b.remote = watched.iter().map(|(t, _)| t.clone()).collect());
            for (thread, width) in &watched {
                let now = Instant::now();
                let pace = paces.entry(thread.clone()).or_insert(Pace { busy: false, due: now, changed_at: now, state: None });
                let project = project_of(thread, cx);
                let state = Browsers::with(|b| {
                    b.restore_if_needed(thread, project.as_deref().filter(|p| p.len() == 36));
                    b.touch(thread);
                    state_of(b, thread)
                });
                if pace.state.as_ref() != Some(&state) {
                    pace.changed_at = now;
                    hub.publish_state(thread, state.clone());
                    pace.state = Some(state);
                }
                if pace.busy || now < pace.due {
                    continue;
                }
                // Quick while things change or the phone acts, slow once the page sits still.
                let quiet = now.duration_since(hub.poked_at(thread).map_or(pace.changed_at, |p| p.max(pace.changed_at)));
                pace.due = now + Duration::from_millis(if quiet < Duration::from_secs(3) { 100 } else if quiet < Duration::from_secs(15) { 300 } else { 1000 });
                let (tx, rx) = flume::bounded(1);
                let tab = Browsers::with(|b| {
                    let s = b.sessions.get(thread)?;
                    let t = s.selected_tab()?;
                    t.web.capture_jpeg(f64::from(*width), QUALITY, tx);
                    Some(t.id)
                });
                let Some(tab) = tab else { continue };
                pace.busy = true;
                let (hub, done, thread) = (hub.clone(), done.clone(), thread.clone());
                std::thread::spawn(move || {
                    let changed = match rx.recv_timeout(Duration::from_secs(5)) {
                        Ok(Some((jpeg, w, h))) => hub.publish_frame(&thread, tab, w, h, &jpeg),
                        _ => false,
                    };
                    let _ = done.send((thread, changed));
                });
            }
        });
    })
    .detach();
}

/// A thread's browser as the phone shows it around the picture.
fn state_of(b: &Browsers, thread: &str) -> BrowserState {
    let Some(s) = b.sessions.get(thread) else { return BrowserState::default() };
    let shown = s.selected_tab();
    let (width, height) = shown.map(|t| t.web.size()).unwrap_or_default();
    BrowserState {
        tabs: s.tabs.iter().map(|t| BrowserTab { id: t.id, title: t.title(), url: t.web.url().unwrap_or_default(), loading: t.web.is_loading() }).collect(),
        selected: shown.map(|t| t.id),
        can_back: shown.is_some_and(|t| t.web.can_go_back()),
        can_forward: shown.is_some_and(|t| t.web.can_go_forward()),
        width,
        height,
        user_control: s.user_has_control,
        editing: EDITING.with(|e| e.borrow().get(thread).copied().unwrap_or(false)),
        dialog: shown.and_then(|t| t.dialog.clone()).map(|(kind, message, default_text)| BrowserDialog { kind, message, default_text }),
    }
}

/// What the phone did in a thread's browser, done here as a click or a key in the panel would.
pub fn input(thread: &str, project: Option<&str>, input: BrowserInput, cx: &mut App) -> Result<(), String> {
    let project = project.filter(|p| p.len() == 36);
    if std::env::var("YH_DATA_DIR").is_ok() {
        eprintln!("BROWSER phone {input:?}");
    }
    if let BrowserInput::Open { url } = &input {
        let url = url_from(url).ok_or("That isn't an address.")?;
        if Browsers::with(|b| b.sessions.get(thread).is_some_and(|s| s.tabs.len() >= MAX_TABS)) {
            return Err(format!("The browser has {MAX_TABS} tabs open already; close one first."));
        }
        Browsers::open_page(thread, project, &url, cx);
        return Ok(());
    }
    // Somewhere else, nothing has the focus.
    if !matches!(input, BrowserInput::Tap { .. } | BrowserInput::Text { .. } | BrowserInput::Key { .. } | BrowserInput::Scroll { .. } | BrowserInput::Dialog { .. }) {
        EDITING.with(|e| e.borrow_mut().remove(thread));
    }
    let result = Browsers::with(|b| -> Result<(), String> {
        let no_page = || "No page is open in this thread's browser.".to_string();
        match input {
            BrowserInput::Navigate { url } => {
                let url = url_from(&url).ok_or("That isn't an address.")?;
                let s = b.session(thread);
                let shown = s.selected_tab().map(|t| t.id);
                match s.tabs.iter_mut().find(|t| Some(t.id) == shown) {
                    Some(t) => t.load(&url),
                    None => {
                        b.open_tab(thread, project, &url);
                    }
                }
            }
            BrowserInput::SelectTab { tab } => {
                let s = b.sessions.get_mut(thread).ok_or_else(no_page)?;
                if !s.tabs.iter().any(|t| t.id == tab) {
                    return Err("That tab is closed.".into());
                }
                s.selected = Some(tab);
                b.place();
            }
            BrowserInput::CloseTab { tab } => b.close_tab(thread, tab),
            other => {
                let s = b.sessions.get_mut(thread).ok_or_else(no_page)?;
                let id = s.selected_tab().map(|t| t.id).ok_or_else(no_page)?;
                s.last_active = Some(Instant::now());
                let t = s.tabs.iter_mut().find(|t| t.id == id).ok_or_else(no_page)?;
                match other {
                    BrowserInput::Tap { x, y } => {
                        t.web.click(x, y, false, 1, 0);
                        // Whether a field took the focus, once the page handled the click: the
                        // phone brings its keyboard up.
                        let (thread, tab) = (thread.to_string(), t.id);
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(250));
                            let (tx, rx) = flume::bounded(1);
                            super::run_on_main(move |b, _| {
                                if let Some(t) = b.sessions.get(&thread).and_then(|s| s.tabs.iter().find(|t| t.id == tab)) {
                                    t.web.call("const a = document.activeElement; return !!a && (a.isContentEditable || /^(input|textarea|select)$/i.test(a.tagName));", false, tx);
                                }
                                let editing = rx;
                                std::thread::spawn(move || {
                                    let on = editing.recv_timeout(Duration::from_secs(5)).ok().and_then(Result::ok).is_some_and(|v| v == "true");
                                    super::run_on_main(move |_, _| {
                                        EDITING.with(|e| e.borrow_mut().insert(thread, on));
                                    });
                                });
                            });
                        });
                    }
                    BrowserInput::Scroll { x, y, dx, dy } => {
                        let (tx, _rx) = flume::bounded(1);
                        t.web.call(&format!("{SCROLL}\nreturn scrollAt({x}, {y}, {dx}, {dy});"), false, tx);
                    }
                    BrowserInput::Text { text } => t.web.insert(&text),
                    BrowserInput::Key { key } => {
                        let k = Key::parse(&key).ok_or("Unknown key.")?;
                        t.web.press(k.code, &k.characters, &k.plain, k.modifiers);
                        if k.modifiers == COMMAND && k.plain == "a" {
                            let (tx, _rx) = flume::bounded(1);
                            t.web.call("__yh.selectAll()", false, tx);
                        }
                    }
                    BrowserInput::Size { preset, width, height } => {
                        let viewport = match preset.as_str() {
                            _ if width.is_some() && height.is_some() => Some((width.unwrap_or(0.).clamp(200., 3840.), height.unwrap_or(0.).clamp(200., 2400.))),
                            "phone" => Some(crate::webview::PHONE),
                            "tablet" => Some(crate::webview::TABLET),
                            "laptop" => Some(crate::webview::LAPTOP),
                            "desktop" => Some(crate::webview::DESKTOP),
                            "fit" => None,
                            _ => return Err("No page size by that name.".into()),
                        };
                        t.web.set_viewport(viewport);
                        b.place();
                    }
                    BrowserInput::Back => t.web.go_back(),
                    BrowserInput::Forward => t.web.go_forward(),
                    BrowserInput::Reload => t.web.reload(),
                    BrowserInput::Dialog { accept, text } => {
                        if t.dialog.take().is_none() {
                            return Err("No dialog is open.".into());
                        }
                        t.web.answer_dialog(accept, text);
                    }
                    BrowserInput::Navigate { .. } | BrowserInput::Open { .. } | BrowserInput::SelectTab { .. } | BrowserInput::CloseTab { .. } => unreachable!("taken above"),
                }
            }
        }
        Ok(())
    });
    super::notify_panel(cx);
    result
}

/// Scrolls what's under a point by (dx, dy): its nearest box that scrolls that way, else the page.
const SCROLL: &str = r#"
function scrollAt(x, y, dx, dy) {
  const can = (el) => {
    const s = getComputedStyle(el);
    const yes = (o) => o === 'auto' || o === 'scroll' || o === 'overlay';
    return (dy && yes(s.overflowY) && el.scrollHeight > el.clientHeight) || (dx && yes(s.overflowX) && el.scrollWidth > el.clientWidth);
  };
  let el = document.elementFromPoint(x, y);
  while (el && el !== document.body && el !== document.documentElement && !can(el)) el = el.parentElement;
  const target = (!el || el === document.body || el === document.documentElement) ? (document.scrollingElement || document.documentElement) : el;
  target.scrollBy({ left: dx, top: dy, behavior: 'instant' });
  return true;
}
"#;
