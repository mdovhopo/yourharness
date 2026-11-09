//! YourHarness: a native harness over AI coding CLIs, in Rust on GPUI. See docs/architecture.md:
//! `state` owns what the app knows, `ui` draws it, `platform` is the AppKit edge, `probe` the
//! performance run.

mod platform;
mod probe;
mod state;
mod theme;
mod ui;
mod webview;
mod keep_awake;
mod browser;
mod video_thumb;

use gpui_kit::{
    point, prelude::*, px, size, App, Bounds, Focusable, KeyBinding, TitlebarOptions,
    WindowBounds, WindowOptions,
};
use state::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;
use ui::root::*;

/// A state was imported at this launch: its Home is fixed up and its sign-ins go to the browser.
static IMPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The main window: closing it quits.
struct MainWindow(gpui_kit::WindowId);

impl gpui_kit::Global for MainWindow {}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn main() {
    // A test run parks the window off screen and still needs frames.
    if env("YH_PERF").is_some() || env("YH_BACKGROUND").as_deref() == Some("1") {
        // SAFETY: first thing in main, before any thread starts.
        unsafe { std::env::set_var("GPUI_KEEP_DRAWING", "1") };
    }
    if env("YH_PERF").is_some() && env("YH_APP_NAP").as_deref() != Some("1") {
        platform::hold_off_app_nap();
    }
    // A state imported (Settings → Import, or `YH_STATE_IMPORT=<file>` on a development run's
    // data folder) is swapped in now, before the database opens.
    {
        let dir = yh_core::library::data_directory();
        if let (Some(file), true) = (env("YH_STATE_IMPORT"), env("YH_DATA_DIR").is_some()) {
            match yh_core::state_archive::open(std::path::Path::new(&file)).and_then(|o| yh_core::state_archive::stage(o, &dir)) {
                Ok(()) => eprintln!("STATE staged {file}"),
                Err(e) => eprintln!("STATE import failed: {e}"),
            }
        }
        if let Some(manifest) = yh_core::state_archive::apply_pending(&dir) {
            eprintln!("STATE imported: {}", ui::state_transfer::summary(&manifest["contents"]));
            IMPORTED.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    // The login shell's environment for `claude` and its tools, read aside (`YH_PATH` skips it).
    yh_core::shell_env::start();
    let mut library = yh_core::library::Library::open();
    if IMPORTED.load(std::sync::atomic::Ordering::SeqCst) {
        let home = library.fix_imported_home(&yh_core::library::data_directory(), env("YH_DATA_DIR").is_some());
        eprintln!("STATE Home runs in {}", home.unwrap_or_default());
    }
    let library = Arc::new(library);
    theme::set_chosen_appearance(library.setting("appearance").and_then(|v| v.as_str().map(str::to_string)));

    let application = gpui_kit::application().with_assets(gpui_kit::assets::Assets);
    // The Dock icon clicked with the app open: its window comes forward (out of the Dock too).
    application.on_reopen(|cx| {
        cx.activate(true);
        if let Some(w) = cx.windows().into_iter().next() {
            let _ = w.update(cx, |_, window, _| window.activate_window());
        }
    });
    application.run(move |cx: &mut App| {
        gpui_kit::init(cx);
        cx.bind_keys([
            KeyBinding::new("cmd-.", Stop, None),
            KeyBinding::new("cmd-enter", Allow, Some("Harness")),
            KeyBinding::new("cmd-backspace", Deny, Some("Harness")),
            KeyBinding::new("cmd-n", NewThread, None),
            KeyBinding::new("cmd-f", Find, None),
            KeyBinding::new("alt-cmd-0", ui::root::ToggleInspector, None),
            KeyBinding::new("ctrl-cmd-s", ui::root::ToggleSidebar, None),
            KeyBinding::new("shift-cmd-m", ui::root::OpenMemory, None),
            KeyBinding::new("shift-cmd-u", ui::root::ToggleUsage, None),
            KeyBinding::new("cmd-s", crate::ui::memory::SaveMemory, Some("MemoryPage")),
            KeyBinding::new("cmd-,", OpenSettings, None),
            KeyBinding::new("cmd-k", OpenSpotlight, None),
            KeyBinding::new("cmd-g", FindNext, None),
            KeyBinding::new("cmd-shift-g", FindPrevious, None),
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("alt-cmd-up", ui::root::PreviousThread, None),
            KeyBinding::new("alt-cmd-down", ui::root::NextThread, None),
            KeyBinding::new("cmd-[", ui::root::PreviousMessage, None),
            KeyBinding::new("cmd-]", ui::root::NextMessage, None),
            KeyBinding::new("shift-cmd-[", ui::root::TopOfThread, None),
            KeyBinding::new("shift-cmd-]", ui::root::EndOfThread, None),
            KeyBinding::new("cmd-o", ui::root::AddProject, None),
            KeyBinding::new("shift-cmd-o", ui::root::ResumeSession, None),
        ]);
        // The standard app and window keys. ⌘Z outside a text
        // field undoes the last thread action; in one, the root decides (`RootView`'s Undo).
        cx.bind_keys([
            KeyBinding::new("cmd-z", ui::root::UndoThreadAction, None),
            KeyBinding::new("cmd-w", ui::root::CloseWindow, None),
            KeyBinding::new("cmd-h", ui::root::HideApp, None),
            KeyBinding::new("alt-cmd-h", ui::root::HideOthers, None),
            KeyBinding::new("cmd-m", ui::root::Minimize, None),
            KeyBinding::new("ctrl-cmd-f", ui::root::ToggleFullScreen, None),
            KeyBinding::new("escape", ui::usage::CloseUsage, Some("UsagePage")),
        ]);
        // Asks first while background jobs run (`ui::jobs::guard_quit`).
        cx.on_action(|_: &Quit, cx| cx.defer(ui::jobs::quit));
        cx.on_action(|_: &ui::root::About, _| platform::show_about_panel());
        cx.on_action(|_: &ui::root::HideApp, cx| cx.hide());
        cx.on_action(|_: &ui::root::HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &ui::root::ShowAll, cx| cx.unhide_other_apps());
        cx.on_action(|_: &ui::root::BringAllToFront, _| platform::bring_all_to_front());
        cx.on_action(|_: &ui::root::OpenHelp, cx| cx.open_url(ui::menus::HELP_URL));
        cx.on_action(|_: &ui::root::Minimize, cx| {
            if let Some(w) = cx.active_window() {
                let _ = w.update(cx, |_, window, _| window.minimize_window());
            }
        });
        cx.on_action(|_: &ui::root::Zoom, cx| {
            if let Some(w) = cx.active_window() {
                let _ = w.update(cx, |_, window, _| window.zoom_window());
            }
        });
        cx.on_action(|_: &ui::root::ToggleFullScreen, cx| {
            if let Some(w) = cx.active_window() {
                let _ = w.update(cx, |_, window, _| window.toggle_fullscreen());
            }
        });
        // ⌘W: the main window's close quits (as its close button does); another window closes.
        cx.on_action(|_: &ui::root::CloseWindow, cx| {
            let Some(w) = cx.active_window() else { return };
            if cx.try_global::<MainWindow>().is_some_and(|m| m.0 == w.window_id()) {
                return cx.defer(ui::jobs::quit);
            }
            let _ = w.update(cx, |_, window, _| window.remove_window());
        });
        ui::menus::set(&ui::menus::MenuState::default(), cx);

        let background = env("YH_BACKGROUND").as_deref() == Some("1") || env("YH_PERF").is_some();
        // Where the window was last time, if that's still on a screen.
        let saved = library.setting("windowFrame").and_then(|v| {
            let a: Vec<f32> = v.as_array()?.iter().filter_map(|x| x.as_f64().map(|x| x as f32)).collect();
            (a.len() == 4 && a[2] >= 640. && a[3] >= 420.).then(|| Bounds::new(point(px(a[0]), px(a[1])), size(px(a[2]), px(a[3]))))
        });
        let on_screen = |b: &Bounds<gpui_kit::Pixels>| cx.displays().iter().any(|d| d.bounds().intersects(b));
        let bounds = saved
            .filter(|b| env("YH_PERF").is_none() && on_screen(b))
            .unwrap_or_else(|| Bounds::centered(None, size(px(1280.), px(820.)), cx));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("YourHarness".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(14.), px(14.))),
            }),
            focus: !background,
            show: true,
            window_min_size: Some(size(px(640.), px(420.))),
            // GPUI draws an inactive window at 30 fps to save power. A perf run's window is never
            // active (it's parked off screen), and it measures the active case.
            inactive_frame_interval: if env("YH_PERF").is_some() { None } else { WindowOptions::default().inactive_frame_interval },
            ..Default::default()
        };
        let library = library.clone();
        let (window, root) = gpui_kit::open_window(options, cx, |window, cx| {
            theme::apply_appearance(window.appearance(), window, cx);
            let workspace = cx.new(|cx| Workspace::new(library.clone(), cx));
            cx.new(|cx| RootView::new(workspace, window, cx))
        })
        .expect("window");
        let workspace = root.read(cx).workspace.clone();
        cx.set_global(MainWindow(window.window_id()));
        // Closing the window quits the app,
        // through the same question as ⌘Q while jobs run: the window stays until it's answered.
        let _ = window.update(cx, |_, window, cx| {
            window.on_window_should_close(cx, |_, cx| {
                // Asked once this window's update is over (the question is a sheet on it).
                cx.defer(ui::jobs::quit);
                false
            });
            // A window saved bigger than its screen comes back fitted to it.
            if !background && platform::fit_on_screen(window) {
                eprintln!("WINDOW fitted on screen");
            }
        });
        ui::menus::follow(workspace.clone(), root.read(cx).find.clone(), cx);
        // The agents' browser: pages laid over this window.
        {
            let (ws, inspector) = (workspace.clone(), root.read(cx).inspector.clone());
            let host = window.update(cx, |_, window, _| platform::ns_view(window)).unwrap_or(std::ptr::null_mut());
            let library = workspace.read(cx).library.clone();
            browser::start(
                host,
                Some(library),
                move |thread, cx| {
                    // In its own window, that window comes forward instead.
                    if ui::browser_window::is_open(thread) {
                        return ui::browser_window::activate(thread, cx);
                    }
                    ws.update(cx, |ws, cx| {
                        let auto_show = ws.library.setting("browserAutoShow").and_then(|v| v.as_bool()).unwrap_or(true);
                        if auto_show && ws.selected.as_deref() == Some(thread) {
                            ws.show_panel(state::workspace::Panel::Browser, cx);
                        }
                    })
                },
                move |cx| {
                    inspector.update(cx, |_, cx| cx.notify());
                    ui::browser_panel::notify_all(cx);
                    ui::browser_window::notify_all(cx);
                },
                cx,
            );
        }

        {
            let root = root.clone();
            let _ = window.update(cx, |_, window, cx| {
                // gpui-component sets its theme up once the window's Root is made: ours on top.
                theme::apply_appearance(window.appearance(), window, cx);
                let input = root.read(cx).composer.read(cx).input.clone();
                input.update(cx, |i, cx| i.focus(window, cx));
                window
                    .observe_window_appearance(|window, cx| {
                        theme::apply_appearance(window.appearance(), window, cx);
                        cx.refresh_windows();
                    })
                    .detach();
                if background {
                    platform::order_back(window);
                } else {
                    cx.activate(true);
                }
                let _ = root.read(cx).focus_handle(cx);
            });
        }

        // `YH_STATE_IMPORT_ASK=<file>`: Import State's question for it, after `YH_DRAFT_DELAY`
        // (`YH_STATE_IMPORT_CONFIRM=1` then says yes, keeping a copy, and restarts).
        if let Some(file) = env("YH_STATE_IMPORT_ASK") {
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let workspace = workspace.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let _ = window.update(cx, |_, window, cx| ui::state_transfer::ask_import(workspace.clone(), std::path::PathBuf::from(&file), window, cx));
            })
            .detach();
        }
        // An import's sign-ins: each of its projects' browser gets exactly the file's.
        if let Some(cookies) = yh_core::state_archive::take_cookies(&yh_core::library::data_directory()) {
            ui::state_transfer::apply_cookies(&workspace, cookies, cx);
        }
        ui::jobs::guard_quit(workspace.clone(), window.into(), cx);
        cx.on_app_quit({
            let workspace = workspace.clone();
            move |cx| {
                browser::Browsers::save_now();
                workspace.update(cx, |ws, cx| ws.prepare_for_quit(cx));
                async {}
            }
        })
        .detach();

        // The thread that was open at quit opens again (not in perf runs and probes, which start
        // from nothing selected).
        let probing = std::env::vars().any(|(k, _)| k == "YH_PERF" || k.ends_with("_PROBE"));
        if !probing {
            workspace.update(cx, |ws, cx| {
                if let Some(id) = ws.library.setting("selectedThreadID").and_then(|v| v.as_str().map(str::to_string)) {
                    if ws.page.is_none() {
                        ws.select(&id, cx);
                    }
                }
            });
        }
        // `YH_BROWSER_OPEN=<url>`: a page in the open thread's browser at launch, as you'd open it.
        if let Some(url) = env("YH_BROWSER_OPEN") {
            let found = workspace.read(cx).selected_thread().map(|t| (t.read(cx).id().to_string(), t.read(cx).meta.project_id.clone()));
            if let Some((thread, project)) = found {
                browser::Browsers::open_for_user(&thread, Some(&project), Some(&url));
                // `YH_BROWSER_PICK=<x,y>`: then the element picker, and a click at that point.
                if let Some((x, y)) = env("YH_BROWSER_PICK").and_then(|p| p.split_once(',').map(|(a, b)| (a.trim().parse::<f64>().ok(), b.trim().parse::<f64>().ok()))).and_then(|(a, b)| Some((a?, b?))) {
                    let inspector = root.read(cx).inspector.clone();
                    cx.spawn(async move |cx| {
                        cx.background_executor().timer(Duration::from_secs(3)).await;
                        let _ = cx.update(|cx| inspector.update(cx, |i, cx| i.pick(&thread, cx)));
                        cx.background_executor().timer(Duration::from_millis(600)).await;
                        let _ = cx.update(|_| browser::Browsers::with_tab(&thread, |t| t.web.click(x, y, false, 1, 0)));
                    })
                    .detach();
                }
            }
        }
        // `YH_OPEN_BROWSER_WINDOW=1`: the open thread's browser in its own window, as the panel's
        // ⋯ → Open in Window does, after `YH_DRAFT_DELAY` (`YH_BROWSER_WINDOW_SNAPSHOT=<png>` draws
        // it 2 s later; `YH_BROWSER_WINDOW_CLOSE=<seconds>` then closes it, the pages going back).
        if env("YH_OPEN_BROWSER_WINDOW").as_deref() == Some("1") {
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let workspace = workspace.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let found = cx.update(|cx| workspace.read(cx).selected_thread().map(|t| (t.read(cx).id().to_string(), t.read(cx).meta.project_id.clone())));
                let Some((thread, project)) = found else { return eprintln!("BROWSER no thread open") };
                let _ = cx.update(|cx| ui::browser_window::open(workspace.clone(), thread.clone(), project, cx));
                cx.background_executor().timer(Duration::from_secs(2)).await;
                let _ = cx.update(|cx| {
                    let report = browser::Browsers::with_tab(&thread, |t| t.web.report_place()).unwrap_or_else(|| "no tab".into());
                    eprintln!("BROWSER popped={} {report}", browser::Browsers::is_popped(&thread));
                    if let Some(file) = env("YH_BROWSER_WINDOW_SNAPSHOT") {
                        for w in cx.windows() {
                            let _ = w.update(cx, |_, window, _| {
                                if window.window_title().starts_with("Browser") {
                                    crate::snapshot(window, &file);
                                }
                            });
                        }
                    }
                });
                if let Some(secs) = env("YH_BROWSER_WINDOW_CLOSE").and_then(|v| v.parse::<f64>().ok()) {
                    cx.background_executor().timer(Duration::from_secs_f64(secs)).await;
                    let _ = cx.update(|cx| ui::browser_window::close(&thread, cx));
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let _ = cx.update(|_| {
                        let report = browser::Browsers::with_tab(&thread, |t| t.web.report_place()).unwrap_or_else(|| "no tab".into());
                        eprintln!("BROWSER closed window, popped={} {report}", browser::Browsers::is_popped(&thread));
                    });
                }
            })
            .detach();
        }
        // `YH_SELECT=<title or id>`: open a thread at launch; `YH_SEND=<text>` then sends it.
        if let Some(wanted) = env("YH_SELECT") {
            workspace.update(cx, |ws, cx| {
                let found = ws.threads.iter().find(|t| {
                    let t = t.read(cx);
                    t.id() == wanted || t.title.starts_with(&wanted)
                });
                match found.map(|t| t.read(cx).id().to_string()) {
                    Some(id) => ws.select(&id, cx),
                    None => eprintln!("no thread {wanted}"),
                }
            });
        }
        // `YH_NEW_THREAD=1`: a new thread in the first project at launch (its empty state).
        if env("YH_NEW_THREAD").as_deref() == Some("1") {
            workspace.update(cx, |ws, cx| {
                if let Some(project) = ws.projects.first().map(|p| p.id.clone()) {
                    ws.new_thread(&project, cx);
                }
            });
        }
        // `YH_BROWSER_PROBE=<steps.json>`: browser tool calls run on the selected thread's browser
        // (a new thread in the first project if none is), each result printed (browser::run_probe).
        // `YH_BROWSER_AUTO_SHOW=0` first turns off showing the browser when a page opens, so it
        // stays parked (saved: development data only).
        if let Some(steps) = env("YH_BROWSER_PROBE") {
            let found = workspace.update(cx, |ws, cx| {
                if env("YH_BROWSER_AUTO_SHOW").as_deref() == Some("0") && env("YH_DATA_DIR").is_some() {
                    ws.library.set_setting("browserAutoShow", &serde_json::json!(false));
                }
                if ws.selected_thread().is_none() {
                    if let Some(project) = ws.projects.first().map(|p| p.id.clone()) {
                        ws.new_thread(&project, cx);
                    }
                }
                // `YH_BROWSER_PROBE_THREAD=<id>`: on that thread's browser while another is open.
                let chosen = env("YH_BROWSER_PROBE_THREAD").and_then(|id| ws.thread(&id)).or_else(|| ws.selected_thread());
                chosen.map(|t| (t.read(cx).id().to_string(), t.read(cx).meta.project_id.clone()))
            });
            match found {
                Some((thread, project)) => browser::run_probe(steps, thread, Some(project)),
                None => eprintln!("BROWSER PROBE: no thread to run on (give it a YH_PROJECT)"),
            }
        }
        // `YH_SEND_FILES=<a.png:b.pdf:folder>` goes with it, through the composer as if attached
        // (images inline, the rest by path).
        if let Some(text) = env("YH_SEND") {
            workspace.update(cx, |ws, cx| {
                if ws.selected.is_none() {
                    if let Some(project) = ws.projects.first().map(|p| p.id.clone()) {
                        ws.new_thread(&project, cx);
                    }
                }
            });
            match env("YH_SEND_FILES") {
                Some(files) => {
                    let paths: Vec<std::path::PathBuf> = files.split(':').map(std::path::PathBuf::from).collect();
                    let composer = root.read(cx).composer.clone();
                    composer.update(cx, |c, cx| c.add_files(&paths, cx));
                    let root = root.clone();
                    cx.spawn(async move |cx| {
                        cx.background_executor().timer(Duration::from_millis(800)).await;
                        let _ = window.update(cx, |_, window, cx| {
                            let composer = root.read(cx).composer.clone();
                            composer.update(cx, |c, cx| {
                                c.type_draft(&text, window, cx);
                                c.submit(false, window, cx);
                            });
                        });
                    })
                    .detach();
                }
                None => workspace.update(cx, |ws, cx| {
                    if let Some(t) = ws.selected_thread() {
                        ws.send(&t, &text, false, cx);
                    }
                }),
            }
        }
        // `YH_DRAFT=<text>`: typed into the composer after `YH_DRAFT_DELAY` seconds (`/co` opens the
        // command menu, `@na` the file menu).
        // `YH_TYPE_KEYS=h,i,shift-space,x`: typed into the composer as real key events through
        // the window (`YH_TYPE_KEYS_DELAY` s after launch, 3), then what it holds is printed.
        if let Some(spec) = env("YH_TYPE_KEYS") {
            let delay: f64 = env("YH_TYPE_KEYS_DELAY").and_then(|v| v.parse().ok()).unwrap_or(3.0);
            let root = root.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                // The window's own pointer: events are sent outside any update, as AppKit hands
                // them to GPUI at once, which would find the window already borrowed.
                // `YH_TYPE_KEYS_CLICKS=0.85,0.4;0.4,0.93`: real clicks first, at those fractions of
                // the window (the page, then the composer, as a person would), instead of
                // focusing the composer directly.
                let clicks: Vec<(f32, f32)> = env("YH_TYPE_KEYS_CLICKS")
                    .map(|v| v.split(';').filter_map(|p| p.split_once(',')).filter_map(|(x, y)| Some((x.trim().parse().ok()?, y.trim().parse().ok()?))).collect())
                    .unwrap_or_default();
                let mut ns: usize = 0;
                let mut size = (0f32, 0f32);
                let _ = window.update(cx, |_, window, cx| {
                    if clicks.is_empty() {
                        let input = root.read(cx).composer.read(cx).input.clone();
                        window.focus(&input.focus_handle(cx), cx);
                    }
                    ns = platform::ns_window(window) as usize;
                    let v = window.viewport_size();
                    size = (f32::from(v.width), f32::from(v.height));
                });
                for (fx, fy) in clicks {
                    #[allow(deprecated)]
                    let target = ns as cocoa::base::id;
                    platform::click(target, fx * size.0, fy * size.1);
                    eprintln!("TYPE clicked at {:.0},{:.0}", fx * size.0, fy * size.1);
                    cx.background_executor().timer(Duration::from_millis(400)).await;
                }
                cx.background_executor().timer(Duration::from_millis(300)).await;
                #[allow(deprecated)]
                {
                    let w = ns as cocoa::base::id;
                    eprintln!("TYPE first responder before typing: {}", platform::first_responder_class(w));
                }
                for step in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    let (shift, key) = match step.strip_prefix("shift-") { Some(k) => (true, k), None => (false, step) };
                    let (plain, code): (String, u16) = match key {
                        "space" => (" ".into(), 49),
                        "return" => ("\r".into(), 36),
                        k if k.len() == 1 => {
                            let c = k.chars().next().unwrap_or('a');
                            (c.to_string(), platform::key_code(c))
                        }
                        other => { eprintln!("TYPE can't read {other}"); continue }
                    };
                    let chars = if shift { plain.to_uppercase() } else { plain.clone() };
                    let flags = if shift { 1u64 << 17 } else { 0 };
                    #[allow(deprecated)]
                    let target = ns as cocoa::base::id;
                    platform::key_event(target, &chars, &plain, code, flags);
                    cx.background_executor().timer(Duration::from_millis(60)).await;
                }
                cx.background_executor().timer(Duration::from_millis(500)).await;
                let _ = window.update(cx, |_, _, cx| {
                    let value = root.read(cx).composer.read(cx).input.read(cx).value().to_string();
                    eprintln!("TYPE composer holds {value:?}");
                });
            })
            .detach();
        }
        if let Some(text) = env("YH_DRAFT").map(|t| t.replace("\\n", "\n")) {
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(0.5);
            let root = root.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let _ = window.update(cx, |_, window, cx| {
                    let composer = root.read(cx).composer.clone();
                    composer.update(cx, |c, cx| c.type_draft(&text, window, cx));
                });
            })
            .detach();
        }
        // `YH_SEND_FILES` without `YH_SEND`: attached to the selected thread's composer, as a drop
        // does (images inline, anything else by path).
        if let (Some(files), None) = (env("YH_SEND_FILES"), env("YH_SEND")) {
            let paths: Vec<std::path::PathBuf> = files.split(':').map(std::path::PathBuf::from).collect();
            let composer = root.read(cx).composer.clone();
            composer.update(cx, |c, cx| c.add_files(&paths, cx));
        }
        // `YH_QUEUE=<text>`: sent `YH_QUEUE_DELAY` seconds after launch (3), while the first turn
        // works, so it waits as a queued message; `YH_QUEUE_ACTION=now|edit|remove` then does to it
        // what its buttons do, `YH_QUEUE_ACTION_DELAY` seconds later (1).
        if let Some(text) = env("YH_QUEUE") {
            let delay: f64 = env("YH_QUEUE_DELAY").and_then(|v| v.parse().ok()).unwrap_or(3.0);
            let after: f64 = env("YH_QUEUE_ACTION_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let workspace = workspace.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let _ = cx.update(|cx| {
                    workspace.update(cx, |ws, cx| {
                        if let Some(t) = ws.selected_thread() {
                            eprintln!("queue: sending while running={}", t.read(cx).run.running);
                            ws.send(&t, &text, false, cx);
                        }
                    })
                });
                let Some(action) = env("YH_QUEUE_ACTION") else { return };
                cx.background_executor().timer(Duration::from_secs_f64(after)).await;
                let _ = cx.update(|cx| {
                    workspace.update(cx, |ws, cx| {
                        let Some(t) = ws.selected_thread() else { return };
                        let Some((id, ..)) = t.read(cx).queued_messages().into_iter().next() else {
                            eprintln!("queue: nothing queued for {action}");
                            return;
                        };
                        eprintln!("queue: {action} {id}");
                        match action.as_str() {
                            "now" => ws.send_now(&t, &id, cx),
                            "edit" => ws.withdraw(&t, &id, true, cx),
                            _ => ws.withdraw(&t, &id, false, cx),
                        }
                    })
                });
            })
            .detach();
        }
        // `YH_STOP=<seconds>`: ⌘. that long after launch, through the window as the shortcut goes
        // (queued messages back into the composer, the turn ending "Interrupted").
        if let Some(delay) = env("YH_STOP").and_then(|v| v.parse::<f64>().ok()) {
            let root = root.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let _ = window.update(cx, |_, window, cx| {
                    let running = root.read(cx).workspace.read(cx).selected_thread().is_some_and(|t| t.read(cx).run.running);
                    eprintln!("STOP pressed (running={running})");
                    window.dispatch_action(Box::new(Stop), cx);
                });
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let _ = window.update(cx, |_, _, cx| {
                    let draft = root.read(cx).composer.read(cx).input.read(cx).value().to_string();
                    eprintln!("STOP composer now holds: {draft:?}");
                });
            })
            .detach();
        }
        // `YH_SEND_SEQ=<a>|<b>|…`: sent to the selected thread one after another (each once the
        // last turn is over), as typed in the composer: for the app's own commands (`/btw`, `/model`…).
        if let Some(seq) = env("YH_SEND_SEQ") {
            let workspace = workspace.clone();
            cx.spawn(async move |cx| {
                for text in seq.split('|').map(str::trim).filter(|t| !t.is_empty()) {
                    for _ in 0..200 {
                        cx.background_executor().timer(Duration::from_millis(300)).await;
                        let busy = cx.update(|cx| workspace.read(cx).selected_thread().is_some_and(|t| t.read(cx).run.running));
                        if !busy {
                            break;
                        }
                    }
                    cx.background_executor().timer(Duration::from_millis(1200)).await;
                    eprintln!("send-seq: {text}");
                    let text = text.to_string();
                    let _ = cx.update(|cx| {
                        workspace.update(cx, |ws, cx| {
                            if let Some(t) = ws.selected_thread() {
                                ws.send(&t, &text, false, cx);
                            }
                        })
                    });
                }
                eprintln!("send-seq: done");
            })
            .detach();
        }
        // `YH_FIND=<words>`: Find opened with them typed, after `YH_DRAFT_DELAY`.
        if let Some(words) = env("YH_FIND") {
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let root = root.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let _ = window.update(cx, |_, window, cx| {
                    let find = root.read(cx).find.clone();
                    find.update(cx, |f, cx| {
                        f.show(window, cx);
                        f.input.update(cx, |i, cx| i.set_value(words.clone(), window, cx));
                        f.search(cx);
                    });
                });
            })
            .detach();
        }
        // `YH_REFRESH_LIMITS=1`: the footer's refresh, clicked (a tiny Haiku request through
        // `claude`; prints `LIMITS refreshed` or why not).
        if env("YH_REFRESH_LIMITS").as_deref() == Some("1") {
            workspace.update(cx, |ws, cx| ws.refresh_limits(cx));
        }
        // `YH_HOVER=<x,y>`: the pointer moved there (window points) after `YH_DRAFT_DELAY`, through
        // the window as a real move goes, for snapshots of hover states (a sidebar row's actions).
        if let Some((x, y)) = env("YH_HOVER").and_then(|v| v.split_once(',').and_then(|(x, y)| Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?)))) {
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                if let Ok(ns) = window.update(cx, |_, window, _| crate::platform::ns_window(window)) {
                    crate::platform::mouse_move(ns, x, y);
                }
            })
            .detach();
        }
        // `YH_CLICK=<x,y>[;<x,y>…]`: clicks there (window points) through the window, a second
        // apart, after `YH_DRAFT_DELAY` (+0.5 s after `YH_HOVER`): for menus and buttons only a
        // click opens (the composer's pickers, the plan card's Approve menu). `r<x,y>` right-clicks
        // (context menus), `h<x,y>` only moves the pointer there (a menu row highlighted).
        if let Some(points) = env("YH_CLICK") {
            let points: Vec<(char, f32, f32)> = points
                .split(';')
                .filter_map(|p| {
                    let p = p.trim();
                    let (kind, p) = match p.chars().next()? {
                        k @ ('r' | 'h') => (k, &p[1..]),
                        _ => ('l', p),
                    };
                    p.split_once(',').and_then(|(x, y)| Some((kind, x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?)))
                })
                .collect();
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0) + 0.5;
            // `YH_CLICK_WINDOW=<title>`: in that window instead (`Settings`), found by its title.
            let in_window = env("YH_CLICK_WINDOW");
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                for (kind, x, y) in points {
                    if let Some(ns) = input_target(window, in_window.as_deref(), cx) {
                        crate::platform::mouse_move(ns, x, y);
                        match kind {
                            'r' => crate::platform::right_click(ns, x, y),
                            'h' => {}
                            _ => crate::platform::click(ns, x, y),
                        }
                        eprintln!("CLICK {kind} {x},{y}");
                    }
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                }
            })
            .detach();
        }
        // `YH_DRAG=<x1,y1,x2,y2>`: the left button pressed at the first window point, dragged to
        // the second and let go, through the window, after `YH_DRAFT_DELAY` + 0.5 s; then prints
        // what ⌘C would copy (`DRAG selected …`).
        if let Some(drag) = env("YH_DRAG") {
            let v: Vec<f32> = drag.split(',').filter_map(|n| n.trim().parse().ok()).collect();
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0) + 0.5;
            if let [x1, y1, x2, y2] = v[..] {
                cx.spawn(async move |cx| {
                    cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                    if let Some(ns) = input_target(window, None, cx) {
                        crate::platform::mouse_move(ns, x1, y1);
                        crate::platform::mouse_button(ns, x1, y1, crate::platform::Press::Down);
                        for step in 1..=10 {
                            let f = step as f32 / 10.;
                            crate::platform::mouse_button(ns, x1 + (x2 - x1) * f, y1 + (y2 - y1) * f, crate::platform::Press::Drag);
                            cx.background_executor().timer(Duration::from_millis(30)).await;
                        }
                        crate::platform::mouse_button(ns, x2, y2, crate::platform::Press::Up);
                    }
                    cx.background_executor().timer(Duration::from_millis(400)).await;
                    let _ = window.update(cx, |_, window, cx| {
                        let text = gpui_kit::base::TextSelection::selected_text(window, cx);
                        eprintln!("DRAG selected ({} chars): {:?}", text.chars().count(), text.chars().take(300).collect::<String>());
                    });
                })
                .detach();
            }
        }
        // `YH_WHEEL=<x,y,dy,count[,dx]>[;…]`: `count` pixel wheel events of `dy` (positive scrolls
        // up, as a trackpad does; `dx` sideways, positive scrolling left) at that window point, through the window as a real wheel goes,
        // after `YH_DRAFT_DELAY` (+0.5 s after `YH_HOVER`); after each step it prints where the
        // open transcript and every capped box that can scroll stand (`WHEEL …`).
        if let Some(steps) = env("YH_WHEEL") {
            let steps: Vec<(f32, f32, i32, u32, i32)> = steps
                .split(';')
                .filter_map(|s| {
                    let v: Vec<&str> = s.split(',').map(str::trim).collect();
                    let dx = v.get(4).and_then(|c| c.parse().ok()).unwrap_or(0);
                    Some((v.first()?.parse().ok()?, v.get(1)?.parse().ok()?, v.get(2)?.parse().ok()?, v.get(3).and_then(|c| c.parse().ok()).unwrap_or(1), dx))
                })
                .collect();
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0) + 0.5;
            let root = root.clone();
            let in_window = env("YH_CLICK_WINDOW");
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let report = |label: String, cx: &mut gpui_kit::AsyncApp| {
                    let _ = window.update(cx, |_, _, cx| {
                        let top = root.read(cx).workspace.read(cx).selected_thread().map(|t| t.read(cx).list.logical_scroll_top());
                        let top = top.map_or("none".to_string(), |t| format!("row {} +{:.0}", t.item_ix, f32::from(t.offset_in_item)));
                        let boxes: Vec<String> = crate::ui::rows::box_offsets().iter().map(|(k, at, max)| format!("{:04x}:{at:.0}/{max:.0}", k & 0xffff)).collect();
                        let settings = crate::ui::settings::wheel_report(cx).map(|s| format!("; {s}")).unwrap_or_default();
                        eprintln!("WHEEL {label}: transcript at {top}; boxes [{}]{settings}", boxes.join(" "));
                    });
                };
                report("start".into(), cx);
                for (n, (x, y, dy, count, dx)) in steps.into_iter().enumerate() {
                    if let Some(ns) = input_target(window, in_window.as_deref(), cx) {
                        crate::platform::mouse_move(ns, x, y);
                    }
                    for _ in 0..count {
                        if let Some(ns) = input_target(window, in_window.as_deref(), cx) {
                            crate::platform::wheel_xy(ns, x, y, dy, dx);
                        }
                        cx.background_executor().timer(Duration::from_millis(30)).await;
                    }
                    cx.background_executor().timer(Duration::from_millis(400)).await;
                    report(format!("step {} at {x},{y} {dy}x{count}", n + 1), cx);
                }
                eprintln!("WHEEL done");
            })
            .detach();
        }
        // `YH_AX_DUMP=<seconds>`: then the window's accessibility tree, as VoiceOver reads it,
        // printed as `AX …` lines (read from inside the app: no permission, no prompt). Asked
        // once to wake GPUI's tree up, then read 1.5 s later.
        if let Some(delay) = env("YH_AX_DUMP").and_then(|v| v.parse::<f64>().ok()) {
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let _ = window.update(cx, |_, window, _| crate::platform::accessibility_tree(crate::platform::ns_window(window), 50));
                cx.background_executor().timer(Duration::from_millis(1500)).await;
                let _ = window.update(cx, |_, window, _| {
                    let lines = crate::platform::accessibility_tree(crate::platform::ns_window(window), 4000);
                    for line in &lines {
                        eprintln!("AX {line}");
                    }
                    eprintln!("AX done: {} elements", lines.len());
                });
            })
            .detach();
        }
        // `YH_DROP=<x,y>` with `YH_DROP_FILES=<a:b>`: files dragged in from Finder to that point
        // (window points) through GPUI's own file-drop events, after `YH_DRAFT_DELAY`; held there
        // `YH_DROP_HOLD` seconds (2: a snapshot then shows the overlay), then dropped.
        if let (Some((x, y)), Some(files)) = (env("YH_DROP").and_then(|v| v.split_once(',').and_then(|(x, y)| Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?)))), env("YH_DROP_FILES")) {
            use gpui_kit::{point, ExternalPaths, FileDropEvent, PlatformInput};
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let hold: f64 = env("YH_DROP_HOLD").and_then(|v| v.parse().ok()).unwrap_or(2.0);
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let position = point(px(x), px(y));
                let paths = ExternalPaths(files.split(':').map(std::path::PathBuf::from).collect());
                let _ = window.update(cx, |_, window, cx| {
                    window.dispatch_event(PlatformInput::FileDrop(FileDropEvent::Entered { position, paths }), cx);
                    window.dispatch_event(PlatformInput::FileDrop(FileDropEvent::Pending { position }), cx);
                });
                eprintln!("DROP over {x},{y}");
                cx.background_executor().timer(Duration::from_secs_f64(hold)).await;
                let _ = window.update(cx, |_, window, cx| {
                    window.dispatch_event(PlatformInput::FileDrop(FileDropEvent::Submit { position }), cx);
                });
                eprintln!("DROP submitted");
            })
            .detach();
        }
        // `YH_RESUME_SESSION=<id>`: the Resume Claude Code Session sheet, filled in, after
        // `YH_DRAFT_DELAY` (`YH_RESUME_CONFIRM=1` then resumes into the suggested project).
        if env("YH_RESUME_SESSION").is_some() {
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let root = root.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let _ = window.update(cx, |_, window, cx| {
                    let sheet = root.read(cx).resume.clone();
                    sheet.update(cx, |s, cx| s.show(window, cx));
                });
            })
            .detach();
        }
        // `YH_NOTIFICATION_CLICK=<seconds>`: then a click on the last notification posted (a test
        // run only prints them), which opens its thread as a real click does.
        if let Some(delay) = env("YH_NOTIFICATION_CLICK").and_then(|v| v.parse::<f64>().ok()) {
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let _ = cx.update(|_| match crate::platform::click_last_notification() {
                    Some(thread) => eprintln!("NOTIFY click on {thread}"),
                    None => eprintln!("NOTIFY click: nothing posted yet"),
                });
            })
            .detach();
        }
        // `YH_SPOTLIGHT=<words>`: ⌘K with them typed, after `YH_DRAFT_DELAY` (a session ID
        // and `YH_SPOTLIGHT_ENTER=1` resumes it).
        if let Some(words) = env("YH_SPOTLIGHT") {
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let enter = env("YH_SPOTLIGHT_ENTER").as_deref() == Some("1");
            let root = root.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let _ = window.update(cx, |_, window, cx| {
                    let s = root.read(cx).spotlight.clone();
                    s.update(cx, |s, cx| {
                        s.show(window, cx);
                        s.input.update(cx, |i, cx| i.set_value(words.clone(), window, cx));
                        s.query(window, cx);
                    });
                });
                // What it found, once the indexes have answered.
                cx.background_executor().timer(Duration::from_millis(700)).await;
                let _ = window.update(cx, |_, _, cx| {
                    for line in root.read(cx).spotlight.read(cx).describe() {
                        eprintln!("SPOTLIGHT {line}");
                    }
                });
                if enter {
                    cx.background_executor().timer(Duration::from_millis(600)).await;
                    let _ = window.update(cx, |_, window, cx| {
                        let s = root.read(cx).spotlight.clone();
                        s.update(cx, |s, cx| s.input.update(cx, |i, cx| { let _ = (i, window, cx); }));
                        let ws = root.read(cx).workspace.clone();
                        let words = words.clone();
                        let _ = ws.update(cx, |ws, cx| ws.resume_session(&words, cx)).map_err(|e| eprintln!("resume: {e}"));
                    });
                }
            })
            .detach();
        }
        // `YH_OPEN_MEMORY=1`: the selected thread's project's Memory page (else the first
        // project's) after `YH_DRAFT_DELAY`, on `YH_MEMORY_TAB=memories|tasks|activity`.
        if env("YH_OPEN_MEMORY").as_deref() == Some("1") {
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let (workspace, root) = (workspace.clone(), root.clone());
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let _ = cx.update(|cx| {
                    let project = workspace
                        .read(cx)
                        .selected_thread()
                        .map(|t| t.read(cx).meta.project_id.clone())
                        .or_else(|| workspace.read(cx).projects.first().map(|p| p.id.clone()));
                    if let Some(project) = project {
                        // `YH_MEMORY_SEED=<n>`: n made-up memories and a board's worth of tasks
                        // added to it first (scratch data only).
                        if let (Some(n), Some(store)) = (env("YH_MEMORY_SEED").and_then(|v| v.parse::<usize>().ok()), workspace.read(cx).memory_store()) {
                            let started = std::time::Instant::now();
                            yh_core::memory::seed(&store, &yh_core::memory::scope_of(&project), n);
                            eprintln!("memory: seeded {n} in {} ms", started.elapsed().as_millis());
                            workspace.update(cx, |ws, cx| ws.memory_changed(cx));
                        }
                        workspace.update(cx, |ws, cx| ws.open_memory(&project, cx));
                    }
                });
                // `YH_MEMORY_IMPORT=<file or folder>` imports into it; `YH_MEMORY_EXPORT=<folder>`
                // writes its memory there as JSON and as a Markdown folder.
                let _ = cx.update(|cx| {
                    if let Some(page) = root.read(cx).memory_page(cx) {
                        if let Some(path) = env("YH_MEMORY_IMPORT") {
                            page.update(cx, |p, cx| p.import_path(std::path::PathBuf::from(path), cx));
                        }
                    }
                });
                if let Some(folder) = env("YH_MEMORY_EXPORT") {
                    cx.background_executor().timer(Duration::from_secs(3)).await;
                    let _ = cx.update(|cx| {
                        if let Some(page) = root.read(cx).memory_page(cx) {
                            let folder = std::path::PathBuf::from(folder);
                            page.update(cx, |p, cx| {
                                p.export_to(false, folder.join("memory.json"), cx);
                                p.export_to(true, folder.join("memory-md"), cx);
                            });
                        }
                    });
                }
                let tab = match env("YH_MEMORY_TAB").as_deref() {
                    Some("tasks") => ui::memory::Tab::Tasks,
                    Some("activity") => ui::memory::Tab::Activity,
                    Some("insights") => ui::memory::Tab::Insights,
                    Some("graph") => ui::memory::Tab::Graph,
                    _ => ui::memory::Tab::Memories,
                };
                let _ = cx.update(|cx| {
                    if let Some(page) = root.read(cx).memory_page(cx) {
                        page.update(cx, |p, cx| {
                            p.tab = tab;
                            cx.notify();
                        });
                    }
                });
                // `YH_MEMORY_GRAPH_HOVER=<index or title>`: the pointer moved over that memory's dot
                // on the Graph tab, through the window as a real move goes, once it's drawn.
                if let Some(which) = env("YH_MEMORY_GRAPH_HOVER") {
                    cx.background_executor().timer(Duration::from_millis(1500)).await;
                    let at = cx.update(|cx| root.read(cx).memory_page(cx).and_then(|p| p.read(cx).graph_dot(&which)));
                    match at {
                        Some((x, y)) => {
                            eprintln!("GRAPH hover {which} at {x:.0},{y:.0}");
                            if let Ok(ns) = window.update(cx, |_, window, _| crate::platform::ns_window(window)) {
                                crate::platform::mouse_move(ns, x, y);
                            }
                        }
                        None => eprintln!("GRAPH hover {which}: no such dot drawn"),
                    }
                }
            })
            .detach();
        }
        // `YH_OPEN_FILE=<path[:line[-end]]>`: opened in the selected thread's Files tab, as a
        // link does, after `YH_DRAFT_DELAY`.
        if let Some(link) = env("YH_OPEN_FILE") {
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let _ = cx.update(|cx| ui::inspector::open_link(&link, cx));
            })
            .detach();
        }
        // `YH_SIDE_ASK=<text>`: asked in the selected thread's side chat after `YH_DRAFT_DELAY`,
        // the right sidebar on Side Chat.
        if let Some(text) = env("YH_SIDE_ASK") {
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let workspace = workspace.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let _ = cx.update(|cx| {
                    let Some(parent) = workspace.read(cx).selected.clone() else { return };
                    workspace.update(cx, |ws, cx| {
                        ws.show_panel(state::workspace::Panel::SideChat, cx);
                        if let Some(chat) = ws.side_chat(&parent, cx) {
                            ws.send(&chat, &text, false, cx);
                        }
                    });
                });
            })
            .detach();
        }
        // `YH_WEBVIEW_SPIKE=<url>`: a web view over the window's right edge loads it, then its
        // title and a screenshot (`<YH_SNAPSHOT>-web.png`) are printed: checks the browser's
        // native view works over GPUI.
        if let Some(url) = env("YH_WEBVIEW_SPIKE") {
            let shot = format!("{}-web.png", env("YH_SNAPSHOT").unwrap_or_else(|| "/tmp/yh".into()).trim_end_matches(".png"));
            cx.spawn(async move |cx| {
                let view = window.update(cx, |_, window, _| {
                    let host = platform::ns_view(window);
                    let size = window.viewport_size();
                    let web = webview::WebView::new(host, None);
                    web.set_frame(Some((f64::from(size.width) - 380., 44., 380., f64::from(size.height) - 44.)));
                    web.load(&url);
                    web
                });
                let Ok(web) = view else { return };
                cx.background_executor().timer(Duration::from_secs(4)).await;
                let (tx, rx) = flume::bounded(1);
                web.evaluate("JSON.stringify({title: document.title, links: document.links.length})", tx);
                eprintln!("WEB eval {:?} url {:?} loading {}", rx.recv_async().await, web.url(), web.is_loading());
                let (tx, rx) = flume::bounded(1);
                web.snapshot(shot.clone(), tx);
                eprintln!("WEB snapshot {shot} {:?}", rx.recv_async().await);
                cx.background_executor().timer(Duration::from_secs(60)).await;
                drop(web);
            })
            .detach();
        }
        // `YH_OPEN_USAGE=1`: Usage opened after `YH_DRAFT_DELAY` (`YH_USAGE_RANGE`,
        // `YH_USAGE_METRIC=cost`, `YH_USAGE_HOVER=<days ago>`).
        if env("YH_OPEN_USAGE").as_deref() == Some("1") {
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let workspace = workspace.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let _ = cx.update(|cx| workspace.update(cx, |ws, cx| ws.toggle_usage(cx)));
            })
            .detach();
        }
        // `YH_OPEN_MCP=1`: the selected thread's `/mcp` sheet after `YH_DRAFT_DELAY`, its servers
        // printed. `YH_MCP_ADD_URL=<url>` or `YH_MCP_ADD_COMMAND=<command line>` fills Add Server
        // (`YH_MCP_ADD_SCOPE=local|project|user`), `YH_MCP_ADD_SUBMIT=1` adds it, `YH_MCP_REMOVE=<name>`
        // removes one; the list is printed again later (`MCP servers later:`). They write Claude
        // Code's config: project scope in a scratch project, or a scratch `CLAUDE_CONFIG_DIR`.
        if matches!(env("YH_OPEN_MCP").as_deref(), Some("1") | Some("everywhere")) {
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let sheet = root.read(cx).mcp.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let everywhere = env("YH_OPEN_MCP").as_deref() == Some("everywhere");
                let _ = cx.update(|cx| sheet.update(cx, |s, cx| if everywhere { s.show_everywhere(cx) } else { s.show(cx) }));
                cx.background_executor().timer(Duration::from_secs(4)).await;
                let source = env("YH_MCP_ADD_URL").or_else(|| env("YH_MCP_ADD_COMMAND"));
                if let Some(source) = source {
                    let scope: &'static str = match env("YH_MCP_ADD_SCOPE").as_deref() {
                        Some("project") => "project",
                        Some("user") => "user",
                        _ => "local",
                    };
                    let _ = window.update(cx, |_, window, cx| sheet.update(cx, |s, cx| s.fill_add(&source, scope, window, cx)));
                    if env("YH_MCP_ADD_SUBMIT").as_deref() == Some("1") {
                        let _ = cx.update(|cx| sheet.update(cx, |s, cx| s.add(cx)));
                    }
                }
                if let Some(name) = env("YH_MCP_REMOVE") {
                    let _ = cx.update(|cx| sheet.update(cx, |s, cx| s.remove_named(&name, cx)));
                }
                cx.background_executor().timer(Duration::from_secs(8)).await;
                let _ = cx.update(|cx| eprintln!("MCP servers later: {}", sheet.read(cx).describe()));
            })
            .detach();
        }
        // `YH_STATE_EXPORT=<file.yhstate>`: the whole state exported at launch, its manifest printed.
        if let Some(file) = env("YH_STATE_EXPORT") {
            ui::state_transfer::export_to(&workspace, std::path::PathBuf::from(file), env("YH_STATE_EXPORT_SIGNINS").as_deref() == Some("1"), None, cx);
        }
        // `YH_IMPORT_COOKIES=<Cookies.binarycookies>`: a Safari-format file's cookies imported into the
        // selected thread's project's browser at launch (a synthetic file: never read your real
        // browsers from a test).
        if let Some(file) = env("YH_IMPORT_COOKIES") {
            let project = workspace.read(cx).selected_thread().map(|t| t.read(cx).meta.project_id.clone());
            match (project, std::fs::read(&file).map_err(|e| e.to_string()).and_then(|d| yh_core::cookies::parse_safari(&d))) {
                (Some(project), Ok(cookies)) => {
                    let (tx, rx) = flume::bounded(1);
                    webview::store_cookies(&project, cookies, tx);
                    cx.spawn(async move |cx| {
                        eprintln!("imported {} cookies", rx.recv_async().await.unwrap_or(0));
                        // Open pages load again with them, as after Import Sign-ins.
                        let _ = cx.update(|_| browser::Browsers::with(|b| b.sessions.values().flat_map(|s| s.tabs.iter()).for_each(|t| t.web.reload())));
                    })
                    .detach();
                }
                (_, Err(e)) => eprintln!("cookie import failed: {e}"),
                (None, _) => eprintln!("cookie import failed: no thread selected"),
            }
        }
        // `YH_MEMORY_AGENT=importer|reconciler`: the selected thread's project's (else the first
        // project's) memory specialist opened after `YH_DRAFT_DELAY`; `YH_MEMORY_AGENT_START=1` sets
        // it going (the importer on `YH_MEMORY_IMPORT_SOURCES=<a:b>`).
        if let Some(role) = env("YH_MEMORY_AGENT") {
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let workspace = workspace.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let _ = cx.update(|cx| {
                    let project = workspace
                        .read(cx)
                        .selected_thread()
                        .map(|t| t.read(cx).meta.project_id.clone())
                        .or_else(|| workspace.read(cx).projects.first().map(|p| p.id.clone()));
                    let Some(project) = project else { return };
                    let sources: Vec<String> = env("YH_MEMORY_IMPORT_SOURCES").map(|s| s.split(':').map(str::to_string).collect()).unwrap_or_default();
                    let start = env("YH_MEMORY_AGENT_START").as_deref() == Some("1");
                    let message = start.then(|| if role == "importer" { yh_core::memory_agents::import_request(&sources, None) } else { yh_core::memory_agents::RECONCILE_REQUEST.to_string() });
                    workspace.update(cx, |ws, cx| ws.open_memory_agent(&role, &project, sources, message, cx));
                });
            })
            .detach();
        }
        // `YH_MEMORY_SETUP=importer|reconciler`: a memory specialist's setup opened over the window
        // after `YH_DRAFT_DELAY`, for the selected thread's project (else the first), with
        // `YH_MEMORY_IMPORT_SOURCES=<a:b>` ticked; `YH_MEMORY_SETUP_START=1` then presses Start.
        if let Some(role) = env("YH_MEMORY_SETUP") {
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let workspace = workspace.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let setup = window.update(cx, |_, window, cx| {
                    let project = workspace
                        .read(cx)
                        .selected_thread()
                        .map(|t| t.read(cx).meta.project_id.clone())
                        .or_else(|| workspace.read(cx).projects.first().map(|p| p.id.clone()))?;
                    let sources: Vec<String> = env("YH_MEMORY_IMPORT_SOURCES").map(|s| s.split(':').map(str::to_string).collect()).unwrap_or_default();
                    Some(ui::memory::open_setup(workspace.clone(), &role, project, sources, window, cx))
                });
                if env("YH_MEMORY_SETUP_START").as_deref() == Some("1") {
                    cx.background_executor().timer(Duration::from_secs(2)).await;
                    if let Ok(Some(setup)) = setup {
                        let _ = window.update(cx, |_, window, cx| setup.update(cx, |s, cx| s.start(window, cx)));
                    }
                }
            })
            .detach();
        }
        // `YH_OPEN_PROJECT=1`: the selected thread's project's page (its PRs and repos), else the
        // first project's, after `YH_DRAFT_DELAY`.
        if env("YH_OPEN_PROJECT").as_deref() == Some("1") {
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let workspace = workspace.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let _ = cx.update(|cx| {
                    let project = workspace.read(cx).selected_thread().map(|t| t.read(cx).meta.project_id.clone()).or_else(|| workspace.read(cx).projects.first().map(|p| p.id.clone()));
                    if let Some(p) = project {
                        workspace.update(cx, |ws, cx| ws.open_project(&p, cx));
                    }
                });
            })
            .detach();
        }
        // `YH_OPEN_SETTINGS=1`: Settings opened after `YH_DRAFT_DELAY`, on `YH_SETTINGS_TAB`.
        if env("YH_OPEN_SETTINGS").as_deref() == Some("1") {
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let workspace = workspace.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let _ = window.update(cx, |_, window, cx| {
                    let tab = env("YH_SETTINGS_TAB").and_then(|t| ui::settings::Tab::from_name(&t)).unwrap_or(ui::settings::Tab::General);
                    ui::settings::open_on(workspace.clone(), tab, window, cx)
                });
            })
            .detach();
        }
        // `YH_ACTIONS_PROBE=1`: the sidebar's actions on the first three threads, through the
        // workspace calls the menu makes, for checking what they save.
        if env("YH_ACTIONS_PROBE").as_deref() == Some("1") {
            workspace.update(cx, |ws, cx| {
                let ids: Vec<String> = ws.threads.iter().take(3).map(|t| t.read(cx).id().to_string()).collect();
                if ids.len() == 3 {
                    ws.set_pinned(&ids[0], true, cx);
                    ws.rename(&ids[0], "Renamed by probe", cx);
                    ws.set_settled(&ids[1], true, cx);
                    ws.delete(&ids[2], cx);
                    eprintln!("ACTIONS pinned+renamed {}, settled {}, deleted {}; notice: {:?}", ids[0], ids[1], ids[2], ws.undo_notice());
                    // `YH_ACTIONS_UNDO=1`: then Undo, which brings the deleted thread back.
                    if env("YH_ACTIONS_UNDO").as_deref() == Some("1") {
                        ws.undo(cx);
                        eprintln!("ACTIONS undone: {} is {}", ids[2], if ws.thread(&ids[2]).is_some() { "back" } else { "still gone" });
                    }
                }
            });
        }
        // `YH_THREAD_ACTIONS=<a>,<b>,…`: what the sidebar and the keyboard do, a second apart after
        // `YH_DRAFT_DELAY`, on the open thread, printing `ACTION <step>: <the open thread>` after
        // each: `settle`, `unsettle:<n>` (the n-th settled), `delete`, `pin`, `unpin`, `undo` (⌘Z
        // outside a text field), `new` (⌘N), `memory` (⇧⌘M), `project` (its page), `add:<folder>`
        // (Add Project), `next`, `inspector` (⌥⌘0), `close` (the window's close button: the quit
        // question), `settings` (⌘,).
        if let Some(steps) = env("YH_THREAD_ACTIONS") {
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let (workspace, root) = (workspace.clone(), root.clone());
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                for step in steps.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    let step = step.to_string();
                    let _ = window.update(cx, |_, window, cx| {
                        let selected = workspace.read(cx).selected.clone().unwrap_or_default();
                        match step.split_once(':').map_or((step.as_str(), ""), |(a, b)| (a, b)) {
                            ("settle", _) => workspace.update(cx, |ws, cx| ws.toggle_settled(&selected, cx)),
                            ("unsettle", n) => {
                                let n: usize = n.parse().unwrap_or(0);
                                let id = workspace.read(cx).threads.iter().filter(|t| t.read(cx).meta.settled_at.is_some()).nth(n).map(|t| t.read(cx).id().to_string());
                                if let Some(id) = id {
                                    workspace.update(cx, |ws, cx| ws.toggle_settled(&id, cx));
                                }
                            }
                            ("delete", _) => workspace.update(cx, |ws, cx| ws.delete(&selected, cx)),
                            ("pin", _) | ("unpin", _) => workspace.update(cx, |ws, cx| ws.toggle_pin(&selected, cx)),
                            ("undo", _) => window.dispatch_action(Box::new(ui::root::UndoThreadAction), cx),
                            ("new", _) => window.dispatch_action(Box::new(NewThread), cx),
                            ("memory", _) => window.dispatch_action(Box::new(OpenMemory), cx),
                            ("project", _) => {
                                let p = workspace.read(cx).selected_thread().map(|t| t.read(cx).meta.project_id.clone());
                                if let Some(p) = p {
                                    workspace.update(cx, |ws, cx| ws.open_project(&p, cx));
                                }
                            }
                            ("add", folder) => {
                                let sidebar = root.read(cx).sidebar.clone();
                                sidebar.update(cx, |s, cx| s.add_project_at(folder, cx));
                            }
                            ("next", _) => window.dispatch_action(Box::new(NextThread), cx),
                            ("inspector", _) => window.dispatch_action(Box::new(ToggleInspector), cx),
                            ("settings", _) => window.dispatch_action(Box::new(OpenSettings), cx),
                            ("close", _) => platform::perform_close(window),
                            // `key:<keystroke>` (`cmd-k`, `alt-cmd-down`): typed into the window as
                            // a real key goes, so a shortcut that doesn't land shows.
                            ("key", ks) => match gpui_kit::Keystroke::parse_with_os(ks, gpui_kit::OperatingSystem::Mac) {
                                Ok(ks) => {
                                    window.dispatch_keystroke(ks, cx);
                                }
                                Err(_) => eprintln!("ACTION bad key {ks}"),
                            },
                            (other, _) => eprintln!("ACTION unknown {other}"),
                        }
                    });
                    cx.background_executor().timer(Duration::from_millis(400)).await;
                    let _ = cx.update(|cx| {
                        let ws = workspace.read(cx);
                        let open = match &ws.page {
                            Some(state::workspace::Page::Memory(p)) => format!("Memory of {}", ws.project(p).map(|p| p.name.clone()).unwrap_or_default()),
                            Some(state::workspace::Page::Project(p)) => format!("project page of {}", ws.project(p).map(|p| p.name.clone()).unwrap_or_default()),
                            Some(state::workspace::Page::Usage) => "Usage".into(),
                            None => ws.selected_thread().map(|t| format!("“{}” in {}", t.read(cx).title, ws.project(&t.read(cx).meta.project_id).map(|p| p.name.clone()).unwrap_or_default())).unwrap_or_else(|| "nothing".into()),
                        };
                        let order: Vec<String> = ws.visible_thread_ids(cx).iter().filter_map(|id| ws.thread(id).map(|t| format!("{}{}", if t.read(cx).meta.pinned { "📌" } else { "" }, t.read(cx).title))).collect();
                        eprintln!("ACTION {step}: {open} · {} threads, {} settled · order: {}", ws.threads.len(), ws.threads.iter().filter(|t| t.read(cx).meta.settled_at.is_some()).count(), order.join(" | "));
                    });
                    cx.background_executor().timer(Duration::from_millis(600)).await;
                }
                eprintln!("ACTION done");
            })
            .detach();
        }
        // `YH_PREVIEW_MEDIA=1`: the selected thread's last pictures in Quick Look (ui::rows).
        if env("YH_PREVIEW_MEDIA").as_deref() == Some("1") {
            let workspace = workspace.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let _ = cx.update(|cx| ui::rows::preview_probe(&workspace, cx));
            })
            .detach();
        }
        // `YH_PREVIEW_MEDIA=draft`: what waits in the composer (`YH_SEND_FILES` alone attaches it),
        // in Quick Look as clicking one does; prints PASS when the panel took it, and closes it.
        if env("YH_PREVIEW_MEDIA").as_deref() == Some("draft") {
            let composer = root.read(cx).composer.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let files = cx.update(|cx| composer.read(cx).attached_paths(cx));
                eprintln!("PREVIEW files: {files:?}");
                let _ = cx.update(|_| crate::platform::quick_look(files, 0));
                cx.background_executor().timer(Duration::from_millis(1500)).await;
                let _ = cx.update(|_| eprintln!("PREVIEW {}", if crate::platform::quick_look_visible(true) { "PASS" } else { "FAIL" }));
            })
            .detach();
        }
        // `YH_SNAPSHOT=<png>`: the window drawn into a PNG after `YH_SNAPSHOT_DELAY` seconds, for
        // looking without capturing the screen (test builds: `--features snapshots`).
        // Several delays (`12,40`) draw it at each, into `<file>-<seconds>.png`.
        if let Some(file) = env("YH_SNAPSHOT") {
            let delays: Vec<f64> = env("YH_SNAPSHOT_DELAY").unwrap_or_else(|| "3".into()).split(',').filter_map(|v| v.trim().parse().ok()).collect();
            let quit = env("YH_SNAPSHOT_QUIT").as_deref() == Some("1");
            cx.spawn(async move |cx| {
                let mut waited = 0.0;
                for delay in &delays {
                    cx.background_executor().timer(Duration::from_secs_f64((delay - waited).max(0.0))).await;
                    waited = *delay;
                    let file = if delays.len() > 1 { format!("{}-{delay}.png", file.trim_end_matches(".png")) } else { file.clone() };
                    // `YH_SNAPSHOT_WINDOW=<title>`: another window (Settings, a browser window) instead.
                    match env("YH_SNAPSHOT_WINDOW") {
                        Some(title) => {
                            let _ = cx.update(|cx| {
                                for w in cx.windows() {
                                    let _ = w.update(cx, |_, window, _| {
                                        if window.window_title().starts_with(title.as_str()) {
                                            snapshot(window, &file);
                                        }
                                    });
                                }
                            });
                        }
                        None => {
                            let _ = window.update(cx, |_, window, _| snapshot(window, &file));
                        }
                    }
                }
                if quit {
                    cx.background_executor().timer(Duration::from_millis(300)).await;
                    cx.update(|cx| cx.quit());
                }
            })
            .detach();
        }
        if let Some(folder) = env("YH_PERF") {
            probe::start(window, root, folder, cx);
        }
    });
}

#[cfg(feature = "snapshots")]
fn snapshot(window: &mut gpui_kit::Window, file: &str) {
    match window.render_to_image().map(|image| image.save(file)) {
        Ok(Ok(())) => eprintln!("SNAPSHOT {file}"),
        other => eprintln!("snapshot failed: {other:?}"),
    }
}

#[cfg(not(feature = "snapshots"))]
fn snapshot(_: &mut gpui_kit::Window, _: &str) {
    eprintln!("snapshot failed: build with --features snapshots");
}

/// The NSWindow test input goes to: the main window, or the one whose title starts with
/// `title` (`YH_CLICK_WINDOW`).
#[allow(deprecated)]
fn input_target(main: gpui_kit::AnyWindowHandle, title: Option<&str>, cx: &mut gpui_kit::AsyncApp) -> Option<cocoa::base::id> {
    match title {
        Some(title) => cx.update(|cx| {
            cx.windows()
                .into_iter()
                .find_map(|w| w.update(cx, |_, window, _| window.window_title().starts_with(title).then(|| platform::ns_window(window))).ok().flatten())
        }),
        None => main.update(cx, |_, window, _| platform::ns_window(window)).ok(),
    }
}
