//! The agents' browser's pages, over the GPUI window: one interface over two engines. Chrome
//! (`chrome.rs`, embedded Chromium: CEF) is the default; WebKit (`webkit.rs`, WKWebView, Safari's
//! engine) is the one in Settings → Browser. GPUI draws everything else itself; a web page can't
//! be, so it's a native view laid over the spot GPUI keeps for it (`page.rs`). What's asked of a
//! page comes back on the main thread, through channels.

#![allow(unexpected_cfgs, deprecated)]

pub mod chrome;
pub mod page;
pub mod webkit;

use block::ConcreteBlock;
use cocoa::base::{id, nil, NO, YES};
use cocoa::foundation::{NSPoint, NSRect, NSSize, NSString};
use objc::{class, msg_send, sel, sel_impl};
use std::ffi::CStr;
use std::sync::atomic::{AtomicU8, Ordering};

pub(super) fn ns_string(text: &str) -> id {
    unsafe { NSString::alloc(nil).init_str(text) }
}

pub(super) fn rust_string(s: id) -> Option<String> {
    if s == nil {
        return None;
    }
    unsafe {
        let utf8: *const std::os::raw::c_char = msg_send![s, UTF8String];
        (!utf8.is_null()).then(|| CStr::from_ptr(utf8).to_string_lossy().into_owned())
    }
}

/// The engine a new page is made with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    Chrome,
    WebKit,
}

impl Engine {
    pub fn name(self) -> &'static str {
        match self {
            Engine::Chrome => "chrome",
            Engine::WebKit => "webkit",
        }
    }

    pub fn from_name(name: &str) -> Option<Engine> {
        match name {
            "chrome" => Some(Engine::Chrome),
            "webkit" => Some(Engine::WebKit),
            _ => None,
        }
    }
}

/// 1 for WebKit, the default while Chrome is tried out behind the setting; 0 for Chrome.
static ENGINE: AtomicU8 = AtomicU8::new(1);

/// The engine new pages use: Settings → Browser's (`set_engine`), `YH_BROWSER_ENGINE` over it.
pub fn engine() -> Engine {
    if let Some(e) = std::env::var("YH_BROWSER_ENGINE").ok().and_then(|e| Engine::from_name(&e)) {
        return e;
    }
    if ENGINE.load(Ordering::Relaxed) == 1 { Engine::WebKit } else { Engine::Chrome }
}

pub fn set_engine(engine: Engine) {
    ENGINE.store(u8::from(engine == Engine::WebKit), Ordering::Relaxed);
}

/// What pages tell the app, from the engine and the page's hooks; `view` says which
/// page (its `key`).
#[derive(Debug)]
pub enum WebEvent {
    /// A console line or a request, as the hooks post them (JSON).
    Log { view: usize, json: String },
    Started { view: usize },
    /// The new page's first bytes came: a new document, whose refs start over.
    Committed { view: usize },
    Finished { view: usize },
    /// A navigation failed (not one another replaced): why, and the address it was going to.
    Failed { view: usize, error: String, url: Option<String> },
    /// The page closed itself (`window.close()` in a page a script opened).
    Closed { view: usize },
    /// Its web content process ended (a crash, or memory): it's loaded again.
    Crashed { view: usize },
    /// The page waits on an alert, confirm or prompt (`kind`), answered with `answer_dialog`.
    Dialog { view: usize, kind: String, message: String, default: Option<String> },
    /// The page opened a file chooser and nothing was waiting to answer it.
    UploadAsked { view: usize },
}

static EVENTS: std::sync::OnceLock<flume::Sender<WebEvent>> = std::sync::OnceLock::new();

/// Where every page's events go; set once, before the first page.
pub fn set_events(sender: flume::Sender<WebEvent>) {
    let _ = EVENTS.set(sender);
}

pub(super) fn post(event: WebEvent) {
    if let Some(s) = EVENTS.get() {
        let _ = s.send(event);
    }
}

thread_local! {
    /// The pages that may take the keyboard now, by key: the one you last clicked in, until you
    /// click anywhere else.
    pub(super) static TYPING: std::cell::RefCell<std::collections::HashSet<usize>> = Default::default();
    /// Pages given the keyboard for now (a press in one, `grant_keyboard`). Your next press
    /// anywhere else takes it back.
    pub(super) static GRANTED: std::cell::RefCell<std::collections::HashSet<usize>> = Default::default();
    /// Your last press in a page: which, and when.
    static LAST_PRESS: std::cell::Cell<Option<(usize, std::time::Instant)>> = const { std::cell::Cell::new(None) };
}

/// Whether you pressed in the page `key` just now (the page wanting the keyboard follows from it).
pub fn pressed_recently(key: usize) -> bool {
    LAST_PRESS.with(|p| p.get()).is_some_and(|(k, at)| k == key && at.elapsed() < std::time::Duration::from_millis(1500))
}

/// A page a page opened (`window.open`, `target=_blank`): WebKit gives the configuration to make
/// it with; Chromium has made it already.
pub enum Popup {
    WebKit(id),
    Chrome(chrome::Popup),
}

/// Makes the tab a page opens: from the opener's key, the popup and where it goes, the new
/// page's key, or None to refuse.
pub type PopupMaker = fn(usize, Popup, Option<String>) -> Option<usize>;
static POPUPS: std::sync::OnceLock<PopupMaker> = std::sync::OnceLock::new();

/// Who makes the tabs pages open; set once, at start.
pub fn set_popups(maker: PopupMaker) {
    let _ = POPUPS.set(maker);
}

pub(super) fn popup_made(opener: usize, popup: Popup, url: Option<String>) -> Option<usize> {
    POPUPS.get().and_then(|open| open(opener, popup, url))
}

/// The GPUI view of `window`, which takes the app's keys; its content view when there's none.
pub(super) fn app_view(window: id) -> id {
    unsafe {
        let content: id = msg_send![window, contentView];
        if content == nil {
            return nil;
        }
        let subviews: id = msg_send![content, subviews];
        let count: usize = msg_send![subviews, count];
        for i in 0..count {
            let view: id = msg_send![subviews, objectAtIndex: i];
            let class: id = msg_send![view, className];
            let utf8: *const std::ffi::c_char = msg_send![class, UTF8String];
            if !utf8.is_null() && CStr::from_ptr(utf8).to_bytes() == b"GPUIView" {
                return view;
            }
        }
        content
    }
}

/// The page `view` is in (a WKWebView, or the view a Chromium page is made in, with what's
/// inside either), or nil.
pub(super) fn web_view_of(mut view: id) -> id {
    unsafe {
        while view != nil {
            let web: bool = msg_send![view, isKindOfClass: class!(WKWebView)];
            if web || chrome::is_page_view(view) {
                return view;
            }
            view = msg_send![view, superview];
        }
        nil
    }
}

/// Keyboard focus as on a web page, watched in every mouse press the app gets before its window
/// handles it: a press in a page gives it the keyboard, and a press anywhere else (the composer,
/// the transcript, the page's own toolbar) takes it back for the app. Pages never take it by
/// themselves (WebKit's first-responder changes stay suppressed, Chromium's view accepts it only
/// after your press), nor do the agent's clicks, which never pass through here.
pub(super) fn watch_focus() {
    install_send_event();
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let handler = ConcreteBlock::new(|event: id| -> id {
            let kind: u64 = msg_send![event, type];
            match kind {
                2 => note_release(event),
                _ => focus_for_press(event),
            }
            event
        })
        .copy();
        // Left, right and other mouse down; left mouse up (a drag that selected text ends there).
        let mask: u64 = (1 << 1) | (1 << 2) | (1 << 3) | (1 << 25);
        let monitor: id = msg_send![class!(NSEvent), addLocalMonitorForEventsMatchingMask: mask handler: &*handler];
        let _: id = msg_send![monitor, retain];
    });
}

/// Whether NSApp is in `sendEvent:` (CEF's `CrAppProtocol` asks).
pub(super) static HANDLING_SEND_EVENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn send_event(this: &objc::runtime::Object, _: objc::runtime::Sel, event: id) {
    if page_edit_shortcut(event) {
        return;
    }
    let was = HANDLING_SEND_EVENT.swap(true, Ordering::Relaxed);
    unsafe {
        let _: () = msg_send![super(this, class!(NSApplication)), sendEvent: event];
    }
    HANDLING_SEND_EVENT.store(was, Ordering::Relaxed);
}

/// NSApp's `sendEvent:` (GPUI's `GPUIApplication` has none of its own): editing shortcuts to a
/// page that has the keyboard, and CEF's sending-an-event flag. Once.
pub(super) fn install_send_event() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        unsafe extern "C" {
            fn object_getClass(obj: id) -> *mut std::ffi::c_void;
            fn class_addMethod(cls: *mut std::ffi::c_void, name: objc::runtime::Sel, imp: *const std::ffi::c_void, types: *const std::ffi::c_char) -> bool;
        }
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        class_addMethod(object_getClass(app), sel!(sendEvent:), send_event as *const std::ffi::c_void, c"v@:@".as_ptr());
    });
}

/// ⌘A, ⌘C, ⌘V, ⌘X, ⌘Z and ⇧⌘Z while a page has the keyboard: the page's editing (its view's
/// `selectAll:`, `copy:`…), not the app's menu's. GPUI's key bindings otherwise take them first,
/// and Select All, Copy and Paste in a page's field did nothing to it.
fn page_edit_shortcut(event: id) -> bool {
    unsafe {
        let kind: u64 = msg_send![event, type];

        if kind != 10 {
            return false;
        }
        let flags: u64 = msg_send![event, modifierFlags];
        let (cmd, shift, others) = (flags & (1 << 20) != 0, flags & (1 << 17) != 0, flags & ((1 << 18) | (1 << 19)) != 0);
        if !cmd || others {
            return false;
        }
        let window: id = msg_send![event, window];
        if window == nil {
            return false;
        }
        let first: id = msg_send![window, firstResponder];
        let is_view: bool = first != nil && msg_send![first, isKindOfClass: class!(NSView)];
        if !is_view || web_view_of(first) == nil {
            return false;
        }
        let key = rust_string(msg_send![event, charactersIgnoringModifiers]).unwrap_or_default().to_lowercase();
        let (action, command) = match (key.as_str(), shift) {
            ("a", false) => (sel!(selectAll:), "selectAll"),
            ("c", false) => (sel!(copy:), "copy"),
            ("v", false) => (sel!(paste:), "paste"),
            ("x", false) => (sel!(cut:), "cut"),
            ("z", false) => (sel!(undo:), "undo"),
            ("z", true) => (sel!(redo:), "redo"),
            _ => return false,
        };
        // Chromium's page view doesn't take editing actions (they went up to the app's): the
        // command goes to the page as its own key event would carry it.
        let page = web_view_of(first);
        if chrome::is_page_view(page) {
            let code: u16 = msg_send![event, keyCode];
            let done = chrome::edit_command(page as usize, command, &key, code, shift);
            if std::env::var_os("YH_CHROME_LOG").is_some() {
                eprintln!("PAGE shortcut ⌘{key}: {command} {}", if done { "sent" } else { "not sent" });
            }
            return done;
        }
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let done: bool = msg_send![app, sendAction: action to: nil from: nil];
        if std::env::var_os("YH_CHROME_LOG").is_some() {
            eprintln!("PAGE shortcut ⌘{key}: {} to {}", if done { "sent" } else { "not taken" }, rust_string(msg_send![first, className]).unwrap_or_default());
        }
        done
    }
}

/// The left button let go over a page: what the page asks for right after (the keyboard, for text
/// a drag selected) counts as yours however long the drag took.
pub fn note_release(event: id) {
    unsafe {
        let window: id = msg_send![event, window];
        if window == nil {
            return;
        }
        let content: id = msg_send![window, contentView];
        if content == nil {
            return;
        }
        let frame: id = msg_send![content, superview];
        let at: NSPoint = msg_send![event, locationInWindow];
        let local: NSPoint = if frame == nil { at } else { msg_send![frame, convertPoint: at fromView: nil] };
        let hit: id = msg_send![content, hitTest: local];
        let page = web_view_of(hit);
        if page != nil {
            LAST_PRESS.with(|p| p.set(Some((page as usize, std::time::Instant::now()))));
        }
    }
}

/// Where a mouse press moves the keyboard (`watch_focus`).
pub fn focus_for_press(event: id) {
    unsafe {
        let window: id = msg_send![event, window];
        if window == nil {
            return;
        }
        let content: id = msg_send![window, contentView];
        let frame: id = if content == nil { nil } else { msg_send![content, superview] };
        let at: NSPoint = msg_send![event, locationInWindow];
        let hit: id = if content == nil {
            nil
        } else if frame == nil {
            msg_send![content, hitTest: at]
        } else {
            let local: NSPoint = msg_send![frame, convertPoint: at fromView: nil];
            msg_send![content, hitTest: local]
        };
        let pressed = web_view_of(hit);
        if pressed != nil {
            LAST_PRESS.with(|p| p.set(Some((pressed as usize, std::time::Instant::now()))));
        }
        let first: id = msg_send![window, firstResponder];
        let is_view: bool = first != nil && msg_send![first, isKindOfClass: class!(NSView)];
        let focused = if is_view { web_view_of(first) } else { nil };
        if pressed != nil {
            GRANTED.with(|g| g.borrow_mut().insert(pressed as usize));
            TYPING.with(|t| t.borrow_mut().insert(pressed as usize));
            // Chromium's page takes it itself, on the press: it accepts the keyboard now.
            if focused != pressed && !chrome::is_page_view(pressed) {
                webkit::take_keyboard(window, pressed);
            }
        } else if focused != nil && focused != pressed {
            let _: bool = msg_send![window, makeFirstResponder: app_view(window)];
        }
        // A page given the keyboard keeps it only until you press elsewhere.
        let back: Vec<usize> = GRANTED.with(|g| g.borrow().iter().copied().filter(|k| *k != pressed as usize).collect());
        for k in back {
            GRANTED.with(|g| g.borrow_mut().remove(&k));
            TYPING.with(|t| t.borrow_mut().remove(&k));
        }
    }
}

/// A parked page is drawn at half its CSS size.
pub(super) const PARKED_SCALE: f64 = 0.5;

/// Where pages wait while no panel shows them: the content
/// view of a borderless, non-activating panel far off screen that never takes focus or the
/// mouse, stays out of the Window menu, Mission Control, Exposé and ⌘` cycling, and stays
/// ordered in, so the engine keeps laying pages out, running their timers and animation frames
/// and drawing screenshots (occlusion detection is off for every page). Made on first use.
pub(crate) fn parking_view() -> id {
    use objc::declare::ClassDecl;
    use objc::runtime::{Object, Sel};
    static VIEW: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    const ORIGIN: NSPoint = NSPoint { x: -30_000., y: -30_000. };
    *VIEW.get_or_init(|| unsafe {
        // AppKit brings windows back onto a screen when screens change (going full screen):
        // this one always goes back where nothing can see it.
        let mut decl = ClassDecl::new("YHParkingPanel", class!(NSPanel)).expect("YHParkingPanel");
        extern "C" fn constrain(_: &Object, _: Sel, rect: NSRect, _: id) -> NSRect {
            NSRect::new(ORIGIN, rect.size)
        }
        extern "C" fn set_frame(this: &Object, _: Sel, rect: NSRect, display: objc::runtime::BOOL) {
            unsafe {
                let _: () = msg_send![super(this, class!(NSPanel)), setFrame: NSRect::new(ORIGIN, rect.size) display: display];
            }
        }
        extern "C" fn set_origin(this: &Object, _: Sel, _: NSPoint) {
            unsafe {
                let _: () = msg_send![super(this, class!(NSPanel)), setFrameOrigin: ORIGIN];
            }
        }
        extern "C" fn never(_: &Object, _: Sel) -> objc::runtime::BOOL {
            NO
        }
        decl.add_method(sel!(constrainFrameRect:toScreen:), constrain as extern "C" fn(&Object, Sel, NSRect, id) -> NSRect);
        decl.add_method(sel!(setFrame:display:), set_frame as extern "C" fn(&Object, Sel, NSRect, objc::runtime::BOOL));
        decl.add_method(sel!(setFrameOrigin:), set_origin as extern "C" fn(&Object, Sel, NSPoint));
        decl.add_method(sel!(canBecomeKeyWindow), never as extern "C" fn(&Object, Sel) -> objc::runtime::BOOL);
        decl.add_method(sel!(canBecomeMainWindow), never as extern "C" fn(&Object, Sel) -> objc::runtime::BOOL);
        let cls = decl.register();
        let panel: id = msg_send![cls, alloc];
        // NSWindowStyleMaskBorderless | NSWindowStyleMaskNonactivatingPanel; buffered.
        let panel: id = msg_send![panel, initWithContentRect: NSRect::new(ORIGIN, NSSize::new(2600., 2000.)) styleMask: 1usize << 7 backing: 2usize defer: NO];
        let _: () = msg_send![panel, setTitle: ns_string("YourHarness Parked Pages")];
        // Panels hide when the app goes to the background; this one's pages must not.
        let _: () = msg_send![panel, setHidesOnDeactivate: NO];
        let _: () = msg_send![panel, setReleasedWhenClosed: NO];
        let _: () = msg_send![panel, setIgnoresMouseEvents: YES];
        let _: () = msg_send![panel, setHasShadow: NO];
        let _: () = msg_send![panel, setExcludedFromWindowsMenu: YES];
        let _: () = msg_send![panel, setCanHide: NO];
        // Transient | Stationary | IgnoresCycle | FullScreenNone: not in Mission Control or ⌘`.
        let behavior: usize = (1 << 3) | (1 << 4) | (1 << 6) | (1 << 9);
        let _: () = msg_send![panel, setCollectionBehavior: behavior];
        // NSWindowAnimationBehaviorNone.
        let _: () = msg_send![panel, setAnimationBehavior: 2isize];
        let content: id = msg_send![class!(NSView), alloc];
        let content: id = msg_send![content, initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(2600., 2000.))];
        let _: () = msg_send![panel, setContentView: content];
        let _: () = msg_send![panel, orderFrontRegardless];
        content as usize
    }) as id
}

/// A view that's drawn but never hit: the mouse goes through it to the page.
pub(super) fn pass_through_class() -> &'static objc::runtime::Class {
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Sel};
    static CLASS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let ptr = *CLASS.get_or_init(|| {
        let mut decl = ClassDecl::new("YHPassThroughView", class!(NSView)).expect("YHPassThroughView");
        extern "C" fn hit_test(_: &Object, _: Sel, _: NSPoint) -> id {
            nil
        }
        unsafe {
            decl.add_method(sel!(hitTest:), hit_test as extern "C" fn(&Object, Sel, NSPoint) -> id);
        }
        decl.register() as *const Class as usize
    });
    unsafe { &*(ptr as *const Class) }
}

/// Fades a layer in.
pub(super) fn fade_in(layer: id) {
    unsafe {
        let fade: id = msg_send![class!(CABasicAnimation), animationWithKeyPath: ns_string("opacity")];
        let from: id = msg_send![class!(NSNumber), numberWithDouble: 0f64];
        let to: id = msg_send![class!(NSNumber), numberWithDouble: 1f64];
        let _: () = msg_send![fade, setFromValue: from];
        let _: () = msg_send![fade, setToValue: to];
        let _: () = msg_send![fade, setDuration: 0.2f64];
        let _: () = msg_send![layer, addAnimation: fade forKey: ns_string("fade")];
    }
}

/// `file://` and the path, its bytes outside a URL's plain ones percent-encoded.
pub fn file_url(path: &str) -> String {
    let mut url = String::from("file://");
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => url.push(b as char),
            _ => url.push_str(&format!("%{b:02X}")),
        }
    }
    url
}

/// The viewport presets (`resize`): a laptop is the default.
pub const LAPTOP: (f64, f64) = (1512., 982.);
pub const DESKTOP: (f64, f64) = (1920., 1080.);
pub const TABLET: (f64, f64) = (834., 1210.);
pub const PHONE: (f64, f64) = (402., 874.);

/// A page, in either engine.
pub enum WebView {
    WebKit(webkit::WebView),
    Chrome(chrome::WebView),
}

macro_rules! both {
    ($self:ident, $w:ident => $e:expr) => {
        match $self {
            WebView::WebKit($w) => $e,
            WebView::Chrome($w) => $e,
        }
    };
}

impl WebView {
    /// A page over the GPUI view `host`, in the engine in use, hidden until it's given a place.
    /// `store`: the project's ID (a UUID), whose cookies and storage it uses; None for the app's own.
    pub fn new(host: id, store: Option<&str>) -> WebView {
        match engine() {
            Engine::WebKit => WebView::WebKit(webkit::WebView::new(host, store)),
            Engine::Chrome => WebView::Chrome(chrome::WebView::new(host, store)),
        }
    }

    /// The page a page opened, over `host`.
    pub fn from_popup(host: id, popup: Popup) -> WebView {
        match popup {
            Popup::WebKit(configuration) => WebView::WebKit(webkit::WebView::with_configuration(host, configuration)),
            Popup::Chrome(p) => WebView::Chrome(chrome::WebView::from_popup(host, p)),
        }
    }

    pub fn engine(&self) -> Engine {
        match self {
            WebView::WebKit(_) => Engine::WebKit,
            WebView::Chrome(_) => Engine::Chrome,
        }
    }

    /// Where it is, for test runs: its window's title, shown or hidden, its frame.
    pub fn report_place(&self) -> String { both!(self, w => w.report_place()) }
    /// Where the page shows over `window`, in its top-left points, when it's shown there.
    pub fn shown_in(&self, window: id) -> Option<(f64, f64, f64, f64)> { both!(self, w => w.shown_in(window)) }
    /// Moves it over another GPUI view (the browser's own window, or back), keeping the page.
    pub fn move_to(&self, host: id) { both!(self, w => w.move_to(host)) }
    /// Where it shows, in the host view's top-left coordinates; `None` parks it.
    pub fn set_frame(&self, bounds: Option<(f64, f64, f64, f64)>) { both!(self, w => w.set_frame(bounds)) }
    /// For a full-page screenshot: as tall as `height` CSS pixels, the layout kept; or back (None).
    pub fn hold_full_height(&self, height: Option<f64>) { both!(self, w => w.hold_full_height(height)) }
    /// The page's viewport in CSS pixels, None to fit the panel.
    pub fn set_viewport(&self, viewport: Option<(f64, f64)>) { both!(self, w => w.set_viewport(viewport)) }
    pub fn viewport(&self) -> Option<(f64, f64)> { both!(self, w => w.viewport()) }
    /// Light or dark for the page's `prefers-color-scheme`; `auto` follows the app.
    pub fn set_color_scheme(&self, scheme: &str) { both!(self, w => w.set_color_scheme(scheme)) }
    /// Answers the page's dialog: OK (with `text` for a prompt) or Cancel. False when none waits.
    pub fn answer_dialog(&self, accept: bool, text: Option<String>) -> bool { both!(self, w => w.answer_dialog(accept, text)) }
    /// Files for the page's next file chooser; true if it's still waiting for one.
    pub fn set_upload(&self, files: Option<Vec<String>>) -> bool { both!(self, w => w.set_upload(files)) }
    pub fn load(&self, url: &str) { both!(self, w => w.load(url)) }
    /// Loads a file on disk, letting it read what's in `folder` too (its pictures, styles and
    /// scripts beside it).
    pub fn load_file(&self, file: &str, folder: &str) {
        match self {
            WebView::WebKit(w) => w.load_file(file, folder),
            WebView::Chrome(w) => w.load(&file_url(file)),
        }
    }
    pub fn url(&self) -> Option<String> { both!(self, w => w.url()) }
    pub fn title(&self) -> Option<String> { both!(self, w => w.title()) }
    pub fn is_loading(&self) -> bool { both!(self, w => w.is_loading()) }
    /// Its key, which `WebEvent`s name it by.
    pub fn key(&self) -> usize { both!(self, w => w.key()) }
    pub fn progress(&self) -> f64 { both!(self, w => w.progress()) }
    pub fn can_go_back(&self) -> bool { both!(self, w => w.can_go_back()) }
    pub fn can_go_forward(&self) -> bool { both!(self, w => w.can_go_forward()) }
    pub fn go_forward(&self) { both!(self, w => w.go_forward()) }
    pub fn go_back(&self) { both!(self, w => w.go_back()) }
    pub fn reload(&self) { both!(self, w => w.reload()) }
    pub fn stop_loading(&self) { both!(self, w => w.stop_loading()) }
    /// Its size in CSS pixels.
    pub fn size(&self) -> (f64, f64) { both!(self, w => w.size()) }
    /// On screen, over a panel or the browser's window (not parked).
    pub fn is_shown(&self) -> bool { both!(self, w => w.is_shown()) }
    /// Runs `body` (a function body; `await` works) in the helpers' world, or the page's, and
    /// replies with what it returned as JSON (`null` for nothing), or the error.
    pub fn call(&self, body: &str, in_page: bool, reply: flume::Sender<Result<String, String>>) { both!(self, w => w.call(body, in_page, reply)) }
    /// The keyboard to the page for now: you clicked into a field or selected text in it.
    pub fn grant_keyboard(&self) {
        let key = self.key();
        if TYPING.with(|t| t.borrow().contains(&key)) {
            return;
        }
        GRANTED.with(|g| g.borrow_mut().insert(key));
        TYPING.with(|t| t.borrow_mut().insert(key));
        both!(self, w => w.take_keyboard());
        // The field you clicked, focused now that the page may have it (WebKit didn't focus it).
        let (tx, _rx) = flume::bounded(1);
        self.call("const f = window.__yhField; if (f && document.activeElement !== f) f.focus(); return true", true, tx);
    }
    /// A click at a point in CSS pixels, as real input: `left` or `right`, `count` times.
    pub fn click(&self, x: f64, y: f64, right: bool, count: isize, modifiers: usize) { both!(self, w => w.click(x, y, right, count, modifiers)) }
    pub fn hover(&self, x: f64, y: f64) { both!(self, w => w.hover(x, y)) }
    pub fn mouse_down(&self, x: f64, y: f64) { both!(self, w => w.mouse_down(x, y)) }
    pub fn mouse_dragged(&self, x: f64, y: f64) { both!(self, w => w.mouse_dragged(x, y)) }
    pub fn mouse_up(&self, x: f64, y: f64) { both!(self, w => w.mouse_up(x, y)) }
    /// Text into what has focus in the page, as input events.
    pub fn insert(&self, text: &str) { both!(self, w => w.insert(text)) }
    /// A key press: real keydown and keyup (`key_code` a Mac virtual key code, `modifiers`
    /// NSEvent's flags).
    pub fn press(&self, key_code: u16, characters: &str, plain: &str, modifiers: usize) { both!(self, w => w.press(key_code, characters, plain, modifiers)) }
    /// Part of what it shows (CSS pixels, None for all of it) as a JPEG at most `max_width`
    /// pixels wide, to a file; replies with its size.
    pub fn capture(&self, rect: Option<(f64, f64, f64, f64)>, max_width: f64, path: String, reply: flume::Sender<Result<(u32, u32), String>>) { both!(self, w => w.capture(rect, max_width, path, reply)) }
    /// The visible page as a JPEG at most `max_px` pixels across, for a phone watching it.
    pub fn capture_jpeg(&self, max_px: f64, quality: f64, reply: flume::Sender<Option<(Vec<u8>, u32, u32)>>) { both!(self, w => w.capture_jpeg(max_px, quality, reply)) }
    /// Ends the page's content process, as a crash would (test runs).
    pub fn end_content_process(&self) -> bool { both!(self, w => w.end_content_process()) }
    /// The engine's inspector for the page (Safari's Web Inspector, Chrome's DevTools).
    pub fn show_inspector(&self) -> bool { both!(self, w => w.show_inspector()) }
    /// What the agent is doing, over the page, and where (CSS pixels), ringed.
    pub fn show_activity(&self, label: &str, rect: Option<(f64, f64, f64, f64)>) -> u64 { both!(self, w => w.show_activity(label, rect)) }
    pub fn clear_activity(&self, seq: u64) { both!(self, w => w.clear_activity(seq)) }
    pub fn shows_activity(&self) -> bool { both!(self, w => w.shows_activity()) }
    pub fn overlay_png(&self, path: &str) -> bool { both!(self, w => w.overlay_png(path)) }
    /// Runs JavaScript in the page; the reply is its result as text, or the error.
    pub fn evaluate(&self, script: &str, reply: flume::Sender<Result<String, String>>) { both!(self, w => w.evaluate(script, reply)) }
    /// What it shows now, as a PNG at `path`.
    pub fn snapshot(&self, path: String, reply: flume::Sender<Result<(), String>>) { both!(self, w => w.snapshot(path, reply)) }
}

/// Standard base64 (CDP's screenshots), whitespace and padding skipped.
pub(super) fn base64_decode(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// Clears everything a project's browser keeps in the engine in use (cookies, storage, caches).
pub fn clear_store(project: &str) {
    match engine() {
        Engine::WebKit => webkit::clear_store(project),
        Engine::Chrome => chrome::clear_store(project),
    }
}

/// Every cookie in a project's browser, in the engine in use (Export State's sign-ins).
pub fn read_cookies(project: &str, reply: flume::Sender<Vec<yh_core::cookies::Cookie>>) {
    match engine() {
        Engine::WebKit => webkit::read_cookies(project, reply),
        Engine::Chrome => chrome::read_cookies(project, reply),
    }
}

/// Puts cookies in a project's browser, in the engine in use; replies with how many went in.
pub fn store_cookies(project: &str, cookies: Vec<yh_core::cookies::Cookie>, reply: flume::Sender<usize>) {
    match engine() {
        Engine::WebKit => webkit::store_cookies(project, cookies, reply),
        Engine::Chrome => chrome::store_cookies(project, cookies, reply),
    }
}

/// JPEGs from `CGImage`s with ImageIO, which is safe off the main thread.
pub(super) mod jpeg {
    use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::{CFString, CFStringRef};
    use std::ffi::c_void;

    #[link(name = "ImageIO", kind = "framework")]
    unsafe extern "C" {
        static kCGImageDestinationLossyCompressionQuality: CFStringRef;
        static kCGImageDestinationBackgroundColor: CFStringRef;
        fn CGImageDestinationCreateWithData(data: *mut c_void, kind: CFStringRef, count: usize, options: *const c_void) -> *mut c_void;
        fn CGImageDestinationAddImage(dest: *mut c_void, image: *const c_void, properties: *const c_void);
        fn CGImageDestinationFinalize(dest: *mut c_void) -> bool;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGImageRetain(image: *const c_void) -> *const c_void;
        fn CGImageRelease(image: *mut c_void);
        fn CGImageGetWidth(image: *const c_void) -> usize;
        fn CGImageGetHeight(image: *const c_void) -> usize;
        fn CGColorCreateSRGB(r: f64, g: f64, b: f64, a: f64) -> *const c_void;
        fn CGColorRelease(color: *const c_void);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFDataCreateMutable(allocator: *const c_void, capacity: isize) -> *mut c_void;
        fn CFDataGetLength(data: *const c_void) -> isize;
        fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    }

    pub fn retain(image: *const c_void) -> *const c_void {
        unsafe { CGImageRetain(image) }
    }

    /// The image as a JPEG (transparency on white), and its size; the image is released.
    pub fn encode(image: *const c_void, quality: f64) -> Option<(Vec<u8>, u32, u32)> {
        unsafe {
            let (w, h) = (CGImageGetWidth(image) as u32, CGImageGetHeight(image) as u32);
            let data = CFDataCreateMutable(std::ptr::null(), 0);
            let kind = CFString::from_static_string("public.jpeg");
            let dest = CGImageDestinationCreateWithData(data, kind.as_concrete_TypeRef(), 1, std::ptr::null());
            let mut out = None;
            if !dest.is_null() {
                let white = CGColorCreateSRGB(1., 1., 1., 1.);
                let props = CFDictionary::from_CFType_pairs(&[
                    (CFString::wrap_under_get_rule(kCGImageDestinationLossyCompressionQuality).as_CFType(), CFNumber::from(quality).as_CFType()),
                    (CFString::wrap_under_get_rule(kCGImageDestinationBackgroundColor).as_CFType(), core_foundation::base::CFType::wrap_under_get_rule(white as CFTypeRef)),
                ]);
                CGImageDestinationAddImage(dest, image, props.as_concrete_TypeRef() as *const c_void);
                if CGImageDestinationFinalize(dest) {
                    let len = CFDataGetLength(data) as usize;
                    out = Some((std::slice::from_raw_parts(CFDataGetBytePtr(data), len).to_vec(), w, h));
                }
                CGColorRelease(white);
                CFRelease(dest as CFTypeRef);
            }
            CFRelease(data as CFTypeRef);
            CGImageRelease(image as *mut c_void);
            out
        }
    }
}
