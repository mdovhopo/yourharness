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

/// A notification from the app (`YH_NOTIFICATIONS=log` prints it instead, so a test run never
/// asks for permission). The first one asks macOS for permission.
pub fn notify(title: &str, body: &str) {
    if std::env::var("YH_NOTIFICATIONS").as_deref() == Ok("log") {
        eprintln!("NOTIFY {title}: {body}");
        return;
    }
    unsafe {
        let center: id = msg_send![class!(UNUserNotificationCenter), currentNotificationCenter];
        if center == nil {
            return;
        }
        static ASKED: std::sync::Once = std::sync::Once::new();
        ASKED.call_once(|| {
            let done = block::ConcreteBlock::new(|_granted: bool, _error: id| {}).copy();
            // Alert and sound.
            let _: () = msg_send![center, requestAuthorizationWithOptions: (1u64 << 1) | (1u64 << 2) completionHandler: &*done];
        });
        let content: id = msg_send![class!(UNMutableNotificationContent), new];
        let _: () = msg_send![content, setTitle: NSString::alloc(nil).init_str(title)];
        let _: () = msg_send![content, setBody: NSString::alloc(nil).init_str(body)];
        let identifier = NSString::alloc(nil).init_str(&yh_core::library::new_uuid());
        let request: id = msg_send![class!(UNNotificationRequest), requestWithIdentifier: identifier content: content trigger: nil];
        let _: () = msg_send![center, addNotificationRequest: request withCompletionHandler: nil];
    }
}
