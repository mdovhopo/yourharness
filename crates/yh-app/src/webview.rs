//! A WKWebView placed over the GPUI window: the agents' browser. GPUI draws everything else
//! itself; a web page can't be, so it's a native view laid over the spot GPUI keeps for it
//! (`set_frame` with the bounds GPUI laid out). Results of what's asked of the page come
//! back on the main thread, from WebKit's completion blocks, through channels.

#![allow(unexpected_cfgs, deprecated)]

use block::ConcreteBlock;
use cocoa::base::{id, nil, NO, YES};
use cocoa::foundation::{NSPoint, NSRect, NSSize, NSString};
use objc::{class, msg_send, sel, sel_impl};
use std::ffi::CStr;

#[link(name = "WebKit", kind = "framework")]
unsafe extern "C" {}

fn ns_string(text: &str) -> id {
    unsafe { NSString::alloc(nil).init_str(text) }
}

fn rust_string(s: id) -> Option<String> {
    if s == nil {
        return None;
    }
    unsafe {
        let utf8: *const std::os::raw::c_char = msg_send![s, UTF8String];
        (!utf8.is_null()).then(|| CStr::from_ptr(utf8).to_string_lossy().into_owned())
    }
}

/// What pages tell the app, from WebKit's delegates and the page's hooks; `view` says which
/// web view (its address).
#[derive(Debug)]
pub enum WebEvent {
    /// A console line or a request, as the hooks post them (JSON).
    Log { view: usize, json: String },
    Started { view: usize },
    Finished { view: usize },
    Failed { view: usize, error: String },
    /// The page waits on an alert, confirm or prompt (`kind`), answered with `answer_dialog`.
    Dialog { view: usize, kind: String, message: String, default: Option<String> },
    /// The page opened a file chooser and nothing was waiting to answer it.
    UploadAsked { view: usize },
}

/// How a page's dialog is answered, kept until it is.
enum DialogReply {
    Alert(block::RcBlock<(), ()>),
    Confirm(block::RcBlock<(objc::runtime::BOOL,), ()>),
    Prompt(block::RcBlock<(id,), ()>, Option<String>),
}

thread_local! {
    static DIALOGS: std::cell::RefCell<std::collections::HashMap<usize, DialogReply>> = Default::default();
    /// Files for a web view's next file chooser (the `upload` tool).
    static UPLOADS: std::cell::RefCell<std::collections::HashMap<usize, Vec<String>>> = Default::default();
}

static EVENTS: std::sync::OnceLock<flume::Sender<WebEvent>> = std::sync::OnceLock::new();

/// Where every web view's events go; set once, before the first web view.
pub fn set_events(sender: flume::Sender<WebEvent>) {
    let _ = EVENTS.set(sender);
}

fn post(event: WebEvent) {
    if let Some(s) = EVENTS.get() {
        let _ = s.send(event);
    }
}

/// One object, alive for the app's life, that's every web view's navigation delegate and
/// the hooks' message handler.
fn delegate() -> id {
    use objc::declare::ClassDecl;
    use objc::runtime::{Object, Sel};
    static DELEGATE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *DELEGATE.get_or_init(|| unsafe {
        let mut decl = ClassDecl::new("YHWebDelegate", class!(NSObject)).expect("YHWebDelegate");
        extern "C" fn message(_: &Object, _: Sel, _: id, message: id) {
            unsafe {
                let view: id = msg_send![message, webView];
                let body: id = msg_send![message, body];
                let valid: bool = msg_send![class!(NSJSONSerialization), isValidJSONObject: body];
                if !valid {
                    return;
                }
                let data: id = msg_send![class!(NSJSONSerialization), dataWithJSONObject: body options: 0usize error: nil];
                let text: id = msg_send![class!(NSString), alloc];
                let text: id = msg_send![text, initWithData: data encoding: 4usize];
                if let Some(json) = rust_string(text) {
                    post(WebEvent::Log { view: view as usize, json });
                }
                let _: () = msg_send![text, release];
            }
        }
        extern "C" fn started(_: &Object, _: Sel, view: id, _: id) {
            post(WebEvent::Started { view: view as usize });
        }
        extern "C" fn finished(_: &Object, _: Sel, view: id, _: id) {
            post(WebEvent::Finished { view: view as usize });
        }
        extern "C" fn failed(_: &Object, _: Sel, view: id, _: id, error: id) {
            unsafe {
                let code: isize = msg_send![error, code];
                // -999: cancelled, as when another navigation replaces it.
                if code == -999 {
                    return;
                }
                let text: id = msg_send![error, localizedDescription];
                post(WebEvent::Failed { view: view as usize, error: rust_string(text).unwrap_or_else(|| "failed".into()) });
            }
        }
        extern "C" fn alert(_: &Object, _: Sel, view: id, message: id, _: id, done: id) {
            let reply = unsafe { block::RcBlock::copy(done as *mut block::Block<(), ()>) };
            DIALOGS.with(|d| d.borrow_mut().insert(view as usize, DialogReply::Alert(reply)));
            post(WebEvent::Dialog { view: view as usize, kind: "alert".into(), message: rust_string(message).unwrap_or_default(), default: None });
        }
        extern "C" fn confirm(_: &Object, _: Sel, view: id, message: id, _: id, done: id) {
            let reply = unsafe { block::RcBlock::copy(done as *mut block::Block<(objc::runtime::BOOL,), ()>) };
            DIALOGS.with(|d| d.borrow_mut().insert(view as usize, DialogReply::Confirm(reply)));
            post(WebEvent::Dialog { view: view as usize, kind: "confirm".into(), message: rust_string(message).unwrap_or_default(), default: None });
        }
        extern "C" fn prompt(_: &Object, _: Sel, view: id, message: id, default: id, _: id, done: id) {
            let reply = unsafe { block::RcBlock::copy(done as *mut block::Block<(id,), ()>) };
            let default = rust_string(default);
            DIALOGS.with(|d| d.borrow_mut().insert(view as usize, DialogReply::Prompt(reply, default.clone())));
            post(WebEvent::Dialog { view: view as usize, kind: "prompt".into(), message: rust_string(message).unwrap_or_default(), default });
        }
        extern "C" fn open_panel(_: &Object, _: Sel, view: id, parameters: id, _: id, done: id) {
            let done = done as *mut block::Block<(id,), ()>;
            unsafe {
                let multiple: bool = msg_send![parameters, allowsMultipleSelection];
                let files = UPLOADS.with(|u| u.borrow_mut().remove(&(view as usize)));
                let urls: id = match files {
                    // The agent's files, waiting for this chooser.
                    Some(mut files) => {
                        if !multiple {
                            files.truncate(1);
                        }
                        let list: id = msg_send![class!(NSMutableArray), array];
                        for f in files {
                            let url: id = msg_send![class!(NSURL), fileURLWithPath: ns_string(&f)];
                            let _: () = msg_send![list, addObject: url];
                        }
                        list
                    }
                    // Yours: the open panel, as Safari shows it.
                    None => {
                        post(WebEvent::UploadAsked { view: view as usize });
                        let panel: id = msg_send![class!(NSOpenPanel), openPanel];
                        let _: () = msg_send![panel, setAllowsMultipleSelection: multiple];
                        let _: () = msg_send![panel, setCanChooseFiles: YES];
                        let answer: isize = msg_send![panel, runModal];
                        if answer == 1 { msg_send![panel, URLs] } else { nil }
                    }
                };
                (*done).call((urls,));
            }
        }
        decl.add_method(sel!(webView:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:), alert as extern "C" fn(&Object, Sel, id, id, id, id));
        decl.add_method(sel!(webView:runJavaScriptConfirmPanelWithMessage:initiatedByFrame:completionHandler:), confirm as extern "C" fn(&Object, Sel, id, id, id, id));
        decl.add_method(sel!(webView:runJavaScriptTextInputPanelWithPrompt:defaultText:initiatedByFrame:completionHandler:), prompt as extern "C" fn(&Object, Sel, id, id, id, id, id));
        decl.add_method(sel!(webView:runOpenPanelWithParameters:initiatedByFrame:completionHandler:), open_panel as extern "C" fn(&Object, Sel, id, id, id, id));
        decl.add_method(sel!(userContentController:didReceiveScriptMessage:), message as extern "C" fn(&Object, Sel, id, id));
        decl.add_method(sel!(webView:didStartProvisionalNavigation:), started as extern "C" fn(&Object, Sel, id, id));
        decl.add_method(sel!(webView:didFinishNavigation:), finished as extern "C" fn(&Object, Sel, id, id));
        decl.add_method(sel!(webView:didFailProvisionalNavigation:withError:), failed as extern "C" fn(&Object, Sel, id, id, id));
        decl.add_method(sel!(webView:didFailNavigation:withError:), failed as extern "C" fn(&Object, Sel, id, id, id));
        let cls = decl.register();
        let object: id = msg_send![cls, new];
        object as usize
    }) as id
}

/// The helpers' own content world, which the page can't see into.
fn helper_world() -> id {
    unsafe { msg_send![class!(WKContentWorld), worldWithName: ns_string("YourHarness")] }
}

fn page_world() -> id {
    unsafe { msg_send![class!(WKContentWorld), pageWorld] }
}

/// Puts cookies in a project's browser data store (by its UUID); replies with how many went in.
pub fn store_cookies(project: &str, cookies: Vec<yh_core::cookies::Cookie>, reply: flume::Sender<usize>) {
    unsafe {
        let identifier: id = msg_send![class!(NSUUID), alloc];
        let identifier: id = msg_send![identifier, initWithUUIDString: ns_string(project)];
        if identifier == nil {
            let _ = reply.send(0);
            return;
        }
        let data: id = msg_send![class!(WKWebsiteDataStore), dataStoreForIdentifier: identifier];
        // Held until the last cookie is in: let go of sooner, its writes never finish.
        let _: id = msg_send![data, retain];
        let store: id = msg_send![data, httpCookieStore];
        let mut made: Vec<id> = vec![];
        for c in cookies {
            let props: id = msg_send![class!(NSMutableDictionary), dictionary];
            let set = |k: &str, v: id| {
                let _: () = msg_send![props, setObject: v forKey: ns_string(k)];
            };
            set("Domain", ns_string(&c.domain));
            set("Path", ns_string(if c.path.is_empty() { "/" } else { &c.path }));
            set("Name", ns_string(&c.name));
            set("Value", ns_string(&c.value));
            if let Some(e) = c.expires {
                let date: id = msg_send![class!(NSDate), dateWithTimeIntervalSince1970: e];
                set("Expires", date);
            }
            if c.secure {
                set("Secure", ns_string("TRUE"));
            }
            if c.http_only {
                set("HttpOnly", ns_string("TRUE"));
            }
            match c.same_site.as_deref() {
                Some("lax") => set("SameSite", ns_string("lax")),
                Some("strict") => set("SameSite", ns_string("strict")),
                _ => {}
            }
            let cookie: id = msg_send![class!(NSHTTPCookie), cookieWithProperties: props];
            if cookie != nil {
                made.push(cookie);
            }
        }
        let total = made.len();
        if total == 0 {
            let _: () = msg_send![data, release];
            let _ = reply.send(0);
            return;
        }
        let left = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(total));
        let data_ptr = data as usize;
        for cookie in made {
            let (left, reply) = (left.clone(), reply.clone());
            let done = ConcreteBlock::new(move || {
                if left.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1 {
                    let _: () = msg_send![data_ptr as id, release];
                    let _ = reply.send(total);
                }
            })
            .copy();
            let _: () = msg_send![store, setCookie: cookie completionHandler: &*done];
        }
        let _: () = msg_send![identifier, release];
    }
}

pub struct WebView {
    /// What sits over the window: it draws the page scaled when the page's viewport is
    /// bigger than the panel (its bounds are CSS pixels, its frame points).
    page: id,
    view: id,
    /// The page's viewport in CSS pixels; None fits the panel.
    viewport: std::cell::Cell<Option<(f64, f64)>>,
}

/// The viewport presets (`resize`): a laptop is the default.
pub const LAPTOP: (f64, f64) = (1512., 982.);
pub const DESKTOP: (f64, f64) = (1920., 1080.);
pub const TABLET: (f64, f64) = (834., 1210.);
pub const PHONE: (f64, f64) = (402., 874.);

impl WebView {
    /// A web view in the window whose GPUI view is `host`, hidden until it's given a place.
    /// Its cookies and storage persist (the default data store), per app.
    /// `store`: the project's ID (a UUID), whose cookies and storage it uses (the Swift app's
    /// stores, so sign-ins carry over); None for the app's default store.
    pub fn new(host: id, store: Option<&str>) -> WebView {
        unsafe {
            let config: id = msg_send![class!(WKWebViewConfiguration), new];
            if let Some(uuid) = store {
                let identifier: id = msg_send![class!(NSUUID), alloc];
                let identifier: id = msg_send![identifier, initWithUUIDString: ns_string(uuid)];
                if identifier != nil {
                    let data: id = msg_send![class!(WKWebsiteDataStore), dataStoreForIdentifier: identifier];
                    let _: () = msg_send![config, setWebsiteDataStore: data];
                    let _: () = msg_send![identifier, release];
                }
            }
            // The helpers in their own world; the console and network hooks in the page's.
            let controller: id = msg_send![config, userContentController];
            for (source, world) in [(include_str!("browser/helpers.js"), helper_world()), (include_str!("browser/hooks.js"), page_world())] {
                let script: id = msg_send![class!(WKUserScript), alloc];
                let script: id = msg_send![script, initWithSource: ns_string(source) injectionTime: 0isize forMainFrameOnly: YES inContentWorld: world];
                let _: () = msg_send![controller, addUserScript: script];
                let _: () = msg_send![script, release];
            }
            let _: () = msg_send![controller, addScriptMessageHandler: delegate() contentWorld: page_world() name: ns_string("yourHarnessLog")];
            let frame = NSRect::new(NSPoint::new(0., 0.), NSSize::new(10., 10.));
            let view: id = msg_send![class!(WKWebView), alloc];
            let view: id = msg_send![view, initWithFrame: frame configuration: config];
            let _: () = msg_send![config, release];
            let _: () = msg_send![view, setHidden: YES];
            // Safari's user agent: sites serve the page a Mac browser gets.
            let _: () = msg_send![view, setCustomUserAgent: ns_string("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15")];
            let _: () = msg_send![view, setNavigationDelegate: delegate()];
            let _: () = msg_send![view, setUIDelegate: delegate()];
            let page: id = msg_send![class!(NSView), alloc];
            let page: id = msg_send![page, initWithFrame: frame];
            let _: () = msg_send![page, setHidden: YES];
            let _: () = msg_send![view, setHidden: NO];
            let _: () = msg_send![page, addSubview: view];
            let _: () = msg_send![host, addSubview: page];
            let web = WebView { page, view, viewport: std::cell::Cell::new(Some(LAPTOP)) };
            web.lay_out(LAPTOP.0, LAPTOP.1, LAPTOP.0, LAPTOP.1);
            web
        }
    }

    /// Where it shows, in the host view's top-left coordinates (GPUI's); `None` hides it (the
    /// page keeps its viewport). A viewport bigger than the panel is drawn scaled to fit,
    /// against the top.
    pub fn set_frame(&self, bounds: Option<(f64, f64, f64, f64)>) {
        unsafe {
            let Some((x, y, w, h)) = bounds else {
                let _: () = msg_send![self.page, setHidden: YES];
                return;
            };
            let (cw, ch) = self.viewport.get().unwrap_or((w, h));
            let scale = (w / cw).min(h / ch);
            let (fw, fh) = (cw * scale, ch * scale);
            let host: id = msg_send![self.page, superview];
            let flipped: bool = msg_send![host, isFlipped];
            let host_frame: NSRect = msg_send![host, bounds];
            let left = x + (w - fw) / 2.;
            let bottom = if flipped { y } else { host_frame.size.height - y - fh };
            let _: () = msg_send![self.page, setFrame: NSRect::new(NSPoint::new(left, bottom), NSSize::new(fw, fh))];
            self.lay_out(fw, fh, cw, ch);
            let _: () = msg_send![self.page, setHidden: NO];
        }
    }

    /// The page view `frame` points big, the web view `css` points: a CSS pixel is a point,
    /// as in Safari, drawn scaled (page zoom would hold text to WebKit's minimum size).
    fn lay_out(&self, fw: f64, fh: f64, cw: f64, ch: f64) {
        unsafe {
            let current: NSRect = msg_send![self.page, frame];
            if current.size.width != fw || current.size.height != fh {
                let _: () = msg_send![self.page, setFrameSize: NSSize::new(fw, fh)];
            }
            let _: () = msg_send![self.page, setBoundsSize: NSSize::new(cw, ch)];
            let _: () = msg_send![self.view, setFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(cw, ch))];
        }
    }

    /// The page's viewport in CSS pixels, None to fit the panel; laid out at the next frame.
    pub fn set_viewport(&self, viewport: Option<(f64, f64)>) {
        self.viewport.set(viewport);
        if let Some((w, h)) = viewport {
            // Hidden, it still takes the size, for a screenshot without the panel.
            let hidden: bool = unsafe { msg_send![self.page, isHidden] };
            if hidden {
                self.lay_out(w, h, w, h);
            }
        }
    }

    pub fn viewport(&self) -> Option<(f64, f64)> {
        self.viewport.get()
    }

    /// Light or dark for the page's `prefers-color-scheme`; `auto` follows the app.
    pub fn set_color_scheme(&self, scheme: &str) {
        unsafe {
            let appearance: id = match scheme {
                "light" => msg_send![class!(NSAppearance), appearanceNamed: ns_string("NSAppearanceNameAqua")],
                "dark" => msg_send![class!(NSAppearance), appearanceNamed: ns_string("NSAppearanceNameDarkAqua")],
                _ => nil,
            };
            let _: () = msg_send![self.view, setAppearance: appearance];
        }
    }

    /// Answers the page's dialog: OK (with `text` for a prompt) or Cancel. False when none waits.
    pub fn answer_dialog(&self, accept: bool, text: Option<String>) -> bool {
        let Some(reply) = DIALOGS.with(|d| d.borrow_mut().remove(&(self.view as usize))) else { return false };
        match reply {
            DialogReply::Alert(b) => unsafe { b.call(()) },
            DialogReply::Confirm(b) => unsafe { b.call((if accept { YES } else { NO },)) },
            DialogReply::Prompt(b, default) => unsafe {
                let answer = if accept { ns_string(&text.or(default).unwrap_or_default()) } else { nil };
                b.call((answer,))
            },
        }
        true
    }

    /// Files for the page's next file chooser; true if it's still waiting for one.
    pub fn set_upload(&self, files: Option<Vec<String>>) -> bool {
        UPLOADS.with(|u| match files {
            Some(f) => {
                u.borrow_mut().insert(self.view as usize, f);
                true
            }
            None => u.borrow_mut().remove(&(self.view as usize)).is_some(),
        })
    }

    pub fn load(&self, url: &str) {
        unsafe {
            let ns_url: id = msg_send![class!(NSURL), URLWithString: ns_string(url)];
            if ns_url == nil {
                return;
            }
            let request: id = msg_send![class!(NSURLRequest), requestWithURL: ns_url];
            let _: id = msg_send![self.view, loadRequest: request];
        }
    }

    pub fn url(&self) -> Option<String> {
        unsafe {
            let url: id = msg_send![self.view, URL];
            if url == nil {
                return None;
            }
            rust_string(msg_send![url, absoluteString])
        }
    }

    pub fn title(&self) -> Option<String> {
        unsafe { rust_string(msg_send![self.view, title]) }
    }

    pub fn is_loading(&self) -> bool {
        unsafe { msg_send![self.view, isLoading] }
    }

    /// Its address, which `WebEvent`s name it by.
    pub fn key(&self) -> usize {
        self.view as usize
    }

    pub fn progress(&self) -> f64 {
        unsafe { msg_send![self.view, estimatedProgress] }
    }

    pub fn can_go_back(&self) -> bool {
        unsafe { msg_send![self.view, canGoBack] }
    }

    pub fn can_go_forward(&self) -> bool {
        unsafe { msg_send![self.view, canGoForward] }
    }

    pub fn go_forward(&self) {
        unsafe {
            let _: id = msg_send![self.view, goForward];
        }
    }

    /// Its size in CSS pixels (the view's bounds).
    pub fn size(&self) -> (f64, f64) {
        unsafe {
            let b: NSRect = msg_send![self.view, bounds];
            (b.size.width, b.size.height)
        }
    }

    pub fn is_shown(&self) -> bool {
        unsafe {
            let hidden: bool = msg_send![self.page, isHidden];
            !hidden
        }
    }

    /// Runs `body` (a function body; `await` works) in the helpers' world, or the page's,
    /// and replies with what it returned as JSON (`null` for nothing), or the error.
    pub fn call(&self, body: &str, in_page: bool, reply: flume::Sender<Result<String, String>>) {
        unsafe {
            let wrapped = format!("const __yhResult = await (async () => {{ {body} }})();\nreturn __yhResult === undefined ? 'null' : JSON.stringify(__yhResult);");
            let block = ConcreteBlock::new(move |result: id, error: id| {
                let answer = if error != nil {
                    let info: id = msg_send![error, userInfo];
                    let message: id = msg_send![info, objectForKey: ns_string("WKJavaScriptExceptionMessage")];
                    let text = rust_string(message).or_else(|| {
                        let d: id = msg_send![error, localizedDescription];
                        rust_string(d)
                    });
                    Err(text.unwrap_or_else(|| "JavaScript error".into()))
                } else {
                    Ok(rust_string(result).unwrap_or_else(|| "null".into()))
                };
                let _ = reply.send(answer);
            });
            let block = block.copy();
            let args: id = msg_send![class!(NSDictionary), dictionary];
            let world = if in_page { page_world() } else { helper_world() };
            let _: () = msg_send![self.view, callAsyncJavaScript: ns_string(&wrapped) arguments: args inFrame: nil inContentWorld: world completionHandler: &*block];
        }
    }

    /// A point in CSS pixels where the window has it, a hundredth of a pixel in (WebKit
    /// truncates the point it converts back).
    fn window_point(&self, x: f64, y: f64) -> NSPoint {
        unsafe {
            let flipped: bool = msg_send![self.view, isFlipped];
            let b: NSRect = msg_send![self.view, bounds];
            let local = NSPoint::new(x + 0.01, if flipped { y + 0.01 } else { b.size.height - y - 0.01 });
            msg_send![self.view, convertPoint: local toView: nil]
        }
    }

    fn mouse_event(&self, kind: usize, at: NSPoint, clicks: isize, modifiers: usize) -> id {
        unsafe {
            let window: id = msg_send![self.view, window];
            let number: isize = if window == nil { 0 } else { msg_send![window, windowNumber] };
            let info: id = msg_send![class!(NSProcessInfo), processInfo];
            let time: f64 = msg_send![info, systemUptime];
            // 2 left up, 4 right up: no pressure.
            let pressure: f32 = if kind == 2 || kind == 4 || kind == 5 { 0. } else { 1. };
            msg_send![class!(NSEvent), mouseEventWithType: kind location: at modifierFlags: modifiers timestamp: time windowNumber: number context: nil eventNumber: 0isize clickCount: clicks pressure: pressure]
        }
    }

    /// A click at a point in CSS pixels, as real mouse events: `left` or `right`, `count` times.
    pub fn click(&self, x: f64, y: f64, right: bool, count: isize, modifiers: usize) {
        let at = self.window_point(x, y);
        self.hover_at(at, modifiers);
        unsafe {
            for n in 1..=count.max(1) {
                if right {
                    let _: () = msg_send![self.view, rightMouseDown: self.mouse_event(3, at, n, modifiers)];
                    let _: () = msg_send![self.view, rightMouseUp: self.mouse_event(4, at, n, modifiers)];
                } else {
                    let _: () = msg_send![self.view, mouseDown: self.mouse_event(1, at, n, modifiers)];
                    let _: () = msg_send![self.view, mouseUp: self.mouse_event(2, at, n, modifiers)];
                }
            }
        }
    }

    pub fn hover(&self, x: f64, y: f64) {
        self.hover_at(self.window_point(x, y), 0);
    }

    /// WebKit hit-tests a plain move only in the key window; a drag-move it hit-tests anywhere.
    fn hover_at(&self, at: NSPoint, modifiers: usize) {
        unsafe {
            let _: () = msg_send![self.view, mouseDragged: self.mouse_event(6, at, 0, modifiers)];
        }
    }

    /// A drag with the mouse, in steps, from one point to another.
    pub fn drag(&self, from: (f64, f64), to: (f64, f64)) {
        let a = self.window_point(from.0, from.1);
        unsafe {
            let _: () = msg_send![self.view, mouseDown: self.mouse_event(1, a, 1, 0)];
            for i in 1..=10 {
                let t = i as f64 / 10.;
                let p = self.window_point(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
                let _: () = msg_send![self.view, mouseDragged: self.mouse_event(6, p, 1, 0)];
            }
            let b = self.window_point(to.0, to.1);
            let _: () = msg_send![self.view, mouseUp: self.mouse_event(2, b, 1, 0)];
        }
    }

    /// Text into what has focus in the page, as input events.
    pub fn insert(&self, text: &str) {
        unsafe {
            let _: () = msg_send![self.view, insertText: ns_string(text)];
        }
    }

    /// A key press: real keydown and keyup.
    pub fn press(&self, key_code: u16, characters: &str, plain: &str, modifiers: usize) {
        unsafe {
            let window: id = msg_send![self.view, window];
            let number: isize = if window == nil { 0 } else { msg_send![window, windowNumber] };
            let make = |kind: usize| -> id {
                msg_send![class!(NSEvent), keyEventWithType: kind location: NSPoint::new(0., 0.) modifierFlags: modifiers timestamp: 0f64 windowNumber: number context: nil characters: ns_string(characters) charactersIgnoringModifiers: ns_string(plain) isARepeat: NO keyCode: key_code]
            };
            let down = make(10);
            if modifiers != 0 {
                let handled: bool = msg_send![self.view, performKeyEquivalent: down];
                if handled {
                    return;
                }
            }
            let _: () = msg_send![self.view, keyDown: down];
            let _: () = msg_send![self.view, keyUp: make(11)];
        }
    }

    /// Part of what it shows (CSS pixels, None for all of it) as a JPEG at most `max_width`
    /// wide, to a file.
    pub fn capture(&self, rect: Option<(f64, f64, f64, f64)>, max_width: f64, path: String, reply: flume::Sender<Result<(u32, u32), String>>) {
        unsafe {
            let (w, h) = self.size();
            let (x, y, cw, ch) = rect.unwrap_or((0., 0., w, h));
            let (x, y) = (x.max(0.), y.max(0.));
            let (cw, ch) = (cw.min(w - x), ch.min(h - y));
            if cw < 1. || ch < 1. {
                let _ = reply.send(Err("That's outside the visible page; scroll it into view first.".into()));
                return;
            }
            let flipped: bool = msg_send![self.view, isFlipped];
            let config: id = msg_send![class!(WKSnapshotConfiguration), new];
            let r = NSRect::new(NSPoint::new(x, if flipped { y } else { h - y - ch }), NSSize::new(cw, ch));
            let _: () = msg_send![config, setRect: r];
            let width = cw.min(max_width);
            let number: id = msg_send![class!(NSNumber), numberWithDouble: width];
            let _: () = msg_send![config, setSnapshotWidth: number];
            let _: () = msg_send![config, setAfterScreenUpdates: YES];
            let out_h = (ch * width / cw).round();
            let block = ConcreteBlock::new(move |image: id, error: id| {
                if error != nil || image == nil {
                    let _ = reply.send(Err("Couldn't capture the page.".into()));
                    return;
                }
                // Drawn at exactly this many pixels: the snapshot comes back at the screen's scale.
                let (pw, ph) = (width.round() as isize, out_h as isize);
                let rep: id = msg_send![class!(NSBitmapImageRep), alloc];
                let rep: id = msg_send![rep, initWithBitmapDataPlanes: std::ptr::null_mut::<*mut u8>() pixelsWide: pw pixelsHigh: ph bitsPerSample: 8isize samplesPerPixel: 4isize hasAlpha: YES isPlanar: NO colorSpaceName: ns_string("NSDeviceRGBColorSpace") bytesPerRow: 0isize bitsPerPixel: 0isize];
                let _: () = msg_send![rep, setSize: NSSize::new(pw as f64, ph as f64)];
                let _: () = msg_send![class!(NSGraphicsContext), saveGraphicsState];
                let context: id = msg_send![class!(NSGraphicsContext), graphicsContextWithBitmapImageRep: rep];
                let _: () = msg_send![class!(NSGraphicsContext), setCurrentContext: context];
                let white: id = msg_send![class!(NSColor), whiteColor];
                let _: () = msg_send![white, setFill];
                let all = NSRect::new(NSPoint::new(0., 0.), NSSize::new(pw as f64, ph as f64));
                unsafe extern "C" {
                    fn NSRectFill(rect: NSRect);
                }
                NSRectFill(all);
                let _: () = msg_send![image, drawInRect: all];
                let _: () = msg_send![class!(NSGraphicsContext), restoreGraphicsState];
                let factor: id = msg_send![class!(NSNumber), numberWithDouble: 0.8f64];
                let key = ns_string("NSImageCompressionFactor");
                let props: id = msg_send![class!(NSDictionary), dictionaryWithObject: factor forKey: key];
                // NSBitmapImageFileTypeJPEG = 3
                let data: id = msg_send![rep, representationUsingType: 3usize properties: props];
                let ok: bool = msg_send![data, writeToFile: ns_string(&path) atomically: YES];
                let _: () = msg_send![rep, release];
                let _ = reply.send(if ok { Ok((pw as u32, ph as u32)) } else { Err("Couldn't write the screenshot.".into()) });
            });
            let block = block.copy();
            let _: () = msg_send![self.view, takeSnapshotWithConfiguration: config completionHandler: &*block];
            let _: () = msg_send![config, release];
        }
    }

    pub fn go_back(&self) {
        unsafe {
            let _: id = msg_send![self.view, goBack];
        }
    }

    pub fn reload(&self) {
        unsafe {
            let _: id = msg_send![self.view, reload];
        }
    }

    /// Runs JavaScript in the page; the reply is its result as JSON text (the script should
    /// return `JSON.stringify(...)`), or the error.
    pub fn evaluate(&self, script: &str, reply: flume::Sender<Result<String, String>>) {
        unsafe {
            let block = ConcreteBlock::new(move |result: id, error: id| {
                let answer = if error != nil {
                    let text: id = msg_send![error, localizedDescription];
                    Err(rust_string(text).unwrap_or_else(|| "JavaScript error".into()))
                } else if result == nil {
                    Ok("null".to_string())
                } else {
                    let is_string: bool = msg_send![result, isKindOfClass: class!(NSString)];
                    if is_string {
                        Ok(rust_string(result).unwrap_or_default())
                    } else {
                        let text: id = msg_send![result, description];
                        Ok(rust_string(text).unwrap_or_default())
                    }
                };
                let _ = reply.send(answer);
            });
            let block = block.copy();
            let _: () = msg_send![self.view, evaluateJavaScript: ns_string(script) completionHandler: &*block];
        }
    }

    /// What it shows now, as a PNG at `path`; the reply says whether it was written.
    pub fn snapshot(&self, path: String, reply: flume::Sender<Result<(), String>>) {
        unsafe {
            let config: id = msg_send![class!(WKSnapshotConfiguration), new];
            let block = ConcreteBlock::new(move |image: id, error: id| {
                if error != nil || image == nil {
                    let _ = reply.send(Err("couldn't take a screenshot".into()));
                    return;
                }
                let tiff: id = msg_send![image, TIFFRepresentation];
                let rep: id = msg_send![class!(NSBitmapImageRep), imageRepWithData: tiff];
                let props: id = msg_send![class!(NSDictionary), dictionary];
                // NSBitmapImageFileTypePNG = 4
                let png: id = msg_send![rep, representationUsingType: 4usize properties: props];
                let ok: bool = msg_send![png, writeToFile: ns_string(&path) atomically: YES];
                let _ = reply.send(if ok { Ok(()) } else { Err("couldn't write the screenshot".into()) });
            });
            let block = block.copy();
            let _: () = msg_send![self.view, takeSnapshotWithConfiguration: config completionHandler: &*block];
            let _: () = msg_send![config, release];
        }
    }
}

impl Drop for WebView {
    fn drop(&mut self) {
        // A dialog left waiting is cancelled, so WebKit isn't left holding its handler.
        self.answer_dialog(false, None);
        unsafe {
            let _: () = msg_send![self.view, stopLoading];
            let _: () = msg_send![self.page, removeFromSuperview];
            let _: () = msg_send![self.view, release];
            let _: () = msg_send![self.page, release];
        }
    }
}
