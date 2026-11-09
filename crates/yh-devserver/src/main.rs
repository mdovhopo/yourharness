//! `yh-devserver <data folder> [--port 7878] [--lan] [--token <t>]`
//!
//! Serves a scratch data folder to the phone app (docs/mobile.md). Loopback only unless
//! `--lan`, which also requires a token (one is made up when none is given).

use std::hash::{BuildHasher, RandomState};
use std::net::TcpListener;
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut directory, mut port, mut lan, mut token) = (None, 7878u16, false, None);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => port = args.next().and_then(|p| p.parse().ok()).unwrap_or_else(|| usage()),
            "--lan" => lan = true,
            "--token" => token = Some(args.next().unwrap_or_else(|| usage())),
            "-h" | "--help" => usage(),
            _ if directory.is_none() => directory = Some(PathBuf::from(arg)),
            _ => usage(),
        }
    }
    let directory = directory.unwrap_or_else(|| usage());
    let directory = directory.canonicalize().unwrap_or(directory);
    let real = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Library/Application Support/YourHarness");
    if real.canonicalize().is_ok_and(|r| r == directory) {
        eprintln!("yh-devserver: that's your real data folder; give it a scratch copy (cp -c -R …)");
        std::process::exit(2);
    }
    // Started before the app has made its database: wait for it.
    if !directory.join("harness.sqlite").exists() {
        eprintln!("yh-devserver: waiting for {}/harness.sqlite", directory.display());
        while !directory.join("harness.sqlite").exists() {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    if lan && token.is_none() {
        token = Some(format!("{:016x}", RandomState::new().hash_one(std::process::id())));
    }
    let bind = if lan { "0.0.0.0" } else { "127.0.0.1" };
    let listener = TcpListener::bind((bind, port)).unwrap_or_else(|e| {
        eprintln!("yh-devserver: can't listen on {bind}:{port}: {e}");
        std::process::exit(1);
    });
    let query = token.as_ref().map(|t| format!("?token={t}")).unwrap_or_default();
    eprintln!("yh-devserver: serving {} on ws://{bind}:{port}/{query}", directory.display());
    let name = std::process::Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Mac".into());
    if let Err(e) = yh_devserver::serve(listener, directory, yh_devserver::Options { token, name }) {
        eprintln!("yh-devserver: {e}");
        std::process::exit(1);
    }
}

fn usage() -> ! {
    eprintln!("usage: yh-devserver <data folder> [--port 7878] [--lan] [--token <t>]");
    std::process::exit(2);
}
