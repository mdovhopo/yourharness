//! The Chrome engine: Chromium embedded through CEF, its pages windowed views laid over the GPUI
//! window like WebKit's (in a `Page`), driven over Chromium's DevTools protocol in process.
//!
//! - The framework isn't in the app: it's downloaded on first use (`install`), a pinned,
//!   hash-checked zip, into the data folder. The helper apps CEF starts are in the app
//!   (`Contents/Frameworks`, `yh-cef-helper`) and load it from there.
//! - CEF starts the first time a Chrome page is made (`start`): its app protocol added to GPUI's
//!   `GPUIApplication`, and Chromium's own event loop entered from a run-loop timer, nested in
//!   GPUI's. (Its external pump busy-waits up to 10 ms at a time on the main thread.)
//! - Chromium's sandbox lets helpers read only the app's own bundle: the policies it compiles in
//!   the app get one read-only rule for the framework's folder (`widen_sandbox`); those it sends
//!   as source, the helper widens.
//! - A page is a `Page` around a holder view, which Chromium's view is made in once CEF and the
//!   project's profile are ready; until then, what's asked of it waits (`with_browser`).
//! - Each project's cookies and storage are a profile of their own (a `RequestContext` with its
//!   folder under `<data>/Chrome`); a project's WebKit sign-ins are copied in the first time.

#![allow(unexpected_cfgs, deprecated)]

use super::page::Page;
use super::{app_view, post, rust_string, watch_focus, WebEvent, TYPING};
use cef::*;
use cocoa::base::{id, nil, YES};
use cocoa::foundation::{NSPoint, NSRect, NSSize};
use objc::runtime::{Object, Sel};
use objc::{class, msg_send, sel, sel_impl};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{c_void, CString};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// The CEF this app is built against (the `cef` crate's), and the framework zip it downloads.
pub const CEF_VERSION: &str = "156.0.28";
const FRAMEWORK: &str = "Chromium Embedded Framework.framework";
const ZIP_URL: &str = "https://github.com/mdovhopo/yourharness/releases/download/cef-156.0.28/Chromium-Embedded-Framework-156.0.28-macos-arm64.zip";
const ZIP_SHA256: &str = "9ca84bbf342e58b8ec4227e4c449663170b0c350950168517b8c866ee82a7511";
const ZIP_BYTES: u64 = 146_390_881;

// ---- The framework: where it is, its download ----

/// Where the framework is, or will be: `YH_CEF_FRAMEWORK` (a folder holding the `.framework`,
/// for test runs), else `<data>/Browser Engine/<version>`.
pub fn framework_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("YH_CEF_FRAMEWORK") {
        return PathBuf::from(dir);
    }
    yh_core::library::data_directory().join("Browser Engine").join(CEF_VERSION)
}

fn installed() -> bool {
    framework_dir().join(FRAMEWORK).join("Chromium Embedded Framework").exists()
}

/// Where the Chrome engine stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Install {
    /// Not downloaded, nothing asked for it yet.
    Missing,
    /// Being downloaded: bytes so far, of how many.
    Downloading(u64, u64),
    /// Downloaded, CEF starting or started.
    Ready,
    Failed(String),
}

static INSTALL: Mutex<Option<Install>> = Mutex::new(None);

pub fn install_state() -> Install {
    let state = INSTALL.lock().unwrap().clone();
    state.unwrap_or_else(|| if installed() { Install::Ready } else { Install::Missing })
}

fn set_install(state: Install) {
    *INSTALL.lock().unwrap() = Some(state);
}

/// After a failed install: downloads again, and pages waiting for it are made once it's in.
pub fn retry() {
    if matches!(install_state(), Install::Failed(_)) {
        *INSTALL.lock().unwrap() = None;
        STARTED.store(false, Ordering::Release);
        install();
    }
}

/// Downloads the framework if it's not here (once at a time), then starts CEF on the main thread.
fn install() {
    if installed() {
        set_install(Install::Ready);
        main_queue(start_on_main);
        return;
    }
    if matches!(install_state(), Install::Downloading(..)) {
        return;
    }
    set_install(Install::Downloading(0, ZIP_BYTES));
    std::thread::spawn(|| {
        let result = download();
        match result {
            Ok(()) => {
                set_install(Install::Ready);
                main_queue(start_on_main);
            }
            Err(e) => {
                eprintln!("CHROME couldn't install the browser engine: {e}");
                // curl's own words for "no network" read as what they mean.
                let e = if ["Failed to connect", "resolve host", "offline", "timed out", "Connection reset"].iter().any(|m| e.contains(m)) {
                    "it couldn't be downloaded: you're offline, or GitHub can't be reached".to_string()
                } else {
                    format!("it couldn't be downloaded: {e}")
                };
                set_install(Install::Failed(e));
                main_queue(|| fail_pending());
            }
        }
    });
}

fn download() -> Result<(), String> {
    let dir = framework_dir();
    let parent = dir.parent().ok_or("no folder")?.to_path_buf();
    std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let scratch = parent.join(format!(".download-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    let zip = scratch.join("framework.zip");
    let stop = AtomicBool::new(false);
    let watch = {
        let zip = zip.clone();
        std::thread::spawn(move || {
            while matches!(install_state(), Install::Downloading(..)) {
                let got = std::fs::metadata(&zip).map(|m| m.len()).unwrap_or(0);
                set_install(Install::Downloading(got, ZIP_BYTES));
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        })
    };
    let url = std::env::var("YH_CEF_DOWNLOAD_URL").unwrap_or_else(|_| ZIP_URL.to_string());
    let fetched = yh_core::updates::download_cancellable(&url, &zip, &stop);
    let result = fetched.and_then(|()| {
        let sum = std::process::Command::new("/usr/bin/shasum").args(["-a", "256"]).arg(&zip).output().map_err(|e| e.to_string())?;
        let sum = String::from_utf8_lossy(&sum.stdout).split_whitespace().next().unwrap_or_default().to_string();
        let want = std::env::var("YH_CEF_DOWNLOAD_SHA256").unwrap_or_else(|_| ZIP_SHA256.to_string());
        if sum != want {
            return Err(format!("the download's SHA-256 is {sum}, not {want}"));
        }
        let unpacked = scratch.join("unpacked");
        let ok = std::process::Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(&zip).arg(&unpacked).status().map_err(|e| e.to_string())?;
        if !ok.success() || !unpacked.join(FRAMEWORK).exists() {
            return Err("the download didn't unpack".into());
        }
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::rename(&unpacked, &dir).map_err(|e| e.to_string())?;
        // Older versions go: only this one is used.
        if let Ok(entries) = std::fs::read_dir(&parent) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name != CEF_VERSION && !name.starts_with('.') {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        Ok(())
    });
    let _ = std::fs::remove_dir_all(&scratch);
    set_install(if result.is_ok() { Install::Ready } else { Install::Failed(String::new()) });
    let _ = watch.join();
    result
}

/// Runs `f` on the main thread, from the main queue.
fn main_queue(f: fn()) {
    crate::platform::run_on_main(Box::new(f));
}

// ---- NSApp: the protocol CEF asks of it ----

static HANDLING_SEND_EVENT: AtomicBool = AtomicBool::new(false);

unsafe extern "C" {
    fn object_getClass(obj: id) -> *mut c_void;
    fn objc_getClass(name: *const std::ffi::c_char) -> *mut c_void;
    fn class_addMethod(cls: *mut c_void, name: Sel, imp: *const c_void, types: *const std::ffi::c_char) -> bool;
    fn class_replaceMethod(cls: *mut c_void, name: Sel, imp: *const c_void, types: *const std::ffi::c_char) -> *const c_void;
    fn objc_getProtocol(name: *const std::ffi::c_char) -> *mut c_void;
    fn class_addProtocol(cls: *mut c_void, protocol: *mut c_void) -> bool;
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
/// a `sendEvent:` that marks events being handled. After the framework is loaded, which declares
/// the protocols.
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

extern "C" fn accepts_first_responder(this: &Object, _: Sel) -> objc::runtime::BOOL {
    let holder = holder_of(this as *const Object as id);
    holder != nil && TYPING.with(|t| t.borrow().contains(&(holder as usize)))
}

/// Chromium's page view takes the keyboard when it's clicked, or when a page focuses a field: it
/// accepts it only while you may type into that page, so the composer keeps it otherwise while
/// your clicks still reach the page.
unsafe fn guard_first_responder() {
    unsafe {
        let cls = objc_getClass(c"RenderWidgetHostViewCocoa".as_ptr());
        if !cls.is_null() {
            class_replaceMethod(cls, sel!(acceptsFirstResponder), accepts_first_responder as *const c_void, c"c@:".as_ptr());
        }
    }
}

// ---- The sandbox: the downloaded framework readable by the helpers ----
//
// Policies for renderers and utilities are compiled here, in the app, by the framework calling
// libsandbox's `sandbox_compile_string`; that import is pointed at `compile_widened`, which adds
// one read-only rule for the framework's folder and calls the real one. If that can't be done,
// the helpers' sandbox can't read the framework and they stop: the browser fails, never
// unsandboxed.

type CompileFn = unsafe extern "C" fn(*const std::ffi::c_char, *mut c_void, *mut *mut std::ffi::c_char) -> *mut c_void;
static REAL_COMPILE: AtomicUsize = AtomicUsize::new(0);
static FRAMEWORK_RULE: OnceLock<CString> = OnceLock::new();

unsafe extern "C" fn compile_widened(profile: *const std::ffi::c_char, params: *mut c_void, error: *mut *mut std::ffi::c_char) -> *mut c_void {
    let real: CompileFn = unsafe { std::mem::transmute(REAL_COMPILE.load(Ordering::Acquire)) };
    let (Some(rule), false) = (FRAMEWORK_RULE.get(), profile.is_null()) else { return unsafe { real(profile, params, error) } };
    let mut text = unsafe { std::ffi::CStr::from_ptr(profile) }.to_bytes().to_vec();
    text.extend_from_slice(rule.as_bytes());
    let widened = CString::new(text).unwrap_or_default();
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
                // LC_SEGMENT_64: __LINKEDIT's place; the symbol-pointer sections of the data segments.
                0x19 => {
                    let segname = std::ffi::CStr::from_ptr(cmd.add(8).cast()).to_bytes().to_vec();
                    if segname == b"__LINKEDIT" {
                        linkedit = Some((read64(cmd.add(24)), read64(cmd.add(40))));
                    } else if segname.starts_with(b"__DATA") || segname.starts_with(b"__AUTH") {
                        let nsects = read32(cmd.add(64));
                        let mut sect = cmd.add(72);
                        for _ in 0..nsects {
                            let flags = read32(sect.add(64));
                            // S_NON_LAZY_SYMBOL_POINTERS, S_LAZY_SYMBOL_POINTERS.
                            if matches!(flags & 0xff, 0x6 | 0x7) {
                                sections.push((read64(sect.add(32)), read64(sect.add(40)), read32(sect.add(68))));
                            }
                            sect = sect.add(80);
                        }
                    }
                }
                // LC_SYMTAB, LC_DYSYMTAB.
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
                // INDIRECT_SYMBOL_LOCAL, INDIRECT_SYMBOL_ABS.
                if sym & 0xC000_0000 != 0 {
                    continue;
                }
                let strx = read32(syms.add(sym as usize * 16));
                if std::ffi::CStr::from_ptr(strs.add(strx as usize).cast()).to_bytes() != symbol.as_bytes() {
                    continue;
                }
                let slot = slots.add(i);
                let page = (slot as usize) & !16383;
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

unsafe fn widen_sandbox(framework: &Path) -> bool {
    let dir = std::fs::canonicalize(framework).unwrap_or_else(|_| framework.to_path_buf());
    let quoted = dir.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    let _ = FRAMEWORK_RULE.set(CString::new(format!("\n; YourHarness: the downloaded framework, read-only.\n(allow file-read* (subpath \"{quoted}\"))\n")).unwrap_or_default());
    match unsafe { rebind("/Chromium Embedded Framework", "_sandbox_compile_string", compile_widened as *const () as usize) } {
        Some(real) => {
            REAL_COMPILE.store(real, Ordering::Release);
            true
        }
        None => false,
    }
}

// ---- Starting CEF ----

static STARTED: AtomicBool = AtomicBool::new(false);
static READY: AtomicBool = AtomicBool::new(false);

wrap_browser_process_handler! {
    struct ProcessHandler;

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            READY.store(true, Ordering::Release);
            if log() {
                eprintln!("CHROME ready");
            }
            make_pending();
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
            // As Safari's pages could: media plays without a click first.
            line.append_switch_with_value(Some(&CefString::from("autoplay-policy")), Some(&CefString::from("no-user-gesture-required")));
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(ProcessHandler::new())
        }
    }
}

fn log() -> bool {
    std::env::var_os("YH_CHROME_LOG").is_some()
}

/// The folder every profile is in.
fn root_cache() -> PathBuf {
    yh_core::library::data_directory().join("Chrome")
}

struct SendSettings(Settings);
// Only ever touched on the main thread.
unsafe impl Send for SendSettings {}
static SETTINGS: Mutex<Option<SendSettings>> = Mutex::new(None);

unsafe extern "C" {
    fn CFRunLoopGetMain() -> *mut c_void;
    fn CFAbsoluteTimeGetCurrent() -> f64;
    fn CFRunLoopTimerCreate(allocator: *const c_void, fire: f64, interval: f64, flags: u64, order: i64, callout: extern "C" fn(*mut c_void, *mut c_void), context: *mut c_void) -> *mut c_void;
    fn CFRunLoopAddTimer(rl: *mut c_void, timer: *mut c_void, mode: *const c_void);
    static kCFRunLoopDefaultMode: *const c_void;
}

/// CEF started inside Chromium's own event loop, which it then runs: it sees NSApp running and
/// runs a loop of its own that hands every event to NSApp as GPUI's does. Entered from a run-loop
/// timer, not the main queue, so GPUI's main-queue work still runs inside it; its run-loop
/// observers must only ever see it running (they crash otherwise), so CEF starts here too.
extern "C" fn enter_run_loop(_: *mut c_void, _: *mut c_void) {
    let Some(SendSettings(settings)) = SETTINGS.lock().unwrap().take() else { return };
    let mut app = HarnessApp::new();
    let args = cef::args::Args::new();
    let started = std::time::Instant::now();
    if initialize(Some(args.as_main_args()), Some(&settings), Some(&mut app), std::ptr::null_mut()) != 1 {
        eprintln!("CHROME CEF didn't start");
        // Chromium runs one process per profile folder: another copy of the app on the same data.
        set_install(Install::Failed("it didn't start (is another copy of YourHarness open on the same data?)".into()));
        fail_pending();
        return;
    }
    if log() {
        eprintln!("CHROME started in {} ms", started.elapsed().as_millis());
    }
    run_message_loop();
}

fn start_on_main() {
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let framework = framework_dir().join(FRAMEWORK);
    unsafe {
        let binary = CString::new(framework.join("Chromium Embedded Framework").to_string_lossy().as_bytes()).unwrap_or_default();
        if load_library(Some(&*binary.as_ptr())) != 1 {
            eprintln!("CHROME couldn't load {}", framework.display());
            set_install(Install::Failed("its files couldn't be loaded".into()));
            fail_pending();
            return;
        }
        let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
        install_app_protocol();
        guard_first_responder();
        if !widen_sandbox(&framework) {
            eprintln!("CHROME couldn't widen the sandbox to the framework");
        }
        let bundle: id = msg_send![class!(NSBundle), mainBundle];
        let bundle_path = rust_string(msg_send![bundle, bundlePath]).unwrap_or_default();
        let root = root_cache();
        let _ = std::fs::create_dir_all(&root);
        let settings = Settings {
            no_sandbox: i32::from(std::env::var_os("YH_CHROME_NO_SANDBOX").is_some()),
            framework_dir_path: CefString::from(framework.to_string_lossy().as_ref()),
            main_bundle_path: CefString::from(bundle_path.as_str()),
            external_message_pump: 0,
            root_cache_path: CefString::from(root.to_string_lossy().as_ref()),
            cache_path: CefString::from(root.join("Default").to_string_lossy().as_ref()),
            persist_session_cookies: 1,
            log_severity: LogSeverity::ERROR,
            background_color: 0xFFFF_FFFF,
            ..Default::default()
        };
        *SETTINGS.lock().unwrap() = Some(SendSettings(settings));
        let timer = CFRunLoopTimerCreate(std::ptr::null(), CFAbsoluteTimeGetCurrent(), 0., 0, 0, enter_run_loop, std::ptr::null_mut());
        CFRunLoopAddTimer(CFRunLoopGetMain(), timer, kCFRunLoopDefaultMode);
    }
}

// ---- Profiles: one per project ----

struct Profile {
    context: RequestContext,
    ready: bool,
    /// First made now (no folder before): the project's WebKit sign-ins are copied in.
    fresh: bool,
    /// Waiting for it to be ready.
    waiting: Vec<Box<dyn FnOnce(&RequestContext)>>,
}

thread_local! {
    static PROFILES: RefCell<HashMap<String, Profile>> = RefCell::new(HashMap::new());
}

wrap_request_context_handler! {
    struct ContextReady {
        key: String,
    }

    impl RequestContextHandler {
        fn on_request_context_initialized(&self, _request_context: Option<&mut RequestContext>) {
            let key = self.key.clone();
            let fresh = PROFILES.with(|p| p.borrow().get(&key).is_some_and(|p| p.fresh));
            // A project's new profile is ready once its WebKit sign-ins are in: its pages' first
            // requests go signed in.
            if fresh && is_uuid(&key) {
                migrate_webkit_cookies(&key);
            } else {
                release_profile(&key);
            }
        }
    }
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Runs `f` with the profile `key` names once it's ready ("" the app's own, `private-…` in
/// memory, else a project's ID): now if it is. Main thread, CEF ready.
fn with_profile(key: &str, f: impl FnOnce(&RequestContext) + 'static) {
    let state = PROFILES.with(|p| p.borrow().get(key).map(|p| p.ready.then(|| p.context.clone())));
    match state {
        Some(Some(context)) => f(&context),
        Some(None) => PROFILES.with(|p| {
            if let Some(profile) = p.borrow_mut().get_mut(key) {
                profile.waiting.push(Box::new(f));
            }
        }),
        None => {
            let private = key.starts_with("private-");
            let folder = (!private).then(|| root_cache().join(if key.is_empty() { "Default".to_string() } else { key.to_string() }));
            let fresh = folder.as_ref().is_some_and(|f| !f.exists());
            let settings = RequestContextSettings {
                cache_path: CefString::from(folder.map(|f| f.to_string_lossy().into_owned()).unwrap_or_default().as_str()),
                persist_session_cookies: 1,
                ..Default::default()
            };
            let mut handler = ContextReady::new(key.to_string());
            let Some(context) = request_context_create_context(Some(&settings), Some(&mut handler)) else { return };
            PROFILES.with(|p| p.borrow_mut().insert(key.to_string(), Profile { context, ready: false, fresh, waiting: vec![Box::new(f)] }));
        }
    }
}

fn profile_now(key: &str, f: impl FnOnce(&RequestContext) + 'static) {
    with_profile(key, f);
}

// ---- Cookies ----

/// Chromium's times are microseconds since 1601.
const WINDOWS_TO_UNIX_SECS: f64 = 11_644_473_600.;

fn to_unix(t: &Basetime) -> f64 {
    t.val as f64 / 1e6 - WINDOWS_TO_UNIX_SECS
}

fn from_unix(secs: f64) -> Basetime {
    Basetime { val: ((secs + WINDOWS_TO_UNIX_SECS) * 1e6) as i64 }
}

/// Sends what it collected when the last reference to it goes (CEF lets a visitor go when it's
/// done, whether or not it visited anything).
struct Collected {
    cookies: Vec<yh_core::cookies::Cookie>,
    reply: Option<flume::Sender<Vec<yh_core::cookies::Cookie>>>,
}

impl Drop for Collected {
    fn drop(&mut self) {
        if let Some(r) = self.reply.take() {
            let _ = r.send(std::mem::take(&mut self.cookies));
        }
    }
}

wrap_cookie_visitor! {
    struct Visitor {
        out: std::sync::Arc<Mutex<Collected>>,
    }

    impl CookieVisitor {
        fn visit(&self, cookie: Option<&Cookie>, _count: i32, _total: i32, _delete_cookie: Option<&mut i32>) -> i32 {
            if let Some(c) = cookie {
                let same_site = match c.same_site {
                    s if s == CookieSameSite::LAX_MODE => Some("lax".to_string()),
                    s if s == CookieSameSite::STRICT_MODE => Some("strict".to_string()),
                    _ => None,
                };
                self.out.lock().unwrap().cookies.push(yh_core::cookies::Cookie {
                    domain: c.domain.to_string(),
                    name: c.name.to_string(),
                    value: c.value.to_string(),
                    path: c.path.to_string(),
                    expires: (c.has_expires == 1).then(|| to_unix(&c.expires)),
                    secure: c.secure == 1,
                    http_only: c.httponly == 1,
                    same_site,
                });
            }
            1
        }
    }
}

struct Counted {
    total: usize,
    reply: Option<flume::Sender<usize>>,
}

impl Drop for Counted {
    fn drop(&mut self) {
        if let Some(r) = self.reply.take() {
            let _ = r.send(self.total);
        }
    }
}

wrap_set_cookie_callback! {
    struct CookieSet {
        done: std::sync::Arc<Mutex<Counted>>,
    }

    impl SetCookieCallback {
        fn on_complete(&self, _success: i32) {}
    }
}

fn put_cookies(context: &RequestContext, cookies: Vec<yh_core::cookies::Cookie>, reply: flume::Sender<usize>) {
    let total = cookies.len();
    let done = std::sync::Arc::new(Mutex::new(Counted { total, reply: Some(reply) }));
    let Some(manager) = context.cookie_manager(None) else { return };
    for c in cookies {
        let host = c.domain.trim_start_matches('.');
        let url = format!("{}://{host}{}", if c.secure { "https" } else { "http" }, if c.path.is_empty() { "/" } else { &c.path });
        let cookie = Cookie {
            name: CefString::from(c.name.as_str()),
            value: CefString::from(c.value.as_str()),
            domain: CefString::from(c.domain.as_str()),
            path: CefString::from(if c.path.is_empty() { "/" } else { c.path.as_str() }),
            secure: i32::from(c.secure),
            httponly: i32::from(c.http_only),
            has_expires: i32::from(c.expires.is_some()),
            expires: c.expires.map(from_unix).unwrap_or(Basetime { val: 0 }),
            same_site: match c.same_site.as_deref() {
                Some("lax") => CookieSameSite::LAX_MODE,
                Some("strict") => CookieSameSite::STRICT_MODE,
                _ => CookieSameSite::UNSPECIFIED,
            },
            ..Default::default()
        };
        let mut callback = CookieSet::new(done.clone());
        manager.set_cookie(Some(&CefString::from(url.as_str())), Some(&cookie), Some(&mut callback));
    }
    manager.flush_store(None);
}

/// The profile `key` is ready: what waited for it runs.
fn release_profile(key: &str) {
    let ready = PROFILES.with(|p| {
        let mut p = p.borrow_mut();
        let profile = p.get_mut(key)?;
        profile.ready = true;
        Some((profile.context.clone(), std::mem::take(&mut profile.waiting)))
    });
    let Some((context, waiting)) = ready else { return };
    for w in waiting {
        w(&context);
    }
}

/// Profiles to release from the main queue (`release_profile`), by key.
static RELEASE: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn release_later(key: String) {
    RELEASE.lock().unwrap().push(key);
    main_queue(|| {
        for key in std::mem::take(&mut *RELEASE.lock().unwrap()) {
            release_profile(&key);
        }
    });
}

/// A project's WebKit sign-ins, into its new Chrome profile, which is then ready. WebKit answers
/// on the main thread from its run loop; they're waited for off it (10 s at most), then put in
/// from the main queue.
fn migrate_webkit_cookies(project: &str) {
    let (tx, rx) = flume::bounded(1);
    super::webkit::read_cookies(project, tx);
    let project = project.to_string();
    std::thread::spawn(move || {
        let cookies = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap_or_default();
        if cookies.is_empty() {
            release_later(project);
            return;
        }
        if log() {
            eprintln!("CHROME copying {} WebKit cookies into {project}", cookies.len());
        }
        MIGRATIONS.lock().unwrap().push((project, cookies));
        main_queue(run_migrations);
    });
}

static MIGRATIONS: Mutex<Vec<(String, Vec<yh_core::cookies::Cookie>)>> = Mutex::new(Vec::new());

fn run_migrations() {
    for (project, cookies) in std::mem::take(&mut *MIGRATIONS.lock().unwrap()) {
        let context = PROFILES.with(|p| p.borrow().get(&project).map(|p| p.context.clone()));
        let Some(context) = context else { continue };
        let (tx, rx) = flume::bounded(1);
        put_cookies(&context, cookies, tx);
        std::thread::spawn(move || {
            let _ = rx.recv_timeout(std::time::Duration::from_secs(5));
            release_later(project);
        });
    }
}

wrap_completion_callback! {
    struct Flushed {
        left: std::sync::Arc<AtomicUsize>,
    }

    impl CompletionCallback {
        fn on_complete(&self) {
            self.left.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

unsafe extern "C" {
    fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source_handled: u8) -> i32;
}

/// As the app quits: every profile's cookies written out (Chromium writes them in batches, every
/// 30 s), waited for up to a second with the run loop turning so its work goes on. Never inside
/// one of the app's own updates.
pub fn flush_for_quit() {
    if !READY.load(Ordering::Acquire) {
        return;
    }
    let contexts: Vec<RequestContext> = PROFILES.try_with(|p| p.borrow().values().filter(|p| p.ready).map(|p| p.context.clone()).collect()).unwrap_or_default();
    let left = std::sync::Arc::new(AtomicUsize::new(contexts.len()));
    for c in &contexts {
        match c.cookie_manager(None) {
            Some(m) if m.flush_store(Some(&mut Flushed::new(left.clone()))) == 1 => {}
            _ => {
                left.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }
    let started = std::time::Instant::now();
    while left.load(Ordering::Acquire) > 0 && started.elapsed() < std::time::Duration::from_secs(1) {
        unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.02, 1) };
    }
    if log() {
        eprintln!("CHROME cookies flushed for quit: {} of {} profiles in {} ms", contexts.len() - left.load(Ordering::Acquire), contexts.len(), started.elapsed().as_millis());
    }
}

/// Every cookie in a project's Chrome profile; nothing while the engine isn't installed.
pub fn read_cookies(project: &str, reply: flume::Sender<Vec<yh_core::cookies::Cookie>>) {
    if !installed() {
        let _ = reply.send(vec![]);
        return;
    }
    ensure_started();
    let key = project_key(Some(project));
    on_ready(Box::new(move || {
        profile_now(&key, move |context| {
            let out = std::sync::Arc::new(Mutex::new(Collected { cookies: vec![], reply: Some(reply) }));
            if let Some(m) = context.cookie_manager(None) {
                m.visit_all_cookies(Some(&mut Visitor::new(out)));
            }
        });
    }));
}

/// Puts cookies in a project's Chrome profile; replies with how many went in.
pub fn store_cookies(project: &str, cookies: Vec<yh_core::cookies::Cookie>, reply: flume::Sender<usize>) {
    if !installed() {
        let _ = reply.send(0);
        return;
    }
    ensure_started();
    let key = project_key(Some(project));
    on_ready(Box::new(move || {
        profile_now(&key, move |context| put_cookies(context, cookies, reply));
    }));
}

/// Clears a project's Chrome profile's cookies and what its open pages keep.
pub fn clear_store(project: &str) {
    if !installed() || !STARTED.load(Ordering::Acquire) {
        return;
    }
    let key = project_key(Some(project));
    on_ready(Box::new(move || {
        let k = key.clone();
        profile_now(&key, move |context| {
            if let Some(m) = context.cookie_manager(None) {
                m.delete_cookies(None, None, None);
                m.flush_store(None);
            }
            // What open pages of this profile keep: their caches, their sites' storage.
            TABS.with(|t| {
                for inner in t.borrow().values() {
                    let inner = inner.borrow();
                    if inner.profile != k {
                        continue;
                    }
                    if let Some(b) = &inner.browser {
                        fire(b, "Network.clearBrowserCache", json!({}));
                        if let Some(origin) = inner.url.as_deref().and_then(origin_of) {
                            fire(b, "Storage.clearDataForOrigin", json!({"origin": origin, "storageTypes": "all"}));
                        }
                    }
                }
            });
        });
    }));
}

fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split('/').next()?;
    (!host.is_empty()).then(|| format!("{scheme}://{host}"))
}

fn project_key(store: Option<&str>) -> String {
    store.unwrap_or("").to_string()
}

thread_local! {
    /// What waits for CEF to be ready.
    static ON_READY: RefCell<Vec<Box<dyn FnOnce()>>> = RefCell::new(Vec::new());
}

fn on_ready(f: Box<dyn FnOnce()>) {
    if READY.load(Ordering::Acquire) {
        f();
    } else {
        ON_READY.with(|r| r.borrow_mut().push(f));
    }
}

fn ensure_started() {
    if STARTED.load(Ordering::Acquire) {
        return;
    }
    install();
}

// ---- Pages ----

/// A page's state, and what waits for its browser.
struct Inner {
    key: usize,
    profile: String,
    browser: Option<Browser>,
    queue: Vec<Box<dyn FnOnce(&Browser)>>,
    url: Option<String>,
    title: Option<String>,
    loading: bool,
    progress: f64,
    /// A dialog the page waits on: how to answer it, and a prompt's default text.
    dialog: Option<(JsdialogCallback, Option<String>)>,
    upload: Option<Vec<String>>,
    /// Closed by the app (`Drop`): CEF's close goes ahead. Otherwise the page asked.
    closing: bool,
    closed: bool,
    /// The device emulation last asked for (`emulate`): None for none, else viewport and scale.
    emulated: Option<Option<(f64, f64, f64)>>,
    _observer: Option<Registration>,
}

thread_local! {
    /// Every page, by key.
    static TABS: RefCell<HashMap<usize, Rc<RefCell<Inner>>>> = RefCell::new(HashMap::new());
    /// Pages by their browser's ID.
    static BY_BROWSER: RefCell<HashMap<i32, usize>> = RefCell::new(HashMap::new());
    /// Pages waiting for CEF, by key.
    static PENDING: RefCell<Vec<usize>> = RefCell::new(Vec::new());
    /// Popups a page is opening: the opener's key, the holder made for it, where it goes.
    static OPENING: RefCell<std::collections::VecDeque<(usize, usize, Option<String>)>> = RefCell::new(Default::default());
}

fn inner_of(key: usize) -> Option<Rc<RefCell<Inner>>> {
    TABS.with(|t| t.borrow().get(&key).cloned())
}

fn key_of(browser: Option<&mut Browser>) -> Option<usize> {
    let id = browser?.identifier();
    BY_BROWSER.with(|b| b.borrow().get(&id).copied())
}

fn with_inner<T>(browser: Option<&mut Browser>, f: impl FnOnce(&mut Inner) -> T) -> Option<T> {
    let inner = inner_of(key_of(browser)?)?;
    let mut i = inner.borrow_mut();
    Some(f(&mut i))
}

/// The view a page's browser is made in: a plain view, known by its class.
fn holder_class() -> &'static objc::runtime::Class {
    use objc::declare::ClassDecl;
    static CLASS: OnceLock<usize> = OnceLock::new();
    let ptr = *CLASS.get_or_init(|| {
        let decl = ClassDecl::new("YHChromeView", class!(NSView)).expect("YHChromeView");
        decl.register() as *const objc::runtime::Class as usize
    });
    unsafe { &*(ptr as *const objc::runtime::Class) }
}

/// Whether `view` is a Chrome page's holder.
pub fn is_page_view(view: id) -> bool {
    if view == nil {
        return false;
    }
    unsafe { msg_send![view, isKindOfClass: holder_class()] }
}

/// The holder `view` is in, or nil.
fn holder_of(mut view: id) -> id {
    unsafe {
        while view != nil {
            if is_page_view(view) {
                return view;
            }
            view = msg_send![view, superview];
        }
        nil
    }
}

fn new_holder() -> id {
    unsafe {
        let view: id = msg_send![holder_class(), alloc];
        let view: id = msg_send![view, initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(super::LAPTOP.0, super::LAPTOP.1))];
        let _: () = msg_send![view, setAutoresizesSubviews: YES];
        // Views don't clip what's in them by default (macOS 14+): Chromium's page view, bigger
        // than the spot under device emulation, drew past it, over the panel around it.
        let _: () = msg_send![view, setClipsToBounds: YES];
        view
    }
}

/// Makes the browsers of pages waiting for CEF (it's ready, or their profile is).
fn make_pending() {
    for f in ON_READY.with(|r| std::mem::take(&mut *r.borrow_mut())) {
        f();
    }
    for key in PENDING.with(|p| std::mem::take(&mut *p.borrow_mut())) {
        let Some(inner) = inner_of(key) else { continue };
        let profile = inner.borrow().profile.clone();
        with_profile(&profile, move |context| make_browser(key, context));
    }
}

/// Pages waiting when the engine can't start: what the agent waits on ends; they stay waiting
/// for a retry (`retry`).
fn fail_pending() {
    let why = match install_state() {
        Install::Failed(e) if !e.is_empty() => format!(": {e}"),
        _ => String::new(),
    };
    for key in PENDING.with(|p| p.borrow().clone()) {
        // What waited on the page ends now (its replies with it); the address stays, to open
        // once the engine is there (`retry`).
        let url = inner_of(key).and_then(|inner| {
            let mut i = inner.borrow_mut();
            i.loading = false;
            i.queue.clear();
            i.url.clone()
        });
        post(WebEvent::Failed { view: key, error: format!("The browser engine (Chrome) isn't available{why}. Try again, or switch to WebKit in Settings → Browser."), url });
    }
}

const HELPERS: &str = include_str!("../browser/helpers.js");
const HOOKS: &str = include_str!("../browser/hooks.js");

fn make_browser(key: usize, context: &RequestContext) {
    let Some(inner) = inner_of(key) else { return };
    if inner.borrow().browser.is_some() {
        return;
    }
    let holder = key as id;
    let bounds: NSRect = unsafe { msg_send![holder, bounds] };
    let info = WindowInfo {
        parent_view: holder as *mut c_void,
        bounds: Rect { x: 0, y: 0, width: bounds.size.width as i32, height: bounds.size.height as i32 },
        runtime_style: RuntimeStyle::ALLOY,
        ..Default::default()
    };
    let mut client = TabClient::new();
    let mut ctx = Some(context.clone());
    let settings = BrowserSettings { background_color: 0xFFFF_FFFF, ..Default::default() };
    let Some(browser) = browser_host_create_browser_sync(Some(&info), Some(&mut client), Some(&CefString::from("about:blank")), Some(&settings), None, ctx.as_mut()) else {
        eprintln!("CHROME couldn't make a browser");
        return;
    };
    adopt(key, browser, false);
}

/// The browser `browser` becomes the page `key`'s: its view fills the holder, its scripts go in,
/// then what waited for it.
fn adopt(key: usize, browser: Browser, popup: bool) {
    let Some(inner) = inner_of(key) else { return };
    BY_BROWSER.with(|b| b.borrow_mut().insert(browser.identifier(), key));
    if let Some(host) = browser.host() {
        let view = host.window_handle() as id;
        if view != nil {
            unsafe {
                let holder = key as id;
                let current: id = msg_send![view, superview];
                if current != holder {
                    let _: () = msg_send![view, removeFromSuperview];
                    let _: () = msg_send![holder, addSubview: view];
                }
                let bounds: NSRect = msg_send![holder, bounds];
                let _: () = msg_send![view, setFrame: bounds];
                // Placed by `fit_view` each time the holder changes, never by autoresizing.
                let _: () = msg_send![view, setAutoresizingMask: 0usize];
            }
        }
        let observer = host.add_dev_tools_message_observer(Some(&mut Observer::new(key)));
        inner.borrow_mut()._observer = observer;
    }
    // The helpers in their own world; the console and network hooks in the page's, posting
    // through a binding (`__yhLog`) instead of WebKit's message handler. Main frame only.
    fire(&browser, "Input.setInterceptDrags", json!({"enabled": true}));
    fire(&browser, "Runtime.enable", json!({}));
    fire(&browser, "Runtime.addBinding", json!({"name": "__yhLog"}));
    fire(&browser, "Page.enable", json!({}));
    fire(&browser, "Page.addScriptToEvaluateOnNewDocument", json!({"source": format!("if (window === window.top) {{\n{HOOKS}\n}}")}));
    fire(&browser, "Page.addScriptToEvaluateOnNewDocument", json!({"source": window_script()}));
    // What waited for the browser goes once the last of these is in, answered by the page's
    // renderer: a load would go straight to the page, ahead of the scripts.
    let b = browser.clone();
    cdp(&browser, "Page.addScriptToEvaluateOnNewDocument", json!({"source": format!("if (window === window.top) {{\n{HELPERS}\n}}"), "worldName": HELPER_WORLD}), move |_| {
        let (queue, kept) = {
            let mut i = inner.borrow_mut();
            i.browser = Some(b.clone());
            (std::mem::take(&mut i.queue), i.url.clone())
        };
        // The holder may have moved or changed size while the browser was being set up.
        fit(key, &b);
        // A page whose load was dropped while the engine couldn't be had (`fail_pending`): its
        // address, now. (A popup is already on its way to it.)
        if queue.is_empty() && !popup {
            if let Some(url) = kept.filter(|u| !u.is_empty() && u != "about:blank") {
                if let Some(f) = b.main_frame() {
                    f.load_url(Some(&CefString::from(url.as_str())));
                }
            }
        }
        for f in queue {
            f(&b);
        }
    });
}

const HELPER_WORLD: &str = "YourHarness";

/// Chromium's view of `browser` exactly over the holder `key` (`WebView::fit_view`).
fn fit(key: usize, browser: &Browser) {
    let Some(host) = browser.host() else { return };
    let view = host.window_handle() as id;
    if view == nil {
        return;
    }
    unsafe {
        let bounds: NSRect = msg_send![key as id, bounds];
        let now: NSRect = msg_send![view, frame];
        if now.origin.x != 0. || now.origin.y != 0. || now.size.width != bounds.size.width || now.size.height != bounds.size.height {
            let _: () = msg_send![view, setFrame: bounds];
            host.was_resized();
        }
    }
}

/// The page's window as a browser window its size would be: its outer size its own (not the
/// app's window, or the parking panel, it's drawn in), the screen's free area the real one (under
/// device emulation Chromium gives the whole screen).
fn window_script() -> String {
    let (aw, ah) = unsafe {
        let screen: id = msg_send![class!(NSScreen), mainScreen];
        if screen == nil {
            (0., 0.)
        } else {
            let visible: NSRect = msg_send![screen, visibleFrame];
            (visible.size.width, visible.size.height)
        }
    };
    let avail = if aw > 0. {
        format!("for (const [k, v] of [['availWidth', {aw}], ['availHeight', {ah}]]) try {{ Object.defineProperty(Screen.prototype, k, {{ get: () => v, configurable: true }}); }} catch {{}}")
    } else {
        String::new()
    };
    format!("(() => {{ if (window !== window.top) return; for (const [k, v] of [['outerWidth', () => innerWidth], ['outerHeight', () => innerHeight]]) try {{ Object.defineProperty(window, k, {{ get: v, configurable: true }}); }} catch {{}} {avail} }})();")
}

// ---- DevTools, in process ----

type Answer = Box<dyn FnOnce(Result<Value, String>)>;
static NEXT_ID: AtomicI32 = AtomicI32::new(1);

thread_local! {
    static CALLS: RefCell<HashMap<i32, Answer>> = RefCell::new(HashMap::new());
    /// A drag the page started that Chromium handed over (`Input.dragIntercepted`), by page.
    static DRAGS: RefCell<HashMap<usize, Value>> = RefCell::new(HashMap::new());
}

/// A DevTools call; `then` gets its result.
fn cdp(browser: &Browser, method: &str, params: Value, then: impl FnOnce(Result<Value, String>) + 'static) {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    CALLS.with(|c| c.borrow_mut().insert(id, Box::new(then)));
    let message = json!({"id": id, "method": method, "params": params}).to_string();
    let sent = browser.host().map(|h| h.send_dev_tools_message(Some(message.as_bytes()))).unwrap_or(0);
    if sent != 1 {
        if let Some(f) = CALLS.with(|c| c.borrow_mut().remove(&id)) {
            f(Err(format!("{method}: the page is gone")));
        }
    }
}

/// A DevTools call whose result doesn't matter (a failure is logged with `YH_CHROME_LOG`).
fn fire(browser: &Browser, method: &str, params: Value) {
    let name = method.to_string();
    cdp(browser, method, params, move |r| {
        if let (Err(e), true) = (r, log()) {
            eprintln!("CHROME {name} failed: {e}");
        }
    });
}

wrap_dev_tools_message_observer! {
    struct Observer {
        key: usize,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(&self, _browser: Option<&mut Browser>, message_id: i32, success: i32, result: Option<&[u8]>) {
            let value: Value = result.and_then(|r| serde_json::from_slice(r).ok()).unwrap_or_default();
            if let Some(f) = CALLS.with(|c| c.borrow_mut().remove(&message_id)) {
                f(if success == 1 { Ok(value) } else { Err(value["message"].as_str().map(str::to_owned).unwrap_or_else(|| value.to_string())) });
            }
        }

        fn on_dev_tools_event(&self, _browser: Option<&mut Browser>, method: Option<&CefString>, params: Option<&[u8]>) {
            let method = method.map(|m| m.to_string()).unwrap_or_default();
            match method.as_str() {
                "Runtime.bindingCalled" => {
                    let v: Value = params.and_then(|p| serde_json::from_slice(p).ok()).unwrap_or_default();
                    if v["name"] == "__yhLog" {
                        post(WebEvent::Log { view: self.key, json: v["payload"].as_str().unwrap_or_default().to_string() });
                    }
                }
                "Input.dragIntercepted" => {
                    let v: Value = params.and_then(|p| serde_json::from_slice(p).ok()).unwrap_or_default();
                    DRAGS.with(|d| d.borrow_mut().insert(self.key, v["data"].clone()));
                }
                _ => {}
            }
        }
    }
}

// ---- What CEF tells about pages ----

fn main_frame(frame: &Option<&mut Frame>) -> bool {
    frame.as_ref().is_some_and(|f| f.is_main() == 1)
}

wrap_load_handler! {
    struct Loads;

    impl LoadHandler {
        fn on_loading_state_change(&self, browser: Option<&mut Browser>, is_loading: i32, _can_go_back: i32, _can_go_forward: i32) {
            with_inner(browser, |i| {
                i.loading = is_loading == 1;
                if is_loading == 1 {
                    i.progress = i.progress.min(0.1);
                }
            });
        }

        fn on_load_start(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, _transition_type: TransitionType) {
            if main_frame(&frame) {
                if let Some(key) = key_of(browser) {
                    post(WebEvent::Committed { view: key });
                }
            }
        }

        fn on_load_end(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, http_status_code: i32) {
            if !main_frame(&frame) {
                return;
            }
            let url = frame.as_ref().map(|f| CefString::from(&f.url()).to_string()).unwrap_or_default();
            let Some(key) = key_of(browser) else { return };
            if url.starts_with("http") {
                let json = json!({ "kind": "request", "method": "GET", "url": url, "type": "document", "status": http_status_code });
                post(WebEvent::Log { view: key, json: json.to_string() });
            }
            post(WebEvent::Finished { view: key });
        }

        fn on_load_error(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, error_code: Errorcode, error_text: Option<&CefString>, failed_url: Option<&CefString>) {
            // ERR_ABORTED: another navigation replaced it, or it became a download.
            if !main_frame(&frame) || error_code == Errorcode::ABORTED {
                return;
            }
            let Some(key) = key_of(browser) else { return };
            let error = error_text.map(|e| e.to_string()).unwrap_or_else(|| "failed".into());
            post(WebEvent::Failed { view: key, error, url: failed_url.map(|u| u.to_string()) });
        }
    }
}

wrap_display_handler! {
    struct Display;

    impl DisplayHandler {
        fn on_address_change(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            if main_frame(&frame) {
                let url = url.map(|u| u.to_string());
                with_inner(browser, |i| i.url = url);
            }
        }

        fn on_title_change(&self, browser: Option<&mut Browser>, title: Option<&CefString>) {
            let title = title.map(|t| t.to_string());
            with_inner(browser, |i| i.title = title);
        }

        fn on_loading_progress_change(&self, browser: Option<&mut Browser>, progress: f64) {
            with_inner(browser, |i| i.progress = progress);
        }
    }
}

wrap_request_handler! {
    struct Requests;

    impl RequestHandler {
        fn on_before_browse(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, _request: Option<&mut Request>, _user_gesture: i32, _is_redirect: i32) -> i32 {
            if main_frame(&frame) {
                if let Some(key) = key_of(browser) {
                    post(WebEvent::Started { view: key });
                }
            }
            0
        }

        fn on_render_process_terminated(&self, browser: Option<&mut Browser>, _status: TerminationStatus, _error_code: i32, _error_string: Option<&CefString>) {
            if let Some(key) = key_of(browser) {
                post(WebEvent::Crashed { view: key });
            }
        }
    }
}

wrap_jsdialog_handler! {
    struct Dialogs;

    impl JsdialogHandler {
        fn on_jsdialog(&self, browser: Option<&mut Browser>, _origin_url: Option<&CefString>, dialog_type: JsdialogType, message_text: Option<&CefString>, default_prompt_text: Option<&CefString>, callback: Option<&mut JsdialogCallback>, _suppress_message: Option<&mut i32>) -> i32 {
            let Some(key) = key_of(browser) else { return 0 };
            let Some(callback) = callback else { return 0 };
            let kind = if dialog_type == JsdialogType::CONFIRM { "confirm" } else if dialog_type == JsdialogType::PROMPT { "prompt" } else { "alert" };
            let default = (kind == "prompt").then(|| default_prompt_text.map(|d| d.to_string()).unwrap_or_default());
            if let Some(inner) = inner_of(key) {
                inner.borrow_mut().dialog = Some((callback.clone(), default.clone()));
            }
            post(WebEvent::Dialog { view: key, kind: kind.into(), message: message_text.map(|m| m.to_string()).unwrap_or_default(), default });
            1
        }

        fn on_before_unload_dialog(&self, _browser: Option<&mut Browser>, _message_text: Option<&CefString>, _is_reload: i32, callback: Option<&mut JsdialogCallback>) -> i32 {
            // Leaving a page always goes ahead.
            if let Some(c) = callback {
                c.cont(1, None);
            }
            1
        }
    }
}

wrap_dialog_handler! {
    struct Files;

    impl DialogHandler {
        fn on_file_dialog(&self, browser: Option<&mut Browser>, mode: FileDialogMode, _title: Option<&CefString>, _default_file_path: Option<&CefString>, _accept_filters: Option<&mut CefStringList>, _accept_extensions: Option<&mut CefStringList>, _accept_descriptions: Option<&mut CefStringList>, callback: Option<&mut FileDialogCallback>) -> i32 {
            let Some(key) = key_of(browser) else { return 0 };
            let files = inner_of(key).and_then(|i| i.borrow_mut().upload.take());
            match (files, callback) {
                // The agent's files, waiting for this chooser.
                (Some(mut files), Some(callback)) => {
                    if mode != FileDialogMode::OPEN_MULTIPLE {
                        files.truncate(1);
                    }
                    let mut list = CefStringList::new();
                    for f in &files {
                        list.append(f);
                    }
                    callback.cont(Some(&mut list));
                    1
                }
                // Yours: the open panel, as Chrome shows it.
                _ => {
                    post(WebEvent::UploadAsked { view: key });
                    0
                }
            }
        }
    }
}

wrap_download_handler! {
    struct Downloads;

    impl DownloadHandler {
        fn can_download(&self, browser: Option<&mut Browser>, url: Option<&CefString>, _request_method: Option<&CefString>) -> i32 {
            if let Some(key) = key_of(browser) {
                let text = format!("Download of {} skipped: the browser doesn't download files.", url.map(|u| u.to_string()).unwrap_or_else(|| "a file".into()));
                post(WebEvent::Log { view: key, json: json!({ "kind": "console", "level": "info", "text": text }).to_string() });
            }
            0
        }
    }
}

wrap_permission_handler! {
    struct Permissions;

    impl PermissionHandler {
        /// The camera and microphone: never (macOS would ask you for the app).
        fn on_request_media_access_permission(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _requesting_origin: Option<&CefString>, _requested_permissions: u32, callback: Option<&mut MediaAccessCallback>) -> i32 {
            if let Some(c) = callback {
                c.cont(0);
            }
            1
        }

        fn on_show_permission_prompt(&self, _browser: Option<&mut Browser>, _prompt_id: u64, _requesting_origin: Option<&CefString>, _requested_permissions: u32, callback: Option<&mut PermissionPromptCallback>) -> i32 {
            if let Some(c) = callback {
                c.cont(PermissionRequestResult::DENY);
            }
            1
        }
    }
}

wrap_focus_handler! {
    struct Focus;

    impl FocusHandler {
        /// A page may take the keyboard only while you may type into it.
        fn on_set_focus(&self, browser: Option<&mut Browser>, _source: FocusSource) -> i32 {
            let allowed = key_of(browser).is_some_and(|k| TYPING.with(|t| t.borrow().contains(&k)));
            i32::from(!allowed)
        }
    }
}

wrap_life_span_handler! {
    struct Life;

    impl LifeSpanHandler {
        /// A page's popup (`window.open`, `target=_blank`): made in a holder of its own, off
        /// screen, then handed to the browser as a new tab (`on_after_created`).
        fn on_before_popup(&self, browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _popup_id: i32, target_url: Option<&CefString>, _target_frame_name: Option<&CefString>, _target_disposition: WindowOpenDisposition, _user_gesture: i32, _popup_features: Option<&PopupFeatures>, window_info: Option<&mut WindowInfo>, _client: Option<&mut Option<Client>>, _settings: Option<&mut BrowserSettings>, _extra_info: Option<&mut Option<DictionaryValue>>, _no_javascript_access: Option<&mut i32>) -> i32 {
            let Some(opener) = key_of(browser) else { return 1 };
            let Some(info) = window_info else { return 1 };
            let holder = new_holder();
            unsafe {
                let parking = super::parking_view();
                let _: () = msg_send![parking, addSubview: holder];
            }
            info.parent_view = holder as *mut c_void;
            info.bounds = Rect { x: 0, y: 0, width: super::LAPTOP.0 as i32, height: super::LAPTOP.1 as i32 };
            info.runtime_style = RuntimeStyle::ALLOY;
            OPENING.with(|o| o.borrow_mut().push_back((opener, holder as usize, target_url.map(|u| u.to_string()))));
            0
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser else { return };
            if BY_BROWSER.with(|b| b.borrow().contains_key(&browser.identifier())) {
                return;
            }
            // Not one the app made: a popup.
            let Some((opener, holder, url)) = OPENING.with(|o| o.borrow_mut().pop_front()) else { return };
            let profile = inner_of(opener).map(|i| i.borrow().profile.clone()).unwrap_or_default();
            let inner = Rc::new(RefCell::new(Inner::new(holder, profile)));
            TABS.with(|t| t.borrow_mut().insert(holder, inner));
            adopt(holder, browser.clone(), true);
            let made = super::popup_made(opener, super::Popup::Chrome(Popup { holder: holder as id }), url);
            if made.is_none() {
                // Past the most tabs: it goes.
                if let Some(inner) = inner_of(holder) {
                    inner.borrow_mut().closing = true;
                }
                if let Some(h) = browser.host() {
                    h.close_browser(1);
                }
            }
        }

        /// The window a page is in is the app's: CEF would close it with the page (its default
        /// for a browser in a window of its own). Only the page goes: its view taken out, then
        /// CEF ends it (`on_before_close`). One the page closed itself goes from its tab too.
        fn do_close(&self, browser: Option<&mut Browser>) -> i32 {
            let Some(browser) = browser else { return 1 };
            let key = BY_BROWSER.with(|b| b.borrow().get(&browser.identifier()).copied());
            if let Some(view) = browser.host().map(|h| h.window_handle() as id) {
                unsafe {
                    let _: () = msg_send![view, removeFromSuperview];
                }
            }
            if let Some(key) = key {
                let by_app = inner_of(key).is_none_or(|i| i.borrow().closing);
                if !by_app {
                    post(WebEvent::Closed { view: key });
                }
            }
            1
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser else { return };
            let id = browser.identifier();
            if let Some(key) = BY_BROWSER.with(|b| b.borrow_mut().remove(&id)) {
                if let Some(inner) = inner_of(key) {
                    let mut i = inner.borrow_mut();
                    i.closed = true;
                    i.browser = None;
                    i._observer = None;
                }
                if inner_of(key).is_some_and(|i| i.borrow().closing) {
                    TABS.with(|t| t.borrow_mut().remove(&key));
                }
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

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(Loads::new())
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(Display::new())
        }

        fn request_handler(&self) -> Option<RequestHandler> {
            Some(Requests::new())
        }

        fn jsdialog_handler(&self) -> Option<JsdialogHandler> {
            Some(Dialogs::new())
        }

        fn dialog_handler(&self) -> Option<DialogHandler> {
            Some(Files::new())
        }

        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(Downloads::new())
        }

        fn permission_handler(&self) -> Option<PermissionHandler> {
            Some(Permissions::new())
        }

        fn focus_handler(&self) -> Option<FocusHandler> {
            Some(Focus::new())
        }
    }
}

impl Inner {
    fn new(key: usize, profile: String) -> Inner {
        Inner {
            key,
            profile,
            browser: None,
            queue: vec![],
            url: None,
            title: None,
            loading: true,
            progress: 0.,
            dialog: None,
            upload: None,
            closing: false,
            closed: false,
            emulated: None,
            _observer: None,
        }
    }
}

/// A page's popup, made by Chromium in `holder`.
pub struct Popup {
    holder: id,
}

/// A page in the Chrome engine.
pub struct WebView {
    page: Page,
    inner: Rc<RefCell<Inner>>,
}

static PRIVATE: AtomicU64 = AtomicU64::new(0);

/// NSEvent's modifier flags as DevTools' (Alt 1, Control 2, Meta 4, Shift 8).
fn modifiers(flags: usize) -> i32 {
    let mut m = 0;
    if flags & (1 << 19) != 0 {
        m |= 1;
    }
    if flags & (1 << 18) != 0 {
        m |= 2;
    }
    if flags & (1 << 20) != 0 {
        m |= 4;
    }
    if flags & (1 << 17) != 0 {
        m |= 8;
    }
    m
}

/// A Mac virtual key code's DOM key, code and Windows key code, for keys that aren't text.
fn named_key(code: u16) -> Option<(&'static str, &'static str, i32)> {
    Some(match code {
        36 => ("Enter", "Enter", 13),
        76 => ("Enter", "NumpadEnter", 13),
        48 => ("Tab", "Tab", 9),
        49 => (" ", "Space", 32),
        51 => ("Backspace", "Backspace", 8),
        117 => ("Delete", "Delete", 46),
        53 => ("Escape", "Escape", 27),
        123 => ("ArrowLeft", "ArrowLeft", 37),
        124 => ("ArrowRight", "ArrowRight", 39),
        125 => ("ArrowDown", "ArrowDown", 40),
        126 => ("ArrowUp", "ArrowUp", 38),
        115 => ("Home", "Home", 36),
        119 => ("End", "End", 35),
        116 => ("PageUp", "PageUp", 33),
        121 => ("PageDown", "PageDown", 34),
        122 => ("F1", "F1", 112),
        120 => ("F2", "F2", 113),
        99 => ("F3", "F3", 114),
        118 => ("F4", "F4", 115),
        96 => ("F5", "F5", 116),
        97 => ("F6", "F6", 117),
        98 => ("F7", "F7", 118),
        100 => ("F8", "F8", 119),
        101 => ("F9", "F9", 120),
        109 => ("F10", "F10", 121),
        103 => ("F11", "F11", 122),
        111 => ("F12", "F12", 123),
        _ => return None,
    })
}

/// A printable key's DOM code and Windows key code (US layout), from its character.
fn char_code(c: char) -> (String, i32) {
    let upper = c.to_ascii_uppercase();
    if upper.is_ascii_alphabetic() {
        return (format!("Key{upper}"), upper as i32);
    }
    if c.is_ascii_digit() {
        return (format!("Digit{c}"), c as i32);
    }
    let (code, vk) = match c {
        '-' | '_' => ("Minus", 189),
        '=' | '+' => ("Equal", 187),
        '[' | '{' => ("BracketLeft", 219),
        ']' | '}' => ("BracketRight", 221),
        '\\' | '|' => ("Backslash", 220),
        ';' | ':' => ("Semicolon", 186),
        '\'' | '"' => ("Quote", 222),
        ',' | '<' => ("Comma", 188),
        '.' | '>' => ("Period", 190),
        '/' | '?' => ("Slash", 191),
        '`' | '~' => ("Backquote", 192),
        _ => ("", 0),
    };
    (code.to_string(), vk)
}

impl WebView {
    /// A page over `host`, in `store`'s profile (a project's ID; None for the app's own). Its
    /// browser is made once CEF and the profile are ready (the engine downloaded first if it's
    /// not here); what's asked of it meanwhile waits.
    pub fn new(host: id, store: Option<&str>) -> WebView {
        let holder = new_holder();
        let key = holder as usize;
        let inner = Rc::new(RefCell::new(Inner::new(key, project_key(store))));
        TABS.with(|t| t.borrow_mut().insert(key, inner.clone()));
        let web = WebView { page: Page::new_beside(host, holder), inner };
        watch_focus();
        if READY.load(Ordering::Acquire) {
            let profile = web.inner.borrow().profile.clone();
            with_profile(&profile, move |context| make_browser(key, context));
        } else {
            PENDING.with(|p| p.borrow_mut().push(key));
            ensure_started();
        }
        web
    }

    /// A page in memory only, forgotten when it closes.
    #[allow(dead_code)]
    pub fn private(host: id) -> WebView {
        let n = PRIVATE.fetch_add(1, Ordering::Relaxed);
        let web = WebView::new(host, None);
        web.inner.borrow_mut().profile = format!("private-{n}");
        web
    }

    /// A popup Chromium made, over `host`.
    pub fn from_popup(host: id, popup: Popup) -> WebView {
        let key = popup.holder as usize;
        let inner = inner_of(key).unwrap_or_else(|| Rc::new(RefCell::new(Inner::new(key, String::new()))));
        unsafe {
            let _: () = msg_send![popup.holder, removeFromSuperview];
        }
        let web = WebView { page: Page::new_beside(host, popup.holder), inner };
        web.emulate();
        web
    }

    /// Runs `f` with the browser now, or once it's made.
    /// Runs `f` with the browser now, or once it's made; dropped (its reply with it, which ends
    /// the wait) while the engine can't be had.
    fn with_browser(&self, f: impl FnOnce(&Browser) + 'static) {
        let browser = self.inner.borrow().browser.clone();
        match browser {
            Some(b) => f(&b),
            None if self.inner.borrow().closed || matches!(install_state(), Install::Failed(_)) => {}
            None => self.inner.borrow_mut().queue.push(Box::new(f)),
        }
    }

    pub fn report_place(&self) -> String {
        self.page.report_place()
    }

    pub fn shown_in(&self, window: id) -> Option<(f64, f64, f64, f64)> {
        self.page.shown_in(window)
    }

    pub fn move_to(&self, host: id) {
        self.page.move_to(host);
        self.fit_view();
    }

    /// Chromium's view exactly over its holder. Not left to autoresizing: that keeps the margins
    /// the view had, and once the holder was squashed (moved between windows, parked small) the
    /// page sat offset from its spot, clicks landing beside where they were aimed or on nothing.
    fn fit_view(&self) {
        if let Some(b) = self.inner.borrow().browser.clone() {
            fit(self.key(), &b);
        }
    }

    pub fn set_frame(&self, bounds: Option<(f64, f64, f64, f64)>) {
        self.page.set_frame(bounds);
        self.fit_view();
        self.emulate();
        if log() {
            eprintln!("CHROME frames after set_frame({bounds:?}): {}", self.view_frames());
        }
    }

    /// The holder's frame and every view inside it, for `YH_CHROME_LOG`.
    fn view_frames(&self) -> String {
        fn walk(view: id, depth: usize, out: &mut Vec<String>) {
            unsafe {
                let f: NSRect = msg_send![view, frame];
                let class = rust_string(msg_send![view, className]).unwrap_or_default();
                out.push(format!("{}{class} {:.0},{:.0} {:.0}x{:.0}", "  ".repeat(depth), f.origin.x, f.origin.y, f.size.width, f.size.height));
                if depth < 3 {
                    let subs: id = msg_send![view, subviews];
                    let n: usize = msg_send![subs, count];
                    for i in 0..n {
                        walk(msg_send![subs, objectAtIndex: i], depth + 1, out);
                    }
                }
            }
        }
        let mut out = vec![];
        walk(self.key() as id, 0, &mut out);
        out.join(" | ")
    }

    /// The page laid out at its CSS size and drawn at the size it has on screen: Chromium sizes
    /// its page by the view, so a viewport other than the view's (a preset, or parked at half
    /// size) is DevTools' device emulation, scaled to fit, as its device mode does.
    fn emulate(&self) {
        let (fw, fh, cw, ch) = self.page.layout();
        if fw < 1. || cw < 1. {
            return;
        }
        let want = ((cw - fw).abs() > 0.5 || (ch - fh).abs() > 0.5).then(|| (cw.round(), ch.round(), fw / cw));
        {
            let mut i = self.inner.borrow_mut();
            if i.emulated == Some(want) {
                return;
            }
            i.emulated = Some(want);
        }
        self.with_browser(move |b| match want {
            Some((w, h, scale)) => fire(b, "Emulation.setDeviceMetricsOverride", json!({"width": w, "height": h, "deviceScaleFactor": 0, "mobile": false, "scale": scale})),
            None => fire(b, "Emulation.clearDeviceMetricsOverride", json!({})),
        });
    }

    /// Chromium captures beyond the viewport: the view stays as it is.
    pub fn hold_full_height(&self, _height: Option<f64>) {}

    pub fn set_viewport(&self, viewport: Option<(f64, f64)>) {
        self.page.set_viewport(viewport);
        self.fit_view();
        self.emulate();
    }

    pub fn viewport(&self) -> Option<(f64, f64)> {
        self.page.viewport()
    }

    pub fn set_color_scheme(&self, scheme: &str) {
        let features = match scheme {
            "light" | "dark" => json!([{"name": "prefers-color-scheme", "value": scheme}]),
            _ => json!([]),
        };
        self.with_browser(move |b| fire(b, "Emulation.setEmulatedMedia", json!({"features": features})));
    }

    pub fn answer_dialog(&self, accept: bool, text: Option<String>) -> bool {
        let Some((callback, default)) = self.inner.borrow_mut().dialog.take() else { return false };
        let answer = if accept { text.or(default) } else { None };
        callback.cont(i32::from(accept), answer.as_deref().map(CefString::from).as_ref());
        true
    }

    pub fn set_upload(&self, files: Option<Vec<String>>) -> bool {
        let mut i = self.inner.borrow_mut();
        match files {
            Some(f) => {
                i.upload = Some(f);
                true
            }
            None => i.upload.take().is_some(),
        }
    }

    pub fn load(&self, url: &str) {
        {
            let mut i = self.inner.borrow_mut();
            if i.browser.is_none() {
                i.url = Some(url.to_string());
            }
        }
        let url = url.to_string();
        self.with_browser(move |b| {
            if let Some(f) = b.main_frame() {
                f.load_url(Some(&CefString::from(url.as_str())));
            }
        });
    }

    pub fn url(&self) -> Option<String> {
        self.inner.borrow().url.clone().filter(|u| !u.is_empty())
    }

    pub fn title(&self) -> Option<String> {
        self.inner.borrow().title.clone()
    }

    pub fn is_loading(&self) -> bool {
        let i = self.inner.borrow();
        match &i.browser {
            Some(b) => b.is_loading() == 1,
            None => !i.closed && !matches!(install_state(), Install::Failed(_)),
        }
    }

    pub fn key(&self) -> usize {
        self.inner.borrow().key
    }

    pub fn progress(&self) -> f64 {
        self.inner.borrow().progress
    }

    pub fn can_go_back(&self) -> bool {
        self.inner.borrow().browser.as_ref().is_some_and(|b| b.can_go_back() == 1)
    }

    pub fn can_go_forward(&self) -> bool {
        self.inner.borrow().browser.as_ref().is_some_and(|b| b.can_go_forward() == 1)
    }

    pub fn go_forward(&self) {
        self.with_browser(|b| b.go_forward());
    }

    pub fn go_back(&self) {
        self.with_browser(|b| b.go_back());
    }

    pub fn reload(&self) {
        self.with_browser(|b| b.reload());
    }

    pub fn stop_loading(&self) {
        self.with_browser(|b| b.stop_load());
    }

    pub fn size(&self) -> (f64, f64) {
        self.page.size()
    }

    pub fn is_shown(&self) -> bool {
        self.page.is_shown()
    }

    /// Runs `body` in the helpers' world (`YourHarness`, made by name each time: Chromium hands
    /// back the same one, with `helpers.js` in it) or the page's.
    pub fn call(&self, body: &str, in_page: bool, reply: flume::Sender<Result<String, String>>) {
        let expression = format!("(async () => {{ {body}\n}})().then(r => r === undefined ? 'null' : JSON.stringify(r))");
        self.with_browser(move |b| {
            if in_page {
                evaluate_in(b, expression, None, reply, true);
                return;
            }
            let b2 = b.clone();
            cdp(b, "Page.getFrameTree", json!({}), move |tree| {
                let frame = tree.ok().and_then(|t| t["frameTree"]["frame"]["id"].as_str().map(str::to_owned)).unwrap_or_default();
                let b3 = b2.clone();
                cdp(&b2, "Page.createIsolatedWorld", json!({"frameId": frame, "worldName": HELPER_WORLD}), move |world| match world {
                    Ok(w) => {
                        let context = w["executionContextId"].as_i64();
                        // A document the helpers didn't go into (one already there when the
                        // browser was made): they go in now.
                        let b4 = b3.clone();
                        cdp(&b3, "Runtime.evaluate", json!({"expression": "typeof __yh", "contextId": context, "returnByValue": true}), move |r| {
                            if r.is_ok_and(|r| r["result"]["value"] == "undefined") {
                                fire(&b4, "Runtime.evaluate", json!({"expression": HELPERS, "contextId": context}));
                            }
                            evaluate_in(&b4, expression, context, reply, true);
                        });
                    }
                    Err(e) => {
                        let _ = reply.send(Err(e));
                    }
                });
            });
        });
    }

    pub fn set_keyboard(&self, allowed: bool) {
        let key = self.key();
        // `try_with`: a page let go of as the app quits may outlive the table.
        let _ = TYPING.try_with(|t| {
            let mut t = t.borrow_mut();
            if allowed { t.insert(key) } else { t.remove(&key) }
        });
        if allowed {
            return;
        }
        unsafe {
            let holder = key as id;
            let window: id = msg_send![holder, window];
            if window == nil {
                return;
            }
            let first: id = msg_send![window, firstResponder];
            let is_view: bool = first != nil && msg_send![first, isKindOfClass: class!(NSView)];
            if is_view && holder_of(first) == holder {
                let _: bool = msg_send![window, makeFirstResponder: app_view(window)];
            }
        }
    }

    fn mouse(&self, kind: &'static str, x: f64, y: f64, button: &'static str, buttons: i32, clicks: isize, mods: usize) {
        let key = self.key();
        // Under device emulation, DevTools takes input where it lands in the view, scaled.
        let scale = self.inner.borrow().emulated.flatten().map_or(1., |(_, _, s)| s);
        let (x, y) = (x * scale, y * scale);
        let params = json!({"type": kind, "x": x, "y": y, "button": button, "buttons": buttons, "clickCount": clicks, "modifiers": modifiers(mods)});
        self.with_browser(move |b| {
            // A drag the page started, handed over: it goes on as drag events.
            let drag = DRAGS.with(|d| d.borrow().get(&key).cloned());
            if let Some(data) = drag {
                let kind = match kind {
                    "mouseMoved" => "dragOver",
                    "mouseReleased" => "drop",
                    _ => "",
                };
                if !kind.is_empty() {
                    if kind == "drop" {
                        DRAGS.with(|d| d.borrow_mut().remove(&key));
                    }
                    let (bx, by) = (params["x"].clone(), params["y"].clone());
                    let b2 = b.clone();
                    let release = params.clone();
                    cdp(b, "Input.dispatchDragEvent", json!({"type": if kind == "dragOver" { "dragEnter" } else { "dragOver" }, "x": bx, "y": by, "data": data, "modifiers": 0}), move |_| {
                        let data2 = data.clone();
                        let b3 = b2.clone();
                        cdp(&b2, "Input.dispatchDragEvent", json!({"type": kind, "x": bx, "y": by, "data": data2, "modifiers": 0}), move |_| {
                            if kind == "drop" {
                                fire(&b3, "Input.dispatchMouseEvent", release);
                            }
                        });
                    });
                    return;
                }
            }
            fire(b, "Input.dispatchMouseEvent", params);
        });
    }

    pub fn click(&self, x: f64, y: f64, right: bool, count: isize, mods: usize) {
        self.mouse("mouseMoved", x, y, "none", 0, 0, mods);
        let (button, buttons) = if right { ("right", 2) } else { ("left", 1) };
        for n in 1..=count.max(1) {
            self.mouse("mousePressed", x, y, button, buttons, n, mods);
            self.mouse("mouseReleased", x, y, button, 0, n, mods);
        }
    }

    pub fn hover(&self, x: f64, y: f64) {
        self.mouse("mouseMoved", x, y, "none", 0, 0, 0);
    }

    pub fn mouse_down(&self, x: f64, y: f64) {
        DRAGS.with(|d| d.borrow_mut().remove(&self.key()));
        self.mouse("mouseMoved", x, y, "none", 0, 0, 0);
        self.mouse("mousePressed", x, y, "left", 1, 1, 0);
    }

    pub fn mouse_dragged(&self, x: f64, y: f64) {
        self.mouse("mouseMoved", x, y, "left", 1, 0, 0);
    }

    pub fn mouse_up(&self, x: f64, y: f64) {
        self.mouse("mouseReleased", x, y, "left", 0, 1, 0);
    }

    pub fn insert(&self, text: &str) {
        let text = text.to_string();
        self.with_browser(move |b| fire(b, "Input.insertText", json!({"text": text})));
    }

    pub fn press(&self, key_code: u16, characters: &str, plain: &str, mods: usize) {
        let m = modifiers(mods);
        let command = mods & (1 << 20) != 0;
        let (key, code, vk, text) = match named_key(key_code) {
            Some((key, code, vk)) => (key.to_string(), code.to_string(), vk, if key == "Enter" { "\r".to_string() } else if key == " " { " ".to_string() } else { String::new() }),
            None => {
                let c = plain.chars().next().unwrap_or(' ');
                let (code, vk) = char_code(c);
                (characters.to_string(), code, vk, characters.to_string())
            }
        };
        // ⌘ shortcuts are the editing commands Chrome's menus run.
        let commands: Vec<&str> = if command {
            match (plain.to_lowercase().as_str(), mods & (1 << 17) != 0) {
                ("a", _) => vec!["selectAll"],
                ("c", _) => vec!["copy"],
                ("x", _) => vec!["cut"],
                ("v", _) => vec!["paste"],
                ("z", false) => vec!["undo"],
                ("z", true) => vec!["redo"],
                _ => vec![],
            }
        } else {
            vec![]
        };
        // Text goes in only when no shortcut modifier is held.
        let text = if m & (1 | 2 | 4) != 0 && m != 8 { String::new() } else { text };
        let down = json!({"type": if text.is_empty() { "rawKeyDown" } else { "keyDown" }, "key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": key_code, "modifiers": m, "text": text, "unmodifiedText": text, "commands": commands});
        let up = json!({"type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": key_code, "modifiers": m});
        self.with_browser(move |b| {
            fire(b, "Input.dispatchKeyEvent", down);
            fire(b, "Input.dispatchKeyEvent", up);
        });
    }

    /// Part of the page (CSS pixels from the viewport's top left; past its bottom for a full
    /// page) as a JPEG at most `max_width` pixels wide.
    pub fn capture(&self, rect: Option<(f64, f64, f64, f64)>, max_width: f64, path: String, reply: flume::Sender<Result<(u32, u32), String>>) {
        let (w, h) = self.size();
        let (x, y, cw, ch) = rect.unwrap_or((0., 0., w, h));
        let (x, y) = (x.max(0.), y.max(0.));
        let cw = cw.min(w - x);
        let beyond = y + ch > h + 0.5;
        let ch = if beyond { ch } else { ch.min(h - y) };
        if cw < 1. || ch < 1. {
            let _ = reply.send(Err("That's outside the visible page; scroll it into view first.".into()));
            return;
        }
        let out_w = cw.min(max_width).round();
        self.with_browser(move |b| {
            let b2 = b.clone();
            cdp(b, "Page.getLayoutMetrics", json!({}), move |metrics| {
                let m = metrics.unwrap_or_default();
                let (px, py) = (m["cssVisualViewport"]["pageX"].as_f64().unwrap_or(0.), m["cssVisualViewport"]["pageY"].as_f64().unwrap_or(0.));
                let css_w = m["cssLayoutViewport"]["clientWidth"].as_f64().unwrap_or(w).max(1.);
                let dpr = (m["layoutViewport"]["clientWidth"].as_f64().unwrap_or(css_w * 2.) / css_w).max(1.);
                let scale = out_w / cw / dpr;
                let params = json!({"format": "jpeg", "quality": 80, "captureBeyondViewport": beyond, "clip": {"x": x + px, "y": y + py, "width": cw, "height": ch, "scale": scale}});
                cdp(&b2, "Page.captureScreenshot", params, move |shot| {
                    let _ = reply.send(shot.and_then(|s| {
                        let bytes = super::base64_decode(s["data"].as_str().unwrap_or_default());
                        let size = imagesize::blob_size(&bytes).map_err(|_| "Couldn't capture the page.".to_string())?;
                        std::fs::write(&path, &bytes).map_err(|_| "Couldn't write the screenshot.".to_string())?;
                        Ok((size.width as u32, size.height as u32))
                    }));
                });
            });
        });
    }

    /// The visible page as a JPEG at most `max_px` pixels across, decoded off the main thread.
    pub fn capture_jpeg(&self, max_px: f64, quality: f64, reply: flume::Sender<Option<(Vec<u8>, u32, u32)>>) {
        let (w, h) = self.size();
        if w < 1. || h < 1. {
            let _ = reply.send(None);
            return;
        }
        let scale = (max_px / (w * 2.)).min(0.5);
        let params = json!({"format": "jpeg", "quality": (quality * 100.).round() as i64, "clip": {"x": 0, "y": 0, "width": w, "height": h, "scale": scale}, "fromSurface": true});
        // A shot of where the page is now: the viewport, scrolled.
        self.with_browser(move |b| {
            let b2 = b.clone();
            cdp(b, "Page.getLayoutMetrics", json!({}), move |metrics| {
                let m = metrics.unwrap_or_default();
                let mut params = params;
                params["clip"]["x"] = json!(m["cssVisualViewport"]["pageX"].as_f64().unwrap_or(0.));
                params["clip"]["y"] = json!(m["cssVisualViewport"]["pageY"].as_f64().unwrap_or(0.));
                cdp(&b2, "Page.captureScreenshot", params, move |shot| {
                    let Ok(s) = shot else {
                        let _ = reply.send(None);
                        return;
                    };
                    let data = s["data"].as_str().unwrap_or_default().to_string();
                    std::thread::spawn(move || {
                        let bytes = super::base64_decode(&data);
                        let size = imagesize::blob_size(&bytes).ok();
                        let _ = reply.send(size.map(|z| (bytes, z.width as u32, z.height as u32)));
                    });
                });
            });
        });
    }

    pub fn end_content_process(&self) -> bool {
        let made = self.inner.borrow().browser.is_some();
        self.with_browser(|b| fire(b, "Page.crash", json!({})));
        made
    }

    /// Chrome's DevTools for the page, in a window of its own.
    pub fn show_inspector(&self) -> bool {
        let Some(host) = self.inner.borrow().browser.as_ref().and_then(|b| b.host()) else { return false };
        let info = WindowInfo { runtime_style: RuntimeStyle::ALLOY, ..Default::default() };
        host.show_dev_tools(Some(&info), None, Some(&BrowserSettings::default()), None);
        true
    }

    pub fn show_activity(&self, label: &str, rect: Option<(f64, f64, f64, f64)>) -> u64 {
        self.page.show_activity(label, rect)
    }

    pub fn clear_activity(&self, seq: u64) {
        self.page.clear_activity(seq)
    }

    pub fn shows_activity(&self) -> bool {
        self.page.shows_activity()
    }

    pub fn overlay_png(&self, path: &str) -> bool {
        self.page.overlay_png(path)
    }

    /// Runs JavaScript in the page: its value as text (a string as it is, anything else as JSON).
    pub fn evaluate(&self, script: &str, reply: flume::Sender<Result<String, String>>) {
        let script = script.to_string();
        self.with_browser(move |b| evaluate_in(b, script, None, reply, false));
    }

    /// What it shows now, as a PNG at `path`.
    pub fn snapshot(&self, path: String, reply: flume::Sender<Result<(), String>>) {
        self.with_browser(move |b| {
            cdp(b, "Page.captureScreenshot", json!({"format": "png"}), move |shot| {
                let _ = reply.send(shot.and_then(|s| {
                    let bytes = super::base64_decode(s["data"].as_str().unwrap_or_default());
                    std::fs::write(&path, &bytes).map_err(|_| "couldn't write the screenshot".to_string())
                }));
            });
        });
    }
}

/// `Runtime.evaluate` of `expression` (in `context`, else the page's world): with `json`, its
/// value is the JSON text the expression made; without, a string as it is, anything else as JSON.
fn evaluate_in(b: &Browser, expression: String, context: Option<i64>, reply: flume::Sender<Result<String, String>>, json_text: bool) {
    let mut params = json!({"expression": expression, "returnByValue": true, "awaitPromise": json_text, "userGesture": true});
    if let Some(c) = context {
        params["contextId"] = c.into();
    }
    cdp(b, "Runtime.evaluate", params, move |result| {
        if log() {
            if let Ok(r) = &result {
                if r.get("exceptionDetails").is_some() {
                    eprintln!("CHROME evaluate failed in {context:?}: {}", r["exceptionDetails"]);
                }
            }
        }
        let answer = result.and_then(|r| {
            if let Some(details) = r.get("exceptionDetails") {
                let text = details["exception"]["description"].as_str().or(details["text"].as_str()).unwrap_or("JavaScript error");
                return Err(text.lines().next().unwrap_or(text).to_string());
            }
            let value = &r["result"]["value"];
            Ok(match value {
                Value::String(s) => s.clone(),
                Value::Null => "null".to_string(),
                v if r["result"]["type"] == "undefined" && v.is_null() => "null".to_string(),
                v => v.to_string(),
            })
        });
        let _ = reply.send(answer);
    });
}

impl Drop for WebView {
    fn drop(&mut self) {
        // A dialog left waiting is cancelled, so Chromium isn't left holding it.
        self.answer_dialog(false, None);
        self.set_keyboard(false);
        let key = self.key();
        // As the app quits, the tables may be gone: nothing more to do then.
        if PENDING.try_with(|p| p.borrow_mut().retain(|k| *k != key)).is_err() {
            return;
        }
        let browser = {
            let mut i = self.inner.borrow_mut();
            i.closing = true;
            i.queue.clear();
            if i.closed { None } else { i.browser.clone() }
        };
        match browser.and_then(|b| b.host()) {
            Some(host) => host.close_browser(1),
            None => {
                let _ = TABS.try_with(|t| t.borrow_mut().remove(&key));
            }
        }
    }
}

/// The browser fidelity check's reference for this engine (`YH_CEF_REFERENCE=<pages>:<out>`):
/// `layout.html` in a plain page whose window is exactly each viewport, unscaled, as Chrome
/// draws it. Writes `reference.json` (what the page measures) and `reference-<size>.png`, then
/// quits (scripts/browser-fidelity).
pub fn reference(pages: String, out: String, cx: &mut gpui_kit::App) {
    use std::time::{Duration, Instant};
    cx.spawn(async move |cx| {
        let sizes: serde_json::Map<String, Value> = std::fs::read_to_string(format!("{pages}/sizes.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let metrics = std::fs::read_to_string(format!("{pages}/metrics.js")).unwrap_or_default();
        let mut results = serde_json::Map::new();
        for (name, size) in sizes {
            let (w, h) = (size[0].as_f64().unwrap_or(1000.), size[1].as_f64().unwrap_or(700.));
            let web = cx.update(|_| unsafe {
                let window: id = msg_send![class!(NSWindow), alloc];
                // Borderless, buffered, far off screen.
                let window: id = msg_send![window, initWithContentRect: NSRect::new(NSPoint::new(-30_000., -30_000.), NSSize::new(w, h)) styleMask: 0usize backing: 2usize defer: cocoa::base::NO];
                let _: () = msg_send![window, setReleasedWhenClosed: cocoa::base::NO];
                let _: () = msg_send![window, orderFrontRegardless];
                let host: id = msg_send![window, contentView];
                let web = WebView::new(host, None);
                web.set_viewport(None);
                web.set_frame(Some((0., 0., w, h)));
                web.load(&format!("file://{pages}/layout.html?{name}"));
                (web, window as usize)
            });
            let (web, window) = web;
            let started = Instant::now();
            cx.background_executor().timer(Duration::from_millis(300)).await;
            while started.elapsed() < Duration::from_secs(20) && cx.update(|_| web.is_loading()) {
                cx.background_executor().timer(Duration::from_millis(50)).await;
            }
            cx.background_executor().timer(Duration::from_millis(600)).await;
            let (tx, rx) = flume::bounded::<Result<String, String>>(1);
            cx.update(|_| web.evaluate(&metrics, tx));
            if let Ok(Ok(json)) = rx.recv_async().await {
                results.insert(name.clone(), serde_json::from_str(&json).unwrap_or_default());
            }
            let (tx, rx) = flume::bounded::<Result<(), String>>(1);
            cx.update(|_| web.snapshot(format!("{out}/reference-{name}.png"), tx));
            let shot = rx.recv_async().await;
            eprintln!("REFERENCE {name} {w}x{h}: {}", if matches!(shot, Ok(Ok(()))) { "ok" } else { "no screenshot" });
            cx.update(|_| {
                drop(web);
                unsafe {
                    let _: () = msg_send![window as id, orderOut: nil];
                }
            });
        }
        let json = serde_json::to_string_pretty(&Value::Object(results)).unwrap_or_default();
        let _ = std::fs::write(format!("{out}/reference.json"), json);
        eprintln!("REFERENCE done");
        cx.background_executor().timer(Duration::from_millis(500)).await;
        std::process::exit(0);
    })
    .detach();
}
