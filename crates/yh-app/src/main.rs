//! YourHarness: a native harness over AI coding CLIs, in Rust on GPUI. See docs/architecture.md:
//! `state` owns what the app knows, `ui` draws it, `platform` is the AppKit edge, `probe` the
//! performance run.

mod platform;
mod probe;
mod state;
mod theme;
mod ui;

use gpui_kit::{
    point, prelude::*, px, size, App, Bounds, Focusable, KeyBinding, Menu, MenuItem, TitlebarOptions,
    WindowBounds, WindowOptions,
};
use state::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;
use ui::root::{Allow, Deny, Find, FindNext, FindPrevious, NewThread, OpenSettings, OpenSpotlight, Quit, RootView, Stop};

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
    let library = Arc::new(yh_core::library::Library::open());
    theme::set_chosen_appearance(library.setting("appearance").and_then(|v| v.as_str().map(str::to_string)));

    gpui_kit::application().run(move |cx: &mut App| {
        gpui_kit::init(cx);
        cx.bind_keys([
            KeyBinding::new("cmd-.", Stop, None),
            KeyBinding::new("cmd-enter", Allow, Some("Harness")),
            KeyBinding::new("cmd-backspace", Deny, Some("Harness")),
            KeyBinding::new("cmd-n", NewThread, None),
            KeyBinding::new("cmd-f", Find, None),
            KeyBinding::new("cmd-,", OpenSettings, None),
            KeyBinding::new("cmd-k", OpenSpotlight, None),
            KeyBinding::new("cmd-g", FindNext, None),
            KeyBinding::new("cmd-shift-g", FindPrevious, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.set_menus(vec![
            Menu {
                name: "YourHarness".into(),
                items: vec![MenuItem::action("Settings…", OpenSettings), MenuItem::separator(), MenuItem::action("Quit", Quit)],
                disabled: false,
            },
            Menu { name: "File".into(), items: vec![MenuItem::action("New Thread", NewThread)], disabled: false },
            Menu {
                name: "Thread".into(),
                items: vec![MenuItem::action("Stop", Stop), MenuItem::action("Allow", Allow), MenuItem::action("Deny", Deny)],
                disabled: false,
            },
        ]);

        let background = env("YH_BACKGROUND").as_deref() == Some("1") || env("YH_PERF").is_some();
        let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
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

        cx.on_app_quit({
            let workspace = workspace.clone();
            move |cx| {
                workspace.update(cx, |ws, cx| ws.prepare_for_quit(cx));
                async {}
            }
        })
        .detach();

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
        // `YH_SEND_FILES=<a.png:b.jpg>` goes with it, through the composer as if attached.
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
        if let Some(text) = env("YH_DRAFT") {
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
        // `YH_OPEN_SETTINGS=1`: Settings opened after `YH_DRAFT_DELAY`.
        if env("YH_OPEN_SETTINGS").as_deref() == Some("1") {
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let workspace = workspace.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let _ = window.update(cx, |_, window, cx| ui::settings::open(workspace.clone(), window, cx));
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
                    eprintln!("ACTIONS pinned+renamed {}, settled {}, deleted {}", ids[0], ids[1], ids[2]);
                }
            });
        }
        // `YH_SNAPSHOT=<png>`: the window drawn into a PNG after `YH_SNAPSHOT_DELAY` seconds, for
        // looking without capturing the screen (test builds: `--features snapshots`).
        if let Some(file) = env("YH_SNAPSHOT") {
            let delay: f64 = env("YH_SNAPSHOT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(3.0);
            let quit = env("YH_SNAPSHOT_QUIT").as_deref() == Some("1");
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let _ = window.update(cx, |_, window, _| snapshot(window, &file));
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
