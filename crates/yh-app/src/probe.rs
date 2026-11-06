//! The performance run (`YH_PERF=<folder>`), the twin of web/ui/src/probe.ts and the native
//! app's PerfProbe: the same phases on the same threads ("Huge …" in the extreme preset) under
//! the same fake `claude`, through the paths a person takes (real wheel, click and key events
//! sent to the NSWindow). Frames are timed at the display link: a callback per vsync, run on
//! the main thread, so a late one is a frame where the app couldn't show what changed.

use crate::app::HarnessApp;
use crate::model::{ItemType, ThreadState};
use crate::platform;
use gpui_kit::{canvas, prelude::*, AsyncApp, Bounds, Entity, Pixels, Window, WindowHandle};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static BOUNDS: RefCell<HashMap<String, Bounds<Pixels>>> = RefCell::new(HashMap::new());
    static BATCHES: RefCell<Vec<f64>> = const { RefCell::new(Vec::new()) };
}

pub fn enabled() -> bool {
    ENABLED.with(|e| e.get())
}

/// Where an element was last drawn, for clicking it as a person would.
pub fn tracker(key: String) -> impl IntoElement {
    canvas(
        move |bounds, _, _| {
            BOUNDS.with(|b| b.borrow_mut().insert(key, bounds));
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

fn bounds(key: &str) -> Option<Bounds<Pixels>> {
    BOUNDS.with(|b| b.borrow().get(key).copied())
}

/// How long applying a batch of Claude's events took on the main thread.
pub fn note_batch(ms: f64, _count: usize) {
    if enabled() {
        BATCHES.with(|b| b.borrow_mut().push(ms));
    }
}

#[derive(Default)]
struct Frames {
    deltas: Vec<f64>,
    last: Option<Instant>,
    running: bool,
    /// Each start makes a new chain of callbacks; an old one still waiting for its next frame
    /// stops when it sees it's not the current generation.
    generation: u64,
}

fn tick(frames: Rc<RefCell<Frames>>, generation: u64, window: &Window) {
    window.on_next_frame(move |window, _| {
        {
            let mut f = frames.borrow_mut();
            if !f.running || f.generation != generation {
                return;
            }
            let now = Instant::now();
            if let Some(last) = f.last {
                f.deltas.push((now - last).as_secs_f64() * 1000.0);
            }
            f.last = Some(now);
        }
        tick(frames, generation, window);
    });
}

fn frame_stats(deltas: &[f64], period: f64) -> Value {
    let (mut vsyncs, mut missed, mut hitches, mut over50, mut over100, mut worst) = (0usize, 0usize, 0, 0, 0, 0.0f64);
    for &d in deltas {
        let spent = ((d / period).round() as usize).max(1);
        vsyncs += spent;
        missed += spent - 1;
        if spent > 1 {
            hitches += 1;
        }
        if d > 50.0 {
            over50 += 1;
        }
        if d > 100.0 {
            over100 += 1;
        }
        worst = worst.max(d);
    }
    let mut sorted = deltas.to_vec();
    sorted.sort_by(f64::total_cmp);
    let total: f64 = deltas.iter().sum();
    json!({
        "fps": if total > 0.0 { deltas.len() as f64 / total * 1000.0 } else { 0.0 },
        "framesMissedPct": if vsyncs > 0 { missed as f64 / vsyncs as f64 * 100.0 } else { 0.0 },
        "onTimePct": if vsyncs > 0 { (vsyncs - missed) as f64 / vsyncs as f64 * 100.0 } else { 0.0 },
        "hitches": hitches,
        "framesOver50ms": over50,
        "framesOver100ms": over100,
        "worstFrameMs": worst,
        "p99FrameMs": sorted.get((sorted.len() as f64 * 0.99) as usize).copied().unwrap_or(0.0),
        "frames": deltas.len(),
    })
}

struct Probe {
    handle: WindowHandle<HarnessApp>,
    cx: AsyncApp,
    started: Instant,
    folder: String,
    phases: Vec<Value>,
    clicks: Vec<Value>,
    phase: String,
    real_events: bool,
    frames: Rc<RefCell<Frames>>,
    sampler: platform::Sampler,
    ns: cocoa::base::id,
}

pub fn log(s: &str) {
    eprintln!("PERF {s}");
}

fn ms_since(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

impl Probe {
    fn with<R>(&mut self, f: impl FnOnce(&mut HarnessApp, &mut Window, &mut gpui_kit::Context<HarnessApp>) -> R) -> R {
        self.handle.update(&mut self.cx, f).expect("window gone")
    }

    /// A read, not an update: the app entity isn't leased, so views reading it (the sidebar) can.
    fn read<R>(&mut self, f: impl FnOnce(&HarnessApp, &gpui_kit::App) -> R) -> R {
        let handle = self.handle;
        self.cx
            .update(|cx| {
                let cx: &gpui_kit::App = cx;
                handle.read(cx).map(|app| f(app, cx))
            })
            .expect("window gone")
    }

    async fn sleep(&self, ms: u64) {
        self.cx.background_executor().timer(Duration::from_millis(ms)).await;
    }

    /// The next display-link callback.
    async fn frame(&mut self) {
        let (tx, rx) = flume::bounded(1);
        self.with(|_, window, _| {
            window.on_next_frame(move |_, _| {
                let _ = tx.send(());
            })
        });
        let _ = rx.recv_async().await;
    }

    /// After the next frame has been drawn: what a person sees.
    async fn painted(&mut self) -> Instant {
        // Callbacks run before the frame's drawing: the second one comes after the first draw.
        self.frame().await;
        self.frame().await;
        Instant::now()
    }

    fn direct(&mut self, why: &str) {
        if self.real_events {
            self.real_events = false;
            log(&format!("DIRECT: {why}; driving the app directly from here on"));
        }
    }

    fn selected(&mut self) -> Option<String> {
        self.read(|app, _| app.selected.clone())
    }

    async fn click_key(&mut self, key: &str) -> Option<Instant> {
        let b = bounds(key)?;
        let x = f32::from(b.origin.x) + (f32::from(b.size.width) / 2.0).min(60.0);
        let y = f32::from(b.origin.y) + f32::from(b.size.height) / 2.0;
        let sent = Instant::now();
        platform::click(self.ns, x, y);
        Some(sent)
    }

    /// The thread's sidebar row in view, behind its project's "Show more" as for a person.
    async fn reveal(&mut self, t: &Entity<ThreadState>) -> bool {
        let (id, project) = self.read(|_, cx| {
            let t = t.read(cx);
            (t.id().to_string(), t.meta.project_id.clone())
        });
        let mut index = self.read(|app, cx| app.sidebar.read(cx).row_index(&id, cx));
        if index.is_none() {
            if let Some(more) = self.read(|app, cx| app.sidebar.read(cx).more_index(&project, cx)) {
                self.with(|app, _, cx| app.sidebar.update(cx, |s, cx| {
                    s.scroll.scroll_to_item(more);
                    cx.notify();
                }));
                self.painted().await;
                let key = format!("more:{project}");
                let clicked = if self.real_events { self.click_key(&key).await.is_some() } else { false };
                self.painted().await;
                index = self.read(|app, cx| app.sidebar.read(cx).row_index(&id, cx));
                if index.is_none() && clicked {
                    self.direct("clicks don't reach the window");
                }
                if index.is_none() {
                    return false;
                }
            }
        }
        let Some(index) = index else { return false };
        self.with(|app, _, cx| app.sidebar.update(cx, |s, cx| {
            s.scroll.scroll_to_item(index);
            cx.notify();
        }));
        self.painted().await;
        true
    }

    async fn switch_to(&mut self, t: &Entity<ThreadState>) {
        let id = self.read(|_, cx| t.read(cx).id().to_string());
        let title = self.read(|_, cx| t.read(cx).title.clone());
        if !self.reveal(t).await {
            log(&format!("FAIL: no sidebar row for {title}"));
        }
        let mut sent = Instant::now();
        let mut selected = false;
        for _attempt in 0..3 {
            if !self.real_events {
                break;
            }
            match self.click_key(&format!("thread:{id}")).await {
                Some(at) => sent = at,
                None => break,
            }
            let start = Instant::now();
            while self.selected().as_deref() != Some(id.as_str()) && start.elapsed() < Duration::from_secs(1) {
                self.frame().await;
            }
            if self.selected().as_deref() == Some(id.as_str()) {
                selected = true;
                break;
            }
            // The first click on a window in the background can go to activating it.
            log(&format!("click on {title} didn't select it; clicking again"));
        }
        if !selected {
            if self.real_events {
                self.direct("clicks don't reach the window");
            }
            sent = Instant::now();
            let id2 = id.clone();
            self.with(move |app, window, cx| app.select(&id2, window, cx));
        }
        let was_loaded = self.read(|_, cx| t.read(cx).loaded);
        let answered = self.painted().await;
        let mut done = answered;
        if !was_loaded {
            let start = Instant::now();
            while !self.read(|_, cx| t.read(cx).loaded) && start.elapsed() < Duration::from_secs(30) {
                self.frame().await;
            }
            done = self.painted().await;
        }
        let rows = self.read(|_, cx| t.read(cx).items.len());
        let phase = self.phase.clone();
        self.clicks.push(json!({
            "phase": phase, "click": "switch",
            "answeredMs": (answered - sent).as_secs_f64() * 1000.0,
            "doneMs": (done - sent).as_secs_f64() * 1000.0,
            "rows": rows,
        }));
    }

    fn transcript_center(&self) -> Option<(f32, f32)> {
        let b = bounds("transcript")?;
        Some((
            f32::from(b.origin.x) + f32::from(b.size.width) / 2.0,
            f32::from(b.origin.y) + f32::from(b.size.height) / 2.0,
        ))
    }

    fn scroll_offset(&mut self) -> f32 {
        self.read(|app, cx| {
            app.selected_thread().map(|t| f32::from(t.read(cx).list.scroll_px_offset_for_scrollbar().y)).unwrap_or(0.0)
        })
    }

    async fn scrolling(&mut self, until: Instant, speed: i32) {
        let (mut direction, mut count) = (1, 0);
        let (mut lowest, mut highest) = (f32::MAX, f32::MIN);
        while Instant::now() < until {
            let before = self.scroll_offset();
            if self.real_events {
                if let Some((x, y)) = self.transcript_center() {
                    platform::wheel(self.ns, x, y, speed * direction);
                }
            } else {
                let dy = (speed * direction) as f32;
                self.with(move |app, window, cx| {
                    if let Some(t) = app.selected_thread() {
                        t.read(cx).list.scroll_by(gpui_kit::px(-dy));
                    }
                    window.refresh();
                });
            }
            self.sleep(8).await;
            let now = self.scroll_offset();
            if self.real_events && count == 20 && highest - lowest < 1.0 && now == before {
                self.direct("wheel events don't reach the window");
            }
            count += 1;
            lowest = lowest.min(now);
            highest = highest.max(now);
            if count % 200 == 0 {
                direction = -direction;
            }
        }
        let top = self.read(|app, cx| app.selected_thread().map(|t| t.read(cx).list.logical_scroll_top().item_ix).unwrap_or(0));
        log(&format!("scroll: {count} steps, ended at row {top}"));
        if highest - lowest < 100.0 {
            log(&format!("FAIL: scrolling moved the transcript only {} px", (highest - lowest).round()));
        } else {
            log(&format!("scrolled across {} px", (highest - lowest).round()));
        }
    }

    async fn switching(&mut self, list: &[Entity<ThreadState>], until: Instant, every: u64) {
        let selected = self.selected();
        let mut index = list.iter().position(|t| self.read(|_, cx| Some(t.read(cx).id().to_string())) == selected).unwrap_or(0);
        while Instant::now() < until {
            index = (index + 1) % list.len();
            self.switch_to(&list[index]).await;
            self.sleep(every).await;
        }
    }

    async fn typing(&mut self, until: Instant) {
        // Into the field itself (the composer's first line), as you'd click to type.
        let Some(b) = bounds("composer") else {
            log("FAIL: no composer");
            return;
        };
        if self.real_events {
            let (x, y) = (f32::from(b.origin.x) + 60.0, f32::from(b.origin.y) + 20.0);
            platform::click(self.ns, x, y);
        }
        self.painted().await;
        let letters: Vec<char> = "the quick brown fox jumps over the lazy dog ".chars().collect();
        let mut i = 0;
        while Instant::now() < until {
            let ch = letters[i % letters.len()];
            i += 1;
            let before = self.read(|app, cx| app.composer.read(cx).input.read(cx).value().len());
            let sent = Instant::now();
            if self.real_events {
                platform::key(self.ns, &ch.to_string());
            } else {
                self.with(move |app, window, cx| {
                    let input = app.composer.read(cx).input.clone();
                    input.update(cx, |i, cx| {
                        let text = format!("{}{ch}", i.value());
                        i.set_value(text, window, cx);
                    });
                });
            }
            let start = Instant::now();
            let mut landed = false;
            while start.elapsed() < Duration::from_secs(1) {
                if self.read(|app, cx| app.composer.read(cx).input.read(cx).value().len()) != before {
                    landed = true;
                    break;
                }
                self.frame().await;
            }
            if !landed {
                self.direct("keys don't reach the composer");
                continue;
            }
            let shown = self.painted().await;
            let phase = self.phase.clone();
            let ms = (shown - sent).as_secs_f64() * 1000.0;
            self.clicks.push(json!({ "phase": phase, "click": "key", "answeredMs": ms, "doneMs": ms }));
            self.sleep(90).await;
            if i % 60 == 0 {
                self.with(|app, window, cx| app.composer.read(cx).input.clone().update(cx, |i, cx| i.set_value("", window, cx)));
            }
        }
        self.with(|app, window, cx| app.composer.read(cx).input.clone().update(cx, |i, cx| i.set_value("", window, cx)));
    }

    async fn start_turns(&mut self, list: &[Entity<ThreadState>], seconds: f64, rate: &str) -> Instant {
        let begin = Instant::now();
        let marks: Vec<usize> = list.iter().map(|t| self.read(|_, cx| t.read(cx).items.len())).collect();
        for t in list {
            let t = t.clone();
            let message = format!("PERF seconds={} rate={rate}", seconds.round());
            self.with(move |app, _, cx| {
                if !t.read(cx).running {
                    app.send(&t, &message, cx);
                }
            });
        }
        let streaming = |p: &mut Probe| {
            list.iter().zip(&marks).all(|(t, mark)| {
                p.read(|_, cx| {
                    let t = t.read(cx);
                    t.running && t.items.len() > mark + 1 && t.items.last().is_some_and(|i| i.ty != ItemType::User)
                })
            })
        };
        while !streaming(self) && begin.elapsed() < Duration::from_secs(120) {
            self.sleep(200).await;
        }
        log(&format!("{} turns streaming after {:.1} s", list.len(), begin.elapsed().as_secs_f64()));
        Instant::now() + Duration::from_secs_f64(seconds)
    }

    async fn wait_for_turns(&mut self) {
        let limit = Instant::now() + Duration::from_secs(60);
        while self.read(|app, cx| app.running_count(cx)) > 0 && Instant::now() < limit {
            self.sleep(250).await;
        }
    }

    fn write(&self) {
        let save = |value: &Value, name: &str| {
            let path = std::path::Path::new(&self.folder).join(name);
            let _ = std::fs::write(path, serde_json::to_string_pretty(value).unwrap());
        };
        save(&Value::Array(self.phases.clone()), "phases.json");
        save(&Value::Array(self.clicks.clone()), "clicks.json");
    }
}

pub fn start(handle: WindowHandle<HarnessApp>, folder: String, cx: &mut gpui_kit::App) {
    ENABLED.with(|e| e.set(true));
    let _ = std::fs::create_dir_all(&folder);
    cx.spawn(async move |cx| {
        let mut p = Probe {
            handle,
            cx: cx.clone(),
            started: Instant::now(),
            folder,
            phases: vec![],
            clicks: vec![],
            phase: String::new(),
            real_events: true,
            frames: Rc::new(RefCell::new(Frames::default())),
            sampler: platform::Sampler::new(),
            ns: cocoa::base::nil,
        };
        p.ns = p.with(|_, window, _| platform::ns_window(window));
        run(&mut p).await;
        log("DONE");
        p.write();
        if std::env::var("YH_PERF_QUIT").as_deref() == Ok("1") {
            p.sleep(500).await;
            let _ = p.cx.update(|cx| {
                if let Ok(()) = p.handle.update(cx, |app, _, _| app.stop_all()) {}
                cx.quit()
            });
        }
    })
    .detach();
}

async fn run(p: &mut Probe) {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let seconds: f64 = env("YH_PERF_SECONDS").and_then(|v| v.parse().ok()).unwrap_or(30.0);
    let only: Option<Vec<String>> = env("YH_PERF_PHASES").map(|v| v.split(',').map(str::to_string).collect());
    let rate = env("YH_PERF_RATE").unwrap_or_else(|| "1".into());
    let front = env("YH_PERF_FRONT").as_deref() == Some("1");

    p.sleep(1500).await;
    let size = p.with(move |_, window, _| platform::place_for_probe(window, front));
    let screen_hz = p.with(|_, window, _| platform::screen_hz(window));
    log(&format!("window {size}, screen {screen_hz} Hz, front {front}"));
    p.sleep(500).await;

    let mut threads: Vec<(String, Entity<ThreadState>)> = p.read(|app, cx| {
        app.threads.iter().map(|t| (t.read(cx).title.clone(), t.clone())).filter(|(title, _)| title.starts_with("Huge")).collect()
    });
    threads.sort_by(|a, b| a.0.cmp(&b.0));
    let threads: Vec<Entity<ThreadState>> = threads.into_iter().map(|(_, t)| t).collect();
    if threads.is_empty() {
        log("FAIL: no threads named Huge");
        return;
    }

    // The app's frame period, from a warm-up: GPUI draws at the screen's rate.
    start_frames(p);
    p.sleep(1500).await;
    let warm = stop_frames(p);
    let mut sorted = warm.clone();
    sorted.sort_by(f64::total_cmp);
    let median = sorted.get(sorted.len() / 2).copied().unwrap_or(1000.0 / 60.0);
    let period = if median < 12.0 { 1000.0 / screen_hz } else { 1000.0 / 60.0 };
    log(&format!(
        "frames: median {median:.2} ms → judging against {:.0} Hz (screen {screen_hz} Hz, warm-up {} frames)",
        1000.0 / period,
        warm.len()
    ));

    // A pointer moving over the window now and then, as yours does: hover is part of the cost.
    let pointer = Rc::new(Cell::new(true));
    {
        let pointer = pointer.clone();
        let handle = p.handle;
        let cx = p.cx.clone();
        cx.clone()
            .spawn(async move |_| {
                let mut seed: u64 = 0x2545F4914F6CDD1D;
                while pointer.get() {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    let (rx, ry) = ((seed % 1000) as f32 / 1000.0, ((seed >> 20) % 1000) as f32 / 1000.0);
                    let mut cx2 = cx.clone();
                    let target = handle.update(&mut cx2, |_, window, _| (platform::ns_window(window), window.viewport_size()));
                    if let Ok((ns, size)) = target {
                        platform::mouse_move(ns, 20.0 + rx * (f32::from(size.width) - 40.0), 60.0 + ry * (f32::from(size.height) - 160.0));
                    }
                    cx.background_executor().timer(Duration::from_millis(500)).await;
                }
            })
            .detach();
    }

    log(&format!(
        "{} huge threads, phases {}, {seconds} s each",
        threads.len(),
        only.as_ref().map_or("all".to_string(), |o| o.join(","))
    ));
    p.switch_to(&threads[0]).await;
    p.sleep(1000).await;

    let names = [
        "open-huge",
        "idle-huge",
        "scroll-huge",
        "scroll-huge-fast",
        "switch-huge",
        "switch-all-idle",
        "stream-1",
        "stream-all",
        "stream-all-scroll",
        "stream-all-switch",
        "stream-all-typing",
    ];
    for name in names {
        if only.as_ref().is_some_and(|o| !o.iter().any(|n| n == name)) {
            continue;
        }
        p.phase = name.to_string();
        if name.starts_with("stream") {
            p.wait_for_turns().await;
        }
        p.sampler.sample();
        let started = Instant::now();
        let clicks_before = p.clicks.len();
        BATCHES.with(|b| b.borrow_mut().clear());
        start_frames(p);

        // Memory and CPU each second, alongside.
        let samples: Rc<RefCell<Vec<(f64, f64)>>> = Rc::new(RefCell::new(vec![]));
        let sampling = Rc::new(Cell::new(true));
        {
            let (samples, sampling) = (samples.clone(), sampling.clone());
            let cx = p.cx.clone();
            cx.clone()
                .spawn(async move |_| {
                    let mut sampler = platform::Sampler::new();
                    while sampling.get() {
                        cx.background_executor().timer(Duration::from_secs(1)).await;
                        samples.borrow_mut().push(sampler.sample());
                    }
                })
                .detach();
        }

        let s = Duration::from_secs_f64(seconds);
        match name {
            "open-huge" => {
                p.switch_to(&threads[0]).await;
                p.sleep(5000).await;
            }
            "idle-huge" => p.sleep((seconds * 1000.0) as u64).await,
            "scroll-huge" => p.scrolling(Instant::now() + s, 40).await,
            "scroll-huge-fast" => p.scrolling(Instant::now() + s, 200).await,
            "switch-huge" => p.switching(&threads[..4.min(threads.len())], Instant::now() + s, 1500).await,
            "switch-all-idle" => {
                for t in &threads {
                    let t2 = t.clone();
                    p.with(move |app, _, cx| app.ensure_loaded(&t2, cx));
                }
                p.switching(&threads, Instant::now() + s, 1500).await;
            }
            "stream-1" => {
                let end = p.start_turns(&threads[..1], seconds, &rate).await;
                p.sleep(end.saturating_duration_since(Instant::now()).as_millis() as u64).await;
            }
            "stream-all" => {
                let end = p.start_turns(&threads, seconds, &rate).await;
                p.sleep(end.saturating_duration_since(Instant::now()).as_millis() as u64).await;
            }
            "stream-all-scroll" => {
                let end = p.start_turns(&threads, seconds, &rate).await;
                p.scrolling(end, 40).await;
            }
            "stream-all-switch" => {
                let end = p.start_turns(&threads, seconds, &rate).await;
                p.switching(&threads, end, 1500).await;
            }
            "stream-all-typing" => {
                let end = p.start_turns(&threads, seconds, &rate).await;
                p.typing(end).await;
            }
            _ => {}
        }

        let deltas = stop_frames(p);
        sampling.set(false);
        let (end_mb, _) = p.sampler.sample();
        let samples = samples.borrow().clone();
        let mut stats = frame_stats(&deltas, period);
        let mut batches = BATCHES.with(|b| b.borrow().clone());
        batches.sort_by(f64::total_cmp);
        let phase_clicks: Vec<Value> = p.clicks[clicks_before..].to_vec();
        let mut done: Vec<f64> = phase_clicks.iter().filter_map(|c| c["doneMs"].as_f64()).collect();
        done.sort_by(f64::total_cmp);
        let cpu = if samples.is_empty() { 0.0 } else { samples.iter().map(|s| s.1).sum::<f64>() / samples.len() as f64 };
        let peak = samples.iter().map(|s| s.0).fold(end_mb, f64::max);
        let valid = platform::run_is_valid();
        let extra = json!({
            "phase": name,
            "seconds": started.elapsed().as_secs_f64(),
            "judgedHz": 1000.0 / period,
            "footprintEndMB": end_mb,
            "footprintPeakMB": peak,
            "cpuPct": cpu,
            "realEvents": p.real_events,
            "eventBatches": batches.len(),
            "eventBatchWorstMs": batches.last().copied().unwrap_or(0.0),
            "eventBatchP99Ms": batches.get((batches.len() as f64 * 0.99) as usize).copied().unwrap_or(0.0),
            "clicks": phase_clicks.len(),
            "clickWorstMs": done.last().copied().unwrap_or(0.0),
            "clickMedianMs": done.get(done.len() / 2).copied().unwrap_or(0.0),
            "valid": valid,
        });
        for (k, v) in extra.as_object().unwrap() {
            stats[k] = v.clone();
        }
        log(&format!(
            "{name}: {:.0} fps, on time {:.1}%, worst {:.0} ms, p99 {:.0} ms, >50ms {}, events worst {:.0} ms, mem {:.0} MB, cpu {:.0}%{}",
            stats["fps"].as_f64().unwrap_or(0.0),
            stats["onTimePct"].as_f64().unwrap_or(0.0),
            stats["worstFrameMs"].as_f64().unwrap_or(0.0),
            stats["p99FrameMs"].as_f64().unwrap_or(0.0),
            stats["framesOver50ms"],
            stats["eventBatchWorstMs"].as_f64().unwrap_or(0.0),
            peak,
            cpu,
            if done.is_empty() {
                String::new()
            } else {
                format!(", clicks median {:.0} ms worst {:.0} ms", done[done.len() / 2], done[done.len() - 1])
            }
        ));
        p.phases.push(stats);
        p.write();
    }
    pointer.set(false);
    p.wait_for_turns().await;
    let _ = ms_since(p.started);
}

fn start_frames(p: &mut Probe) {
    {
        let mut f = p.frames.borrow_mut();
        f.deltas.clear();
        f.last = None;
        f.running = true;
        f.generation += 1;
    }
    let frames = p.frames.clone();
    let generation = frames.borrow().generation;
    p.with(move |_, window, _| tick(frames, generation, window));
}

fn stop_frames(p: &mut Probe) -> Vec<f64> {
    let mut f = p.frames.borrow_mut();
    f.running = false;
    std::mem::take(&mut f.deltas)
}
