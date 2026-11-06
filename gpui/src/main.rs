//! YourHarness on GPUI: the web POC's question asked of a third renderer. Does a GPU-drawn,
//! immediate-mode UI (Zed's) stay smooth where SwiftUI didn't? Same data, same fake `claude`,
//! same phases as web/ (see gpui/README.md). Everything here is Rust: no HarnessCore, the same
//! tables, files and stream-json read directly, which is also a first look at the Rust backend.

mod app;
mod claude;
mod history;
mod input;
mod library;
mod markdown;
mod model;
mod platform;
mod probe;
mod rows;
mod theme;

use app::{Allow, Deny, HarnessApp, Quit, Stop};
use gpui::{
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
    if env("YHG_PERF").is_some() || env("YHG_BACKGROUND").as_deref() == Some("1") {
        std::env::set_var("GPUI_KEEP_DRAWING", "1");
    }
    if env("YHG_PERF").is_some() && env("YHG_APP_NAP").as_deref() != Some("1") {
        platform::hold_off_app_nap();
    }
    if env("YHG_SNAPSHOT").is_some() {
        std::env::set_var("GPUI_SNAPSHOTS", "1");
    }
    let library = Arc::new(library::Library::open());

    Application::new().run(move |cx: &mut App| {
        input::bind_keys(cx);
        cx.bind_keys([
            KeyBinding::new("cmd-.", Stop, None),
            KeyBinding::new("cmd-enter", Allow, None),
            KeyBinding::new("cmd-backspace", Deny, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.set_menus(vec![
            Menu { name: "YourHarness GPUI".into(), items: vec![MenuItem::action("Quit", Quit)] },
            Menu { name: "Thread".into(), items: vec![MenuItem::action("Stop", Stop), MenuItem::action("Allow", Allow), MenuItem::action("Deny", Deny)] },
        ]);

        let background = env("YHG_BACKGROUND").as_deref() == Some("1") || env("YHG_PERF").is_some();
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
                window.focus(&app.composer.read(cx).input.read(cx).focus_handle(cx));
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

        // `YHG_SELECT=<title or id>`: open a thread at launch; `YHG_SEND=<text>` then sends it.
        if let Some(wanted) = env("YHG_SELECT") {
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
        if let Some(text) = env("YHG_SEND") {
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
        if let Some(file) = env("YHG_SNAPSHOT") {
            let delay: f64 = env("YHG_SNAPSHOT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(3.0);
            let quit = env("YHG_SNAPSHOT_QUIT").as_deref() == Some("1");
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                *gpui::SNAPSHOT_REQUEST.lock().unwrap() = Some(file.into());
                let _ = window.update(cx, |_, window, _| window.refresh());
                if quit {
                    cx.background_executor().timer(Duration::from_millis(800)).await;
                    let _ = cx.update(|cx| cx.quit());
                }
            })
            .detach();
        }
        if let Some(folder) = env("YHG_PERF") {
            probe::start(window, folder, cx);
        }
    });
}
