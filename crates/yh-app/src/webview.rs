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

/// Makes the tab a page opens (`window.open`, `target=_blank`): from the opener's address, the
/// configuration to make it with and where it goes, the new web view, or None to refuse.
pub type PopupMaker = fn(usize, id, Option<String>) -> Option<id>;
static POPUPS: std::sync::OnceLock<PopupMaker> = std::sync::OnceLock::new();

/// Who makes the tabs pages open; set once, at start.
pub fn set_popups(maker: PopupMaker) {
    let _ = POPUPS.set(maker);
}

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
                // -999: cancelled, as when another navigation replaces it; 102: the load became
                // a download (or another policy change), said on its own.
                if code == -999 || code == 102 {
                    return;
                }
                let text: id = msg_send![error, localizedDescription];
                let info: id = msg_send![error, userInfo];
                let failing: id = if info == nil { nil } else { msg_send![info, objectForKey: ns_string("NSErrorFailingURLKey")] };
                let url = if failing == nil { None } else { rust_string(msg_send![failing, absoluteString]) };
                post(WebEvent::Failed { view: view as usize, error: rust_string(text).unwrap_or_else(|| "failed".into()), url });
            }
        }
        extern "C" fn committed(_: &Object, _: Sel, view: id, _: id) {
            post(WebEvent::Committed { view: view as usize });
        }
        // The main frame's response: logged with its status (a 404 or 500 shows in the network
        // log and in what the agent's next result says). What can't be shown is a download,
        // which is skipped.
        extern "C" fn response_policy(_: &Object, _: Sel, view: id, response: id, decide: id) {
            unsafe {
                let decide = decide as *mut block::Block<(isize,), ()>;
                let main: bool = msg_send![response, isForMainFrame];
                let can_show: bool = msg_send![response, canShowMIMEType];
                if main {
                    let r: id = msg_send![response, response];
                    let url: id = msg_send![r, URL];
                    let url = if url == nil { String::new() } else { rust_string(msg_send![url, absoluteString]).unwrap_or_default() };
                    let http: bool = msg_send![r, isKindOfClass: class!(NSHTTPURLResponse)];
                    let status: Option<isize> = if http { Some(msg_send![r, statusCode]) } else { None };
                    let json = serde_json::json!({ "kind": "request", "method": "GET", "url": url, "type": "document", "status": status });
                    post(WebEvent::Log { view: view as usize, json: json.to_string() });
                }
                // WKNavigationResponsePolicyAllow 1, …Download 2.
                (*decide).call((if can_show { 1 } else { 2 },));
            }
        }
        extern "C" fn became_download(_: &Object, _: Sel, view: id, response: id, download: id) {
            unsafe {
                let r: id = msg_send![response, response];
                let url: id = msg_send![r, URL];
                let url = if url == nil { None } else { rust_string(msg_send![url, absoluteString]) };
                // Nothing is saved: the download stops here.
                let _: () = msg_send![download, cancel: nil];
                let text = format!("Download of {} skipped: the browser doesn't download files.", url.unwrap_or_else(|| "a file".into()));
                let json = serde_json::json!({ "kind": "console", "level": "info", "text": text });
                post(WebEvent::Log { view: view as usize, json: json.to_string() });
            }
        }
        // `target=_blank`, `window.open`, a sign-in popup: a new tab in the same browser, made with
        // the configuration WebKit gives (the opener's, so `window.opener` works); nil past the
        // most tabs there can be.
        extern "C" fn create_web_view(_: &Object, _: Sel, view: id, configuration: id, action: id, _: id) -> id {
            unsafe {
                let request: id = msg_send![action, request];
                let url: id = if request == nil { nil } else { msg_send![request, URL] };
                let url = if url == nil { None } else { rust_string(msg_send![url, absoluteString]) };
                match POPUPS.get() {
                    Some(open) => open(view as usize, configuration, url).unwrap_or(nil),
                    None => nil,
                }
            }
        }
        extern "C" fn did_close(_: &Object, _: Sel, view: id) {
            post(WebEvent::Closed { view: view as usize });
        }
        extern "C" fn terminated(_: &Object, _: Sel, view: id) {
            post(WebEvent::Crashed { view: view as usize });
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
        decl.add_method(sel!(webView:didCommitNavigation:), committed as extern "C" fn(&Object, Sel, id, id));
        decl.add_method(sel!(webView:decidePolicyForNavigationResponse:decisionHandler:), response_policy as extern "C" fn(&Object, Sel, id, id, id));
        decl.add_method(sel!(webView:navigationResponse:didBecomeDownload:), became_download as extern "C" fn(&Object, Sel, id, id, id));
        decl.add_method(sel!(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:), create_web_view as extern "C" fn(&Object, Sel, id, id, id, id) -> id);
        decl.add_method(sel!(webViewDidClose:), did_close as extern "C" fn(&Object, Sel, id));
        decl.add_method(sel!(webViewWebContentProcessDidTerminate:), terminated as extern "C" fn(&Object, Sel, id));
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

/// Clears everything a project's browser keeps (cookies, storage, caches): its sign-ins go too.
pub fn clear_store(project: &str) {
    unsafe {
        let identifier: id = msg_send![class!(NSUUID), alloc];
        let identifier: id = msg_send![identifier, initWithUUIDString: ns_string(project)];
        if identifier == nil {
            return;
        }
        let data: id = msg_send![class!(WKWebsiteDataStore), dataStoreForIdentifier: identifier];
        let _: id = msg_send![data, retain];
        let types: id = msg_send![class!(WKWebsiteDataStore), allWebsiteDataTypes];
        let since: id = msg_send![class!(NSDate), distantPast];
        let keep = data as usize;
        let done = block::ConcreteBlock::new(move || {
            let data = keep as id;
            let _: () = msg_send![data, release];
        })
        .copy();
        let _: () = msg_send![data, removeDataOfTypes: types modifiedSince: since completionHandler: &*done];
    }
}

/// Every cookie in a project's browser data store (by its UUID), for Export State's sign-ins.
pub fn read_cookies(project: &str, reply: flume::Sender<Vec<yh_core::cookies::Cookie>>) {
    unsafe {
        let identifier: id = msg_send![class!(NSUUID), alloc];
        let identifier: id = msg_send![identifier, initWithUUIDString: ns_string(project)];
        if identifier == nil {
            let _ = reply.send(vec![]);
            return;
        }
        let data: id = msg_send![class!(WKWebsiteDataStore), dataStoreForIdentifier: identifier];
        let _: id = msg_send![data, retain];
        let store: id = msg_send![data, httpCookieStore];
        let data_ptr = data as usize;
        let done = ConcreteBlock::new(move |cookies: id| {
            let mut out = vec![];
            let count: usize = if cookies == nil { 0 } else { msg_send![cookies, count] };
            for i in 0..count {
                let c: id = msg_send![cookies, objectAtIndex: i];
                let (domain, name, value, path): (id, id, id, id) = (msg_send![c, domain], msg_send![c, name], msg_send![c, value], msg_send![c, path]);
                let expires: id = msg_send![c, expiresDate];
                let expires = if expires == nil { None } else { Some(msg_send![expires, timeIntervalSince1970]) };
                let secure: bool = msg_send![c, isSecure];
                let http_only: bool = msg_send![c, isHTTPOnly];
                let same: id = msg_send![c, sameSitePolicy];
                out.push(yh_core::cookies::Cookie {
                    domain: rust_string(domain).unwrap_or_default(),
                    name: rust_string(name).unwrap_or_default(),
                    value: rust_string(value).unwrap_or_default(),
                    path: rust_string(path).unwrap_or_else(|| "/".into()),
                    expires,
                    secure,
                    http_only,
                    same_site: rust_string(same).map(|s| s.to_lowercase()),
                });
            }
            let _: () = msg_send![data_ptr as id, release];
            let _ = reply.send(out);
        })
        .copy();
        let _: () = msg_send![store, getAllCookies: &*done];
        let _: () = msg_send![identifier, release];
    }
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

/// A WebKit switch with no public API (`_setWindowOcclusionDetectionEnabled:` and friends),
/// looked up first so a WebKit without it just behaves as before.
fn spi_bool(object: id, name: &str, value: bool) {
    unsafe {
        let selector = objc::runtime::Sel::register(name);
        let responds: bool = msg_send![object, respondsToSelector: selector];
        if !responds {
            return;
        }
        let imp: *const std::ffi::c_void = msg_send![object, methodForSelector: selector];
        let setter: extern "C" fn(id, objc::runtime::Sel, objc::runtime::BOOL) = std::mem::transmute(imp);
        setter(object, selector, if value { YES } else { NO });
    }
}

/// Lays the page out at `size` whatever the view's size, or at the view's size again (None):
/// viewport units and media queries follow the layout size.
fn fix_layout_size(view: id, size: Option<(f64, f64)>) {
    unsafe {
        let (set_size, set_mode) = (objc::runtime::Sel::register("_setFixedLayoutSize:"), objc::runtime::Sel::register("_setLayoutMode:"));
        let (a, b): (bool, bool) = (msg_send![view, respondsToSelector: set_size], msg_send![view, respondsToSelector: set_mode]);
        if !a || !b {
            return;
        }
        if let Some((w, h)) = size {
            let imp: *const std::ffi::c_void = msg_send![view, methodForSelector: set_size];
            let f: extern "C" fn(id, objc::runtime::Sel, NSSize) = std::mem::transmute(imp);
            f(view, set_size, NSSize::new(w, h));
        }
        // _WKLayoutModeViewSize, _WKLayoutModeFixedSize.
        let imp: *const std::ffi::c_void = msg_send![view, methodForSelector: set_mode];
        let f: extern "C" fn(id, objc::runtime::Sel, isize) = std::mem::transmute(imp);
        f(view, set_mode, if size.is_some() { 1 } else { 0 });
    }
}

/// The mouse buttons WebKit sees held while the agent's mouse events go in. WebKit reads them
/// (`event.buttons`) from the real mouse, `+[NSEvent pressedMouseButtons]`, which is up: every
/// press and drag said no button was down, and pages that check gave up on a drag halfway. So
/// that method answers `HELD` while it's set (the Swift app's `SyntheticMouse`).
mod synthetic_mouse {
    use objc::runtime::{Class, Method, Sel};
    use objc::{class, sel, sel_impl};
    use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

    static HELD: AtomicIsize = AtomicIsize::new(-1);
    static ORIGINAL: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" {
        fn class_getClassMethod(cls: *const Class, name: Sel) -> *mut Method;
        fn method_setImplementation(m: *mut Method, imp: *const std::ffi::c_void) -> *const std::ffi::c_void;
    }

    extern "C" fn pressed(cls: &Class, sel: Sel) -> usize {
        let held = HELD.load(Ordering::SeqCst);
        if held >= 0 {
            return held as usize;
        }
        let original = ORIGINAL.load(Ordering::SeqCst);
        if original == 0 {
            return 0;
        }
        let f: extern "C" fn(&Class, Sel) -> usize = unsafe { std::mem::transmute(original) };
        f(cls, sel)
    }

    fn install() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| unsafe {
            let method = class_getClassMethod(class!(NSEvent), sel!(pressedMouseButtons));
            if !method.is_null() {
                let old = method_setImplementation(method, pressed as *const std::ffi::c_void);
                ORIGINAL.store(old as usize, Ordering::SeqCst);
            }
        });
    }

    /// `buttons` as `pressedMouseButtons` (1 left, 2 right) while `body` sends events.
    pub fn holding<T>(buttons: isize, body: impl FnOnce() -> T) -> T {
        install();
        HELD.store(buttons, Ordering::SeqCst);
        let out = body();
        HELD.store(-1, Ordering::SeqCst);
        out
    }
}

/// Safari's user agent, with the installed Safari's version (as Safari sends it), so sites serve
/// the page this Mac's Safari gets; WebKit's own leaves out `Version/… Safari/…`.
fn user_agent() -> String {
    static AGENT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    AGENT
        .get_or_init(|| {
            let version = unsafe {
                let bundle: id = msg_send![class!(NSBundle), bundleWithPath: ns_string("/Applications/Safari.app")];
                if bundle == nil {
                    None
                } else {
                    let info: id = msg_send![bundle, infoDictionary];
                    rust_string(msg_send![info, objectForKey: ns_string("CFBundleShortVersionString")])
                }
            };
            let version = version.filter(|v| v.chars().all(|c| c.is_ascii_digit() || c == '.')).unwrap_or_else(|| "26.0".into());
            format!("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/{version} Safari/605.1.15")
        })
        .clone()
}

pub struct WebView {
    /// What sits over the window: it draws the page scaled when the page's viewport is
    /// bigger than the panel (its bounds are CSS pixels, its frame points).
    page: id,
    view: id,
    /// The page's viewport in CSS pixels; None fits the panel.
    viewport: std::cell::Cell<Option<(f64, f64)>>,
    /// While a full-page screenshot is taken: the view is this many CSS pixels tall, the page
    /// still laid out at the viewport's size.
    full_height: std::cell::Cell<Option<f64>>,
    /// Where it was last given a place (`set_frame`), to lay it out again.
    bounds: std::cell::Cell<Option<(f64, f64, f64, f64)>>,
    /// Over the page, under nothing that takes the mouse: where the agent just acted, ringed,
    /// and what it's doing, labelled (`show_activity`). Not part of the page, so screenshots
    /// and snapshots never see it. Made on first use.
    overlay: std::cell::Cell<id>,
    activity: std::cell::Cell<u64>,
    /// In the parking window (`parking_view`), off screen, rather than over a panel.
    parked: std::cell::Cell<bool>,
    /// The page may have the keyboard: only after Take Control (`set_keyboard`).
    keyboard: std::cell::Cell<bool>,
}

/// A parked page is drawn at half its CSS size, as the Swift app parked it.
const PARKED_SCALE: f64 = 0.5;

/// Where pages wait while no panel shows them (the Swift app's `BrowserParking`): the content
/// view of a borderless, non-activating panel far off screen that never takes focus or the
/// mouse, stays out of the Window menu, Mission Control, Exposé and ⌘` cycling, and stays
/// ordered in, so WebKit keeps laying pages out, running their timers and animation frames
/// and drawing screenshots (its occlusion detection is off for every page). Made on first use.
fn parking_view() -> id {
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

fn yes_number() -> id {
    unsafe { msg_send![class!(NSNumber), numberWithBool: YES] }
}

/// A view that's drawn but never hit: the mouse goes through it to the page.
fn pass_through_class() -> &'static objc::runtime::Class {
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

/// Fades a layer in, as the Swift app's overlay does.
fn fade_in(layer: id) {
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
            // As Safari: pages may open windows (each a tab here), play media, go full screen.
            let prefs: id = msg_send![config, preferences];
            let _: () = msg_send![prefs, setJavaScriptCanOpenWindowsAutomatically: YES];
            let _: () = msg_send![prefs, setElementFullscreenEnabled: YES];
            let _: () = msg_send![config, setMediaTypesRequiringUserActionForPlayback: 0usize];
            let web = WebView::with_configuration(host, config);
            let _: () = msg_send![config, release];
            web
        }
    }

    /// A web view made with `config` as it is: a page's popup, whose configuration WebKit gives
    /// (the opener's scripts and data store already in it).
    pub fn with_configuration(host: id, config: id) -> WebView {
        unsafe {
            let frame = NSRect::new(NSPoint::new(0., 0.), NSSize::new(10., 10.));
            let view: id = msg_send![class!(WKWebView), alloc];
            let view: id = msg_send![view, initWithFrame: frame configuration: config];
            let _: () = msg_send![view, setHidden: YES];
            // Safari's Develop menu and Show Web Inspector can attach.
            let prefs: id = msg_send![config, preferences];
            let _: () = msg_send![prefs, setValue: yes_number() forKey: ns_string("developerExtrasEnabled")];
            let inspectable = sel!(setInspectable:);
            let can: bool = msg_send![view, respondsToSelector: inspectable];
            if can {
                let _: () = msg_send![view, setInspectable: YES];
            }
            // Safari's user agent: sites serve the page a Mac browser gets.
            let _: () = msg_send![view, setCustomUserAgent: ns_string(&user_agent())];
            let _: () = msg_send![view, setNavigationDelegate: delegate()];
            let _: () = msg_send![view, setUIDelegate: delegate()];
            // A page parked, or in a window behind others, isn't treated as hidden: its timers
            // and animations run at full speed while the agent works.
            spi_bool(view, "_setWindowOcclusionDetectionEnabled:", false);
            let page: id = msg_send![class!(NSView), alloc];
            let page: id = msg_send![page, initWithFrame: frame];
            let _: () = msg_send![page, setHidden: YES];
            let _: () = msg_send![view, setHidden: NO];
            let _: () = msg_send![page, addSubview: view];
            let _: () = msg_send![host, addSubview: page];
            let web = WebView {
                page,
                view,
                viewport: std::cell::Cell::new(Some(LAPTOP)),
                full_height: Default::default(),
                bounds: Default::default(),
                overlay: std::cell::Cell::new(nil),
                activity: Default::default(),
                parked: Default::default(),
                keyboard: Default::default(),
            };
            // The page never takes the keyboard by itself (a page focusing a field, your click
            // in it): what you type goes to the composer until you take control.
            web.set_keyboard(false);
            web.lay_out(LAPTOP.0, LAPTOP.1, LAPTOP.0, LAPTOP.1);
            web
        }
    }

    /// Where it is, for test runs: its window's title, shown or hidden, its frame.
    pub fn report_place(&self) -> String {
        unsafe {
            let window: id = msg_send![self.page, window];
            let title = if window == nil {
                "no window".to_string()
            } else {
                let t: id = msg_send![window, title];
                let utf8: *const std::ffi::c_char = msg_send![t, UTF8String];
                if utf8.is_null() { String::new() } else { std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned() }
            };
            let hidden: bool = msg_send![self.page, isHidden];
            let frame: NSRect = msg_send![self.page, frame];
            let state = if self.parked.get() { "parked, hidden off screen," } else if hidden { "hidden" } else { "shown" };
            format!("in window \"{title}\" {state} at {:.0},{:.0} {:.0}x{:.0}", frame.origin.x, frame.origin.y, frame.size.width, frame.size.height)
        }
    }

    /// Moves it over another GPUI view (the browser's own window, or back), keeping the page.
    pub fn move_to(&self, host: id) {
        unsafe {
            let current: id = msg_send![self.page, superview];
            if current == host || host == nil {
                return;
            }
            // Held while it has no superview, which would let it go.
            let _: () = msg_send![self.page, retain];
            let _: () = msg_send![self.page, removeFromSuperview];
            let _: () = msg_send![host, addSubview: self.page];
            let _: () = msg_send![self.page, release];
        }
    }

    /// Where it shows, in the host view's top-left coordinates (GPUI's); `None` hides it (the
    /// page keeps its viewport). A viewport bigger than the panel is drawn scaled to fit,
    /// against the top.
    pub fn set_frame(&self, bounds: Option<(f64, f64, f64, f64)>) {
        self.bounds.set(bounds);
        unsafe {
            let Some((x, y, w, h)) = bounds else {
                // Parked: off screen in a window of its own, where WebKit keeps laying it out,
                // running its timers and animation frames and drawing screenshots (hidden in
                // the main window, it got no frames, and every action waited on them). It keeps
                // its viewport's size: a screenshot needs no panel.
                let (cw, ch) = self.viewport.get().unwrap_or(self.size());
                let ch = self.full_height.get().unwrap_or(ch);
                let parking = parking_view();
                if parking != nil {
                    self.move_to(parking);
                    self.parked.set(true);
                    self.lay_out(cw * PARKED_SCALE, ch * PARKED_SCALE, cw, ch);
                    let _: () = msg_send![self.page, setFrameOrigin: NSPoint::new(0., 0.)];
                    let _: () = msg_send![self.page, setHidden: NO];
                } else {
                    let _: () = msg_send![self.page, setHidden: YES];
                    self.lay_out(cw, ch, cw, ch);
                }
                return;
            };
            self.parked.set(false);
            let (cw, ch) = self.viewport.get().unwrap_or((w, h));
            let scale = (w / cw).min(h / ch);
            let (fw, fh) = (cw * scale, ch * scale);
            // A full-page shot: as tall as the page, from the same top-left.
            let (css_h, frame_h) = match self.full_height.get() {
                Some(full) => (full, full * scale),
                None => (ch, fh),
            };
            let host: id = msg_send![self.page, superview];
            let flipped: bool = msg_send![host, isFlipped];
            let host_frame: NSRect = msg_send![host, bounds];
            let left = x + (w - fw) / 2.;
            let bottom = if flipped { y } else { host_frame.size.height - y - frame_h };
            let _: () = msg_send![self.page, setFrame: NSRect::new(NSPoint::new(left, bottom), NSSize::new(fw, frame_h))];
            self.lay_out(fw, frame_h, cw, css_h);
            let _: () = msg_send![self.page, setHidden: NO];
        }
    }

    /// For a full-page screenshot: the view `height` CSS pixels tall with the page still laid out
    /// at its viewport (a `100vh` hero stays one screen), or back as it was (None).
    pub fn hold_full_height(&self, height: Option<f64>) {
        let (cw, ch) = self.size_for_layout();
        fix_layout_size(self.view, height.map(|_| (cw, ch)));
        self.full_height.set(height);
        self.set_frame(self.bounds.get());
    }

    /// The viewport's size in CSS pixels, whatever the view is now.
    fn size_for_layout(&self) -> (f64, f64) {
        match (self.viewport.get(), self.bounds.get()) {
            (Some(v), _) => v,
            (None, Some((_, _, w, h))) => (w, h),
            (None, None) => self.size(),
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
            let overlay = self.overlay.get();
            if overlay != nil {
                let _: () = msg_send![overlay, setFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(cw, ch))];
            }
        }
    }

    /// The page's viewport in CSS pixels, None to fit the panel; laid out at the next frame.
    pub fn set_viewport(&self, viewport: Option<(f64, f64)>) {
        self.viewport.set(viewport);
        if let Some((w, h)) = viewport {
            // Parked or hidden, it still takes the size, for a screenshot without the panel.
            let hidden: bool = unsafe { msg_send![self.page, isHidden] };
            if self.parked.get() {
                self.lay_out(w * PARKED_SCALE, h * PARKED_SCALE, w, h);
            } else if hidden {
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
        // `try_with`: a web view let go of as the app quits may outlive the table.
        let Some(reply) = DIALOGS.try_with(|d| d.borrow_mut().remove(&(self.view as usize))).ok().flatten() else { return false };
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

    /// On screen, over a panel or the browser's window (not parked).
    pub fn is_shown(&self) -> bool {
        unsafe {
            let hidden: bool = msg_send![self.page, isHidden];
            !hidden && !self.parked.get()
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
            // 2 left up, 4 right up, a plain move: no pressure.
            let pressure: f32 = if kind == 2 || kind == 4 || kind == 5 || clicks == 0 { 0. } else { 1. };
            msg_send![class!(NSEvent), mouseEventWithType: kind location: at modifierFlags: modifiers timestamp: time windowNumber: number context: nil eventNumber: 0isize clickCount: clicks pressure: pressure]
        }
    }

    /// A click at a point in CSS pixels, as real mouse events: `left` or `right`, `count` times.
    /// WebKit makes the page the window's first responder when it's clicked, which would pull
    /// the keyboard out of the composer you're typing in: the agent's input mustn't. Off again
    /// once the page has handled the events, so your own clicks focus it as usual.
    fn without_taking_focus(&self, body: impl FnOnce()) {
        // Without control the page can't take the keyboard at all: nothing to guard.
        if !self.keyboard.get() {
            body();
            return;
        }
        unsafe {
            let window: id = msg_send![self.view, window];
            let first: id = if window == nil { nil } else { msg_send![window, firstResponder] };
            let selector = sel!(_setShouldSuppressFirstResponderChanges:);
            let can: bool = msg_send![self.view, respondsToSelector: selector];
            if first == self.view || !can {
                body();
                return;
            }
            let _: () = msg_send![self.view, _setShouldSuppressFirstResponderChanges: YES];
            body();
            let view = self.view as usize;
            let done = ConcreteBlock::new(move |_result: id, _error: id| {
                let view = view as id;
                let _: () = msg_send![view, _setShouldSuppressFirstResponderChanges: NO];
            })
            .copy();
            let _: () = msg_send![self.view, evaluateJavaScript: ns_string("0") completionHandler: &*done];
        }
    }

    /// Whether the page may have the keyboard. Off (the default), WebKit never makes it the
    /// window's first responder, so keys go to the app, and if it had the keyboard it goes
    /// back to the app's own view. On (Take Control), it gets the keyboard now.
    pub fn set_keyboard(&self, allowed: bool) {
        self.keyboard.set(allowed);
        unsafe {
            let selector = sel!(_setShouldSuppressFirstResponderChanges:);
            let can: bool = msg_send![self.view, respondsToSelector: selector];
            if can {
                let suppress = if allowed { NO } else { YES };
                let _: () = msg_send![self.view, _setShouldSuppressFirstResponderChanges: suppress];
            }
            let window: id = msg_send![self.view, window];
            if window == nil {
                return;
            }
            if allowed {
                let _: () = msg_send![window, makeFirstResponder: self.view];
                return;
            }
            let first: id = msg_send![window, firstResponder];
            if first == nil {
                return;
            }
            let is_view: bool = msg_send![first, isKindOfClass: class!(NSView)];
            let inside: bool = is_view && msg_send![first, isDescendantOf: self.view];
            if inside {
                let content: id = msg_send![window, contentView];
                let _: () = msg_send![window, makeFirstResponder: content];
            }
        }
    }

    pub fn click(&self, x: f64, y: f64, right: bool, count: isize, modifiers: usize) {
        self.without_taking_focus(|| self.click_now(x, y, right, count, modifiers));
    }

    fn click_now(&self, x: f64, y: f64, right: bool, count: isize, modifiers: usize) {
        let at = self.window_point(x, y);
        self.hover_at(at, modifiers);
        unsafe {
            for n in 1..=count.max(1) {
                if right {
                    synthetic_mouse::holding(2, || {
                        let _: () = msg_send![self.view, rightMouseDown: self.mouse_event(3, at, n, modifiers)];
                    });
                    synthetic_mouse::holding(0, || {
                        let _: () = msg_send![self.view, rightMouseUp: self.mouse_event(4, at, n, modifiers)];
                    });
                } else {
                    synthetic_mouse::holding(1, || {
                        let _: () = msg_send![self.view, mouseDown: self.mouse_event(1, at, n, modifiers)];
                    });
                    synthetic_mouse::holding(0, || {
                        let _: () = msg_send![self.view, mouseUp: self.mouse_event(2, at, n, modifiers)];
                    });
                }
            }
        }
    }

    pub fn hover(&self, x: f64, y: f64) {
        self.hover_at(self.window_point(x, y), 0);
    }

    /// The pointer to a window point, as the page sees a mouse move: `:hover`, mouseover,
    /// mousemove, no button down. WebKit hit-tests a plain move only in the key window (in any
    /// other it just updates scroll bars), and these pages rarely are in it. A move with the
    /// left button held is hit-tested anywhere, and the page sees no button, since it reads
    /// them from `pressedMouseButtons`; with no mouse down before it, nothing is selected.
    fn hover_at(&self, at: NSPoint, modifiers: usize) {
        synthetic_mouse::holding(0, || unsafe {
            let _: () = msg_send![self.view, mouseDragged: self.mouse_event(6, at, 0, modifiers)];
        });
    }

    /// The left button goes down at a point in CSS pixels (after the pointer moves there).
    pub fn mouse_down(&self, x: f64, y: f64) {
        let at = self.window_point(x, y);
        self.hover_at(at, 0);
        self.without_taking_focus(|| {
            synthetic_mouse::holding(1, || unsafe {
                let _: () = msg_send![self.view, mouseDown: self.mouse_event(1, at, 1, 0)];
            })
        });
    }

    /// The pointer moves to a point with the left button held.
    pub fn mouse_dragged(&self, x: f64, y: f64) {
        let at = self.window_point(x, y);
        synthetic_mouse::holding(1, || unsafe {
            let _: () = msg_send![self.view, mouseDragged: self.mouse_event(6, at, 1, 0)];
        });
    }

    pub fn mouse_up(&self, x: f64, y: f64) {
        let at = self.window_point(x, y);
        synthetic_mouse::holding(0, || unsafe {
            let _: () = msg_send![self.view, mouseUp: self.mouse_event(2, at, 1, 0)];
        });
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

    /// Ends the page's web content process, as a crash would (test runs: `{"crash": true}`).
    pub fn end_content_process(&self) -> bool {
        unsafe {
            let selector = sel!(_webProcessIdentifier);
            let can: bool = msg_send![self.view, respondsToSelector: selector];
            if !can {
                return false;
            }
            let pid: i32 = msg_send![self.view, _webProcessIdentifier];
            pid > 0 && libc::kill(pid, libc::SIGKILL) == 0
        }
    }

    pub fn stop_loading(&self) {
        unsafe {
            let _: () = msg_send![self.view, stopLoading];
        }
    }

    /// Safari's Web Inspector for the page, through WebKit's inspector object; false when this
    /// WebKit has none (Safari's Develop menu can still attach: the page is inspectable).
    pub fn show_inspector(&self) -> bool {
        unsafe {
            let selector = sel!(_inspector);
            let can: bool = msg_send![self.view, respondsToSelector: selector];
            if !can {
                return false;
            }
            let inspector: id = msg_send![self.view, _inspector];
            if inspector == nil {
                return false;
            }
            let shows: bool = msg_send![inspector, respondsToSelector: sel!(show)];
            if shows {
                let _: () = msg_send![inspector, show];
            }
            shows
        }
    }

    /// The overlay over the page, made the first time it's needed.
    fn ensure_overlay(&self) -> id {
        unsafe {
            let existing = self.overlay.get();
            if existing != nil {
                return existing;
            }
            let b: NSRect = msg_send![self.view, frame];
            let overlay: id = msg_send![pass_through_class(), alloc];
            let overlay: id = msg_send![overlay, initWithFrame: b];
            let _: () = msg_send![overlay, setWantsLayer: YES];
            let _: () = msg_send![self.page, addSubview: overlay];
            self.overlay.set(overlay);
            overlay
        }
    }

    /// What the agent is doing (`label`: Click, Type, Scroll…), over the page's bottom edge, and
    /// where (CSS pixels), ringed, as the Swift app shows it. Returns its number, for
    /// `clear_activity` to take down this one and not a newer one.
    pub fn show_activity(&self, label: &str, rect: Option<(f64, f64, f64, f64)>) -> u64 {
        let seq = self.activity.get() + 1;
        self.activity.set(seq);
        unsafe {
            let overlay = self.ensure_overlay();
            let layer: id = msg_send![overlay, layer];
            if layer == nil {
                return seq;
            }
            let _: () = msg_send![layer, setSublayers: nil];
            let (cw, ch) = self.size();
            let page_frame: NSRect = msg_send![self.page, frame];
            // Screen points per CSS pixel: the ring and the label keep their size on screen.
            let zoom = if cw > 0. && page_frame.size.width > 0. { (page_frame.size.width / cw).clamp(0.05, 4.) } else { 1. };
            let window: id = msg_send![self.page, window];
            let backing: f64 = if window == nil { 2. } else { msg_send![window, backingScaleFactor] };
            let accent: id = msg_send![class!(NSColor), controlAccentColor];
            let cg = |c: id| -> id { msg_send![c, CGColor] };
            let add = |l: id| {
                let _: () = msg_send![layer, addSublayer: l];
                fade_in(l);
            };
            if let Some((x, y, w, h)) = rect {
                let (w, h) = (w.max(10. / zoom), h.max(10. / zoom));
                let ring: id = msg_send![class!(CALayer), layer];
                let _: () = msg_send![ring, setFrame: NSRect::new(NSPoint::new(x, ch - y - h), NSSize::new(w, h))];
                let _: () = msg_send![ring, setBorderWidth: 2. / zoom];
                let _: () = msg_send![ring, setBorderColor: cg(accent)];
                let soft: id = msg_send![accent, colorWithAlphaComponent: 0.12f64];
                let _: () = msg_send![ring, setBackgroundColor: cg(soft)];
                let _: () = msg_send![ring, setCornerRadius: 4. / zoom];
                add(ring);
            }
            if !label.is_empty() {
                let size = 11. / zoom;
                let font: id = msg_send![class!(NSFont), systemFontOfSize: size weight: 0.23f64];
                let attrs: id = msg_send![class!(NSDictionary), dictionaryWithObject: font forKey: ns_string("NSFont")];
                let text = ns_string(label);
                let measured: NSSize = msg_send![text, sizeWithAttributes: attrs];
                let (tw, th) = (measured.width.ceil(), measured.height.ceil());
                let (pad_x, pad_y) = (10. / zoom, 5. / zoom);
                let (pw, ph) = (tw + pad_x * 2., th + pad_y * 2.);
                let pill: id = msg_send![class!(CALayer), layer];
                let _: () = msg_send![pill, setFrame: NSRect::new(NSPoint::new(((cw - pw) / 2.).max(0.), 8. / zoom), NSSize::new(pw, ph))];
                let back: id = msg_send![class!(NSColor), windowBackgroundColor];
                let back: id = msg_send![back, colorWithAlphaComponent: 0.94f64];
                let _: () = msg_send![pill, setBackgroundColor: cg(back)];
                let _: () = msg_send![pill, setCornerRadius: ph / 2.];
                let line: id = msg_send![class!(NSColor), separatorColor];
                let _: () = msg_send![pill, setBorderColor: cg(line)];
                let _: () = msg_send![pill, setBorderWidth: 1. / zoom];
                let words: id = msg_send![class!(CATextLayer), layer];
                let _: () = msg_send![words, setFrame: NSRect::new(NSPoint::new(pad_x, pad_y), NSSize::new(tw, th))];
                let _: () = msg_send![words, setString: text];
                let _: () = msg_send![words, setFont: font];
                let _: () = msg_send![words, setFontSize: size];
                let ink: id = msg_send![class!(NSColor), labelColor];
                let _: () = msg_send![words, setForegroundColor: cg(ink)];
                let _: () = msg_send![words, setAlignmentMode: ns_string("center")];
                let _: () = msg_send![words, setContentsScale: (backing * zoom).clamp(0.5, 8.)];
                let _: () = msg_send![pill, addSublayer: words];
                add(pill);
            }
        }
        seq
    }

    /// Takes the overlay down, unless a newer one is up.
    pub fn clear_activity(&self, seq: u64) {
        if self.activity.get() != seq {
            return;
        }
        unsafe {
            let overlay = self.overlay.get();
            if overlay == nil {
                return;
            }
            let layer: id = msg_send![overlay, layer];
            if layer != nil {
                let _: () = msg_send![layer, setSublayers: nil];
            }
        }
    }

    /// Whether the overlay shows anything now, for test runs.
    pub fn shows_activity(&self) -> bool {
        unsafe {
            let overlay = self.overlay.get();
            if overlay == nil {
                return false;
            }
            let layer: id = msg_send![overlay, layer];
            let subs: id = if layer == nil { nil } else { msg_send![layer, sublayers] };
            let n: usize = if subs == nil { 0 } else { msg_send![subs, count] };
            n > 0
        }
    }

    /// The overlay alone drawn into a PNG (its CSS-pixel size), for test runs: what the user sees
    /// over the page, which the page's own screenshots leave out.
    pub fn overlay_png(&self, path: &str) -> bool {
        unsafe {
            let overlay = self.overlay.get();
            if overlay == nil {
                return false;
            }
            let layer: id = msg_send![overlay, layer];
            if layer == nil {
                return false;
            }
            let (w, h) = self.size();
            let (pw, ph) = (w.round() as isize, h.round() as isize);
            let rep: id = msg_send![class!(NSBitmapImageRep), alloc];
            let rep: id = msg_send![rep, initWithBitmapDataPlanes: std::ptr::null_mut::<*mut u8>() pixelsWide: pw pixelsHigh: ph bitsPerSample: 8isize samplesPerPixel: 4isize hasAlpha: YES isPlanar: NO colorSpaceName: ns_string("NSDeviceRGBColorSpace") bytesPerRow: 0isize bitsPerPixel: 0isize];
            let context: id = msg_send![class!(NSGraphicsContext), graphicsContextWithBitmapImageRep: rep];
            let cg: *mut std::ffi::c_void = msg_send![context, CGContext];
            let _: () = msg_send![layer, renderInContext: cg];
            let props: id = msg_send![class!(NSDictionary), dictionary];
            let png: id = msg_send![rep, representationUsingType: 4usize properties: props];
            let ok: bool = msg_send![png, writeToFile: ns_string(path) atomically: YES];
            let _: () = msg_send![rep, release];
            ok
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
            let overlay = self.overlay.get();
            if overlay != nil {
                let _: () = msg_send![overlay, release];
            }
            let _: () = msg_send![self.page, release];
        }
    }
}
