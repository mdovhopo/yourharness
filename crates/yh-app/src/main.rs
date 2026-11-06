//! YourHarness: a native harness over AI coding CLIs, in Rust on GPUI. See docs/architecture.md:
//! `state` owns what the app knows, `ui` draws it, `platform` is the AppKit edge, `probe` the
//! performance run.

mod platform;
mod probe;
mod state;
mod theme;
mod ui;
mod webview;
mod browser;

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
        }
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
            KeyBinding::new("alt-cmd-0", ui::root::ToggleInspector, None),
            KeyBinding::new("shift-cmd-m", ui::root::OpenMemory, None),
            KeyBinding::new("shift-cmd-u", ui::root::ToggleUsage, None),
            KeyBinding::new("cmd-s", crate::ui::memory::SaveMemory, Some("MemoryPage")),
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
        // The agents' browser: pages laid over this window.
        {
            let (ws, inspector) = (workspace.clone(), root.read(cx).inspector.clone());
            let host = window.update(cx, |_, window, _| platform::ns_view(window)).unwrap_or(std::ptr::null_mut());
            browser::start(
                host,
                move |thread, cx| {
                    ws.update(cx, |ws, cx| {
                        if ws.selected.as_deref() == Some(thread) {
                            ws.show_panel(state::workspace::Panel::Browser, cx);
                        }
                    })
                },
                move |cx| inspector.update(cx, |_, cx| cx.notify()),
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
        if env("YH_OPEN_MCP").as_deref() == Some("1") {
            let delay: f64 = env("YH_DRAFT_DELAY").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let sheet = root.read(cx).mcp.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
                let _ = cx.update(|cx| sheet.update(cx, |s, cx| s.show(cx)));
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
            ui::state_transfer::export_to(&workspace, std::path::PathBuf::from(file), cx);
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
                    cx.spawn(async move |_| eprintln!("imported {} cookies", rx.recv_async().await.unwrap_or(0))).detach();
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
