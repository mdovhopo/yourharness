//! A CEF helper process: loads the framework from where the app found it, then runs as
//! whatever kind of process CEF started it as.
//!
//! The framework isn't in the bundle (it's downloaded on first use), so its folder comes from
//! `--framework-dir-path`, which CEF passes its children, or `YH_CEF_FRAMEWORK`, which they
//! inherit from the app.

use cef::*;
use std::ffi::CString;

fn main() {
    let from_switch = std::env::args().find_map(|a| a.strip_prefix("--framework-dir-path=").map(str::to_owned));
    let framework = from_switch.clone().or_else(|| std::env::var("YH_CEF_FRAMEWORK").ok().map(|d| format!("{d}/Chromium Embedded Framework.framework")));
    let Some(framework) = framework else {
        eprintln!("yh-cef-helper: no framework path");
        std::process::exit(1);
    };
    if std::env::var_os("YH_CEF_LOG").is_some() {
        eprintln!("yh-cef-helper args: {:?}", std::env::args().skip(1).filter(|a| !a.starts_with("--field-trial") && !a.starts_with("--enable-features") && !a.starts_with("--disable-features")).collect::<Vec<_>>());
        eprintln!("yh-cef-helper: framework from {} ({framework})", if from_switch.is_some() { "--framework-dir-path" } else { "YH_CEF_FRAMEWORK" });
    }
    // The sandbox first, before anything else is loaded (Chromium's seatbelt, from the framework's
    // own library); the app turns it off for a run with `--no-sandbox`.
    let args = cef::args::Args::new();
    let sandboxed = !std::env::args().any(|a| a == "--no-sandbox");
    let mut _sandbox = std::ptr::null_mut();
    if sandboxed {
        let lib = CString::new(format!("{framework}/Libraries/libcef_sandbox.dylib")).unwrap_or_default();
        unsafe {
            let handle = libc::dlopen(lib.as_ptr(), libc::RTLD_NOW);
            let init = if handle.is_null() { std::ptr::null_mut() } else { libc::dlsym(handle, c"cef_sandbox_initialize".as_ptr()) };
            if init.is_null() {
                eprintln!("yh-cef-helper: no sandbox library in {framework}");
                std::process::exit(1);
            }
            let init: extern "C" fn(i32, *mut *mut std::ffi::c_char) -> *mut std::ffi::c_void = std::mem::transmute(init);
            let main_args = args.as_main_args();
            _sandbox = init(main_args.argc, main_args.argv);
            if _sandbox.is_null() {
                eprintln!("yh-cef-helper: the sandbox didn't start");
                std::process::exit(1);
            }
        }
    }
    let Ok(binary) = CString::new(format!("{framework}/Chromium Embedded Framework")) else { std::process::exit(1) };
    if load_library(Some(unsafe { &*binary.as_ptr() })) != 1 {
        eprintln!("yh-cef-helper: couldn't load {framework}");
        std::process::exit(1);
    }
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    std::process::exit(execute_process(Some(args.as_main_args()), None::<&mut App>, std::ptr::null_mut()));
}
