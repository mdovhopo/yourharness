//! What GPUI doesn't say about the Mac: the window's NSWindow (to park it, and to send it real
//! events as the window server would, the way web/host/HostProbe.swift does), the screen's
//! refresh rate, memory and CPU, and whether a run can be trusted.

#![allow(non_upper_case_globals, unexpected_cfgs, deprecated)]

use cocoa::base::{id, nil, NO, YES};
use cocoa::foundation::{NSPoint, NSRect, NSSize, NSString};
use gpui_kit::Window;
use objc::{class, msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::ffi::c_void;
use std::time::Instant;

/// GPUI's own view in the window: where native views (the browser) are laid over it.
pub fn ns_view(window: &Window) -> id {
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return nil };
    match handle.as_raw() {
        RawWindowHandle::AppKit(h) => h.ns_view.as_ptr() as id,
        _ => nil,
    }
}

/// Events sent to it dispatch synchronously into GPUI: send them outside any entity update.
pub fn ns_window(window: &Window) -> id {
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return nil };
    match handle.as_raw() {
        RawWindowHandle::AppKit(h) => unsafe {
            let view = h.ns_view.as_ptr() as id;
            msg_send![view, window]
        },
        _ => nil,
    }
}

pub fn screen_hz(window: &Window) -> f64 {
    unsafe {
        let w = ns_window(window);
        let mut screen: id = msg_send![w, screen];
        if screen == nil {
            screen = msg_send![class!(NSScreen), mainScreen];
        }
        let hz: isize = msg_send![screen, maximumFramesPerSecond];
        if hz > 0 { hz as f64 } else { 60.0 }
    }
}

fn main_screen_frame(w: id) -> (NSRect, NSRect) {
    unsafe {
        let mut screen: id = msg_send![w, screen];
        if screen == nil {
            screen = msg_send![class!(NSScreen), mainScreen];
        }
        let frame: NSRect = msg_send![screen, frame];
        let visible: NSRect = msg_send![screen, visibleFrame];
        (frame, visible)
    }
}

/// Below the screen, where it can't take your focus or be seen (as the native and web probes
/// park theirs), or in front filling the screen with `front`.
pub fn place_for_probe(window: &Window, front: bool) -> String {
    unsafe {
        let w = ns_window(window);
        let (frame, visible) = main_screen_frame(w);
        if front {
            let _: () = msg_send![w, setFrame: visible display: YES];
            let _: () = msg_send![w, setLevel: 3isize]; // NSFloatingWindowLevel
            let app: id = msg_send![class!(NSApplication), sharedApplication];
            let _: () = msg_send![app, activateIgnoringOtherApps: YES];
            let _: () = msg_send![w, makeKeyAndOrderFront: nil];
        } else {
            let size = NSSize::new(1440., 900.);
            let rect = NSRect::new(NSPoint::new(frame.origin.x, frame.origin.y - size.height - 200.), size);
            let _: () = msg_send![w, setFrame: rect display: YES];
            let _: () = msg_send![w, orderFrontRegardless];
        }
        let now: NSRect = msg_send![w, frame];
        format!("{}x{}", now.size.width as i64, now.size.height as i64)
    }
}

/// Behind other windows without taking focus (`YH_BACKGROUND=1`).
pub fn order_back(window: &Window) {
    unsafe {
        let w = ns_window(window);
        let _: () = msg_send![w, orderBack: nil];
    }
}

fn content_height(w: id) -> f64 {
    unsafe {
        let view: id = msg_send![w, contentView];
        let frame: NSRect = msg_send![view, frame];
        frame.size.height
    }
}

/// GPUI's window coordinates (top left) into AppKit's (bottom left of the content).
fn window_point(w: id, x: f32, y: f32) -> NSPoint {
    NSPoint::new(x as f64, content_height(w) - y as f64)
}

fn uptime() -> f64 {
    unsafe {
        let info: id = msg_send![class!(NSProcessInfo), processInfo];
        msg_send![info, systemUptime]
    }
}

fn mouse_event(w: id, kind: u64, point: NSPoint, clicks: isize) -> id {
    unsafe {
        let number: isize = msg_send![w, windowNumber];
        msg_send![class!(NSEvent), mouseEventWithType: kind location: point modifierFlags: 0u64
            timestamp: uptime() windowNumber: number context: nil eventNumber: 0isize
            clickCount: clicks pressure: if kind == 1 { 1.0f32 } else { 0.0f32 }]
    }
}

const NSLeftMouseDown: u64 = 1;
const NSLeftMouseUp: u64 = 2;
const NSMouseMoved: u64 = 5;
const NSKeyDown: u64 = 10;
const NSKeyUp: u64 = 11;

pub fn click(w: id, x: f32, y: f32) {
    unsafe {
        let point = window_point(w, x, y);
        for kind in [NSLeftMouseDown, NSLeftMouseUp] {
            let event = mouse_event(w, kind, point, 1);
            if event != nil {
                let _: () = msg_send![w, sendEvent: event];
            }
        }
    }
}

pub fn mouse_move(w: id, x: f32, y: f32) {
    unsafe {
        let _: () = msg_send![w, setAcceptsMouseMovedEvents: YES];
        let event = mouse_event(w, NSMouseMoved, window_point(w, x, y), 0);
        if event != nil {
            let _: () = msg_send![w, sendEvent: event];
        }
    }
}

type CGEventRef = *mut c_void;
#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreateScrollWheelEvent2(source: *const c_void, units: u32, count: u32, w1: i32, w2: i32, w3: i32) -> CGEventRef;
    fn CGEventSetLocation(event: CGEventRef, point: CGPoint);
    fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
    fn CGMainDisplayID() -> u32;
    fn CGDisplayIsAsleep(display: u32) -> u32;
    fn CGSessionCopyCurrentDictionary() -> *const c_void;
}

#[link(name = "UserNotifications", kind = "framework")]
unsafe extern "C" {}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
}

/// A pixel scroll, as a trackpad sends: through the window, which finds what's under it.
pub fn wheel(w: id, x: f32, y: f32, dy: i32) {
    unsafe {
        let point = window_point(w, x, y);
        let screen_point: NSPoint = msg_send![w, convertPointToScreen: point];
        let screens: id = msg_send![class!(NSScreen), screens];
        let first: id = msg_send![screens, objectAtIndex: 0usize];
        let first_frame: NSRect = msg_send![first, frame];
        let cg = CGEventCreateScrollWheelEvent2(std::ptr::null(), 0 /* pixel */, 1, dy, 0, 0);
        if cg.is_null() {
            return;
        }
        CGEventSetLocation(cg, CGPoint { x: screen_point.x, y: first_frame.size.height - screen_point.y });
        let number: isize = msg_send![w, windowNumber];
        CGEventSetIntegerValueField(cg, 91, number as i64); // kCGMouseEventWindowUnderMousePointer
        CGEventSetIntegerValueField(cg, 92, number as i64); // …ThatCanHandleThisEvent
        let mut event: id = msg_send![class!(NSEvent), eventWithCGEvent: cg];
        // Off screen, the window server finds no window under the point, and an event with no
        // window has its location in screen coordinates; GPUI reads `locationInWindow`, so put
        // the point there instead.
        let event_window: id = if event != nil { msg_send![event, window] } else { nil };
        if event_window != w {
            CGEventSetLocation(cg, CGPoint { x: point.x, y: first_frame.size.height - point.y });
            event = msg_send![class!(NSEvent), eventWithCGEvent: cg];
        }
        if event != nil {
            let _: () = msg_send![w, sendEvent: event];
        }
        CFRelease(cg);
    }
}

pub fn key(w: id, text: &str) {
    unsafe {
        let number: isize = msg_send![w, windowNumber];
        for ch in text.chars() {
            let s = NSString::alloc(nil).init_str(&ch.to_string());
            let code: u16 = if ch == '\r' { 36 } else { 0 };
            for kind in [NSKeyDown, NSKeyUp] {
                let event: id = msg_send![class!(NSEvent), keyEventWithType: kind location: NSPoint::new(0., 0.)
                    modifierFlags: 0u64 timestamp: uptime() windowNumber: number context: nil
                    characters: s charactersIgnoringModifiers: s isARepeat: NO keyCode: code];
                if event != nil {
                    let _: () = msg_send![w, sendEvent: event];
                }
            }
            let _: () = msg_send![s, release];
        }
    }
}

/// Footprint (MB) and CPU (% since the last sample) of this process: GPUI draws in it, so
/// there's nothing else to add, unlike the web side's content and GPU processes.
pub struct Sampler {
    last_cpu: f64,
    last_at: Instant,
}

impl Sampler {
    pub fn new() -> Self {
        let mut s = Sampler { last_cpu: 0.0, last_at: Instant::now() };
        s.sample();
        s
    }

    pub fn sample(&mut self) -> (f64, f64) {
        let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            libc::proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V4, &mut info as *mut _ as *mut libc::rusage_info_t)
        } == 0;
        if !ok {
            return (0.0, 0.0);
        }
        #[allow(deprecated)]
        let mut timebase = libc::mach_timebase_info { numer: 0, denom: 0 };
        #[allow(deprecated)]
        unsafe { libc::mach_timebase_info(&mut timebase) };
        let scale = timebase.numer as f64 / timebase.denom as f64;
        let cpu_seconds = (info.ri_user_time + info.ri_system_time) as f64 * scale / 1e9;
        let elapsed = self.last_at.elapsed().as_secs_f64().max(0.001);
        let cpu = (cpu_seconds - self.last_cpu) / elapsed * 100.0;
        self.last_cpu = cpu_seconds;
        self.last_at = Instant::now();
        (info.ri_phys_footprint as f64 / 1_048_576.0, cpu)
    }
}

/// A locked screen or a sleeping display stops frames with nothing wrong in the app.
pub fn run_is_valid() -> bool {
    unsafe {
        let asleep = CGDisplayIsAsleep(CGMainDisplayID()) != 0;
        let mut locked = false;
        let dict = CGSessionCopyCurrentDictionary();
        if !dict.is_null() {
            use core_foundation::base::TCFType;
            use core_foundation::dictionary::CFDictionary;
            use core_foundation::string::CFString;
            let dict: CFDictionary<CFString, core_foundation::base::CFType> =
                CFDictionary::wrap_under_create_rule(dict as *const _);
            if let Some(value) = dict.find(CFString::from_static_string("CGSSessionScreenIsLocked")) {
                locked = value.downcast::<core_foundation::boolean::CFBoolean>().map(bool::from).unwrap_or(false);
            }
        }
        if locked || asleep {
            eprintln!("PERF INVALID locked={locked} displayAsleep={asleep}");
        }
        !locked && !asleep
    }
}

/// Keeps macOS from napping the app (App Nap: timers and the display link slowed down for an
/// app in the background that nobody seems to watch). A parked test window is exactly that, and
/// a run collapsed to 9 fps with the main thread idle about five minutes in until this was on.
pub fn hold_off_app_nap() {
    unsafe {
        let info: id = msg_send![class!(NSProcessInfo), processInfo];
        // NSActivityUserInitiated | NSActivityLatencyCritical
        let options: u64 = 0x00FFFFFF | 0xFF00000000;
        let reason = NSString::alloc(nil).init_str("Performance run");
        let token: id = msg_send![info, beginActivityWithOptions: options reason: reason];
        let _: id = msg_send![token, retain];
    }
}

/// The Dock icon's badge: how many threads wait for you (none: no badge).
pub fn set_badge(count: usize) {
    if std::env::var("YH_NOTIFICATIONS").as_deref() == Ok("log") {
        eprintln!("BADGE {count}");
        return;
    }
    unsafe {
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let tile: id = msg_send![app, dockTile];
        let label = if count == 0 { nil } else { NSString::alloc(nil).init_str(&count.to_string()) };
        let _: () = msg_send![tile, setBadgeLabel: label];
    }
}

/// Whether the app is the one you're using.
pub fn app_is_active() -> bool {
    unsafe {
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let active: bool = msg_send![app, isActive];
        active
    }
}

static NOTIFICATIONS_DENIED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether macOS has notifications off for the app, as last asked (`refresh_notification_status`).
pub fn notifications_denied() -> bool {
    NOTIFICATIONS_DENIED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Asks macOS whether the app may notify; the answer arrives a moment later.
pub fn refresh_notification_status() {
    if std::env::var("YH_NOTIFICATIONS").as_deref() == Ok("log") {
        return;
    }
    unsafe {
        let center: id = msg_send![class!(UNUserNotificationCenter), currentNotificationCenter];
        if center == nil {
            return;
        }
        let done = block::ConcreteBlock::new(|settings: id| {
            // UNAuthorizationStatusDenied = 1
            let status: i64 = msg_send![settings, authorizationStatus];
            NOTIFICATIONS_DENIED.store(status == 1, std::sync::atomic::Ordering::Relaxed);
        })
        .copy();
        let _: () = msg_send![center, getNotificationSettingsWithCompletionHandler: &*done];
    }
}

fn logs_notifications() -> bool {
    std::env::var("YH_NOTIFICATIONS").as_deref() == Ok("log")
}

/// The notification center, outside a test run (which only prints) and inside an app bundle.
fn notification_center() -> Option<id> {
    if logs_notifications() {
        return None;
    }
    unsafe {
        let bundle: id = msg_send![class!(NSBundle), mainBundle];
        let identifier: id = msg_send![bundle, bundleIdentifier];
        if identifier == nil {
            return None;
        }
        let center: id = msg_send![class!(UNUserNotificationCenter), currentNotificationCenter];
        (center != nil).then_some(center)
    }
}

fn ns_string(s: &str) -> id {
    unsafe {
        let s = NSString::alloc(nil).init_str(s);
        msg_send![s, autorelease]
    }
}

/// A notification from the app (`YH_NOTIFICATIONS=log` prints it instead, so a test run never
/// asks for permission). The first one asks macOS for permission.
pub fn notify(title: &str, body: &str) {
    post(title, "", body, None);
}

/// Threads' notifications, clicked: their thread IDs, for the workspace to open.
fn clicks() -> &'static (flume::Sender<String>, flume::Receiver<String>) {
    static CLICKS: std::sync::OnceLock<(flume::Sender<String>, flume::Receiver<String>)> = std::sync::OnceLock::new();
    CLICKS.get_or_init(flume::unbounded)
}

/// The thread of the last notification posted (for `YH_NOTIFICATION_CLICK`).
static LAST_NOTIFIED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
/// Threads whose notifications were cleared since their last one: switching threads asks
/// nothing of the notification center then (it's another process).
static CLEARED: std::sync::Mutex<Option<std::collections::HashSet<String>>> = std::sync::Mutex::new(None);

/// About a thread: `kind` (`finished`, `needsYou`) keeps one of each per thread on screen, a
/// newer one replacing it; its project is the subtitle, with the default sound. Clicking it
/// opens the thread (`notification_clicks`).
pub fn notify_thread(kind: &str, thread: &str, title: &str, subtitle: &str, body: &str) {
    *LAST_NOTIFIED.lock().unwrap() = Some(thread.to_string());
    if let Some(set) = CLEARED.lock().unwrap().as_mut() {
        set.remove(thread);
    }
    if logs_notifications() {
        eprintln!("NOTIFY {kind}: {title} · {subtitle} · {body}");
        return;
    }
    post(title, subtitle, body, Some((thread, &format!("{thread}.{kind}"))));
}

fn post(title: &str, subtitle: &str, body: &str, thread: Option<(&str, &str)>) {
    if logs_notifications() {
        eprintln!("NOTIFY {title}: {body}");
        return;
    }
    let Some(center) = notification_center() else { return };
    install_notification_delegate();
    unsafe {
        static ASKED: std::sync::Once = std::sync::Once::new();
        ASKED.call_once(|| {
            let done = block::ConcreteBlock::new(|_granted: bool, _error: id| {}).copy();
            // Alert and sound.
            let _: () = msg_send![center, requestAuthorizationWithOptions: (1u64 << 1) | (1u64 << 2) completionHandler: &*done];
            std::mem::forget(done);
        });
        let content: id = msg_send![class!(UNMutableNotificationContent), new];
        let _: () = msg_send![content, setTitle: ns_string(title)];
        if !subtitle.is_empty() {
            let _: () = msg_send![content, setSubtitle: ns_string(subtitle)];
        }
        let _: () = msg_send![content, setBody: ns_string(body)];
        let sound: id = msg_send![class!(UNNotificationSound), defaultSound];
        let _: () = msg_send![content, setSound: sound];
        let identifier = match thread {
            Some((thread, identifier)) => {
                let _: () = msg_send![content, setThreadIdentifier: ns_string(thread)];
                let info: id = msg_send![class!(NSDictionary), dictionaryWithObject: ns_string(thread) forKey: ns_string("thread")];
                let _: () = msg_send![content, setUserInfo: info];
                ns_string(identifier)
            }
            None => ns_string(&yh_core::library::new_uuid()),
        };
        let request: id = msg_send![class!(UNNotificationRequest), requestWithIdentifier: identifier content: content trigger: nil];
        let _: () = msg_send![center, addNotificationRequest: request withCompletionHandler: nil];
        let _: () = msg_send![content, release];
    }
}

/// You're looking at the thread now: its notifications have done their job.
pub fn clear_notifications(thread: &str) {
    {
        let mut cleared = CLEARED.lock().unwrap();
        if !cleared.get_or_insert_with(Default::default).insert(thread.to_string()) {
            return;
        }
    }
    let Some(center) = notification_center() else { return };
    unsafe {
        let a = ns_string(&format!("{thread}.finished"));
        let b = ns_string(&format!("{thread}.needsYou"));
        let list: id = msg_send![class!(NSMutableArray), array];
        let _: () = msg_send![list, addObject: a];
        let _: () = msg_send![list, addObject: b];
        let _: () = msg_send![center, removeDeliveredNotificationsWithIdentifiers: list];
    }
}

/// Clicked notifications' threads. Becomes the notification center's delegate, so a click
/// reaches the app (`UNUserNotificationCenterDelegate`), and shows notifications while it's
/// in front too: they're about other threads.
pub fn notification_clicks() -> flume::Receiver<String> {
    install_notification_delegate();
    clicks().1.clone()
}

/// What a click on the last notification would do (`YH_NOTIFICATION_CLICK`): the test path,
/// since a test run never posts a real one.
pub fn click_last_notification() -> Option<String> {
    let thread = LAST_NOTIFIED.lock().unwrap().clone()?;
    let _ = clicks().0.send(thread.clone());
    Some(thread)
}

fn install_notification_delegate() {
    use objc::declare::ClassDecl;
    use objc::runtime::{Object, Sel};
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    let Some(center) = notification_center() else { return };
    extern "C" fn did_receive(_: &Object, _: Sel, _center: id, response: id, handler: id) {
        unsafe {
            let notification: id = msg_send![response, notification];
            let request: id = msg_send![notification, request];
            let content: id = msg_send![request, content];
            let info: id = msg_send![content, userInfo];
            let value: id = msg_send![info, objectForKey: ns_string("thread")];
            if value != nil {
                let utf8: *const std::ffi::c_char = msg_send![value, UTF8String];
                if !utf8.is_null() {
                    let thread = std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned();
                    let _ = clicks().0.send(thread);
                }
            }
            if handler != nil {
                let block = &*(handler as *const block::Block<(), ()>);
                block.call(());
            }
        }
    }
    extern "C" fn will_present(_: &Object, _: Sel, _center: id, _notification: id, handler: id) {
        unsafe {
            if handler != nil {
                // Sound, list and banner.
                let block = &*(handler as *const block::Block<(u64,), ()>);
                block.call(((1 << 1) | (1 << 3) | (1 << 4),));
            }
        }
    }
    INSTALLED.call_once(|| unsafe {
        let Some(mut decl) = ClassDecl::new("YHNotificationDelegate", class!(NSObject)) else { return };
        decl.add_method(
            sel!(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:),
            did_receive as extern "C" fn(&Object, Sel, id, id, id),
        );
        decl.add_method(
            sel!(userNotificationCenter:willPresentNotification:withCompletionHandler:),
            will_present as extern "C" fn(&Object, Sel, id, id, id),
        );
        let cls = decl.register();
        // The center holds its delegate weakly: this one lives as long as the app.
        let delegate: id = msg_send![cls, new];
        let _: () = msg_send![center, setDelegate: delegate];
    });
}

/// In front, the window shown: what clicking a notification does. A test run in the
/// background (`YH_BACKGROUND=1`) stays where it is, so it never takes your focus.
pub fn bring_to_front() {
    if std::env::var("YH_BACKGROUND").is_ok() {
        return;
    }
    unsafe {
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, activateIgnoringOtherApps: YES];
        let windows: id = msg_send![app, windows];
        let count: usize = msg_send![windows, count];
        for i in 0..count {
            let w: id = msg_send![windows, objectAtIndex: i];
            let main: bool = msg_send![w, canBecomeMainWindow];
            if main {
                let mini: bool = msg_send![w, isMiniaturized];
                if mini {
                    let _: () = msg_send![w, deminiaturize: nil];
                }
                let _: () = msg_send![w, makeKeyAndOrderFront: nil];
                break;
            }
        }
    }
}

// MARK: Quick Look

#[link(name = "Quartz", kind = "framework")]
unsafe extern "C" {}

thread_local! {
    /// What Quick Look shows: files, and the one to start at.
    static PREVIEW: std::cell::RefCell<(Vec<String>, usize)> = const { std::cell::RefCell::new((vec![], 0)) };
}

/// Opens files in Quick Look's panel (`QLPreviewPanel`), the arrow keys going through the
/// others, as Finder's space bar does. The panel asks the responder chain for a controller,
/// so the app's delegate is taught to be one, and feeds it from `PREVIEW`.
pub fn quick_look(paths: Vec<String>, at: usize) {
    use objc::runtime::{Class, Object, Sel, BOOL};
    if paths.is_empty() {
        return;
    }
    let at = at.min(paths.len() - 1);
    PREVIEW.with(|p| *p.borrow_mut() = (paths, at));
    extern "C" fn count(_: &Object, _: Sel, _: id) -> isize {
        PREVIEW.with(|p| p.borrow().0.len() as isize)
    }
    extern "C" fn item(_: &Object, _: Sel, _: id, index: isize) -> id {
        let path = PREVIEW.with(|p| p.borrow().0.get(index.max(0) as usize).cloned()).unwrap_or_default();
        unsafe {
            let path = NSString::alloc(nil).init_str(&path);
            let _: () = msg_send![path, autorelease];
            msg_send![class!(NSURL), fileURLWithPath: path]
        }
    }
    extern "C" fn accepts(_: &Object, _: Sel, _: id) -> BOOL {
        YES
    }
    extern "C" fn begin(this: &Object, _: Sel, panel: id) {
        unsafe {
            let _: () = msg_send![panel, setDataSource: this];
            let _: () = msg_send![panel, reloadData];
            let at = PREVIEW.with(|p| p.borrow().1) as isize;
            let _: () = msg_send![panel, setCurrentPreviewItemIndex: at];
        }
    }
    extern "C" fn end(_: &Object, _: Sel, panel: id) {
        unsafe {
            let _: () = msg_send![panel, setDataSource: nil];
        }
    }
    static TAUGHT: std::sync::Once = std::sync::Once::new();
    unsafe {
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let delegate: id = msg_send![app, delegate];
        if delegate == nil {
            return;
        }
        TAUGHT.call_once(|| {
            let cls = objc::runtime::object_getClass(delegate as *const Object) as *mut Class;
            let add = |name: Sel, imp: objc::runtime::Imp, types: &'static [u8]| {
                objc::runtime::class_addMethod(cls, name, imp, types.as_ptr() as *const std::ffi::c_char);
            };
            add(sel!(numberOfPreviewItemsInPreviewPanel:), std::mem::transmute::<extern "C" fn(&Object, Sel, id) -> isize, objc::runtime::Imp>(count), b"q@:@\0");
            add(sel!(previewPanel:previewItemAtIndex:), std::mem::transmute::<extern "C" fn(&Object, Sel, id, isize) -> id, objc::runtime::Imp>(item), b"@@:@q\0");
            add(sel!(acceptsPreviewPanelControl:), std::mem::transmute::<extern "C" fn(&Object, Sel, id) -> BOOL, objc::runtime::Imp>(accepts), b"c@:@\0");
            add(sel!(beginPreviewPanelControl:), std::mem::transmute::<extern "C" fn(&Object, Sel, id), objc::runtime::Imp>(begin), b"v@:@\0");
            add(sel!(endPreviewPanelControl:), std::mem::transmute::<extern "C" fn(&Object, Sel, id), objc::runtime::Imp>(end), b"v@:@\0");
        });
        let panel: id = msg_send![class!(QLPreviewPanel), sharedPreviewPanel];
        if panel == nil {
            return;
        }
        let visible: BOOL = msg_send![panel, isVisible];
        if visible == YES {
            // Already ours: show the new files.
            let _: () = msg_send![panel, reloadData];
            let _: () = msg_send![panel, setCurrentPreviewItemIndex: at as isize];
        } else {
            let _: () = msg_send![panel, makeKeyAndOrderFront: nil];
        }
    }
}

/// Whether Quick Look's panel is up (for test runs), and closes it when `close`.
pub fn quick_look_visible(close: bool) -> bool {
    use objc::runtime::BOOL;
    unsafe {
        let exists: BOOL = msg_send![class!(QLPreviewPanel), sharedPreviewPanelExists];
        if exists != YES {
            return false;
        }
        let panel: id = msg_send![class!(QLPreviewPanel), sharedPreviewPanel];
        let visible: BOOL = msg_send![panel, isVisible];
        let item: id = msg_send![panel, currentPreviewItem];
        if close {
            let _: () = msg_send![panel, orderOut: nil];
        }
        visible == YES && item != nil
    }
}

// MARK: The app's life

/// While Claude works, macOS doesn't put the app to sleep behind your other windows (App Nap):
/// its permission requests and notifications would come late. The Mac itself may still sleep
/// (Keep Awake is separate). The Swift app's `workingChanged`.
pub fn set_working_activity(working: bool) {
    thread_local! {
        static TOKEN: std::cell::Cell<id> = const { std::cell::Cell::new(nil) };
    }
    TOKEN.with(|token| unsafe {
        let info: id = msg_send![class!(NSProcessInfo), processInfo];
        if working && token.get() == nil {
            // NSActivityUserInitiatedAllowingIdleSystemSleep
            let options: u64 = 0x00FF_FFFF & !(1u64 << 20);
            let reason = NSString::alloc(nil).init_str("Claude is working");
            let t: id = msg_send![info, beginActivityWithOptions: options reason: reason];
            let _: id = msg_send![t, retain];
            token.set(t);
            if std::env::var("YH_DATA_DIR").is_ok() {
                eprintln!("APP NAP held off: Claude is working");
            }
        } else if !working && token.get() != nil {
            let t = token.replace(nil);
            let _: () = msg_send![info, endActivity: t];
            let _: () = msg_send![t, release];
            if std::env::var("YH_DATA_DIR").is_ok() {
                eprintln!("APP NAP allowed again");
            }
        }
    });
}

static QUIT_ALLOWED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static TERMINATE_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

thread_local! {
    /// Asks whether to quit (the jobs question): the app's `ui::jobs` guard.
    static ASK_TO_QUIT: std::cell::RefCell<Option<Box<dyn Fn()>>> = const { std::cell::RefCell::new(None) };
}

/// Quitting was decided (asked, or nothing to ask): the next `terminate:` goes through.
pub fn allow_quit() {
    QUIT_ALLOWED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// A quit from outside the app (Dock → Quit, logging out, restarting the Mac) is waiting for
/// the answer: true goes on with it, false stops it. False when none was waiting.
pub fn answer_terminate(quit: bool) -> bool {
    if !TERMINATE_PENDING.swap(false, std::sync::atomic::Ordering::SeqCst) {
        return false;
    }
    if quit {
        allow_quit();
    }
    unsafe {
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, replyToApplicationShouldTerminate: if quit { YES } else { NO }];
    }
    true
}

/// Every way of quitting asks first, as ⌘Q does: GPUI's delegate is taught
/// `applicationShouldTerminate:`, which lets a quit the app decided go (`allow_quit`) and holds
/// any other (`NSTerminateLater`) until `ask` has called `answer_terminate`.
pub fn guard_terminate(ask: impl Fn() + 'static) {
    use objc::runtime::{Class, Object, Sel};
    ASK_TO_QUIT.with(|a| *a.borrow_mut() = Some(Box::new(ask)));
    extern "C" fn should_terminate(_: &Object, _: Sel, _: id) -> u64 {
        if QUIT_ALLOWED.load(std::sync::atomic::Ordering::SeqCst) {
            return 1; // NSTerminateNow
        }
        if !ASK_TO_QUIT.with(|a| a.borrow().is_some()) {
            return 1;
        }
        TERMINATE_PENDING.store(true, std::sync::atomic::Ordering::SeqCst);
        // Asked once this callback has returned: the app may be borrowed right now.
        run_on_main(Box::new(|| {
            ASK_TO_QUIT.with(|a| {
                if let Some(ask) = a.borrow().as_ref() {
                    ask();
                }
            })
        }));
        2 // NSTerminateLater
    }
    static TAUGHT: std::sync::Once = std::sync::Once::new();
    unsafe {
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let delegate: id = msg_send![app, delegate];
        if delegate == nil {
            return;
        }
        TAUGHT.call_once(|| {
            let cls = objc::runtime::object_getClass(delegate as *const Object) as *mut Class;
            let imp = std::mem::transmute::<extern "C" fn(&Object, Sel, id) -> u64, objc::runtime::Imp>(should_terminate);
            let types = b"Q@:@\0";
            if !objc::runtime::class_addMethod(cls, sel!(applicationShouldTerminate:), imp, types.as_ptr() as *const std::ffi::c_char) {
                let m = objc::runtime::class_getInstanceMethod(cls, sel!(applicationShouldTerminate:));
                objc::runtime::method_setImplementation(m as *mut _, imp);
            }
        });
    }
}

#[link(name = "System", kind = "dylib")]
unsafe extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_async_f(queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
}

/// Runs on the main thread at the next turn of its run loop, outside whatever is going on now.
fn run_on_main(job: Box<dyn FnOnce()>) {
    extern "C" fn run(ctx: *mut c_void) {
        let job = unsafe { Box::from_raw(ctx as *mut Box<dyn FnOnce()>) };
        job();
    }
    let ctx = Box::into_raw(Box::new(job)) as *mut c_void;
    unsafe { dispatch_async_f(&raw const _dispatch_main_q, ctx, run) };
}

/// Simulates Dock → Quit (or logging out): `terminate:` sent to the app as AppKit does, outside
/// any update (`YH_TERMINATE_AFTER`, for testing that it asks too).
pub fn terminate_from_outside() {
    run_on_main(Box::new(|| unsafe {
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, terminate: nil];
    }));
}

/// About YourHarness: the standard panel, its version and build from the bundle.
pub fn show_about_panel() {
    unsafe {
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, orderFrontStandardAboutPanel: nil];
        if std::env::var("YH_BACKGROUND").is_err() {
            let _: () = msg_send![app, activateIgnoringOtherApps: YES];
        }
    }
}

/// Window → Bring All to Front.
pub fn bring_all_to_front() {
    unsafe {
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, arrangeInFront: nil];
    }
}

/// The window's frame and its screen's visible frame, in AppKit's points (bottom left), and
/// whether it's full screen.
fn frames(w: id) -> (NSRect, NSRect, bool) {
    unsafe {
        let frame: NSRect = msg_send![w, frame];
        let (_, visible) = main_screen_frame(w);
        let mask: u64 = msg_send![w, styleMask];
        (frame, visible, mask & (1 << 14) != 0)
    }
}

/// Sets an NSWindow's frame on a later turn of the run loop: the resize it causes calls into
/// GPUI, which must not be in the middle of an update then.
fn set_frame_later(w: id, f: NSRect) {
    let w = w as usize;
    run_on_main(Box::new(move || unsafe {
        let _: () = msg_send![w as id, setFrame: f display: YES];
    }));
}

/// A window taller or wider than its screen comes back fitted to it (`fitOnScreen` in the
/// Swift app: 0.0.2, 0.0.3 and 0.0.7 could grow one past the bottom, and saved it so).
/// Returns whether it will change.
pub fn fit_on_screen(window: &Window) -> bool {
    let w = ns_window(window);
    if w == nil {
        return false;
    }
    let (frame, visible, full) = frames(w);
    if full || (frame.size.height <= visible.size.height + 1. && frame.size.width <= visible.size.width + 1.) {
        return false;
    }
    let size = NSSize::new(frame.size.width.min(visible.size.width), frame.size.height.min(visible.size.height));
    let x = frame.origin.x.max(visible.origin.x).min(visible.origin.x + visible.size.width - size.width);
    let y = frame.origin.y.max(visible.origin.y).min(visible.origin.y + visible.size.height - size.height);
    set_frame_later(w, NSRect::new(NSPoint::new(x, y), size));
    true
}

/// Widens the window by `by` points, as far as its screen allows, keeping it on screen (the
/// right sidebar opening on a window too narrow for it). Returns the frame it had and the one
/// it gets, to give the width back later (`set_frame`) if it's still that; None when it can't.
pub fn widen(window: &Window, by: f64) -> Option<([f64; 4], [f64; 4])> {
    let w = ns_window(window);
    if w == nil || by <= 0. {
        return None;
    }
    let (frame, visible, full) = frames(w);
    if full {
        return None;
    }
    let width = (frame.size.width + by).min(visible.size.width);
    if width <= frame.size.width + 0.5 {
        return None;
    }
    let x = frame.origin.x.min(visible.origin.x + visible.size.width - width).max(visible.origin.x);
    set_frame_later(w, NSRect::new(NSPoint::new(x, frame.origin.y), NSSize::new(width, frame.size.height)));
    Some(([frame.origin.x, frame.origin.y, frame.size.width, frame.size.height], [x, frame.origin.y, width, frame.size.height]))
}

/// The window's frame now, in AppKit's points.
pub fn frame_of(window: &Window) -> Option<[f64; 4]> {
    let w = ns_window(window);
    if w == nil {
        return None;
    }
    let (f, _, _) = frames(w);
    Some([f.origin.x, f.origin.y, f.size.width, f.size.height])
}

/// Puts the window back to a frame `widen` returned (not in full screen).
pub fn set_frame(window: &Window, f: [f64; 4]) {
    let w = ns_window(window);
    if w == nil || frames(w).2 {
        return;
    }
    set_frame_later(w, NSRect::new(NSPoint::new(f[0], f[1]), NSSize::new(f[2], f[3])));
}

/// The text over the next open panel (GPUI's `prompt_for_paths` has no message): put on the
/// panel once it's up, which GPUI does on a later turn of the run loop.
pub fn set_open_panel_message(message: &str) {
    fn attempt(message: String, left: u32) {
        run_on_main(Box::new(move || {
            let done = unsafe {
                let app: id = msg_send![class!(NSApplication), sharedApplication];
                let windows: id = msg_send![app, windows];
                let count: usize = msg_send![windows, count];
                let mut done = false;
                for i in 0..count {
                    let w: id = msg_send![windows, objectAtIndex: i];
                    let is_panel: bool = msg_send![w, isKindOfClass: class!(NSOpenPanel)];
                    if is_panel {
                        let text = NSString::alloc(nil).init_str(&message);
                        let _: () = msg_send![w, setMessage: text];
                        done = true;
                    }
                }
                done
            };
            if !done && left > 0 {
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    attempt(message, left - 1);
                });
            }
        }));
    }
    attempt(message.to_string(), 40);
}

/// System Settings → Notifications, for when they're turned off for the app.
pub fn open_notification_settings() {
    let _ = std::process::Command::new("open").arg("x-apple.systempreferences:com.apple.Notifications-Settings.extension").spawn();
}
