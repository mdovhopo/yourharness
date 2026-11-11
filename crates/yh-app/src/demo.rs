#![allow(deprecated)]
//! Demo recordings (`YH_DEMO=<script.json>`, test builds: `--features snapshots`): the window
//! drawn into a video through `ffmpeg` while a script moves a drawn pointer, clicks, types and
//! scrolls through the window as a person would, and puts up captions, which are written next to
//! the video with the times they showed (`<out>.captions.json`) for the edit to draw.
//!
//! ```json
//! {"out": "/tmp/scene.mp4", "fps": 30, "quit": true, "steps": [
//!   {"wait": 1},
//!   {"caption": "Threads across projects", "for": 3},
//!   {"click": "title:Fix the login flow"},
//!   {"click": "composer", "at": [0.1, 0.5]},
//!   {"type": "Add a dark mode toggle", "cps": 20},
//!   {"key": "enter"},
//!   {"wheel": "transcript", "dy": -30, "count": 40},
//!   {"move": [600, 400], "dur": 0.8},
//!   {"drag": [100, 200, 500, 260], "dur": 0.8},
//!   {"cursor": false},
//!   {"wait_for": "idle", "max": 60}
//! ]}
//! ```
//!
//! Targets are window points (`[x, y]`), a tracked element (`composer`, `transcript`,
//! `thread:<id>`, `inspector-edge`, `job`), or a thread by its title (`title:<words>`); `at`
//! picks a point inside an element by fractions (centre by default), `dx`/`dy` nudge it.

use crate::platform;
use crate::ui::root::RootView;
use gpui_kit::{AnyWindowHandle, AsyncApp, Entity};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
struct Pointer {
    x: f32,
    y: f32,
    shown: bool,
    /// When the last click went in, for its ripple.
    clicked: Option<Instant>,
}

struct Frame {
    at: f64,
    width: u32,
    height: u32,
    scale: f32,
    pixels: Vec<u8>,
    pointer: Pointer,
    clicked_ago: Option<f64>,
    /// Where the browser's page shows, in window points: WebKit draws it, not GPUI, so it's
    /// captured on its own and drawn in.
    page: Option<(f64, f64, f64, f64)>,
}

pub fn start(handle: AnyWindowHandle, root: Entity<RootView>, script: String, cx: &mut gpui_kit::App) {
    let spec: Value = match std::fs::read_to_string(&script).ok().and_then(|s| serde_json::from_str(&s).ok()) {
        Some(v) => v,
        None => {
            eprintln!("DEMO can't read {script}");
            return;
        }
    };
    crate::probe::enable_tracking();
    let out = spec["out"].as_str().unwrap_or("/tmp/yh-demo.mp4").to_string();
    let fps = spec["fps"].as_f64().unwrap_or(30.0);
    let lead = spec["lead"].as_f64().unwrap_or(1.0);
    // `"record": false`: only the script, for making demo data with real turns.
    let record = spec["record"].as_bool().unwrap_or(true);
    let pointer = Rc::new(RefCell::new(Pointer { x: 640.0, y: 420.0, shown: true, clicked: None }));
    let started = Rc::new(RefCell::new(None::<Instant>));
    let stop = Rc::new(RefCell::new(false));
    let (tx, rx) = mpsc::sync_channel::<Frame>(3);
    let (page_tx, page_rx) = flume::bounded::<Option<(Vec<u8>, u32, u32)>>(4);
    let encoder = {
        let out = out.clone();
        std::thread::spawn(move || encode(rx, page_rx, &out, fps))
    };

    // The recorder: the window drawn `fps` times a second; a frame the encoder can't take yet
    // is dropped, and the one after stands in for it (frames carry their time).
    {
        let pointer = pointer.clone();
        let started = started.clone();
        let stop = stop.clone();
        cx.spawn(async move |cx: &mut AsyncApp| {
            cx.background_executor().timer(Duration::from_secs_f64(lead)).await;
            let t0 = Instant::now();
            *started.borrow_mut() = Some(t0);
            eprintln!("DEMO {}", if record { "recording" } else { "running" });
            *stop.borrow_mut() |= !record;
            let interval = Duration::from_secs_f64(1.0 / fps);
            let mut next = t0;
            let mut last_page = Instant::now() - Duration::from_secs(1);
            let (mut grabs, mut grab_ms) = (0u32, 0f64);
            while !*stop.borrow() {
                let p = *pointer.borrow();
                let began = Instant::now();
                let grabbed = handle.update(cx, |_, window, _| {
                    let points = f32::from(window.viewport_size().width);
                    let page = crate::browser::Browsers::shown_pages(platform::ns_window(window)).into_iter().next();
                    render(window).map(|(w, h, px)| (w, h, w as f32 / points.max(1.0), px, page))
                });
                grabs += 1;
                grab_ms += began.elapsed().as_secs_f64() * 1000.0;
                if grabs == 150 {
                    eprintln!("DEMO frames take {:.1} ms to grab", grab_ms / grabs as f64);
                    (grabs, grab_ms) = (0, 0.0);
                }
                if let Ok(Some((width, height, scale, pixels, page))) = grabbed {
                    // The page, a few times a second (WebKit takes its picture asynchronously).
                    if let Some((key, rect)) = page {
                        if last_page.elapsed() > Duration::from_millis(120) {
                            last_page = Instant::now();
                            let tx = page_tx.clone();
                            let _ = cx.update(|_| crate::browser::Browsers::capture_page(key, rect.2 * scale as f64, tx));
                        }
                    }
                    let page = page.map(|(_, r)| r);
                    let frame = Frame {
                        at: t0.elapsed().as_secs_f64(),
                        width,
                        height,
                        scale,
                        pixels,
                        pointer: p,
                        clicked_ago: p.clicked.map(|c| c.elapsed().as_secs_f64()),
                        page,
                    };
                    let _ = tx.try_send(frame);
                }
                next += interval;
                let now = Instant::now();
                if next > now {
                    cx.background_executor().timer(next - now).await;
                } else {
                    next = now;
                }
            }
            drop(tx);
        })
        .detach();
    }

    // The script.
    let steps = spec["steps"].as_array().cloned().unwrap_or_default();
    let quit = spec["quit"].as_bool().unwrap_or(true);
    let tail = spec["tail"].as_f64().unwrap_or(1.0);
    cx.spawn(async move |cx: &mut AsyncApp| {
        while started.borrow().is_none() {
            cx.background_executor().timer(Duration::from_millis(50)).await;
        }
        let t0 = started.borrow().unwrap();
        let mut d = Director { handle, root, pointer, cx: cx.clone(), captions: Vec::new(), zooms: Vec::new(), names: Default::default(), t0 };
        for (n, step) in steps.iter().enumerate() {
            eprintln!("DEMO {:.2} step {n}: {step}", t0.elapsed().as_secs_f64());
            d.run(step).await;
        }
        d.sleep(tail).await;
        *stop.borrow_mut() = true;
        let end = t0.elapsed().as_secs_f64();
        for c in d.captions.iter_mut() {
            if c["end"].as_f64().is_none_or(|e| e > end) {
                c["end"] = json!(end);
            }
        }
        let captions = json!({"duration": end, "captions": d.captions, "zooms": d.zooms});
        let _ = std::fs::write(format!("{out}.captions.json"), serde_json::to_string_pretty(&captions).unwrap_or_default());
        // The encoder finishes once the recorder's sender is gone.
        let done = cx.background_executor().spawn(async move { encoder.join().ok().flatten() }).await;
        eprintln!("DEMO done {out} ({} frames, {end:.1} s)", done.unwrap_or(0));
        if quit {
            let _ = cx.update(|cx| cx.quit());
        }
    })
    .detach();
}

#[cfg(feature = "snapshots")]
fn render(window: &mut gpui_kit::Window) -> Option<(u32, u32, Vec<u8>)> {
    let image = window.render_to_image().ok()?;
    let (w, h) = (image.width(), image.height());
    Some((w, h, image.into_raw()))
}

#[cfg(not(feature = "snapshots"))]
fn render(_: &mut gpui_kit::Window) -> Option<(u32, u32, Vec<u8>)> {
    eprintln!("DEMO needs a build with --features snapshots");
    None
}

struct Director {
    handle: AnyWindowHandle,
    root: Entity<RootView>,
    pointer: Rc<RefCell<Pointer>>,
    cx: AsyncApp,
    captions: Vec<Value>,
    zooms: Vec<Value>,
    /// Threads named by `remember`, for `@name` targets.
    names: std::collections::HashMap<String, String>,
    t0: Instant,
}

impl Director {
    async fn sleep(&self, seconds: f64) {
        self.cx.background_executor().timer(Duration::from_secs_f64(seconds.max(0.0))).await;
    }

    fn ns(&mut self) -> Option<cocoa::base::id> {
        self.handle.update(&mut self.cx, |_, window, _| platform::ns_window(window)).ok()
    }

    fn now(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }

    /// Where a target is, in window points.
    fn point(&mut self, target: &Value, step: &Value) -> Option<(f32, f32)> {
        if let Some(a) = target.as_array() {
            return Some((a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32));
        }
        let mut key = target.as_str()?.to_string();
        if let Some(id) = key.strip_prefix('@').and_then(|n| self.names.get(n)) {
            key = format!("thread:{id}");
        }
        if let Some(title) = key.strip_prefix("title:") {
            let title = title.to_lowercase();
            let root = self.root.clone();
            let id = self.cx.update(|cx| {
                let ws = root.read(cx).workspace.read(cx);
                ws.threads.iter().find(|t| t.read(cx).title.to_lowercase().contains(&title)).map(|t| t.read(cx).id().to_string())
            })?;
            key = format!("thread:{id}");
        }
        let b = crate::probe::tracked(&key)?;
        let (fx, fy) = step["at"].as_array().map(|a| (a[0].as_f64().unwrap_or(0.5) as f32, a[1].as_f64().unwrap_or(0.5) as f32)).unwrap_or((0.5, 0.5));
        let x = f32::from(b.origin.x) + f32::from(b.size.width) * fx + step["dx"].as_f64().unwrap_or(0.0) as f32;
        let y = f32::from(b.origin.y) + f32::from(b.size.height) * fy + step["dy"].as_f64().unwrap_or(0.0) as f32;
        Some((x, y))
    }

    /// The pointer glides there (eased), sending real moves on the way, for hover states.
    async fn glide(&mut self, to: (f32, f32), dur: f64) {
        let from = { let p = self.pointer.borrow(); (p.x, p.y) };
        let frames = ((dur * 60.0) as usize).max(1);
        for i in 1..=frames {
            let t = i as f32 / frames as f32;
            let e = if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 };
            let (x, y) = (from.0 + (to.0 - from.0) * e, from.1 + (to.1 - from.1) * e);
            { let mut p = self.pointer.borrow_mut(); p.x = x; p.y = y; }
            if let Some(ns) = self.ns() {
                platform::mouse_move(ns, x, y);
            }
            self.sleep(1.0 / 60.0).await;
        }
    }

    async fn run(&mut self, step: &Value) {
        let dur = step["dur"].as_f64().unwrap_or(0.7);
        if let Some(s) = step["wait"].as_f64() {
            self.sleep(s).await;
        } else if let Some(text) = step["caption"].as_str() {
            let start = self.now();
            // A caption stays until the next one, or `for` seconds.
            if let Some(last) = self.captions.last_mut() {
                if last["end"].as_f64().is_none_or(|e| e > start) {
                    last["end"] = json!(start);
                }
            }
            let end = step["for"].as_f64().map(|f| start + f);
            self.captions.push(json!({"start": start, "end": end, "text": text, "style": step["style"]}));
            if let Some(w) = step["hold"].as_f64() {
                self.sleep(w).await;
            }
        } else if let Some(target) = step.get("zoom") {
            // The edit's camera: in on a rect of window points (or an element), `null` back out.
            let rect = match target {
                Value::Null => Value::Null,
                Value::Array(a) if a.len() == 4 => target.clone(),
                _ => match target.as_str().map(|k| k.to_string()) {
                    Some(key) => {
                        let key = if let Some(title) = key.strip_prefix("title:") {
                            let title = title.to_lowercase();
                            let root = self.root.clone();
                            self.cx.update(|cx| {
                                let ws = root.read(cx).workspace.read(cx);
                                ws.threads.iter().find(|t| t.read(cx).title.to_lowercase().contains(&title)).map(|t| format!("thread:{}", t.read(cx).id()))
                            }).unwrap_or(key)
                        } else {
                            key
                        };
                        match crate::probe::tracked(&key) {
                            Some(b) => json!([f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)]),
                            None => {
                                eprintln!("DEMO no target {key}");
                                return;
                            }
                        }
                    }
                    None => return,
                },
            };
            self.zooms.push(json!({"t": self.now(), "rect": rect, "dur": step["dur"].as_f64().unwrap_or(1.0), "pad": step["pad"]}));
            if let Some(w) = step["hold"].as_f64() {
                self.sleep(w).await;
            }
        } else if let Some(target) = step.get("move") {
            if let Some(p) = self.point(target, step) {
                self.glide(p, dur).await;
            } else {
                eprintln!("DEMO no target {target}");
            }
        } else if let Some(target) = step.get("click").or(step.get("right_click")).or(step.get("double_click")) {
            let Some(p) = self.point(target, step) else {
                eprintln!("DEMO no target {target}");
                return;
            };
            self.glide(p, dur).await;
            self.sleep(0.12).await;
            self.pointer.borrow_mut().clicked = Some(Instant::now());
            if let Some(ns) = self.ns() {
                if step.get("right_click").is_some() {
                    platform::right_click(ns, p.0, p.1);
                } else {
                    platform::click(ns, p.0, p.1);
                    if step.get("double_click").is_some() {
                        platform::click(ns, p.0, p.1);
                    }
                }
            }
            self.sleep(step["after"].as_f64().unwrap_or(0.4)).await;
        } else if let Some(text) = step["type"].as_str() {
            let cps = step["cps"].as_f64().unwrap_or(22.0);
            for (i, ch) in text.chars().enumerate() {
                if let Some(ns) = self.ns() {
                    platform::key(ns, &ch.to_string());
                }
                // A person's rhythm: a little uneven, a beat after a space.
                let jitter = ((i * 7919) % 13) as f64 / 13.0 * 0.5 + 0.75;
                let pause = if ch == ' ' { 1.4 } else { 1.0 };
                self.sleep(jitter * pause / cps).await;
            }
        } else if let Some(ks) = step["key"].as_str() {
            let ks = ks.to_string();
            let _ = self.handle.update(&mut self.cx, |_, window, cx| {
                match gpui_kit::Keystroke::parse_with_os(&ks, gpui_kit::OperatingSystem::Mac) {
                    Ok(k) => {
                        window.dispatch_keystroke(k, cx);
                    }
                    Err(_) => eprintln!("DEMO bad key {ks}"),
                }
            });
            self.sleep(step["after"].as_f64().unwrap_or(0.3)).await;
        } else if let Some(target) = step.get("wheel") {
            let Some(p) = self.point(target, step) else { return };
            if step["glide"].as_bool().unwrap_or(true) {
                self.glide(p, dur.min(0.5)).await;
            }
            let dy = step["by"].as_i64().unwrap_or(-20) as i32;
            let dx = step["bx"].as_i64().unwrap_or(0) as i32;
            let gap = step["gap"].as_f64().unwrap_or(1.0 / 60.0);
            for _ in 0..step["count"].as_u64().unwrap_or(20) {
                if let Some(ns) = self.ns() {
                    platform::wheel_xy(ns, p.0, p.1, dy, dx);
                }
                self.sleep(gap).await;
            }
        } else if let Some(v) = step["drag"].as_array() {
            let n: Vec<f32> = v.iter().filter_map(|x| x.as_f64().map(|x| x as f32)).collect();
            let [x1, y1, x2, y2] = n[..] else { return };
            self.glide((x1, y1), 0.5).await;
            if let Some(ns) = self.ns() {
                platform::mouse_button(ns, x1, y1, platform::Press::Down);
            }
            let frames = ((dur * 60.0) as usize).max(2);
            for i in 1..=frames {
                let t = i as f32 / frames as f32;
                let (x, y) = (x1 + (x2 - x1) * t, y1 + (y2 - y1) * t);
                { let mut p = self.pointer.borrow_mut(); p.x = x; p.y = y; }
                if let Some(ns) = self.ns() {
                    platform::mouse_button(ns, x, y, platform::Press::Drag);
                }
                self.sleep(1.0 / 60.0).await;
            }
            if let Some(ns) = self.ns() {
                platform::mouse_button(ns, x2, y2, platform::Press::Up);
            }
        } else if let Some(text) = step["send"].as_str() {
            // Straight into the composer and sent, no typing shown (making demo data).
            let (text, root) = (text.to_string(), self.root.clone());
            let _ = self.handle.update(&mut self.cx, |_, window, cx| {
                let composer = root.read(cx).composer.clone();
                composer.update(cx, |c, cx| {
                    c.type_draft(&text, window, cx);
                    c.submit(false, window, cx);
                });
            });
            self.sleep(step["after"].as_f64().unwrap_or(0.5)).await;
        } else if let Some(name) = step["remember"].as_str() {
            // The open thread, named for `@name` targets later.
            let root = self.root.clone();
            if let Some(id) = self.cx.update(|cx| root.read(cx).workspace.read(cx).selected.clone()) {
                self.names.insert(name.to_string(), id);
            }
        } else if let Some(wanted) = step["select"].as_str() {
            let wanted = match wanted.strip_prefix('@').and_then(|n| self.names.get(n)) {
                Some(id) => id.clone(),
                None => wanted.to_string(),
            };
            let (wanted, root) = (wanted.to_lowercase(), self.root.clone());
            let _ = self.cx.update(|cx| {
                let ws = root.read(cx).workspace.clone();
                ws.update(cx, |ws, cx| {
                    let id = ws.threads.iter().find(|t| t.read(cx).id().to_lowercase() == wanted || t.read(cx).title.to_lowercase().contains(&wanted)).map(|t| t.read(cx).id().to_string());
                    match id {
                        Some(id) => ws.select(&id, cx),
                        None => eprintln!("DEMO no thread {wanted}"),
                    }
                });
            });
            self.sleep(step["after"].as_f64().unwrap_or(0.5)).await;
        } else if let Some(file) = step["shot"].as_str() {
            // A still of the window as it stands (no pointer), for screenshots.
            let file = file.to_string();
            let _ = self.handle.update(&mut self.cx, |_, window, _| crate::snapshot(window, &file));
        } else if let Some(name) = step["panel"].as_str() {
            // A right sidebar tab: the pointer goes there and clicks, and the panel is chosen as
            // the tab does (a click made up in the title bar's band starts a window drag instead).
            let Some(panel) = crate::state::workspace::Panel::from_name(name) else {
                eprintln!("DEMO no panel {name}");
                return;
            };
            let i = crate::state::workspace::Panel::ALL.iter().position(|p| *p == panel).unwrap_or(0);
            let at = step["at"].as_array().map(|a| (a[0].as_f64().unwrap_or(0.0) as f32, a[1].as_f64().unwrap_or(0.0) as f32));
            let at = at.unwrap_or(([1108.0, 1180.0, 1250.0, 1320.0, 1391.0][i], 21.0));
            self.glide(at, dur).await;
            self.sleep(0.12).await;
            self.pointer.borrow_mut().clicked = Some(Instant::now());
            let root = self.root.clone();
            let _ = self.cx.update(|cx| {
                let inspector = root.read(cx).inspector.clone();
                inspector.update(cx, |i, cx| i.choose_panel(panel, cx));
            });
            self.sleep(step["after"].as_f64().unwrap_or(0.4)).await;
        } else if let Some(open) = step["inspector"].as_bool() {
            // The right sidebar open or closed, whichever it is now.
            let root = self.root.clone();
            let _ = self.handle.update(&mut self.cx, |_, window, cx| {
                if root.read(cx).workspace.read(cx).shows_inspector != open {
                    window.dispatch_action(Box::new(crate::ui::root::ToggleInspector), cx);
                }
            });
            self.sleep(step["after"].as_f64().unwrap_or(0.6)).await;
        } else if let Some(shown) = step["cursor"].as_bool() {
            self.pointer.borrow_mut().shown = shown;
        } else if let Some(what) = step["wait_for"].as_str() {
            // `idle`: the open thread's turn over (or `max` seconds).
            let max = step["max"].as_f64().unwrap_or(60.0);
            let start = Instant::now();
            let root = self.root.clone();
            // `turn`: a turn that may not have started yet, waited for to start, then to end.
            if what == "turn" {
                while start.elapsed().as_secs_f64() < 10.0 {
                    let running = self.cx.update(|cx| root.read(cx).workspace.read(cx).selected_thread().is_some_and(|t| t.read(cx).run.running));
                    if running {
                        break;
                    }
                    self.sleep(0.2).await;
                }
            }
            while start.elapsed().as_secs_f64() < max {
                // `browser`: until a page shows over the window.
                if what == "browser" {
                    let shown = self.handle.update(&mut self.cx, |_, window, _| !crate::browser::Browsers::shown_pages(platform::ns_window(window)).is_empty()).unwrap_or(false);
                    // …or the turn over without one (the page is kept: show its panel next).
                    let root = self.root.clone();
                    let idle = self.cx.update(|cx| !root.read(cx).workspace.read(cx).selected_thread().is_some_and(|t| t.read(cx).run.running));
                    if shown || (idle && start.elapsed().as_secs_f64() > 2.0) {
                        break;
                    }
                    self.sleep(0.25).await;
                    continue;
                }
                let busy = self.cx.update(|cx| {
                    let ws = root.read(cx).workspace.read(cx);
                    match what {
                        "idle" | "turn" => ws.selected_thread().is_some_and(|t| t.read(cx).run.running),
                        _ => false,
                    }
                });
                if !busy {
                    break;
                }
                self.sleep(0.25).await;
            }
            self.sleep(step["after"].as_f64().unwrap_or(0.0)).await;
        } else {
            eprintln!("DEMO unknown step {step}");
        }
    }
}

/// Frames to `ffmpeg` at a steady rate: each frame repeated until the next one's time. Returns
/// how many frames went out.
fn encode(rx: mpsc::Receiver<Frame>, pages: flume::Receiver<Option<(Vec<u8>, u32, u32)>>, out: &str, fps: f64) -> Option<usize> {
    let first = rx.recv().ok()?;
    let (w, h) = (first.width, first.height);
    let ffmpeg = ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg"].into_iter().find(|p| std::path::Path::new(p).exists()).unwrap_or("ffmpeg");
    let mut child = std::process::Command::new(ffmpeg)
        .args(["-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{w}x{h}"), "-r", &fps.to_string(), "-i", "-"])
        .args(["-c:v", "h264_videotoolbox", "-b:v", "60M", "-pix_fmt", "yuv420p", out])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| eprintln!("DEMO can't start ffmpeg: {e}"))
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let cursor = cursor_sprite(first.scale);
    let mut written = 0usize;
    let mut last: Option<Vec<u8>> = None;
    let mut pending = Some(first);
    let mut page: Option<Page> = None;
    loop {
        let Some(mut frame) = pending.take().or_else(|| rx.recv().ok()) else { break };
        if frame.width != w || frame.height != h {
            continue;
        }
        while let Ok(shot) = pages.try_recv() {
            if let Some(decoded) = shot.and_then(|(jpeg, _, _)| decode_jpeg(&jpeg)) {
                page = Some(Page { image: decoded, sized: None });
            }
        }
        if let (Some(rect), Some(p)) = (frame.page, page.as_mut()) {
            draw_page(&mut frame, rect, p);
        }
        draw_pointer(&mut frame, &cursor);
        let due = (frame.at * fps).round() as usize;
        // Fill up to this frame's time with the last one.
        if let Some(prev) = &last {
            while written < due {
                if stdin.write_all(prev).is_err() {
                    break;
                }
                written += 1;
            }
        }
        if stdin.write_all(&frame.pixels).is_err() {
            break;
        }
        written += 1;
        last = Some(frame.pixels);
    }
    drop(stdin);
    let _ = child.wait();
    Some(written)
}

struct Page {
    image: (u32, u32, Vec<u8>),
    /// The image resized to where it last showed, in pixels.
    sized: Option<(u32, u32, Vec<u8>)>,
}

#[cfg(feature = "snapshots")]
fn decode_jpeg(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg).ok()?.to_rgba8();
    Some((img.width(), img.height(), img.into_raw()))
}

#[cfg(not(feature = "snapshots"))]
fn decode_jpeg(_: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    None
}

#[cfg(feature = "snapshots")]
fn resize(src: &(u32, u32, Vec<u8>), w: u32, h: u32) -> (u32, u32, Vec<u8>) {
    match image::RgbaImage::from_raw(src.0, src.1, src.2.clone()) {
        Some(img) if (src.0, src.1) != (w, h) => (w, h, image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle).into_raw()),
        _ => src.clone(),
    }
}

#[cfg(not(feature = "snapshots"))]
fn resize(src: &(u32, u32, Vec<u8>), _: u32, _: u32) -> (u32, u32, Vec<u8>) {
    src.clone()
}

/// The page's picture over where the window leaves room for it.
fn draw_page(frame: &mut Frame, rect: (f64, f64, f64, f64), page: &mut Page) {
    let s = frame.scale as f64;
    let (x, y) = ((rect.0 * s).round() as i64, (rect.1 * s).round() as i64);
    let (w, h) = ((rect.2 * s).round() as u32, (rect.3 * s).round() as u32);
    if w == 0 || h == 0 {
        return;
    }
    if page.sized.as_ref().is_none_or(|p| (p.0, p.1) != (w, h)) {
        page.sized = Some(resize(&page.image, w, h));
    }
    let Some((pw, ph, px)) = page.sized.as_ref() else { return };
    let (fw, fh) = (frame.width as i64, frame.height as i64);
    for row in 0..*ph as i64 {
        let ty = y + row;
        if ty < 0 || ty >= fh {
            continue;
        }
        let x0 = x.max(0);
        let x1 = (x + *pw as i64).min(fw);
        if x1 <= x0 {
            continue;
        }
        let src = ((row * *pw as i64 + (x0 - x)) * 4) as usize;
        let dst = ((ty * fw + x0) * 4) as usize;
        let len = ((x1 - x0) * 4) as usize;
        frame.pixels[dst..dst + len].copy_from_slice(&px[src..src + len]);
    }
}

/// macOS's arrow pointer, drawn at the window's scale: black with a white edge and a soft shadow.
fn cursor_sprite(scale: f32) -> (u32, u32, Vec<u8>) {
    // The arrow's outline in points, tip at 0,0.
    let shape: [(f32, f32); 7] = [(0.0, 0.0), (0.0, 16.5), (4.0, 12.8), (6.8, 19.0), (9.4, 17.9), (6.7, 11.9), (12.0, 11.9)];
    let size = (26.0 * scale) as u32;
    let mut px = vec![0u8; (size * size * 4) as usize];
    let inside = |x: f32, y: f32, grow: f32| -> bool {
        // Point-in-polygon on the outline pushed out by `grow` points from its centre.
        let (cx, cy) = (4.5, 11.0);
        let pts: Vec<(f32, f32)> = shape.iter().map(|&(px, py)| {
            let (dx, dy) = (px - cx, py - cy);
            let l = (dx * dx + dy * dy).sqrt().max(0.001);
            (px + dx / l * grow, py + dy / l * grow)
        }).collect();
        let mut c = false;
        let mut j = pts.len() - 1;
        for i in 0..pts.len() {
            let (xi, yi) = pts[i];
            let (xj, yj) = pts[j];
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                c = !c;
            }
            j = i;
        }
        c
    };
    let ss = 4;
    let off = 3.0; // room for the edge and shadow, in points
    for py in 0..size {
        for pxl in 0..size {
            let (mut black, mut white, mut shadow) = (0, 0, 0);
            for sy in 0..ss {
                for sx in 0..ss {
                    let x = (pxl as f32 + (sx as f32 + 0.5) / ss as f32) / scale - off;
                    let y = (py as f32 + (sy as f32 + 0.5) / ss as f32) / scale - off;
                    if inside(x, y, 0.0) {
                        black += 1;
                    } else if inside(x, y, 1.6) {
                        white += 1;
                    } else if inside(x - 0.6, y - 1.4, 2.6) {
                        shadow += 1;
                    }
                }
            }
            let n = (ss * ss) as f32;
            let (b, wt, s) = (black as f32 / n, white as f32 / n, shadow as f32 / n * 0.28);
            let a = b + wt + s;
            let i = ((py * size + pxl) * 4) as usize;
            if a > 0.0 {
                let v = (wt * 255.0) / a;
                px[i] = v as u8;
                px[i + 1] = v as u8;
                px[i + 2] = v as u8;
                px[i + 3] = (a.min(1.0) * 255.0) as u8;
            }
        }
    }
    (size, size, px)
}

fn draw_pointer(frame: &mut Frame, cursor: &(u32, u32, Vec<u8>)) {
    let p = frame.pointer;
    if !p.shown {
        return;
    }
    let s = frame.scale;
    let (fw, fh) = (frame.width as i32, frame.height as i32);
    // A click: a soft ring that grows and fades for a third of a second.
    if let Some(ago) = frame.clicked_ago.filter(|a| *a < 0.35) {
        let t = (ago / 0.35) as f32;
        let r = (8.0 + 14.0 * t) * s;
        let alpha = (1.0 - t) * 0.45;
        let (cx, cy) = (p.x * s, p.y * s);
        let (x0, x1) = ((cx - r - 2.0) as i32, (cx + r + 2.0) as i32);
        let (y0, y1) = ((cy - r - 2.0) as i32, (cy + r + 2.0) as i32);
        for y in y0.max(0)..y1.min(fh) {
            for x in x0.max(0)..x1.min(fw) {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                let a = (1.0 - (d - r).abs() / (2.5 * s)).clamp(0.0, 1.0) * alpha + if d < r { alpha * 0.35 } else { 0.0 };
                if a > 0.0 {
                    let i = ((y * fw + x) * 4) as usize;
                    // The system accent blue.
                    for (c, v) in [10.0f32, 132.0, 255.0].iter().enumerate() {
                        frame.pixels[i + c] = (frame.pixels[i + c] as f32 * (1.0 - a) + v * a) as u8;
                    }
                }
            }
        }
    }
    let (cw, ch, cpx) = cursor;
    let ox = (p.x * s - 3.0 * s) as i32;
    let oy = (p.y * s - 3.0 * s) as i32;
    for y in 0..*ch as i32 {
        for x in 0..*cw as i32 {
            let (tx, ty) = (ox + x, oy + y);
            if tx < 0 || ty < 0 || tx >= fw || ty >= fh {
                continue;
            }
            let si = ((y * *cw as i32 + x) * 4) as usize;
            let a = cpx[si + 3] as f32 / 255.0;
            if a == 0.0 {
                continue;
            }
            let di = ((ty * fw + tx) * 4) as usize;
            for c in 0..3 {
                frame.pixels[di + c] = (frame.pixels[di + c] as f32 * (1.0 - a) + cpx[si + c] as f32 * a) as u8;
            }
        }
    }
}
