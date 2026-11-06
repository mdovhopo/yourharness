//! Export State and Import State (Settings): everything the app keeps as one `.yhstate` file,
//! the Swift app's format (`yh_core::state_archive`). Export writes it off the main thread.
//! Import checks the file, says what's in it and asks before replacing anything; the swap
//! happens at the next launch, before the database opens, so the app restarts for it.

use crate::state::workspace::Workspace;
use gpui_kit::{App, Entity, PromptLevel, Window};
use serde_json::{json, Value};
use std::path::PathBuf;

fn app_info() -> Value {
    json!({
        "name": "YourHarness Next",
        "bundleID": "com.mdovhopo.yourharness.next",
        "version": env!("CARGO_PKG_VERSION"),
        "build": Value::Null,
        "commit": option_env!("YH_GIT_COMMIT"),
    })
}

fn say(title: &str, body: &str) {
    eprintln!("STATE {title}: {body}");
    crate::platform::notify(title, body);
}

/// A state's contents in a line: "3 projects, 120 threads, 40,213 rows, 12 memories".
pub fn summary(contents: &Value) -> String {
    let n = |k: &str| contents[k].as_i64().unwrap_or(0);
    let projects = contents["projects"].as_array().map(|a| a.len()).unwrap_or(0);
    format!("{projects} projects, {} threads, {} rows, {} memories, {} attachments", n("threads"), n("entries"), n("memories"), n("attachments"))
}

/// Writes the state to `file`, off the main thread.
pub fn export_to(workspace: &Entity<Workspace>, file: PathBuf, cx: &mut App) {
    let ws = workspace.read(cx);
    let (Some(db), dir) = (ws.library.db.clone(), ws.data_dir.clone()) else { return };
    cx.spawn(async move |_| {
        let f = file.clone();
        let result = std::thread::spawn(move || yh_core::state_archive::export(&db, &dir, app_info(), &f)).join().unwrap_or_else(|_| Err("The export crashed.".into()));
        match result {
            Ok(manifest) => {
                eprintln!("STATE manifest: {}", serde_json::to_string(&manifest).unwrap_or_default());
                say("State exported", &format!("{} ({})", file.display(), summary(&manifest["contents"])));
            }
            Err(e) => say("Couldn't export the state", &e),
        }
    })
    .detach();
}

pub fn export(workspace: Entity<Workspace>, cx: &mut App) {
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default().join("Desktop");
    let name = format!("YourHarness {}.yhstate", yh_core::library::iso_now().get(..10).unwrap_or(""));
    let chosen = cx.prompt_for_new_path(&home, Some(&name));
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(file))) = chosen.await else { return };
        let _ = cx.update(|cx| export_to(&workspace, file, cx));
    })
    .detach();
}

/// Asks for a file, checks it, says what's in it, and with your yes restarts into it.
pub fn import(workspace: Entity<Workspace>, window: &mut Window, cx: &mut App) {
    let chosen = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Import".into()) });
    let window_handle = window.window_handle();
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = chosen.await else { return };
        let Some(file) = paths.into_iter().next() else { return };
        let opened = std::thread::spawn(move || yh_core::state_archive::open(&file)).join().unwrap_or_else(|_| Err("Reading the file crashed.".into()));
        let opened = match opened {
            Ok(o) => o,
            Err(e) => return say("Couldn't import the state", &e),
        };
        let detail = format!(
            "It holds {}, exported {} from {}. Everything YourHarness keeps here is replaced (your current state is moved aside), and the app restarts.",
            summary(&opened.manifest["contents"]),
            opened.manifest["exportedAt"].as_str().unwrap_or("at an unknown time"),
            opened.manifest["system"]["model"].as_str().unwrap_or("another Mac"),
        );
        let answer = window_handle.update(cx, |_, window, cx| window.prompt(PromptLevel::Warning, "Replace this Mac's state with the file's?", Some(&detail), &["Import and Restart", "Cancel"], cx));
        let Ok(answer) = answer else { return opened.discard() };
        if answer.await != Ok(0) {
            return opened.discard();
        }
        let dir = cx.update(|cx| workspace.read(cx).data_dir.clone());
        if let Err(e) = yh_core::state_archive::stage(opened, &dir) {
            return say("Couldn't import the state", &e);
        }
        let _ = cx.update(|cx| restart(&workspace, cx));
    })
    .detach();
}

/// Quits, everything saved, and opens the app again (with a development run's data folder).
pub fn restart(workspace: &Entity<Workspace>, cx: &mut App) {
    workspace.update(cx, |ws, cx| ws.prepare_for_quit(cx));
    if let Some(bundle) = std::env::current_exe().ok().and_then(|p| p.ancestors().nth(3).map(|a| a.to_path_buf())).filter(|b| b.extension().is_some_and(|e| e == "app")) {
        let mut args = vec!["-n".to_string()];
        if std::env::var("YH_DATA_DIR").is_ok() {
            args.insert(0, "-g".into());
        }
        for key in ["YH_DATA_DIR", "YH_PATH", "YH_BACKGROUND", "YH_NOTIFICATIONS"] {
            if let Ok(v) = std::env::var(key) {
                args.push("--env".into());
                args.push(format!("{key}={v}"));
            }
        }
        let command = format!("sleep 1; /usr/bin/open {} {:?}", args.iter().map(|a| format!("{a:?}")).collect::<Vec<_>>().join(" "), bundle.to_string_lossy());
        let _ = std::process::Command::new("/bin/sh").args(["-c", &command]).spawn();
    }
    cx.quit();
}
