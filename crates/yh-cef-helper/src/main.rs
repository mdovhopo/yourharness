//! A CEF helper process: loads the framework from where the app found it, then runs as
//! whatever kind of process CEF started it as.
//!
//! The framework isn't in the bundle (it's downloaded on first use), so its folder comes from
//! `--framework-dir-path`, which CEF passes its children, or `YH_CEF_FRAMEWORK`, which they
//! inherit from the app.
//!
//! Chromium's sandbox only lets helpers read the app's own bundle, and the framework is outside
//! it. Each helper gets its sandbox policy from the app over `--seatbelt-client=<fd>`; before
//! Chromium applies it (`cef_sandbox_initialize`), `widen_policy` adds one read-only rule for
//! the framework's folder and hands Chromium the result on a pipe of its own. Nothing else in the
//! policy changes.

use cef::*;
use std::ffi::CString;

fn read_varint(data: &[u8], at: &mut usize) -> Option<u64> {
    let (mut value, mut shift) = (0u64, 0);
    loop {
        let byte = *data.get(*at)?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
        if shift >= 32 {
            return None;
        }
    }
}

fn write_varint(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn read_exact(fd: i32, buf: &mut [u8]) -> bool {
    let mut done = 0;
    while done < buf.len() {
        let n = unsafe { libc::read(fd, buf[done..].as_mut_ptr().cast(), buf.len() - done) };
        if n <= 0 {
            if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return false;
        }
        done += n as usize;
    }
    true
}

/// The sandbox policy Chromium sent (`sandbox/mac/seatbelt_exec.cc`: a u64 length, then
/// `sandbox_serializer.cc`'s encoding: a varint mode, 0 for source, then the profile as a
/// varint-length string, then its parameters), with `(allow file-read* (subpath <dir>))` added
/// at the end of the profile. A compiled policy (the app widened it as Chromium compiled it,
/// `chrome.rs`) goes on as it came. Returns the new policy's read end, or why not.
fn widen_policy(fd: i32, dir: &str) -> Result<i32, String> {
    let mut len = [0u8; 8];
    if !read_exact(fd, &mut len) {
        return Err("no policy length".into());
    }
    let mut policy = vec![0u8; u64::from_ne_bytes(len) as usize];
    if !read_exact(fd, &mut policy) {
        return Err("short policy".into());
    }
    unsafe { libc::close(fd) };
    let mut at = 0;
    let mode = read_varint(&policy, &mut at).ok_or("no mode")?;
    let out = if mode == 0 {
        widened_source(&policy, at, dir)?
    } else {
        policy
    };
    let mut pipe = [0i32; 2];
    if unsafe { libc::pipe(pipe.as_mut_ptr()) } != 0 {
        return Err("pipe failed".into());
    }
    let write_fd = pipe[1];
    // Bigger than a pipe holds: written from a thread while Chromium reads it.
    std::thread::spawn(move || {
        let mut data = (out.len() as u64).to_ne_bytes().to_vec();
        data.extend_from_slice(&out);
        let mut done = 0;
        while done < data.len() {
            let n = unsafe { libc::write(write_fd, data[done..].as_ptr().cast(), data.len() - done) };
            if n <= 0 {
                break;
            }
            done += n as usize;
        }
        unsafe { libc::close(write_fd) };
    });
    Ok(pipe[0])
}

fn widened_source(policy: &[u8], mut at: usize, dir: &str) -> Result<Vec<u8>, String> {
    let profile_len = read_varint(policy, &mut at).ok_or("no profile length")? as usize;
    let profile = policy.get(at..at + profile_len).ok_or("short profile")?;
    let params = &policy[at + profile_len..];
    // SBPL string: backslashes and quotes escaped.
    let quoted = dir.replace('\\', "\\\\").replace('"', "\\\"");
    let mut widened = profile.to_vec();
    widened.extend_from_slice(format!("\n; YourHarness: the downloaded framework, read-only.\n(allow file-read* (subpath \"{quoted}\"))\n").as_bytes());
    let mut out = Vec::with_capacity(policy.len() + 200);
    write_varint(0, &mut out);
    write_varint(widened.len() as u64, &mut out);
    out.extend_from_slice(&widened);
    out.extend_from_slice(params);
    Ok(out)
}

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
            // The policy, widened to the framework's folder (its real path: the sandbox sees those).
            let dir = std::fs::canonicalize(&framework).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| framework.clone());
            let mut argv: Vec<String> = std::env::args().collect();
            for arg in argv.iter_mut() {
                if let Some(fd) = arg.strip_prefix("--seatbelt-client=").and_then(|f| f.parse::<i32>().ok()) {
                    match widen_policy(fd, &dir) {
                        Ok(new_fd) => *arg = format!("--seatbelt-client={new_fd}"),
                        Err(why) => {
                            eprintln!("yh-cef-helper: couldn't widen the sandbox policy: {why}");
                            std::process::exit(1);
                        }
                    }
                }
            }
            let owned: Vec<CString> = argv.iter().map(|a| CString::new(a.as_str()).unwrap_or_default()).collect();
            let mut ptrs: Vec<*mut std::ffi::c_char> = owned.iter().map(|c| c.as_ptr() as *mut _).collect();
            ptrs.push(std::ptr::null_mut());
            _sandbox = init(owned.len() as i32, ptrs.as_mut_ptr());
            if _sandbox.is_null() {
                eprintln!("yh-cef-helper: the sandbox didn't start");
                std::process::exit(1);
            }
            if std::env::var_os("YH_CEF_LOG").is_some() {
                // What the sandbox lets it read: inside the framework yes, next to it no.
                let kind = std::env::args().filter_map(|a| a.strip_prefix("--type=").or_else(|| a.strip_prefix("--utility-sub-type=")).map(str::to_owned)).collect::<Vec<_>>().join(" ");
                let inside = std::fs::read(format!("{dir}/Resources/Info.plist")).is_ok();
                let beside = std::path::Path::new(&dir).parent().map(|p| p.join("CREDITS.html")).map(|p| std::fs::read(p).is_ok()).unwrap_or(false);
                eprintln!("yh-cef-helper sandbox [{kind}]: framework readable {inside} · next to it {beside}");
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
