//! YourHarness on GPUI: the web POC's question asked of a third renderer. Does a GPU-drawn,
//! immediate-mode UI (Zed's) stay smooth where SwiftUI didn't? Same data, same fake `claude`,
//! same phases as web/ (see gpui/README.md). Everything here is Rust: no HarnessCore, the same
//! tables, files and stream-json read directly, which is also a first look at the Rust backend.

mod app;





mod model;
mod platform;
mod probe;
mod rows;
mod theme;

use app::{Allow, Deny, HarnessApp, Quit, Stop};
use gpui_kit::{
    point, prelude::*, px, size, App, Application, Focusable, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBounds,
    WindowOptions,
};
use std::sync::Arc;
use std::time::Duration;
use theme::Theme;

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn main() {
    // A test run parks the window off screen and still needs frames; snapshots read them back.
    if env("YH_PERF").is_some() || env("YH_BACKGROUND").as_deref() == Some("1") {
        // SAFETY: first thing in main, before any thread starts.
        unsafe { std::env::set_var("GPUI_KEEP_DRAWING", "1") };
    }
    if env("YH_PERF").is_some() && env("YH_APP_NAP").as_deref() != Some("1") {
        platform::hold_off_app_nap();
    }
    let library = Arc::new(yh_core::library::Library::open());

    gpui_kit::application().run(move |cx: &mut App| {
        gpui_kit::init(cx);
        cx.bind_keys([
            KeyBinding::new("cmd-.", Stop, None),
            KeyBinding::new("cmd-enter", Allow, None),
            KeyBinding::new("cmd-backspace", Deny, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.set_menus(vec![
            Menu { name: "YourHarness".into(), items: vec![MenuItem::action("Quit", Quit)], disabled: false },
            Menu { name: "Thread".into(), items: vec![MenuItem::action("Stop", Stop), MenuItem::action("Allow", Allow), MenuItem::action("Deny", Deny)], disabled: false },
        ]);

        let background = env("YH_BACKGROUND").as_deref() == Some("1") || env("YH_PERF").is_some();
        let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("YourHarness GPUI".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(14.), px(14.))),
                    }),
                    focus: !background,
                    show: true,
                    window_min_size: Some(size(px(640.), px(420.))),
                    // GPUI draws an inactive window at 30 fps to save power. A perf run's window
                    // is never active (it's parked off screen), and it measures the active case.
                    inactive_frame_interval: if env("YH_PERF").is_some() {
                        None
                    } else {
                        WindowOptions::default().inactive_frame_interval
                    },
                    ..Default::default()
                },
                |window, cx| {
                    cx.set_global(Theme::for_appearance(window.appearance()));
                    cx.new(|cx| HarnessApp::new(library.clone(), window, cx))
                },
            )
            .unwrap();

        window
            .update(cx, |app, window, cx| {
                let input = app.composer.read(cx).input.clone();
                input.update(cx, |i, cx| i.focus(window, cx));
                cx.observe_window_appearance(window, |_, window, cx| {
                    cx.set_global(Theme::for_appearance(window.appearance()));
                    cx.refresh_windows();
                })
                .detach();
                if background {
                    platform::order_back(window);
                } else {
                    cx.activate(true);
                }
            })
            .unwrap();

        cx.on_app_quit({
            let window = window;
            move |cx| {
                let _ = window.update(cx, |app, _, _| app.stop_all());
                async {}
            }
        })
        .detach();

        // `YH_SELECT=<title or id>`: open a thread at launch; `YH_SEND=<text>` then sends it.
        if let Some(wanted) = env("YH_SELECT") {
            let _ = window.update(cx, |app, window, cx| {
                let found = app.threads.iter().find(|t| {
                    let t = t.read(cx);
                    t.id() == wanted || t.title.starts_with(&wanted)
                });
                if let Some(id) = found.map(|t| t.read(cx).id().to_string()) {
                    app.select(&id, window, cx);
                } else {
                    eprintln!("no thread {wanted}");
                }
            });
        }
        if let Some(text) = env("YH_SEND") {
            let _ = window.update(cx, |app, window, cx| {
                if app.selected.is_none() {
                    if let Some(project) = app.projects.first().cloned() {
                        app.new_thread(&project, window, cx);
                    }
                }
                if let Some(t) = app.selected_thread() {
                    app.send(&t, &text, cx);
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
            probe::start(window, folder, cx);
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
