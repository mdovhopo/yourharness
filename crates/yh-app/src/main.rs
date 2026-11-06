//! YourHarness: a native harness over AI coding CLIs, in Rust on GPUI. See docs/architecture.md:
//! `state` owns what the app knows, `ui` draws it, `platform` is the AppKit edge, `probe` the
//! performance run.

mod platform;
mod probe;
mod state;
mod theme;
mod ui;

use gpui_kit::component::{Theme as ComponentTheme, ThemeMode};
use gpui_kit::{
    point, prelude::*, px, size, App, Bounds, Focusable, KeyBinding, Menu, MenuItem, TitlebarOptions, Window, WindowAppearance,
    WindowBounds, WindowOptions,
};
use state::workspace::Workspace;
use std::sync::Arc;
use std::time::Duration;
use theme::Theme;
use ui::root::{Allow, Deny, NewThread, Quit, RootView, Stop};

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// The app's palette and gpui-component's (menus, dialogs, inputs) follow light and dark.
fn apply_appearance(appearance: WindowAppearance, window: &mut Window, cx: &mut App) {
    cx.set_global(Theme::for_appearance(appearance));
    let mode = match appearance {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeMode::Dark,
        _ => ThemeMode::Light,
    };
    ComponentTheme::change(mode, Some(window), cx);
    // Its primary buttons in the app's accent, as macOS's default buttons are.
    let accent = Theme::get(cx).accent;
    let theme = ComponentTheme::global_mut(cx);
    let colors = &mut theme.colors;
    colors.primary = accent;
    colors.primary_hover = accent.opacity(0.9);
    colors.primary_active = accent.opacity(0.8);
    colors.primary_foreground = gpui_kit::white();
    colors.button_primary = accent;
    colors.button_primary_hover = accent.opacity(0.9);
    colors.button_primary_active = accent.opacity(0.8);
    colors.button_primary_foreground = gpui_kit::white();
    // Buttons read tokens made from the colours.
    theme.tokens = gpui_kit::component::theme::ThemeTokens::from(&theme.colors);
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

    gpui_kit::application().run(move |cx: &mut App| {
        gpui_kit::init(cx);
        cx.bind_keys([
            KeyBinding::new("cmd-.", Stop, None),
            KeyBinding::new("cmd-enter", Allow, Some("Harness")),
            KeyBinding::new("cmd-backspace", Deny, Some("Harness")),
            KeyBinding::new("cmd-n", NewThread, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.set_menus(vec![
            Menu { name: "YourHarness".into(), items: vec![MenuItem::action("Quit", Quit)], disabled: false },
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
            apply_appearance(window.appearance(), window, cx);
            let workspace = cx.new(|cx| Workspace::new(library.clone(), cx));
            cx.new(|cx| RootView::new(workspace, window, cx))
        })
        .expect("window");
        let workspace = root.read(cx).workspace.clone();

        {
            let root = root.clone();
            let _ = window.update(cx, |_, window, cx| {
                // gpui-component sets its theme up once the window's Root is made: ours on top.
                apply_appearance(window.appearance(), window, cx);
                let input = root.read(cx).composer.read(cx).input.clone();
                input.update(cx, |i, cx| i.focus(window, cx));
                window
                    .observe_window_appearance(|window, cx| {
                        apply_appearance(window.appearance(), window, cx);
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
