//! What GPUI doesn't say about the Mac: the window's NSWindow (to park it, and to send it real
//! events as the window server would, the way web/host/HostProbe.swift does), the screen's
//! refresh rate, memory and CPU, and whether a run can be trusted.

#![allow(non_upper_case_globals, unexpected_cfgs, deprecated)]

use cocoa::base::{id, nil, NO, YES};
use cocoa::foundation::{NSPoint, NSRect, NSSize, NSString};
use gpui::Window;
use objc::{class, msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::ffi::c_void;
use std::time::Instant;

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

/// Behind other windows without taking focus (`YHG_BACKGROUND=1`).
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

pub fn click(window: &Window, x: f32, y: f32) {
    unsafe {
        let w = ns_window(window);
        let point = window_point(w, x, y);
        for kind in [NSLeftMouseDown, NSLeftMouseUp] {
            let event = mouse_event(w, kind, point, 1);
            if event != nil {
                let _: () = msg_send![w, sendEvent: event];
            }
        }
    }
}

pub fn mouse_move(window: &Window, x: f32, y: f32) {
    unsafe {
        let w = ns_window(window);
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
extern "C" {
    fn CGEventCreateScrollWheelEvent2(source: *const c_void, units: u32, count: u32, w1: i32, w2: i32, w3: i32) -> CGEventRef;
    fn CGEventSetLocation(event: CGEventRef, point: CGPoint);
    fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
    fn CGMainDisplayID() -> u32;
    fn CGDisplayIsAsleep(display: u32) -> u32;
    fn CGSessionCopyCurrentDictionary() -> *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *const c_void);
}

/// A pixel scroll, as a trackpad sends: through the window, which finds what's under it.
pub fn wheel(window: &Window, x: f32, y: f32, dy: i32) {
    unsafe {
        let w = ns_window(window);
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
        let event: id = msg_send![class!(NSEvent), eventWithCGEvent: cg];
        if event != nil {
            let _: () = msg_send![w, sendEvent: event];
        }
        CFRelease(cg);
    }
}

pub fn key(window: &Window, text: &str) {
    unsafe {
        let w = ns_window(window);
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
