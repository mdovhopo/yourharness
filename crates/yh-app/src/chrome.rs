//! Chromium (CEF) in the app, for the browser's Chrome engine. For now the spike only
//! (`YH_CEF_SPIKE`): CEF started inside GPUI's application, one windowed browser per tab laid
//! over the window as WKWebViews are, driven over its in-process DevTools protocol.
//!
//! - CEF needs `NSApp` to answer `isHandlingSendEvent`: `install_app_protocol` adds that to
//!   GPUI's own `GPUIApplication` class at run time, so GPUI needs no patch for it.
//! - Its work runs on the main thread as GPUI's does: CEF asks for it (`on_schedule_message_pump_work`,
//!   from any thread) and it's dispatched to the main queue (`external_message_pump`).
//! - The framework is loaded from wherever it was downloaded (`YH_CEF_FRAMEWORK`); the helper
//!   apps in `Contents/Frameworks` load it from there too (`yh-cef-helper`).

#![allow(unexpected_cfgs, deprecated)]

use cef::*;
use cocoa::base::{id, nil};
use objc::runtime::{Object, Sel};
use objc::{class, msg_send, sel, sel_impl};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{c_void, CString};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The framework's binary, inside the folder it was unpacked to.
const FRAMEWORK: &str = "Chromium Embedded Framework.framework";

// ---- The sandbox: the downloaded framework readable by the helpers ----
//
// Chromium's sandbox lets helpers read the app's own bundle, and the framework is downloaded
// outside it. Policies for renderers and utilities are compiled here, in the app, by the
// framework calling libsandbox's `sandbox_compile_string`; that import is pointed at
// `compile_widened`, which adds one read-only rule for the framework's folder and calls the real
// one. (Other helpers get their policy as source and widen it themselves: `yh-cef-helper`.)

type CompileFn = unsafe extern "C" fn(*const std::ffi::c_char, *mut c_void, *mut *mut std::ffi::c_char) -> *mut c_void;
static REAL_COMPILE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static FRAMEWORK_RULE: OnceLock<CString> = OnceLock::new();
static WIDENED: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" fn compile_widened(profile: *const std::ffi::c_char, params: *mut c_void, error: *mut *mut std::ffi::c_char) -> *mut c_void {
    let real: CompileFn = unsafe { std::mem::transmute(REAL_COMPILE.load(Ordering::Acquire)) };
    let (Some(rule), false) = (FRAMEWORK_RULE.get(), profile.is_null()) else { return unsafe { real(profile, params, error) } };
    let mut text = unsafe { std::ffi::CStr::from_ptr(profile) }.to_bytes().to_vec();
    text.extend_from_slice(rule.as_bytes());
    let widened = CString::new(text).unwrap_or_default();
    WIDENED.fetch_add(1, Ordering::Relaxed);
    unsafe { real(widened.as_ptr(), params, error) }
}

unsafe extern "C" {
    fn _dyld_image_count() -> u32;
    fn _dyld_get_image_header(index: u32) -> *const u8;
    fn _dyld_get_image_vmaddr_slide(index: u32) -> isize;
    fn _dyld_get_image_name(index: u32) -> *const std::ffi::c_char;
    static mach_task_self_: u32;
    fn vm_protect(task: u32, address: usize, size: usize, set_maximum: i32, protection: i32) -> i32;
}

/// Points every import of `symbol` in the loaded image whose path ends with `image` at
/// `replacement` (as fishhook does: its symbol-pointer sections, through the indirect symbol
/// table), returning the address it pointed at, or None when there's none.
unsafe fn rebind(image: &str, symbol: &str, replacement: usize) -> Option<usize> {
    unsafe {
        let index = (0.._dyld_image_count()).find(|&i| {
            let name = _dyld_get_image_name(i);
            !name.is_null() && std::ffi::CStr::from_ptr(name).to_string_lossy().ends_with(image)
        })?;
        let header = _dyld_get_image_header(index);
        let slide = _dyld_get_image_vmaddr_slide(index);
        let read32 = |p: *const u8| (p as *const u32).read_unaligned();
        let read64 = |p: *const u8| (p as *const u64).read_unaligned();
        let ncmds = read32(header.add(16));
        let mut cmd = header.add(32);
        let (mut linkedit, mut symtab, mut dysymtab) = (None, None, None);
        let mut sections = Vec::new();
        for _ in 0..ncmds {
            let (kind, size) = (read32(cmd), read32(cmd.add(4)));
            match kind {
                0x19 => {
                    let segname = std::ffi::CStr::from_ptr(cmd.add(8).cast()).to_bytes().to_vec();
                    if segname == b"__LINKEDIT" {
                        linkedit = Some((read64(cmd.add(24)), read64(cmd.add(40))));
                    } else if segname.starts_with(b"__DATA") || segname.starts_with(b"__AUTH") {
                        let nsects = read32(cmd.add(64));
                        let mut sect = cmd.add(72);
                        for _ in 0..nsects {
                            let flags = read32(sect.add(64));
                            if matches!(flags & 0xff, 0x6 | 0x7) {
                                sections.push((read64(sect.add(32)), read64(sect.add(40)), read32(sect.add(68))));
                            }
                            sect = sect.add(80);
                        }
                    }
                }
                0x2 => symtab = Some((read32(cmd.add(8)), read32(cmd.add(16)))),
                0xb => dysymtab = Some(read32(cmd.add(56))),
                _ => {}
            }
            cmd = cmd.add(size as usize);
        }
        let ((link_vm, link_off), (symoff, stroff), indirectoff) = (linkedit?, symtab?, dysymtab?);
        let base = (slide as i64 + link_vm as i64 - link_off as i64) as usize as *const u8;
        let (syms, strs, indirect) = (base.add(symoff as usize), base.add(stroff as usize), base.add(indirectoff as usize));
        let mut previous = None;
        for (addr, size, first) in sections {
            let slots = (slide as i64 + addr as i64) as usize as *mut usize;
            for i in 0..(size / 8) as usize {
                let sym = read32(indirect.add((first as usize + i) * 4));
                if sym & 0xC000_0000 != 0 {
                    continue;
                }
                let strx = read32(syms.add(sym as usize * 16));
                if std::ffi::CStr::from_ptr(strs.add(strx as usize).cast()).to_bytes() != symbol.as_bytes() {
                    continue;
                }
                let slot = slots.add(i);
                let page = (slot as usize) & !(16383);
                // VM_PROT_READ | VM_PROT_WRITE | VM_PROT_COPY: a private copy of the page.
                if vm_protect(mach_task_self_, page, 16384, 0, 0x1 | 0x2 | 0x10) != 0 {
                    return previous;
                }
                previous.get_or_insert(slot.read());
                slot.write(replacement);
            }
        }
        previous
    }
}

/// Widens the sandbox Chromium compiles to the framework at `framework` (its `.framework`).
unsafe fn widen_sandbox(framework: &std::path::Path) -> bool {
    let dir = std::fs::canonicalize(framework).unwrap_or_else(|_| framework.to_path_buf());
    let quoted = dir.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    let _ = FRAMEWORK_RULE.set(CString::new(format!("\n; YourHarness: the downloaded framework, read-only.\n(allow file-read* (subpath \"{quoted}\"))\n")).unwrap_or_default());
    match unsafe { rebind("/Chromium Embedded Framework", "_sandbox_compile_string", compile_widened as usize) } {
        Some(real) => {
            REAL_COMPILE.store(real, Ordering::Release);
            true
        }
        None => false,
    }
}

// ---- NSApp: the protocol CEF asks of it ----

static HANDLING_SEND_EVENT: AtomicBool = AtomicBool::new(false);

unsafe extern "C" {
    fn object_getClass(obj: id) -> *mut c_void;
    fn class_addMethod(cls: *mut c_void, name: Sel, imp: *const c_void, types: *const std::ffi::c_char) -> bool;
    fn objc_getProtocol(name: *const std::ffi::c_char) -> *mut c_void;
    fn class_addProtocol(cls: *mut c_void, protocol: *mut c_void) -> bool;
    fn objc_getClass(name: *const std::ffi::c_char) -> *mut c_void;
    fn class_replaceMethod(cls: *mut c_void, name: Sel, imp: *const c_void, types: *const std::ffi::c_char) -> *const c_void;
}

extern "C" fn accepts_first_responder(_: &Object, _: Sel) -> objc::runtime::BOOL {
    ALLOW_FOCUS.load(Ordering::Relaxed)
}

/// Chromium's page view takes the keyboard when it's clicked, as WKWebView did; with Typing in
/// Page off it doesn't accept it, so the composer keeps it while the click still reaches the page.
unsafe fn guard_first_responder() {
    unsafe {
        let cls = objc_getClass(c"RenderWidgetHostViewCocoa".as_ptr());
        eprintln!("CEF page view class found {}", !cls.is_null());
        if !cls.is_null() {
            class_replaceMethod(cls, sel!(acceptsFirstResponder), accepts_first_responder as *const c_void, c"c@:".as_ptr());
        }
    }
}

extern "C" fn is_handling_send_event(_: &Object, _: Sel) -> objc::runtime::BOOL {
    HANDLING_SEND_EVENT.load(Ordering::Relaxed)
}

extern "C" fn set_handling_send_event(_: &Object, _: Sel, value: objc::runtime::BOOL) {
    HANDLING_SEND_EVENT.store(value, Ordering::Relaxed);
}

extern "C" fn send_event(this: &Object, _: Sel, event: id) {
    let was = HANDLING_SEND_EVENT.swap(true, Ordering::Relaxed);
    unsafe {
        let _: () = msg_send![super(this, class!(NSApplication)), sendEvent: event];
    }
    HANDLING_SEND_EVENT.store(was, Ordering::Relaxed);
}

/// Adds CEF's `CefAppProtocol` to NSApp's class (GPUI's `GPUIApplication`): the two methods and
/// a `sendEvent:` that marks events being handled. Call after the framework is loaded, which
/// declares the protocols.
unsafe fn install_app_protocol() {
    unsafe {
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let cls = object_getClass(app);
        class_addMethod(cls, sel!(isHandlingSendEvent), is_handling_send_event as *const c_void, c"c@:".as_ptr());
        class_addMethod(cls, sel!(setHandlingSendEvent:), set_handling_send_event as *const c_void, c"v@:c".as_ptr());
        class_addMethod(cls, sel!(sendEvent:), send_event as *const c_void, c"v@:@".as_ptr());
        for name in [c"CrAppProtocol", c"CrAppControlProtocol", c"CefAppProtocol"] {
            let p = objc_getProtocol(name.as_ptr());
            if !p.is_null() {
                class_addProtocol(cls, p);
            }
        }
    }
}

// ---- Or Chromium's own pump: its event loop, nested in GPUI's ----

unsafe extern "C" {
    fn CFRunLoopGetMain() -> *mut c_void;
    fn CFAbsoluteTimeGetCurrent() -> f64;
    fn CFRunLoopTimerCreate(allocator: *const c_void, fire: f64, interval: f64, flags: u64, order: i64, callout: extern "C" fn(*mut c_void, *mut c_void), context: *mut c_void) -> *mut c_void;
    fn CFRunLoopAddTimer(rl: *mut c_void, timer: *mut c_void, mode: *const c_void);
    static kCFRunLoopDefaultMode: *const c_void;
}

struct SendSettings(Settings);
// Only ever touched on the main thread.
unsafe impl Send for SendSettings {}
static NATIVE_SETTINGS: Mutex<Option<SendSettings>> = Mutex::new(None);

extern "C" fn enter_run_loop(_: *mut c_void, _: *mut c_void) {
    let Some(SendSettings(settings)) = NATIVE_SETTINGS.lock().unwrap().take() else { return };
    let mut app = HarnessApp::new();
    let args = cef::args::Args::new();
    let t = Instant::now();
    let ok = initialize(Some(args.as_main_args()), Some(&settings), Some(&mut app), std::ptr::null_mut());
    eprintln!("CEF initialize (native pump) {ok} took {} ms", t.elapsed().as_millis());
    if ok != 1 {
        return;
    }
    eprintln!("CEF entering Chromium's run loop");
    // Chromium's pump sees NSApp already running and runs its own event loop, which hands every
    // event to NSApp as GPUI's does. Entered from a run-loop timer, not the main queue, so GPUI's
    // main-queue tasks still run inside it. Returns when CEF quits.
    run_message_loop();
    eprintln!("CEF left Chromium's run loop");
}

/// Enters Chromium's run loop at the main run loop's next turn.
fn run_loop_soon() {
    unsafe {
        let timer = CFRunLoopTimerCreate(std::ptr::null(), CFAbsoluteTimeGetCurrent(), 0., 0, 0, enter_run_loop, std::ptr::null_mut());
        CFRunLoopAddTimer(CFRunLoopGetMain(), timer, kCFRunLoopDefaultMode);
    }
}

// ---- The message pump, on the main queue ----

#[repr(C)]
struct DispatchQueue {
    _p: [u8; 0],
}
unsafe extern "C" {
    static _dispatch_main_q: DispatchQueue;
    fn dispatch_time(when: u64, delta: i64) -> u64;
    fn dispatch_after_f(when: u64, queue: *const DispatchQueue, context: *mut c_void, work: extern "C" fn(*mut c_void));
}

/// Longest CEF goes without work: its own timers are due at most this late.
const MAX_PUMP_DELAY_MS: i64 = 33;
/// The pump's next run, as ms since `epoch()`; 0 when none is scheduled.
static PUMP_DUE: AtomicU64 = AtomicU64::new(0);
static IN_WORK: AtomicBool = AtomicBool::new(false);
/// A run came due while one was going: CEF runs the main run loop inside its work, and the main
/// queue with it. Rescheduling right then spun until CEF's slice ended; it runs once after instead.
static AGAIN: AtomicBool = AtomicBool::new(false);
/// Spike: pump callbacks that came while CEF's work ran.
static NESTED: AtomicU64 = AtomicU64::new(0);

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    epoch().elapsed().as_millis() as u64 + 1
}

/// How long each `do_message_loop_work` took, for the spike's report.
#[derive(Default, Clone, Debug)]
pub struct PumpStats {
    pub runs: u64,
    pub total_ms: f64,
    pub worst_ms: f64,
    pub over_8ms: u64,
    pub over_16ms: u64,
    /// The five longest, in ms, each with the main thread's CPU time in it.
    pub top: Vec<(f64, f64)>,
    /// Pump callbacks that came while CEF's work ran.
    pub nested: u64,
}

fn thread_cpu_ms() -> f64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    ts.tv_sec as f64 * 1000. + ts.tv_nsec as f64 / 1e6
}
static STATS: Mutex<Option<PumpStats>> = Mutex::new(None);

pub fn pump_stats(reset: bool) -> PumpStats {
    let mut s = STATS.lock().unwrap();
    let mut out = s.clone().unwrap_or_default();
    out.nested = if reset { NESTED.swap(0, Ordering::Relaxed) } else { NESTED.load(Ordering::Relaxed) };
    if reset {
        *s = Some(PumpStats::default());
    }
    out
}

/// Runs CEF's work `delay_ms` from now (0: as soon as the main queue gets to it), unless a run
/// is already due by then. Any thread.
fn schedule_pump(delay_ms: i64) {
    let delay = delay_ms.clamp(0, MAX_PUMP_DELAY_MS) as u64;
    let due = now_ms() + delay;
    let current = PUMP_DUE.load(Ordering::Acquire);
    if current != 0 && current <= due {
        return;
    }
    PUMP_DUE.store(due, Ordering::Release);
    unsafe {
        dispatch_after_f(dispatch_time(0, (delay * 1_000_000) as i64), &raw const _dispatch_main_q, std::ptr::null_mut(), pump);
    }
}

extern "C" fn pump(_: *mut c_void) {
    let due = PUMP_DUE.load(Ordering::Acquire);
    if due == 0 || due > now_ms() {
        // A sooner run replaced this one, or this is a later one that's not yet due.
        return;
    }
    PUMP_DUE.store(0, Ordering::Release);
    if IN_WORK.swap(true, Ordering::AcqRel) {
        AGAIN.store(true, Ordering::Release);
        NESTED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let start = Instant::now();
    let cpu = thread_cpu_ms();
    do_message_loop_work();
    let ms = start.elapsed().as_secs_f64() * 1000.;
    let cpu = thread_cpu_ms() - cpu;
    IN_WORK.store(false, Ordering::Release);
    if let Some(s) = STATS.lock().unwrap().as_mut() {
        s.runs += 1;
        s.total_ms += ms;
        s.worst_ms = s.worst_ms.max(ms);
        s.over_8ms += u64::from(ms > 8.);
        s.over_16ms += u64::from(ms > 16.);
        s.top.push(((ms * 10.).round() / 10., (cpu * 10.).round() / 10.));
        s.top.sort_by(|a, b| b.0.total_cmp(&a.0));
        s.top.truncate(5);
    }
    // CEF's timers need running even when it asks for nothing.
    if AGAIN.swap(false, Ordering::AcqRel) {
        schedule_pump(0);
    } else if std::env::var_os("YH_CEF_NO_FALLBACK").is_none() {
        schedule_pump(MAX_PUMP_DELAY_MS);
    }
}

wrap_browser_process_handler! {
    struct PumpHandler;

    impl BrowserProcessHandler {
        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            schedule_pump(delay_ms);
        }

        fn on_context_initialized(&self) {
            READY.store(true, Ordering::Release);
        }
    }
}

wrap_app! {
    struct HarnessApp;

    impl App {
        fn on_before_command_line_processing(&self, process_type: Option<&CefString>, command_line: Option<&mut CommandLine>) {
            let browser = process_type.map(|p| p.to_string()).unwrap_or_default().is_empty();
            let Some(line) = command_line else { return };
            if !browser {
                return;
            }
            // Never the login keychain: no prompt, in tests or for the user.
            line.append_switch(Some(&CefString::from("use-mock-keychain")));
            // Pages parked off screen, or behind other windows, keep running at full speed.
            for s in ["disable-renderer-backgrounding", "disable-background-timer-throttling", "disable-backgrounding-occluded-windows"] {
                line.append_switch(Some(&CefString::from(s)));
            }
            line.append_switch_with_value(Some(&CefString::from("disable-features")), Some(&CefString::from("MacWebContentsOcclusion")));
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(PumpHandler::new())
        }
    }
}

static STARTED: OnceLock<Result<(), String>> = OnceLock::new();
/// Browsers can be made: CEF's context is up (`on_context_initialized`).
static READY: AtomicBool = AtomicBool::new(false);

pub fn ready() -> bool {
    READY.load(Ordering::Acquire)
}

/// Loads the framework from `framework_dir` (the folder holding the `.framework`) and starts
/// CEF, its profiles under `root_cache`. Once; main thread.
pub fn start(framework_dir: &std::path::Path, root_cache: &std::path::Path) -> Result<(), String> {
    STARTED
        .get_or_init(|| unsafe {
            let framework = framework_dir.join(FRAMEWORK);
            let binary = CString::new(framework.join("Chromium Embedded Framework").to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
            let t = Instant::now();
            if load_library(Some(&*binary.as_ptr())) != 1 {
                return Err(format!("couldn't load {}", framework.display()));
            }
            eprintln!("CEF framework loaded in {} ms", t.elapsed().as_millis());
            let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
            install_app_protocol();
            guard_first_responder();
            eprintln!("CEF sandbox widened to the framework: {}", widen_sandbox(&framework));
            *STATS.lock().unwrap() = Some(PumpStats::default());
            // `YH_CEF_BUNDLE`: the downloaded browser bundle (the framework and the helpers in its
            // Frameworks), which CEF takes as its main bundle: the sandbox lets helpers read it.
            let (bundle_path, subprocess) = match std::env::var("YH_CEF_BUNDLE") {
                Ok(b) => {
                    let helper = format!("{b}/Contents/Frameworks/YourHarness Helper.app/Contents/MacOS/YourHarness Helper");
                    (b, helper)
                }
                Err(_) => {
                    let bundle: id = msg_send![class!(NSBundle), mainBundle];
                    let bundle_path: id = msg_send![bundle, bundlePath];
                    (rust_string(bundle_path).unwrap_or_default(), String::new())
                }
            };
            std::fs::create_dir_all(root_cache).map_err(|e| e.to_string())?;
            let settings = Settings {
                no_sandbox: i32::from(std::env::var_os("YH_CEF_NO_SANDBOX").is_some()),
                framework_dir_path: CefString::from(framework.to_string_lossy().as_ref()),
                main_bundle_path: CefString::from(bundle_path.as_str()),
                browser_subprocess_path: CefString::from(subprocess.as_str()),
                external_message_pump: i32::from(std::env::var_os("YH_CEF_NATIVE_PUMP").is_none()),
                root_cache_path: CefString::from(root_cache.to_string_lossy().as_ref()),
                persist_session_cookies: 1,
                log_severity: LogSeverity::WARNING,
                ..Default::default()
            };
            if std::env::var_os("YH_CEF_NATIVE_PUMP").is_some() {
                *NATIVE_SETTINGS.lock().unwrap() = Some(SendSettings(settings));
                run_loop_soon();
                return Ok(());
            }
            let mut app = HarnessApp::new();
            let args = cef::args::Args::new();
            let t = Instant::now();
            if initialize(Some(args.as_main_args()), Some(&settings), Some(&mut app), std::ptr::null_mut()) != 1 {
                return Err("CEF didn't initialize".into());
            }
            eprintln!("CEF initialize took {} ms", t.elapsed().as_millis());
            schedule_pump(0);
            Ok(())
        })
        .clone()
}

fn rust_string(s: id) -> Option<String> {
    if s == nil {
        return None;
    }
    unsafe {
        let utf8: *const std::ffi::c_char = msg_send![s, UTF8String];
        (!utf8.is_null()).then(|| std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned())
    }
}

// ---- A tab: one windowed browser ----

/// Whether pages may take the keyboard (Typing in Page); off, CEF's focus requests are refused.
static ALLOW_FOCUS: AtomicBool = AtomicBool::new(false);
static NEXT_ID: AtomicI32 = AtomicI32::new(1);

thread_local! {
    /// DevTools calls waiting on their results, by message id.
    static CALLS: RefCell<HashMap<i32, flume::Sender<Result<serde_json::Value, String>>>> = RefCell::new(HashMap::new());
    /// DevTools events awaited, by method: their params.
    static EVENT_WAITS: RefCell<HashMap<String, flume::Sender<serde_json::Value>>> = RefCell::new(HashMap::new());
    /// Loads finished, by browser id: the main frame's HTTP status.
    static LOADS: RefCell<HashMap<i32, flume::Sender<i32>>> = RefCell::new(HashMap::new());
}

wrap_dev_tools_message_observer! {
    struct Observer;

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(&self, _browser: Option<&mut Browser>, message_id: i32, success: i32, result: Option<&[u8]>) {
            let value: serde_json::Value = result.and_then(|r| serde_json::from_slice(r).ok()).unwrap_or_default();
            let reply = if success == 1 { Ok(value) } else { Err(value.to_string()) };
            if let Some(tx) = CALLS.with(|c| c.borrow_mut().remove(&message_id)) {
                let _ = tx.send(reply);
            }
        }

        fn on_dev_tools_event(&self, _browser: Option<&mut Browser>, method: Option<&CefString>, params: Option<&[u8]>) {
            let method = method.map(|m| m.to_string()).unwrap_or_default();
            if let Some(tx) = EVENT_WAITS.with(|w| w.borrow_mut().remove(&method)) {
                let _ = tx.send(params.and_then(|p| serde_json::from_slice(p).ok()).unwrap_or_default());
            }
            if std::env::var_os("YH_CEF_LOG").is_some() && !method.starts_with("Tracing.") {
                let params = params.map(|p| String::from_utf8_lossy(&p[..p.len().min(300)]).into_owned()).unwrap_or_default();
                eprintln!("CEF event {method} {params}");
            }
        }
    }
}

wrap_focus_handler! {
    struct Focus;

    impl FocusHandler {
        fn on_set_focus(&self, _browser: Option<&mut Browser>, source: FocusSource) -> i32 {
            let allow = ALLOW_FOCUS.load(Ordering::Relaxed);
            if std::env::var_os("YH_CEF_LOG").is_some() {
                eprintln!("CEF focus asked ({:?}): {}", source.as_ref(), if allow { "allowed" } else { "refused" });
            }
            i32::from(!allow)
        }
    }
}

wrap_load_handler! {
    struct Loads;

    impl LoadHandler {
        fn on_load_end(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, http_status_code: i32) {
            let main = frame.map(|f| f.is_main() == 1).unwrap_or(false);
            let Some(browser) = browser else { return };
            if main {
                let id = browser.identifier();
                if let Some(tx) = LOADS.with(|l| l.borrow_mut().remove(&id)) {
                    let _ = tx.send(http_status_code);
                }
            }
        }
    }
}

wrap_life_span_handler! {
    struct Life;

    impl LifeSpanHandler {
        /// The window it's in is the app's: CEF would close it with the browser (its default for
        /// a browser in a window of its own). True: only the browser goes.
        fn do_close(&self, browser: Option<&mut Browser>) -> i32 {
            // Its view taken out of the window: then CEF destroys it (`on_before_close`).
            if let Some(view) = browser.and_then(|b| b.host()).map(|h| h.window_handle() as id) {
                unsafe {
                    let _: () = msg_send![view, removeFromSuperview];
                }
            }
            1
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            if std::env::var_os("YH_CEF_LOG").is_some() {
                eprintln!("CEF browser {} closed", browser.map(|b| b.identifier()).unwrap_or(-1));
            }
        }
    }
}

wrap_client! {
    struct TabClient;

    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(Life::new())
        }

        fn focus_handler(&self) -> Option<FocusHandler> {
            Some(Focus::new())
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(Loads::new())
        }
    }
}

/// One tab's browser. Main thread only.
pub struct Tab {
    browser: Browser,
    _observer: Option<Registration>,
    context: Option<RequestContext>,
}

wrap_request_context_handler! {
    struct ContextReady {
        ready: flume::Sender<()>,
    }

    impl RequestContextHandler {
        fn on_request_context_initialized(&self, _request_context: Option<&mut RequestContext>) {
            let _ = self.ready.send(());
        }
    }
}

/// A profile kept in `cache` (a folder under the root cache), or in memory with None, and
/// when it's ready for browsers.
pub fn profile(cache: Option<&std::path::Path>) -> (Option<RequestContext>, flume::Receiver<()>) {
    let settings = RequestContextSettings {
        cache_path: CefString::from(cache.map(|c| c.to_string_lossy().into_owned()).unwrap_or_default().as_str()),
        persist_session_cookies: 1,
        ..Default::default()
    };
    let (tx, rx) = flume::bounded(1);
    (request_context_create_context(Some(&settings), Some(&mut ContextReady::new(tx))), rx)
}

async fn ready_profile(cache: Option<&std::path::Path>, cx: &gpui_kit::AsyncApp) -> Option<RequestContext> {
    let t = Instant::now();
    let (ctx, ready) = profile(cache);
    let ok = within(ready, 10, cx).await.is_some();
    eprintln!("CEF profile {:?} ready {ok} in {} ms", cache.map(|c| c.file_name()), t.elapsed().as_millis());
    ctx
}

impl Tab {
    /// A browser in `host` (an NSView) at `url`, in `context`'s profile.
    pub fn new(host: id, url: &str, context: Option<RequestContext>) -> Option<Tab> {
        let info = WindowInfo {
            parent_view: host as *mut c_void,
            bounds: Rect { x: 0, y: 0, width: 400, height: 400 },
            runtime_style: RuntimeStyle::ALLOY,
            ..Default::default()
        };
        let mut client = TabClient::new();
        let mut ctx = context.clone();
        let browser = browser_host_create_browser_sync(Some(&info), Some(&mut client), Some(&CefString::from(url)), Some(&BrowserSettings::default()), None, ctx.as_mut())?;
        let observer = browser.host().and_then(|h| h.add_dev_tools_message_observer(Some(&mut Observer::new())));
        Some(Tab { browser, _observer: observer, context })
    }

    /// The browser's own NSView (CEF's), to place and move.
    pub fn view(&self) -> id {
        self.browser.host().map(|h| h.window_handle() as id).unwrap_or(nil)
    }

    /// Where it shows, in its superview's top-left coordinates.
    pub fn set_frame(&self, x: f64, y: f64, w: f64, h: f64) {
        use cocoa::foundation::{NSPoint, NSRect, NSSize};
        let view = self.view();
        unsafe {
            let host: id = msg_send![view, superview];
            let flipped: bool = msg_send![host, isFlipped];
            let bounds: NSRect = msg_send![host, bounds];
            let bottom = if flipped { y } else { bounds.size.height - y - h };
            let _: () = msg_send![view, setFrame: NSRect::new(NSPoint::new(x, bottom), NSSize::new(w, h))];
        }
        if let Some(host) = self.browser.host() {
            host.was_resized();
        }
    }

    /// Moves its view into `host` (another window's view, or the parking panel's).
    pub fn move_to(&self, host: id) {
        let view = self.view();
        unsafe {
            let _: () = msg_send![view, retain];
            let _: () = msg_send![view, removeFromSuperview];
            let _: () = msg_send![host, addSubview: view];
            let _: () = msg_send![view, release];
        }
    }

    /// Resolves once the main frame has loaded, with its HTTP status.
    pub fn loaded(&self) -> flume::Receiver<i32> {
        let (tx, rx) = flume::bounded(1);
        LOADS.with(|l| l.borrow_mut().insert(self.browser.identifier(), tx));
        rx
    }

    pub fn load(&self, url: &str) {
        if let Some(f) = self.browser.main_frame() {
            f.load_url(Some(&CefString::from(url)));
        }
    }

    /// A DevTools protocol call, answered with its result.
    pub fn cdp(&self, method: &str, params: serde_json::Value) -> flume::Receiver<Result<serde_json::Value, String>> {
        let (tx, rx) = flume::bounded(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        CALLS.with(|c| c.borrow_mut().insert(id, tx.clone()));
        let message = serde_json::json!({"id": id, "method": method, "params": params}).to_string();
        let sent = self.browser.host().map(|h| h.send_dev_tools_message(Some(message.as_bytes()))).unwrap_or(0);
        if sent != 1 {
            CALLS.with(|c| c.borrow_mut().remove(&id));
            let _ = tx.send(Err(format!("{method} not sent")));
        }
        rx
    }

    /// The next DevTools event named `method`, its params.
    pub fn next_event(&self, method: &str) -> flume::Receiver<serde_json::Value> {
        let (tx, rx) = flume::bounded(1);
        EVENT_WAITS.with(|w| w.borrow_mut().insert(method.to_string(), tx));
        rx
    }

    /// Flushes its profile's cookies to disk.
    pub fn flush_cookies(&self) {
        if let Some(m) = self.context.as_ref().and_then(|c| c.cookie_manager(None)) {
            m.flush_store(None);
        }
    }

    pub fn close(&self) {
        if let Some(h) = self.browser.host() {
            h.close_browser(1);
        }
    }
}

pub fn set_allow_focus(on: bool) {
    ALLOW_FOCUS.store(on, Ordering::Relaxed);
}

// ---- The spike ----

/// Awaits `rx` up to `secs`.
async fn within<T>(rx: flume::Receiver<T>, secs: u64, cx: &gpui_kit::AsyncApp) -> Option<T> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        match rx.try_recv() {
            Ok(v) => return Some(v),
            Err(flume::TryRecvError::Disconnected) => return None,
            Err(flume::TryRecvError::Empty) if Instant::now() > deadline => return None,
            Err(flume::TryRecvError::Empty) => cx.background_executor().timer(Duration::from_millis(5)).await,
        }
    }
}

fn base64_decode(text: &str) -> Vec<u8> {
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

/// `Runtime.evaluate` of `expr` in the page (or the isolated world `context`), its value.
async fn eval(tab: &Tab, expr: &str, context: Option<i64>, cx: &gpui_kit::AsyncApp) -> String {
    let mut params = serde_json::json!({"expression": expr, "returnByValue": true, "awaitPromise": true});
    if let Some(c) = context {
        params["contextId"] = c.into();
    }
    match within(tab.cdp("Runtime.evaluate", params), 10, cx).await {
        Some(Ok(v)) => v["result"]["value"].to_string(),
        Some(Err(e)) => format!("error {e}"),
        None => "timed out".into(),
    }
}

async fn screenshot(tab: &Tab, path: &str, cx: &gpui_kit::AsyncApp) -> String {
    match within(tab.cdp("Page.captureScreenshot", serde_json::json!({"format": "png"})), 10, cx).await {
        Some(Ok(v)) => {
            let bytes = base64_decode(v["data"].as_str().unwrap_or_default());
            let _ = std::fs::write(path, &bytes);
            format!("{} bytes → {path}", bytes.len())
        }
        Some(Err(e)) => format!("error {e}"),
        None => "timed out".into(),
    }
}

/// rAF frames and 10 ms ticks the page counted over two seconds.
async fn speed(tab: &Tab, cx: &gpui_kit::AsyncApp) -> String {
    let before = eval(tab, "[raf, ticks]", None, cx).await;
    cx.background_executor().timer(Duration::from_secs(2)).await;
    let after = eval(tab, "[raf, ticks]", None, cx).await;
    let parse = |s: &str| serde_json::from_str::<Vec<i64>>(s).unwrap_or_default();
    let (b, a) = (parse(&before), parse(&after));
    if a.len() == 2 && b.len() == 2 {
        format!("rAF {} /2s, timers {} /2s", a[0] - b[0], a[1] - b[1])
    } else {
        format!("unreadable: {before} → {after}")
    }
}

fn first_responder(window: id) -> String {
    unsafe {
        let r: id = msg_send![window, firstResponder];
        if r == nil {
            return "nil".into();
        }
        let class: id = msg_send![r, className];
        rust_string(class).unwrap_or_default()
    }
}

unsafe extern "C" {
    fn sandbox_check(pid: libc::pid_t, operation: *const std::ffi::c_char, filter: i32, ...) -> i32;
}

/// Memory of the app and of every process it started, in MB, and whether each is sandboxed.
fn memory() -> String {
    let me = std::process::id();
    let Ok(out) = std::process::Command::new("ps").args(["-axo", "pid=,ppid=,rss=,comm="]).output() else { return "ps failed".into() };
    let text = String::from_utf8_lossy(&out.stdout);
    let rows: Vec<(u32, u32, u64, String)> = text
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let ppid = it.next()?.parse().ok()?;
            let rss = it.next()?.parse().ok()?;
            Some((pid, ppid, rss, it.collect::<Vec<_>>().join(" ")))
        })
        .collect();
    let own = rows.iter().find(|r| r.0 == me).map(|r| r.2).unwrap_or(0);
    let mut helpers = Vec::new();
    let mut total = 0;
    for r in rows.iter().filter(|r| r.1 == me) {
        let name = r.3.rsplit('/').next().unwrap_or(&r.3).to_string();
        let sandboxed = unsafe { sandbox_check(r.0 as libc::pid_t, std::ptr::null(), 0) } != 0;
        let _ = WIDENED.load(Ordering::Relaxed);
        helpers.push(format!("{name} {} MB{}", r.2 / 1024, if sandboxed { " (sandboxed)" } else { "" }));
        total += r.2;
    }
    format!("policies widened {} · app {} MB; {} children {} MB: {}", WIDENED.load(Ordering::Relaxed), own / 1024, helpers.len(), total / 1024, helpers.join(", "))
}

/// Spike: a main-queue heartbeat every 2 ms, how late each one ran (ms × 100, worst and >8 ms).
static BEAT_WORST: AtomicU64 = AtomicU64::new(0);
static BEAT_LATE: AtomicU64 = AtomicU64::new(0);
static BEATS: AtomicU64 = AtomicU64::new(0);
static BEAT_DUE: AtomicU64 = AtomicU64::new(0);

extern "C" fn beat(_: *mut c_void) {
    let now = epoch().elapsed().as_micros() as u64;
    let due = BEAT_DUE.load(Ordering::Relaxed);
    let late = now.saturating_sub(due);
    BEAT_WORST.fetch_max(late, Ordering::Relaxed);
    BEAT_LATE.fetch_add(u64::from(late > 8_000), Ordering::Relaxed);
    BEATS.fetch_add(1, Ordering::Relaxed);
    BEAT_DUE.store(now + 2_000, Ordering::Relaxed);
    unsafe { dispatch_after_f(dispatch_time(0, 2_000_000), &raw const _dispatch_main_q, std::ptr::null_mut(), beat) };
}

fn start_beat() {
    BEAT_DUE.store(epoch().elapsed().as_micros() as u64 + 2_000, Ordering::Relaxed);
    unsafe { dispatch_after_f(dispatch_time(0, 2_000_000), &raw const _dispatch_main_q, std::ptr::null_mut(), beat) };
}

/// Since the last call: the main thread's CPU time and how late the heartbeat ran.
fn main_load(since_cpu: &mut f64) -> String {
    let cpu = thread_cpu_ms();
    let used = cpu - *since_cpu;
    *since_cpu = cpu;
    format!(
        "main thread CPU {used:.0} ms · heartbeat {} beats, worst late {:.1} ms, >8 ms late {}",
        BEATS.swap(0, Ordering::Relaxed),
        BEAT_WORST.swap(0, Ordering::Relaxed) as f64 / 1000.,
        BEAT_LATE.swap(0, Ordering::Relaxed)
    )
}

thread_local! {
    /// Gaps between GPUI frames (ms), drawn at every vsync while the spike runs.
    static FRAMES: RefCell<Vec<f64>> = const { RefCell::new(Vec::new()) };
}

fn frame_tick(last: Instant, window: &mut gpui_kit::Window) {
    window.on_next_frame(move |window, _| {
        let now = Instant::now();
        FRAMES.with(|f| f.borrow_mut().push((now - last).as_secs_f64() * 1000.));
        window.refresh();
        frame_tick(now, window);
    });
    window.refresh();
}

/// Chromium's trace of `secs` seconds: the browser process's main-thread tasks over 4 ms,
/// longest first.
async fn trace(tab: &Tab, secs: u64, cx: &gpui_kit::AsyncApp) -> String {
    let started = within(tab.cdp("Tracing.start", serde_json::json!({"categories": "toplevel,toplevel.flow,ipc,mojom,cc,viz,gpu,benchmark,disabled-by-default-devtools.timeline", "transferMode": "ReturnAsStream"})), 5, cx).await;
    if !matches!(started, Some(Ok(_))) {
        return format!("Tracing.start: {started:?}");
    }
    cx.background_executor().timer(Duration::from_secs(secs)).await;
    let done = tab.next_event("Tracing.tracingComplete");
    let _ = within(tab.cdp("Tracing.end", serde_json::json!({})), 5, cx).await;
    let Some(done) = within(done, 20, cx).await else { return "no tracingComplete".into() };
    let stream = done["stream"].as_str().unwrap_or_default().to_string();
    let mut text = String::new();
    loop {
        let Some(Ok(chunk)) = within(tab.cdp("IO.read", serde_json::json!({"handle": stream, "size": 1 << 20})), 10, cx).await else { break };
        text += chunk["data"].as_str().unwrap_or_default();
        if chunk["eof"].as_bool().unwrap_or(true) {
            break;
        }
    }
    let _ = tab.cdp("IO.close", serde_json::json!({"handle": stream}));
    let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let events = json["traceEvents"].as_array().cloned().unwrap_or_default();
    let me = std::process::id() as i64;
    // The browser process's main thread: CrBrowserMain.
    let main_tid = events.iter().find(|e| e["pid"].as_i64() == Some(me) && e["name"] == "thread_name" && e["args"]["name"] == "CrBrowserMain").and_then(|e| e["tid"].as_i64());
    let mut by: HashMap<String, (f64, usize)> = HashMap::new();
    let names: HashMap<i64, String> = events
        .iter()
        .filter(|e| e["pid"].as_i64() == Some(me) && e["name"] == "thread_name")
        .filter_map(|e| Some((e["tid"].as_i64()?, e["args"]["name"].as_str().unwrap_or("?").to_string())))
        .collect();
    for e in events.iter().filter(|e| e["pid"].as_i64() == Some(me) && e["ph"] == "X" && e["name"] == "ThreadControllerImpl::RunTask") {
        let src = e["args"]["src_file"].as_str().or(e["args"]["src"].as_str()).unwrap_or("");
        let func = e["args"]["src_func"].as_str().unwrap_or("");
        let tid = e["tid"].as_i64().unwrap_or(0);
        let thread = names.get(&tid).cloned().unwrap_or_else(|| format!("tid {tid}"));
        let key = format!("[{thread}] {}:{func}", src.rsplit('/').next().unwrap_or(src));
        let v = by.entry(key).or_default();
        v.0 += e["dur"].as_f64().unwrap_or(0.) / 1000.;
        v.1 += 1;
    }
    let mut long: Vec<(f64, String)> = by.into_iter().map(|(k, (ms, n))| (ms, format!("{k} ×{n}"))).collect();
    format!("{} events, main tid {main_tid:?}; most main-thread time: {}", events.len(), long.iter().map(|(d, n)| format!("{d:.1} ms {n}")).collect::<Vec<_>>().join("\n    "))
}

/// Frames since the last call: how many were late at `hz`, the worst gap.
fn frames(hz: f64) -> String {
    let d = FRAMES.with(|f| std::mem::take(&mut *f.borrow_mut()));
    let period = 1000. / hz;
    let (mut vsyncs, mut missed) = (0usize, 0usize);
    for &g in &d {
        let spent = ((g / period).round() as usize).max(1);
        vsyncs += spent;
        missed += spent - 1;
    }
    let worst = d.iter().copied().fold(0., f64::max);
    format!("frames {} · on time {:.1}% · worst {:.1} ms", d.len(), if vsyncs > 0 { (vsyncs - missed) as f64 / vsyncs as f64 * 100. } else { 0. }, worst)
}

/// `YH_CEF_SPIKE=<url of spike.html>` with `YH_CEF_FRAMEWORK=<folder>`: the spike's checks, each
/// printed as `CEF …`, then quits. `YH_CEF_HEAVY=<url>` is loaded last to time the pump.
pub fn spike(window: gpui_kit::AnyWindowHandle, url: String, cx: &mut gpui_kit::App) {
    let framework = std::env::var("YH_CEF_FRAMEWORK").unwrap_or_default();
    let data = std::env::var("YH_DATA_DIR").unwrap_or_else(|_| "/tmp/yh-cef".into());
    let out = std::env::var("YH_CEF_OUT").unwrap_or_else(|_| "/tmp/yh-cef-out".into());
    let _ = std::fs::create_dir_all(&out);
    cx.spawn(async move |cx| {
        cx.background_executor().timer(Duration::from_secs(2)).await;
        crate::platform::hold_off_app_nap();
        let hz = window.update(cx, |_, window, _| {
            if std::env::var_os("YH_CEF_NO_FRAMES").is_none() {
                frame_tick(Instant::now(), window);
            }
            crate::platform::screen_hz(window)
        }).unwrap_or(60.);
        start_beat();
        let mut cpu = thread_cpu_ms();
        cx.background_executor().timer(Duration::from_secs(1)).await;
        frames(hz);
        main_load(&mut cpu);
        cx.background_executor().timer(Duration::from_secs(5)).await;
        eprintln!("CEF GPUI alone, 5 s at {hz:.0} Hz: {} · {}", frames(hz), main_load(&mut cpu));
        let root = std::path::PathBuf::from(&data).join("browser").join("chrome");
        let t = Instant::now();
        let started = start(std::path::Path::new(&framework), &root);
        eprintln!("CEF start {:?} in {} ms", started, t.elapsed().as_millis());
        if started.is_err() {
            std::process::exit(1);
        }
        let t = Instant::now();
        while !ready() && t.elapsed() < Duration::from_secs(20) {
            cx.background_executor().timer(Duration::from_millis(5)).await;
        }
        eprintln!("CEF context ready {} after {} ms more; pump {:?}", ready(), t.elapsed().as_millis(), pump_stats(true));
        let Ok((host, ns_window, size)) = window.update(cx, |_, window, _| (crate::platform::ns_view(window) as usize, crate::platform::ns_window(window) as usize, window.viewport_size())) else { return };
        let (host, ns_window) = (host as id, ns_window as id);
        let _ = ns_window;
        let (w, h) = (f64::from(size.width), f64::from(size.height));
        let panel = (w - 420., 44., 420., h - 44.);

        let t = Instant::now();
        let Some(a) = Tab::new(host, "about:blank", ready_profile(Some(&root.join("project-a")), cx).await) else {
            eprintln!("CEF no browser");
            std::process::exit(1);
        };
        eprintln!("CEF browser made in {} ms, view {}", t.elapsed().as_millis(), unsafe { rust_string(msg_send![a.view(), className]).unwrap_or_default() });
        a.set_frame(panel.0, panel.1, panel.2, panel.3);
        let loaded = a.loaded();
        a.load(&url);
        eprintln!("CEF load status {:?} in {} ms", within(loaded, 20, cx).await, t.elapsed().as_millis());
        eprintln!("CEF title {}", eval(&a, "document.title", None, cx).await);
        eprintln!("CEF cookie from the last run {}", eval(&a, "document.cookie", None, cx).await);

        // Isolated world: sees the DOM, not the page's script.
        let frame = within(a.cdp("Page.getFrameTree", serde_json::json!({})), 10, cx).await;
        let frame_id = frame.and_then(|f| f.ok()).map(|f| f["frameTree"]["frame"]["id"].as_str().unwrap_or_default().to_string()).unwrap_or_default();
        let world = within(a.cdp("Page.createIsolatedWorld", serde_json::json!({"frameId": frame_id, "worldName": "yh"})), 10, cx).await;
        let context = world.and_then(|w| w.ok()).and_then(|w| w["executionContextId"].as_i64());
        eprintln!(
            "CEF isolated world {:?}: page's script {} · DOM {} · page sees ours {}",
            context,
            eval(&a, "typeof window.clicks", context, cx).await,
            eval(&a, "(window.__yh = 1, document.querySelectorAll('button').length)", context, cx).await,
            eval(&a, "typeof window.__yh", None, cx).await
        );

        // Input over DevTools: trusted, without the keyboard focus.
        for kind in ["mousePressed", "mouseReleased"] {
            let _ = within(a.cdp("Input.dispatchMouseEvent", serde_json::json!({"type": kind, "x": 100, "y": 60, "button": "left", "clickCount": 1})), 5, cx).await;
        }
        for kind in ["mousePressed", "mouseReleased"] {
            let _ = within(a.cdp("Input.dispatchMouseEvent", serde_json::json!({"type": kind, "x": 140, "y": 135, "button": "left", "clickCount": 1})), 5, cx).await;
        }
        let _ = within(a.cdp("Input.insertText", serde_json::json!({"text": "hello"})), 5, cx).await;
        for kind in ["keyDown", "keyUp"] {
            let _ = within(a.cdp("Input.dispatchKeyEvent", serde_json::json!({"type": kind, "key": "Enter", "code": "Enter", "windowsVirtualKeyCode": 13})), 5, cx).await;
        }
        eprintln!(
            "CEF agent input: clicks {} · field {} · keys {} · first responder {}",
            eval(&a, "JSON.stringify(clicks)", None, cx).await,
            eval(&a, "document.getElementById('i').value", None, cx).await,
            eval(&a, "JSON.stringify(keys)", None, cx).await,
            first_responder(ns_window)
        );

        // A person's click through the window, typing off: the page gets it, the keyboard stays.
        let (bx, by) = (panel.0 + 100., panel.1 + 60.);
        let _ = cx.update(|_| crate::platform::click(ns_window, bx as f32, by as f32));
        cx.background_executor().timer(Duration::from_millis(500)).await;
        eprintln!("CEF your click, typing off: clicks {} · first responder {}", eval(&a, "JSON.stringify(clicks)", None, cx).await, first_responder(ns_window));
        set_allow_focus(true);
        let _ = cx.update(|_| crate::platform::click(ns_window, (panel.0 + 140.) as f32, (panel.1 + 135.) as f32));
        cx.background_executor().timer(Duration::from_millis(500)).await;
        eprintln!("CEF your click, typing on: active {} · first responder {}", eval(&a, "document.activeElement.id", None, cx).await, first_responder(ns_window));
        set_allow_focus(false);

        let loaded_at = eval(&a, "loadedAt", None, cx).await;
        eprintln!("CEF shown: {}", speed(&a, cx).await);
        eprintln!("CEF shown screenshot {}", screenshot(&a, &format!("{out}/shown.png"), cx).await);

        // Parked: off screen in a panel of its own.
        let parking = crate::webview::parking_view();
        a.move_to(parking);
        a.set_frame(0., 0., 1280., 800.);
        cx.background_executor().timer(Duration::from_millis(300)).await;
        eprintln!("CEF parked: {} · same document {}", speed(&a, cx).await, eval(&a, "loadedAt", None, cx).await == loaded_at);
        eprintln!("CEF parked screenshot {}", screenshot(&a, &format!("{out}/parked.png"), cx).await);
        eprintln!("CEF parked viewport {}", eval(&a, "[innerWidth, innerHeight, devicePixelRatio]", None, cx).await);
        a.move_to(host);
        a.set_frame(panel.0, panel.1, panel.2, panel.3);
        cx.background_executor().timer(Duration::from_millis(300)).await;
        eprintln!("CEF back in the window: same document {} · viewport {}", eval(&a, "loadedAt", None, cx).await == loaded_at, eval(&a, "[innerWidth, innerHeight]", None, cx).await);

        // A second project: its own cookies.
        let b = Tab::new(host, "about:blank", ready_profile(Some(&root.join("project-b")), cx).await);
        let Some(b) = b else { return };
        b.set_frame(panel.0 - 430., panel.1, 420., 300.);
        let loaded = b.loaded();
        b.load(&url);
        let _ = within(loaded, 20, cx).await;
        let _ = eval(&a, "document.cookie = 'who=A; max-age=999999'", None, cx).await;
        eprintln!("CEF cookies: A {} · B {}", eval(&a, "document.cookie", None, cx).await, eval(&b, "document.cookie", None, cx).await);
        a.flush_cookies();
        let p = ready_profile(None, cx).await;
        let private = p.and_then(|p| Tab::new(host, "about:blank", Some(p)));
        eprintln!("CEF private tab made {}", private.is_some());

        eprintln!("CEF memory with 2 tabs: {}", memory());
        eprintln!("CEF pump so far: {:?} · {}", pump_stats(true), frames(hz));
        if let Ok(heavy) = std::env::var("YH_CEF_HEAVY") {
            let t = Instant::now();
            let loaded = a.loaded();
            a.load(&heavy);
            eprintln!("CEF heavy load {:?} in {} ms", within(loaded, 30, cx).await, t.elapsed().as_millis());
            cx.background_executor().timer(Duration::from_secs(2)).await;
            eprintln!("CEF pump during heavy load: {:?} · {}", pump_stats(true), frames(hz));
            eprintln!("CEF memory after heavy page: {}", memory());
            eprintln!("CEF heavy screenshot {}", screenshot(&a, &format!("{out}/heavy.png"), cx).await);
        }
        for (label, url_b) in [("static pages", "about:blank"), ("one page animating (rAF)", url.as_str())] {
            let (la, lb) = (a.loaded(), b.loaded());
            a.load("about:blank");
            b.load(url_b);
            let _ = within(la, 10, cx).await;
            let _ = within(lb, 10, cx).await;
            cx.background_executor().timer(Duration::from_secs(1)).await;
            pump_stats(true);
            frames(hz);
            main_load(&mut cpu);
            cx.background_executor().timer(Duration::from_secs(5)).await;
            eprintln!("CEF pump 5 s, {label}: {:?} · {} · {}", pump_stats(true), frames(hz), main_load(&mut cpu));
            if label == "static pages" {
                eprintln!("CEF trace, static pages: {}", trace(&a, 4, cx).await);
                eprintln!("CEF pump while tracing: {:?}", pump_stats(true));
            }
        }
        if std::env::var_os("YH_CEF_KEEP_OPEN").is_none() {
            b.close();
            cx.background_executor().timer(Duration::from_secs(2)).await;
            eprintln!("CEF closed B: A still answers {}", eval(&a, "1+1", None, cx).await);
            a.close();
            cx.background_executor().timer(Duration::from_secs(2)).await;
            eprintln!("CEF closed A too");
        }
        cx.background_executor().timer(Duration::from_secs(1)).await;
        eprintln!("CEF done");
        std::process::exit(0);
    })
    .detach();
}
