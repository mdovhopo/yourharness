//! Export State and Import State (Settings, File): everything the app keeps as one `.yhstate`
//! file, in the format 0.0.x wrote (`yh_core::state_archive`). Export asks whether the browser's
//! sign-ins go too, writes the file off the main thread and says when it's done, with Show in
//! Finder. Import checks the file and shows what you have now against what's in it, with "Save
//! a copy first", before replacing anything; the swap happens at the next launch, before the
//! database opens, so the app restarts for it.

use crate::state::workspace::Workspace;
use crate::theme::{radius, space, text, Theme};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::{Disableable, Icon, IconName, WindowExt};
use gpui_kit::{div, prelude::*, px, AnyWindowHandle, App, Context, Entity, FontWeight, PromptLevel, Window};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use yh_core::cookies::Cookie;

/// This app as its bundle says: name, ID, version, build and commit (`YHGitCommit`, written by
/// scripts/bundle.sh). A bare build says what it can.
fn app_info() -> Value {
    let bundle = std::env::current_exe().ok().and_then(|p| p.ancestors().nth(3).map(|a| a.to_path_buf())).filter(|b| b.extension().is_some_and(|e| e == "app"));
    let read = |key: &str| -> Option<String> {
        let plist = bundle.as_ref()?.join("Contents/Info.plist");
        let out = std::process::Command::new("/usr/bin/plutil").args(["-extract", key, "raw", "-o", "-"]).arg(plist).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|v| !v.is_empty())
    };
    json!({
        "name": read("CFBundleName").unwrap_or_else(|| "YourHarness".into()),
        "bundleID": read("CFBundleIdentifier"),
        "version": read("CFBundleShortVersionString").unwrap_or_else(|| env!("CARGO_PKG_VERSION").into()),
        "build": read("CFBundleVersion"),
        "commit": read("YHGitCommit"),
    })
}

fn say(title: &str, body: &str) {
    eprintln!("STATE {title}: {body}");
}

/// A state's contents in a line: "3 projects, 120 threads, 40,213 rows, 12 memories".
pub fn summary(contents: &Value) -> String {
    let n = |k: &str| contents[k].as_i64().unwrap_or(0);
    let projects = contents["projects"].as_array().map(|a| a.len()).unwrap_or(0);
    let mut s = format!("{projects} projects, {} threads, {} rows, {} memories, {} attachments", n("threads"), n("entries"), n("memories"), n("attachments"));
    if let Some(c) = contents["browserCookies"].as_i64() {
        s.push_str(&format!(", {c} browser cookies"));
    }
    s
}

/// Every project's browser cookies that has any, by project ID: read from WebKit, which
/// answers on the main thread, so this waits off it.
async fn browser_cookies(projects: Vec<String>, cx: &mut gpui_kit::AsyncApp) -> BTreeMap<String, Vec<Cookie>> {
    let mut all = BTreeMap::new();
    for p in projects {
        let (tx, rx) = flume::bounded(1);
        let id = p.clone();
        let _ = cx.update(|_| crate::webview::read_cookies(&id, tx));
        let got = futures_timeout(rx).await;
        if !got.is_empty() {
            all.insert(p, got);
        }
    }
    all
}

async fn futures_timeout(rx: flume::Receiver<Vec<Cookie>>) -> Vec<Cookie> {
    rx.recv_async().await.unwrap_or_default()
}

/// Writes the state to `file`, off the main thread, then says so (over `window`, with Show in
/// Finder, when there is one).
pub fn export_to(workspace: &Entity<Workspace>, file: PathBuf, sign_ins: bool, window: Option<AnyWindowHandle>, cx: &mut App) {
    let ws = workspace.read(cx);
    let (Some(db), dir) = (ws.library.db.clone(), ws.data_dir.clone()) else { return };
    let projects: Vec<String> = ws.projects.iter().map(|p| p.id.clone()).filter(|id| !id.starts_with("dir:")).collect();
    cx.spawn(async move |cx| {
        let cookies = if sign_ins { Some(browser_cookies(projects, cx).await) } else { None };
        let f = file.clone();
        let result = cx.background_executor().spawn(async move { yh_core::state_archive::export(&db, &dir, app_info(), cookies.as_ref(), &f) }).await;
        let (title, detail) = match &result {
            Ok(manifest) => {
                eprintln!("STATE manifest: {}", serde_json::to_string(manifest).unwrap_or_default());
                let mut d = format!("{}.", summary(&manifest["contents"]));
                if manifest["contents"]["browserCookies"].is_number() {
                    d.push_str("\n\nIt includes your browser sign-ins: anyone with the file could use them.");
                }
                ("State exported".to_string(), d)
            }
            Err(e) => ("Couldn't export the state".to_string(), e.clone()),
        };
        say(&title, &detail);
        let Some(window) = window else { return };
        let ok = result.is_ok();
        let buttons: &[&str] = if ok { &["OK", "Show in Finder"] } else { &["OK"] };
        if let Ok(answer) = std::env::var("YH_STATE_DONE_ANSWER") {
            eprintln!("STATE asks: {title} / {detail} -> {answer}");
            return;
        }
        let Ok(answer) = window.update(cx, |_, window, cx| window.prompt(if ok { PromptLevel::Info } else { PromptLevel::Critical }, &title, Some(&detail), buttons, cx)) else { return };
        if answer.await == Ok(1) {
            let _ = cx.update(|cx| cx.reveal_path(&file));
        }
    })
    .detach();
}

/// Export State…: whether the browser's sign-ins go too, then where.
pub fn export(workspace: Entity<Workspace>, window: &mut Window, cx: &mut App) {
    let view = cx.new(|_| ExportOptions { workspace, sign_ins: std::env::var("YH_STATE_EXPORT_SIGNINS").as_deref() == Ok("1") });
    window.open_dialog(cx, move |d, _, _| d.title("Export State").w(px(460.)).child(view.clone()));
}

struct ExportOptions {
    workspace: Entity<Workspace>,
    sign_ins: bool,
}

impl ExportOptions {
    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.close_dialog(cx);
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default().join("Desktop");
        let name = format!("YourHarness {}.yhstate", yh_core::library::iso_now().get(..10).unwrap_or(""));
        let chosen = cx.prompt_for_new_path(&home, Some(&name));
        let (workspace, sign_ins, handle) = (self.workspace.clone(), self.sign_ins, window.window_handle());
        cx.spawn(async move |_, cx| {
            let Ok(Ok(Some(file))) = chosen.await else { return };
            let _ = cx.update(|cx| export_to(&workspace, file, sign_ins, Some(handle), cx));
        })
        .detach();
    }
}

impl Render for ExportOptions {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        div()
            .flex()
            .flex_col()
            .gap(px(space::M))
            .child(div().text_color(theme.secondary).child("Every project, thread, memory and setting, with details of this Mac and app, in one file: for another Mac, or for reproducing a bug exactly."))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(Checkbox::new("export-sign-ins").label("Include browser sign-ins (cookies)").checked(self.sign_ins).on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.sign_ins = *on;
                        cx.notify();
                    })))
                    .child(div().pl(px(22.)).text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child("Anyone with the file could use them. Leave this off for a file you'll share.")),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(space::S))
                    .child(Button::new("export-cancel").outline().label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(Button::new("export-choose").primary().label("Export…").on_click(cx.listener(|this, _, window, cx| this.choose(window, cx)))),
            )
    }
}

/// Asks for a file, checks it, and shows Import State's question for it.
pub fn import(workspace: Entity<Workspace>, window: &mut Window, cx: &mut App) {
    let chosen = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Import".into()) });
    let window_handle = window.window_handle();
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = chosen.await else { return };
        let Some(file) = paths.into_iter().next() else { return };
        let _ = window_handle.update(cx, |_, window, cx| ask_import(workspace, file, window, cx));
    })
    .detach();
}

/// Import State's question for `file` (`YH_STATE_IMPORT_ASK=<file>` opens it at launch): what's
/// in it against what's here, and that there's no undo.
pub fn ask_import(workspace: Entity<Workspace>, file: PathBuf, window: &mut Window, cx: &mut App) {
    let f = file.clone();
    let opened = std::thread::spawn(move || yh_core::state_archive::open(&f)).join().unwrap_or_else(|_| Err("Reading the file crashed.".into()));
    let opened = match opened {
        Ok(o) => o,
        Err(e) => {
            say("Couldn't import the state", &e);
            let _ = window.prompt(PromptLevel::Critical, "Couldn't import the state", Some(&e), &["OK"], cx);
            return;
        }
    };
    let ws = workspace.read(cx);
    let current = yh_core::state_archive::contents(&ws.data_dir);
    let working = ws.threads.iter().filter(|t| t.read(cx).run.running).count();
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let view = cx.new(|_| ImportSheet { workspace, opened: Some(opened), name, current, working, keep_copy: true, busy: None, problem: None });
    if std::env::var("YH_STATE_IMPORT_ASK").is_ok() {
        eprintln!("STATE asks to import {}", file.display());
    }
    let v = view.clone();
    window.open_dialog(cx, move |d, _, _| d.w(px(540.)).overlay_closable(false).child(v.clone()));
    if std::env::var("YH_STATE_IMPORT_CONFIRM").as_deref() == Ok("1") {
        cx.spawn(async move |cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
            let _ = cx.update(|cx| view.update(cx, |s, cx| s.confirm(cx)));
        })
        .detach();
    }
}

struct ImportSheet {
    workspace: Entity<Workspace>,
    opened: Option<yh_core::state_archive::Opened>,
    name: String,
    current: Value,
    working: usize,
    keep_copy: bool,
    busy: Option<&'static str>,
    problem: Option<String>,
}

impl ImportSheet {
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(o) = self.opened.take() {
            o.discard();
        }
        window.close_dialog(cx);
    }

    /// Replace Everything and Restart: a copy of what's here first if asked (into `backups/`,
    /// sign-ins included), then the file staged and the app restarted into it.
    fn confirm(&mut self, cx: &mut Context<Self>) {
        let Some(opened) = self.opened.take() else { return };
        let ws = self.workspace.read(cx);
        let (db, dir) = (ws.library.db.clone(), ws.data_dir.clone());
        let projects: Vec<String> = ws.projects.iter().map(|p| p.id.clone()).filter(|id| !id.starts_with("dir:")).collect();
        let keep = self.keep_copy;
        self.busy = Some(if keep { "Saving a copy…" } else { "Importing…" });
        cx.notify();
        let workspace = self.workspace.clone();
        cx.spawn(async move |this, cx| {
            if keep {
                if let Some(db) = db.clone() {
                    let cookies = browser_cookies(projects, cx).await;
                    let backups = dir.join("backups");
                    let file = backups.join(format!("Before import {}.yhstate", yh_core::library::iso_now().replace(':', "-")));
                    let d = dir.clone();
                    let saved = cx
                        .background_executor()
                        .spawn(async move {
                            std::fs::create_dir_all(&backups).map_err(|e| e.to_string())?;
                            yh_core::state_archive::export(&db, &d, app_info(), Some(&cookies), &file).map(|_| file)
                        })
                        .await;
                    match saved {
                        Ok(f) => eprintln!("STATE kept a copy in {}", f.display()),
                        Err(e) => {
                            let _ = this.update(cx, |s, cx| {
                                s.problem = Some(format!("Couldn't save a copy, so nothing was replaced: {e}"));
                                s.busy = None;
                                cx.notify();
                            });
                            opened.discard();
                            return;
                        }
                    }
                }
            }
            let d = dir.clone();
            let staged = cx.background_executor().spawn(async move { yh_core::state_archive::stage(opened, &d) }).await;
            if let Err(e) = staged {
                let _ = this.update(cx, |s, cx| {
                    s.problem = Some(e);
                    s.busy = None;
                    cx.notify();
                });
                return;
            }
            let _ = cx.update(|cx| restart(&workspace, cx));
        })
        .detach();
    }
}

impl Render for ImportSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let Some(opened) = &self.opened else {
            return div().p(px(space::L)).child(self.busy.unwrap_or("Importing…")).into_any_element();
        };
        let m = &opened.manifest;
        let file = &m["contents"];
        let now = &self.current;
        let origin = {
            let app = &m["app"];
            let mut t = format!("Exported from {} {}", app["name"].as_str().unwrap_or("YourHarness"), app["version"].as_str().unwrap_or("of an unknown version"));
            if let Some(b) = app["build"].as_str().or(app["build"].as_i64().map(|_| "")).filter(|b| !b.is_empty()) {
                t.push_str(&format!(" ({b})"));
            }
            if let Some(at) = m["exportedAt"].as_str().and_then(yh_core::library::parse_iso) {
                t.push_str(&format!(" on {}", yh_core::clock::short_date_time(at)));
            }
            if let Some(model) = m["system"]["model"].as_str() {
                t.push_str(&format!(", on a {model}"));
            }
            format!("{t} with {}.", m["system"]["os"].as_str().unwrap_or("macOS"))
        };
        let count = |v: &Value, k: &str| v[k].as_i64().unwrap_or(0).to_string();
        let projects = |v: &Value| v["projects"].as_array().map(|a| a.len()).unwrap_or(0).to_string();
        let bytes = |v: &Value| {
            let b = v["bytes"].as_f64().unwrap_or(0.);
            if b >= 1e9 { format!("{:.1} GB", b / 1e9) } else if b >= 1e6 { format!("{:.1} MB", b / 1e6) } else { format!("{:.0} KB", b / 1e3) }
        };
        let cookies = file["browserCookies"].as_i64();
        let grid_row = |label: &str, a: String, b: String, header: bool| {
            div()
                .flex()
                .gap(px(space::L))
                .py(px(4.))
                .when(header, |d| d.text_size(px(text::CAPTION)).font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary))
                .when(!header, |d| d.border_t_1().border_color(theme.separator))
                .child(div().w(px(170.)).child(label.to_string()))
                .child(div().w(px(130.)).child(a))
                .child(div().flex_1().child(b))
        };
        let names: Vec<String> = file["projects"].as_array().map(|a| a.iter().filter_map(|p| p.as_str().map(str::to_string)).collect()).unwrap_or_default();
        let note = |icon: IconName, t: String| div().flex().items_start().gap(px(6.)).text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(div().mt(px(2.)).child(Icon::new(icon).size(px(13.)).text_color(theme.secondary))).child(div().flex_1().min_w_0().child(t));
        let busy = self.busy;
        div()
            .flex()
            .flex_col()
            .gap(px(space::L))
            .child(
                div()
                    .flex()
                    .gap(px(space::M + 2.))
                    .child(Icon::new(IconName::TriangleAlert).size(px(28.)).text_color(theme.warning))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child("Replace everything with this state?"))
                            .child(div().w_full().child(format!("Every project, thread, memory and setting you have now is replaced by the ones in “{}”. This is destructive and can't be undone.", self.name)))
                            .child(div().w_full().text_color(theme.secondary).child(origin)),
                    ),
            )
            .child(
                div()
                    .p(px(space::M))
                    .rounded(px(radius::MEDIUM + 1.))
                    .bg(theme.card)
                    .border_1()
                    .border_color(theme.separator)
                    .text_size(px(text::FOOTNOTE))
                    .child(grid_row("", "You have now".into(), "In the file".into(), true))
                    .child(grid_row("Projects", projects(now), projects(file), false))
                    .child(grid_row("Threads", count(now, "threads"), count(file, "threads"), false))
                    .child(grid_row("Memories and tasks", count(now, "memories"), count(file, "memories"), false))
                    .child(grid_row("Images", count(now, "attachments"), count(file, "attachments"), false))
                    .child(grid_row("Size", bytes(now), bytes(file), false))
                    .child(grid_row("Browser sign-ins", if cookies.is_some() { "Replaced".into() } else { "Kept".into() }, cookies.map_or("Not in the file".into(), |c| format!("{c} cookie{}", if c == 1 { "" } else { "s" })), false)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .when(!names.is_empty(), |d| d.child(note(IconName::Folder, format!("Projects in the file: {}.", names.join(", ")))))
                    .when(self.working > 0, |d| d.child(note(IconName::CircleX, format!("{} working now. They're stopped.", if self.working == 1 { "1 thread is".to_string() } else { format!("{} threads are", self.working) }))))
                    .child(note(IconName::RefreshCw, "YourHarness restarts to bring it in. Your git worktrees and project folders aren't touched.".into())),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(Checkbox::new("import-keep-copy").label("Save a copy of my current state first").checked(self.keep_copy).disabled(busy.is_some()).on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.keep_copy = *on;
                        cx.notify();
                    })))
                    .child(div().pl(px(22.)).text_size(px(text::CAPTION)).text_color(theme.secondary).child("Into “backups” in YourHarness's data folder, with your browser sign-ins, so Import State can bring it back.")),
            )
            .when_some(self.problem.clone(), |d, p| d.child(div().text_size(px(text::FOOTNOTE)).text_color(theme.error).child(p)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S))
                    .when_some(busy, |d, b| d.child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(b)))
                    .child(div().flex_1())
                    .child(Button::new("import-cancel").outline().label("Cancel").disabled(busy.is_some()).on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx))))
                    // Not the default button: Return mustn't replace everything.
                    .child(Button::new("import-replace").danger().label("Replace Everything and Restart").disabled(busy.is_some()).on_click(cx.listener(|this, _, _, cx| this.confirm(cx)))),
            )
            .into_any_element()
    }
}

/// After an import with sign-ins: each of the file's projects' browser gets exactly the file's.
pub fn apply_cookies(workspace: &Entity<Workspace>, cookies: BTreeMap<String, Vec<Cookie>>, cx: &mut App) {
    let projects: Vec<String> = workspace.read(cx).projects.iter().map(|p| p.id.clone()).filter(|id| !id.starts_with("dir:")).collect();
    for p in &projects {
        crate::webview::clear_store(p);
    }
    cx.spawn(async move |cx| {
        // Once the stores are cleared.
        cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
        let mut total = 0;
        for p in projects {
            let list = cookies.get(&p).cloned().unwrap_or_default();
            if list.is_empty() {
                continue;
            }
            let (tx, rx) = flume::bounded(1);
            let _ = cx.update(|_| crate::webview::store_cookies(&p, list, tx));
            total += rx.recv_async().await.unwrap_or(0);
        }
        eprintln!("STATE sign-ins put back: {total} cookies");
    })
    .detach();
}

/// Quits, everything saved, and opens the app again (with a development run's data folder,
/// without the state hooks, which would import again), once this process is gone.
pub fn restart(workspace: &Entity<Workspace>, cx: &mut App) {
    workspace.update(cx, |ws, cx| ws.prepare_for_quit(cx));
    if let Some(bundle) = std::env::current_exe().ok().and_then(|p| p.ancestors().nth(3).map(|a| a.to_path_buf())).filter(|b| b.extension().is_some_and(|e| e == "app")) {
        let mut args: Vec<String> = vec!["-n".into()];
        if std::env::var("YH_DATA_DIR").is_ok() {
            args.insert(0, "-g".into());
        }
        let mut vars: Vec<(String, String)> = std::env::vars().filter(|(k, _)| k.starts_with("YH_") && !k.starts_with("YH_STATE_")).collect();
        vars.sort();
        for (k, v) in vars {
            args.push("--env".into());
            args.push(format!("{k}={v}"));
        }
        let script = "pid=$1 app=$2; shift 2\ni=0\nwhile kill -0 \"$pid\" 2>/dev/null; do sleep 0.1; i=$((i+1)); [ $i -gt 600 ] && exit 1; done\nopen \"$@\" \"$app\"";
        let mut command = vec!["-c".to_string(), script.into(), "sh".into(), std::process::id().to_string(), bundle.to_string_lossy().into_owned()];
        command.extend(args);
        let _ = std::process::Command::new("/bin/sh").args(&command).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
    }
    crate::ui::jobs::quit_now(cx);
}
