#![allow(deprecated)]
//! The performance run (`YH_PERF=<folder>`): fixed phases on the
//! `extreme` preset's "Huge …" threads under a fake `claude`, through the paths a person takes (real wheel, click and key events
//! sent to the NSWindow). Frames are timed at the display link: a callback per vsync, run on
//! the main thread, so a late one is a frame where the app couldn't show what changed.

use crate::state::workspace::Workspace;
use crate::ui::root::RootView;
use crate::state::thread::{ItemType, ThreadState};
use crate::platform;
use gpui_kit::{canvas, prelude::*, AnyWindowHandle, AsyncApp, Bounds, Entity, Pixels, Window};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static BOUNDS: RefCell<HashMap<String, Bounds<Pixels>>> = RefCell::new(HashMap::new());
    static BATCHES: RefCell<Vec<f64>> = const { RefCell::new(Vec::new()) };
    static PHASE: RefCell<String> = const { RefCell::new(String::new()) };
    /// What the probe did last, for the `PERF stall` lines: what led to a long frame.
    static ACTION: RefCell<String> = const { RefCell::new(String::new()) };
    /// A drag holds the mouse button: the pointer that wanders over the window waits.
    static DRAGGING: Cell<bool> = const { Cell::new(false) };
    static STARTED: Instant = Instant::now();
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

/// Elements tracked without a perf run: for demo recordings (`demo.rs`).
pub fn enable_tracking() {
    ENABLED.with(|e| e.set(true));
}

pub fn tracked(key: &str) -> Option<Bounds<Pixels>> {
    bounds(key)
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
                let ms = (now - last).as_secs_f64() * 1000.0;
                // A long frame says when and in what phase, to line up with what was happening.
                if ms > 50.0 {
                    let phase = PHASE.with(|p| p.borrow().clone());
                    let action = ACTION.with(|a| a.borrow().clone());
                    let after = if action.is_empty() { String::new() } else { format!(" after {action}") };
                    eprintln!("PERF stall {ms:.0} ms in {phase} at {:.1} s{after}", STARTED.with(|s| s.elapsed().as_secs_f64()));
                }
                f.deltas.push(ms);
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
    handle: AnyWindowHandle,
    root: Entity<RootView>,
    cx: AsyncApp,
    started: Instant,
    folder: String,
    phases: Vec<Value>,
    clicks: Vec<Value>,
    phase: String,
    real_events: bool,
    /// A long file for the Files tab (`files`): the extreme preset's folders are empty.
    file: String,
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
    /// In the window: for sending events and changing state.
    fn with<R>(&mut self, f: impl FnOnce(&Entity<RootView>, &mut Window, &mut gpui_kit::App) -> R) -> R {
        let root = self.root.clone();
        self.handle.update(&mut self.cx, |_, window, cx| f(&root, window, cx)).expect("window gone")
    }

    fn ws(&mut self) -> Entity<Workspace> {
        let root = self.root.clone();
        self.cx.update(|cx| root.read(cx).workspace.clone())
    }

    /// Changes the workspace (selection, sending…), as the views' own calls do.
    fn ws_update<R>(&mut self, f: impl FnOnce(&mut Workspace, &mut gpui_kit::Context<Workspace>) -> R) -> R {
        let ws = self.ws();
        self.cx.update(|cx| ws.update(cx, f))
    }

    /// A read, not an update: nothing is leased, so views reading the same entities can.
    fn read<R>(&mut self, f: impl FnOnce(&RootView, &Workspace, &gpui_kit::App) -> R) -> R {
        let root = self.root.clone();
        self.cx.update(|cx| {
            let cx: &gpui_kit::App = cx;
            let r = root.read(cx);
            f(r, r.workspace.read(cx), cx)
        })
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
        self.read(|_, ws, _| ws.selected.clone())
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
        let (id, project) = self.read(|_, _, cx| {
            let t = t.read(cx);
            (t.id().to_string(), t.meta.project_id.clone())
        });
        // Hidden (⌃⌘S): shown first, as you'd do to click in it.
        if self.read(|r, _, _| r.sidebar_hidden()) {
            self.with(|_, window, cx| window.dispatch_action(Box::new(crate::ui::root::ToggleSidebar), cx));
            self.painted().await;
        }
        let mut index = self.read(|r, _, cx| r.sidebar.read(cx).row_index(&id, cx));
        if index.is_none() {
            if let Some(more) = self.read(|r, _, cx| r.sidebar.read(cx).more_index(&project, cx)) {
                self.with(|r, _, cx| r.read(cx).sidebar.clone().update(cx, |s, cx| {
                    s.scroll.scroll_to_item(more);
                    cx.notify();
                }));
                self.painted().await;
                let key = format!("more:{project}");
                let clicked = if self.real_events { self.click_key(&key).await.is_some() } else { false };
                self.painted().await;
                index = self.read(|r, _, cx| r.sidebar.read(cx).row_index(&id, cx));
                if index.is_none() && clicked {
                    self.direct("clicks don't reach the window");
                }
                if index.is_none() {
                    return false;
                }
            }
        }
        let Some(index) = index else { return false };
        self.with(|r, _, cx| r.read(cx).sidebar.clone().update(cx, |s, cx| {
            s.scroll.scroll_to_item(index);
            cx.notify();
        }));
        self.painted().await;
        true
    }

    async fn switch_to(&mut self, t: &Entity<ThreadState>) {
        let id = self.read(|_, _, cx| t.read(cx).id().to_string());
        let title = self.read(|_, _, cx| t.read(cx).title.clone());
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
            let b = bounds(&format!("thread:{id}"));
            let (hidden, inspector, page) = self.read(|r, ws, _| (r.sidebar_hidden(), ws.shows_inspector, ws.page.clone()));
            let size = self.with(|_, w, _| w.viewport_size());
            log(&format!("click on {title} didn't select it (row at {b:?}, window {size:?}, sidebar hidden {hidden}, right sidebar {inspector}, page {page:?}); clicking again"));
        }
        if !selected {
            if self.real_events {
                self.direct("clicks don't reach the window");
            }
            sent = Instant::now();
            let id2 = id.clone();
            self.ws_update(move |ws, cx| ws.select(&id2, cx));
        }
        let was_loaded = self.read(|_, _, cx| t.read(cx).loaded);
        let answered = self.painted().await;
        let mut done = answered;
        if !was_loaded {
            let start = Instant::now();
            while !self.read(|_, _, cx| t.read(cx).loaded) && start.elapsed() < Duration::from_secs(30) {
                self.frame().await;
            }
            done = self.painted().await;
        }
        let rows = self.read(|_, _, cx| t.read(cx).items.len());
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
        self.read(|_, ws, cx| ws.selected_thread().map(|t| f32::from(t.read(cx).list.scroll_px_offset_for_scrollbar().y)).unwrap_or(0.0))
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
                self.with(move |r, window, cx| {
                    if let Some(t) = r.read(cx).workspace.read(cx).selected_thread() {
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
        let top = self.read(|_, ws, cx| ws.selected_thread().map(|t| t.read(cx).list.logical_scroll_top().item_ix).unwrap_or(0));
        log(&format!("scroll: {count} steps, ended at row {top}"));
        if highest - lowest < 100.0 {
            log(&format!("FAIL: scrolling moved the transcript only {} px", (highest - lowest).round()));
        } else {
            log(&format!("scrolled across {} px", (highest - lowest).round()));
        }
    }

    async fn switching(&mut self, list: &[Entity<ThreadState>], until: Instant, every: u64) {
        let selected = self.selected();
        let mut index = list.iter().position(|t| self.read(|_, _, cx| Some(t.read(cx).id().to_string())) == selected).unwrap_or(0);
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
            let before = self.read(|r, _, cx| r.composer.read(cx).input.read(cx).value().len());
            let sent = Instant::now();
            if self.real_events {
                platform::key(self.ns, &ch.to_string());
            } else {
                self.with(move |r, window, cx| {
                    let input = r.read(cx).composer.read(cx).input.clone();
                    input.update(cx, |i, cx| {
                        let text = format!("{}{ch}", i.value());
                        i.set_value(text, window, cx);
                    });
                });
            }
            let start = Instant::now();
            let mut landed = false;
            while start.elapsed() < Duration::from_secs(1) {
                if self.read(|r, _, cx| r.composer.read(cx).input.read(cx).value().len()) != before {
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
                self.with(|r, window, cx| r.read(cx).composer.read(cx).input.clone().update(cx, |i, cx| i.set_value("", window, cx)));
            }
        }
        self.with(|r, window, cx| r.read(cx).composer.read(cx).input.clone().update(cx, |i, cx| i.set_value("", window, cx)));
    }

    async fn start_turns(&mut self, list: &[Entity<ThreadState>], seconds: f64, rate: &str) -> Instant {
        let begin = Instant::now();
        let marks: Vec<usize> = list.iter().map(|t| self.read(|_, _, cx| t.read(cx).items.len())).collect();
        for t in list {
            let t = t.clone();
            let message = format!("PERF seconds={} rate={rate}", seconds.round());
            self.ws_update(move |ws, cx| {
                if !t.read(cx).run.running {
                    ws.send(&t, &message, false, cx);
                }
            });
        }
        let streaming = |p: &mut Probe| {
            list.iter().zip(&marks).all(|(t, mark)| {
                p.read(|_, _, cx| {
                    let t = t.read(cx);
                    t.run.running && t.items.len() > mark + 1 && t.items.last().is_some_and(|i| i.ty != ItemType::User)
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
        while self.read(|_, ws, cx| ws.running_count(cx)) > 0 && Instant::now() < limit {
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

/// What `everything` runs, in this order, each round (`YH_PERF_ONLY=<a,b>` picks some).
const GROUPS: [&str; 17] = [
    "type", "draft", "tabs", "files", "inspector-resize", "spotlight", "find", "jump", "work", "earlier", "sidebar", "pages",
    "settings", "new-thread", "jobs", "resize", "sidebar-toggle",
];

/// What `chaos` picks from, and how often.
const CHAOS: [(&str, u64); 33] = [
    ("switch", 12),
    ("switch-ordinary", 2),
    ("jump-top", 3),
    ("jump-bottom", 3),
    ("jump-previous", 4),
    ("jump-next", 3),
    ("wheel-up", 6),
    ("wheel-down", 4),
    ("wheel-fling", 2),
    ("work", 4),
    ("big-tool", 2),
    ("earlier-turns", 1),
    ("inspector", 3),
    ("tab", 4),
    ("inspector-resize", 1),
    ("files", 2),
    ("spotlight", 2),
    ("find", 2),
    ("type", 3),
    ("draft", 1),
    ("new-thread", 2),
    ("project-page", 2),
    ("memory", 1),
    ("usage", 1),
    ("settings", 1),
    ("sidebar-order", 2),
    ("undo", 1),
    ("resize", 2),
    ("jobs", 1),
    ("sidebar-toggle", 1),
    ("stop", 1),
    ("send", 2),
    ("pages-back", 1),
];

/// The chaos run's dice: xorshift, so a seed plays the same run again.
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

fn ws_of(r: &Entity<RootView>, cx: &gpui_kit::App) -> Entity<Workspace> {
    r.read(cx).workspace.clone()
}

/// The open thread's last finished turn's work: (its summary row, its first row's number,
/// whether it's open).
fn last_work(t: &ThreadState) -> Option<(usize, i64, bool)> {
    (0..t.items.len()).rev().find_map(|ix| match t.fold_at(ix) {
        crate::state::thread::Fold::Summary { open, .. } => Some((ix, t.items[ix].pos, open)),
        _ => None,
    })
}

impl Probe {
    fn note(&self, what: &str) {
        ACTION.with(|a| *a.borrow_mut() = what.to_string());
    }

    fn record(&mut self, kind: &str, sent: Instant, answered: Instant, done: Instant) {
        let phase = self.phase.clone();
        self.clicks.push(json!({
            "phase": phase, "click": kind,
            "answeredMs": (answered - sent).as_secs_f64() * 1000.0,
            "doneMs": (done - sent).as_secs_f64() * 1000.0,
            "at": STARTED.with(|s| s.elapsed().as_secs_f64()),
        }));
    }

    /// Does `f` in the window, timed to the frame that shows it.
    async fn act(&mut self, kind: &str, f: impl FnOnce(&Entity<RootView>, &mut Window, &mut gpui_kit::App)) {
        self.note(kind);
        let sent = Instant::now();
        self.with(f);
        let shown = self.painted().await;
        self.record(kind, sent, shown, shown);
    }

    /// As `act`, done once `done` holds (what's asynchronous: a page read in, results found).
    async fn act_until(
        &mut self,
        kind: &str,
        f: impl FnOnce(&Entity<RootView>, &mut Window, &mut gpui_kit::App),
        done: impl Fn(&RootView, &Workspace, &gpui_kit::App) -> bool,
        limit_ms: u64,
    ) {
        self.note(kind);
        let sent = Instant::now();
        self.with(f);
        let answered = self.painted().await;
        let mut finished = answered;
        if !self.read(&done) {
            let start = Instant::now();
            while !self.read(&done) && start.elapsed() < Duration::from_millis(limit_ms) {
                self.frame().await;
            }
            finished = self.painted().await;
        }
        self.record(kind, sent, answered, finished);
    }

    /// A workspace call, as a view's button makes it.
    async fn act_ws(&mut self, kind: &str, f: impl FnOnce(&mut Workspace, &mut gpui_kit::Context<Workspace>)) {
        self.act(kind, move |r, _, cx| ws_of(r, cx).update(cx, f)).await;
    }

    async fn dispatch(&mut self, kind: &str, action: Box<dyn gpui_kit::Action>) {
        self.act(kind, move |_, window, cx| window.dispatch_action(action, cx)).await;
    }

    /// A shortcut typed (`key` with its key code, ⌘ and maybe ⇧) through the window. When
    /// `landed` is given and doesn't hold soon after, that's `WRONG`, and its action is
    /// dispatched instead.
    async fn press(
        &mut self,
        kind: &str,
        (key, code, shift): (&str, u16, bool),
        action: Box<dyn gpui_kit::Action>,
        landed: Option<&dyn Fn(&RootView, &Workspace, &gpui_kit::App) -> bool>,
    ) {
        self.note(kind);
        let sent = Instant::now();
        platform::shortcut(self.ns, key, code, shift);
        let answered = self.painted().await;
        if let Some(landed) = landed {
            let start = Instant::now();
            while !self.read(landed) && start.elapsed() < Duration::from_millis(500) {
                self.frame().await;
            }
            if !self.read(landed) {
                // Once, a closed Spotlight or page kept the keyboard and shortcuts went nowhere:
                // each time is told, and the action dispatched to go on.
                self.wrong(&format!("{kind} as a key event didn't land; dispatching its action"));
                return self.dispatch(kind, action).await;
            }
        }
        self.record(kind, sent, answered, answered);
    }

    /// Letters typed into whatever has focus, each timed.
    async fn type_keys(&mut self, kind: &str, text: &str, gap: u64) {
        for ch in text.chars() {
            self.note(kind);
            let sent = Instant::now();
            platform::key(self.ns, &ch.to_string());
            let shown = self.painted().await;
            self.record(kind, sent, shown, shown);
            self.sleep(gap).await;
        }
    }

    fn composer_text(&mut self) -> String {
        self.read(|r, _, cx| r.composer.read(cx).input.read(cx).value().to_string())
    }

    async fn set_composer(&mut self, kind: &str, text: String) {
        self.act(kind, move |r, window, cx| {
            let input = r.read(cx).composer.read(cx).input.clone();
            input.update(cx, |i, cx| i.set_value(text, window, cx));
        })
        .await;
    }

    /// A click into the composer, as you'd click to type.
    async fn focus_composer(&mut self) -> bool {
        let Some(b) = bounds("composer") else { return false };
        platform::click(self.ns, f32::from(b.origin.x) + 60.0, f32::from(b.origin.y) + 20.0);
        self.painted().await;
        true
    }

    async fn wheel_at(&mut self, x: f32, y: f32, steps: usize, dy: i32) {
        self.note(&format!("wheel {dy}"));
        for _ in 0..steps {
            platform::wheel(self.ns, x, y, dy);
            self.sleep(8).await;
        }
        self.painted().await;
    }

    async fn wheel_transcript(&mut self, steps: usize, dy: i32) {
        if let Some((x, y)) = self.transcript_center() {
            self.wheel_at(x, y, steps, dy).await;
        }
    }

    fn selected_thread(&mut self) -> Option<Entity<ThreadState>> {
        self.read(|_, ws, _| ws.selected_thread())
    }

    fn page_open(&mut self) -> bool {
        self.read(|_, ws, _| ws.page.is_some())
    }

    // The groups, each of them used by both `everything` and `chaos`.

    async fn g_type(&mut self) {
        if !self.focus_composer().await {
            return;
        }
        let before = self.composer_text();
        self.type_keys("type-key", "fix the parser", 40).await;
        let after = self.composer_text();
        if !after.ends_with("fix the parser") {
            log(&format!("FAIL: typed \"fix the parser\", the composer has \"{}\" (was \"{}\")", after.chars().take(40).collect::<String>(), before.chars().take(40).collect::<String>()));
        }
        self.set_composer("clear-draft", String::new()).await;
    }

    /// A long draft (as pasted: 120 lines), typed at, then cleared.
    async fn g_draft(&mut self) {
        let text: String = (0..120).map(|i| format!("Line {i}: the parser drops the timezone when the migration runs twice\n")).collect();
        self.set_composer("long-draft", text).await;
        if self.focus_composer().await {
            self.type_keys("draft-key", "and more", 40).await;
        }
        self.set_composer("clear-draft", String::new()).await;
    }

    async fn tab(&mut self, panel: crate::state::workspace::Panel) {
        let kind = format!("tab-{}", panel.label().to_lowercase().replace(' ', "-"));
        self.act_ws(&kind, move |ws, cx| ws.show_panel(panel, cx)).await;
    }

    async fn g_tabs(&mut self, list: &[Entity<ThreadState>], index: usize) {
        use crate::state::workspace::Panel;
        for panel in Panel::ALL {
            self.tab(panel).await;
            self.sleep(300).await;
        }
        // The browser shown, then hidden by another tab.
        self.tab(Panel::Browser).await;
        self.sleep(300).await;
        self.act_ws("browser-hide", |ws, cx| ws.show_panel(Panel::PullRequests, cx)).await;
        self.sleep(300).await;
        // And switching with it open, as you'd have it.
        if list.len() > 1 {
            self.switch_to(&list[(index + 1) % list.len()]).await;
            self.sleep(300).await;
            self.switch_to(&list[index]).await;
            self.sleep(300).await;
        }
        self.inspector(false).await;
    }

    async fn inspector(&mut self, on: bool) {
        let shows = self.read(|_, ws, _| ws.shows_inspector);
        if shows != on {
            self.dispatch(if on { "inspector-open" } else { "inspector-close" }, Box::new(crate::ui::root::ToggleInspector)).await;
        }
    }

    /// A long file opened as a link does, scrolled by the wheel over it, its tab left.
    async fn g_files(&mut self) {
        if self.page_open() || self.selected_thread().is_none() {
            return;
        }
        let path = self.file.clone();
        self.act_until(
            "file-open",
            move |_, _, cx| crate::ui::inspector::open_link(&path, cx),
            |r, _, cx| r.inspector.read(cx).open_file_lines(cx).is_some(),
            3000,
        )
        .await;
        self.sleep(200).await;
        if let (Some(edge), Some(size)) = (bounds("inspector-edge"), self.with(|_, w, _| Some(w.viewport_size()))) {
            let x = (f32::from(edge.origin.x) + f32::from(size.width)) / 2.0;
            let y = f32::from(size.height) / 2.0;
            for dy in [-120, 120] {
                for _ in 0..3 {
                    self.note("file-wheel");
                    let sent = Instant::now();
                    for _ in 0..10 {
                        platform::wheel(self.ns, x, y, dy);
                        self.sleep(8).await;
                    }
                    let shown = self.painted().await;
                    self.record("file-wheel", sent, shown, shown);
                }
            }
        }
    }

    /// The right sidebar's edge dragged wider, narrower and back, through the window's events.
    async fn g_inspector_resize(&mut self) {
        if self.page_open() || self.selected_thread().is_none() {
            return;
        }
        self.inspector(true).await;
        self.sleep(200).await;
        let Some(edge) = bounds("inspector-edge") else { return log("FAIL: no right sidebar edge") };
        let (x, y) = (f32::from(edge.origin.x) + f32::from(edge.size.width) / 2.0, f32::from(edge.origin.y) + 200.0);
        self.note("inspector-drag");
        DRAGGING.with(|d| d.set(true));
        platform::mouse_button(self.ns, x, y, platform::Press::Down);
        self.painted().await;
        // Wider, narrower, and back where it was.
        let steps: Vec<i32> = (1..=10).chain((-8..10).rev()).chain(-7..=0).collect();
        for step in steps {
            self.note("inspector-drag-step");
            let sent = Instant::now();
            platform::mouse_button(self.ns, x - step as f32 * 12.0, y, platform::Press::Drag);
            let shown = self.painted().await;
            self.record("inspector-drag-step", sent, shown, shown);
        }
        platform::mouse_button(self.ns, x, y, platform::Press::Up);
        DRAGGING.with(|d| d.set(false));
        self.painted().await;
        let moved = bounds("inspector-edge").map(|b| (f32::from(b.origin.x) - f32::from(edge.origin.x)).abs());
        if moved.is_some_and(|m| m > 1.0) {
            log(&format!("FAIL: the right sidebar's edge didn't come back ({:.0} pt off)", moved.unwrap_or(0.0)));
        }
    }

    async fn g_spotlight(&mut self) {
        let open = |r: &RootView, _: &Workspace, cx: &gpui_kit::App| r.spotlight.read(cx).open;
        self.press("spotlight-open", ("k", 40, false), Box::new(crate::ui::root::OpenSpotlight), Some(&open)).await;
        self.sleep(150).await;
        if !self.read(open) {
            return log("FAIL: ⌘K didn't open Spotlight");
        }
        self.type_keys("spotlight-key", "timezone", 40).await;
        let typed = self.read(|r, _, cx| r.spotlight.read(cx).input.read(cx).value().to_string());
        if typed != "timezone" {
            // Keys didn't reach its field: what typing would have done.
            self.act("spotlight-type", |r, window, cx| {
                let s = r.read(cx).spotlight.clone();
                s.update(cx, |s, cx| {
                    s.input.update(cx, |i, cx| i.set_value("timezone", window, cx));
                    s.query(window, cx);
                });
            })
            .await;
        }
        self.sleep(600).await;
        self.note("spotlight-close");
        let sent = Instant::now();
        platform::key(self.ns, "\u{1b}");
        let shown = self.painted().await;
        if self.read(open) {
            self.act("spotlight-close", |r, _, cx| r.read(cx).spotlight.clone().update(cx, |s, cx| s.close(cx))).await;
        } else {
            self.record("spotlight-close", sent, shown, shown);
        }
    }

    async fn g_find(&mut self) {
        if self.page_open() || self.selected_thread().is_none() {
            return;
        }
        // From the composer: ⌘F with an open file focused finds in the file instead.
        self.focus_composer().await;
        let open = |r: &RootView, _: &Workspace, cx: &gpui_kit::App| r.find.read(cx).open;
        self.press("find-open", ("f", 3, false), Box::new(crate::ui::root::Find), Some(&open)).await;
        self.sleep(150).await;
        // Typed: every letter searches the thread's rows.
        self.type_keys("find-key", "migration", 60).await;
        let typed = self.read(|r, _, cx| r.find.read(cx).input.read(cx).value().to_string());
        if typed != "migration" {
            self.act("find-type", |r, window, cx| {
                let f = r.read(cx).find.clone();
                f.update(cx, |f, cx| {
                    f.input.update(cx, |i, cx| i.set_value("migration", window, cx));
                    f.search(cx);
                });
            })
            .await;
        }
        self.sleep(300).await;
        for _ in 0..3 {
            self.press("find-next", ("g", 5, false), Box::new(crate::ui::root::FindNext), None).await;
            self.sleep(150).await;
        }
        self.press("find-previous", ("g", 5, true), Box::new(crate::ui::root::FindPrevious), None).await;
        self.sleep(150).await;
        self.act("find-close", |r, _, cx| r.read(cx).find.clone().update(cx, |f, cx| f.close(cx))).await;
    }

    async fn jump(&mut self, which: &str) {
        use crate::ui::root::{EndOfThread, NextMessage, PreviousMessage, TopOfThread};
        let kind = format!("jump-{which}");
        match which {
            "previous" => self.press(&kind, ("[", 33, false), Box::new(PreviousMessage), None).await,
            "next" => self.press(&kind, ("]", 30, false), Box::new(NextMessage), None).await,
            "top" => self.press(&kind, ("[", 33, true), Box::new(TopOfThread), None).await,
            _ => self.press(&kind, ("]", 30, true), Box::new(EndOfThread), None).await,
        }
    }

    async fn g_jump(&mut self) {
        if self.page_open() {
            return;
        }
        for which in ["previous", "previous", "next", "top", "bottom"] {
            self.jump(which).await;
            self.sleep(400).await;
        }
    }

    /// The last finished turn's work opened or closed, as its summary's click does (the row's
    /// own click isn't sent: its place in the list moves as rows stream in).
    async fn toggle_work(&mut self) -> bool {
        let Some(t) = self.selected_thread() else { return false };
        let Some((ix, pos, open)) = self.read(|_, _, cx| last_work(t.read(cx))) else { return false };
        if !open {
            t.update(&mut self.cx, |t, cx| {
                t.list.scroll_to(gpui_kit::ListOffset { item_ix: ix, offset_in_item: gpui_kit::px(0.) });
                cx.notify();
            });
            self.painted().await;
        }
        self.act(if open { "collapse-work" } else { "expand-work" }, move |_, _, cx| {
            t.update(cx, |t, cx| {
                t.toggle_turn(pos);
                cx.notify();
            })
        })
        .await;
        !open
    }

    /// The biggest output in the last turn's work, opened and closed as its step's click does.
    async fn big_tool(&mut self) {
        let Some(t) = self.selected_thread() else { return };
        let opened = self.read(|_, _, cx| last_work(t.read(cx)).is_some_and(|w| w.2));
        if !opened && !self.toggle_work().await {
            return;
        }
        let big = self.read(|_, _, cx| {
            let t = t.read(cx);
            let turn = t.turns.last()?;
            t.items_at(&turn.items)
                .flat_map(|i| i.entries.iter().map(move |e| (i.pos, e.clone())))
                .filter(|(_, e)| e.borrow().data.tool_name.is_some())
                .max_by_key(|(_, e)| e.borrow().data.tool_result.as_ref().map_or(0, |r| r.len()))
        });
        let Some((pos, entry)) = big else { return };
        let ix = self.read(|_, _, cx| t.read(cx).items.iter().position(|i| i.pos == pos));
        if let Some(ix) = ix {
            t.update(&mut self.cx, |t, cx| {
                t.list.scroll_to(gpui_kit::ListOffset { item_ix: ix, offset_in_item: gpui_kit::px(0.) });
                cx.notify();
            });
            self.painted().await;
        }
        for kind in ["open-big-tool", "close-big-tool"] {
            let (t, entry) = (t.clone(), entry.clone());
            self.act(kind, move |_, _, cx| {
                t.update(cx, |t, cx| {
                    t.toggle_step(pos, &entry);
                    cx.notify();
                })
            })
            .await;
            self.sleep(300).await;
        }
    }

    async fn g_work(&mut self) {
        if self.page_open() {
            return;
        }
        if self.toggle_work().await {
            self.sleep(400).await;
            self.big_tool().await;
            self.sleep(300).await;
            self.toggle_work().await;
        }
        self.sleep(300).await;
        self.jump("bottom").await;
    }

    async fn g_earlier(&mut self) {
        let Some(t) = self.selected_thread() else { return };
        if self.read(|_, _, cx| t.read(cx).before == 0) {
            return;
        }
        let before = self.read(|_, _, cx| t.read(cx).before);
        let t2 = t.clone();
        self.act_until("earlier-turns", move |r, _, cx| ws_of(r, cx).update(cx, |ws, cx| ws.load_earlier(&t2, cx)), move |_, _, cx| t.read(cx).before != before, 5000).await;
        self.sleep(300).await;
        self.jump("bottom").await;
    }

    /// Pin, settle and collapse on a thread that isn't one of the huge ones (on the run's
    /// clone of the data), each undone with ⌘Z or toggled back.
    async fn g_sidebar(&mut self, ordinary: &[String], dice: &mut Dice) {
        let Some(id) = ordinary.get(dice.below(ordinary.len())).cloned() else { return };
        let project = self.read(|_, ws, cx| ws.thread(&id).map(|t| t.read(cx).meta.project_id.clone()));
        let undo = || Box::new(crate::ui::root::UndoThreadAction);
        let Some(start) = self.flags(&id) else { return };
        let selected = self.selected();
        let i = id.clone();
        self.act_ws("pin", move |ws, cx| ws.toggle_pin(&i, cx)).await;
        self.sleep(300).await;
        self.expect_flags(&id, (!start.0, start.1), "after pin");
        self.dispatch("undo-pin", undo()).await;
        self.sleep(300).await;
        self.expect_flags(&id, start, "after ⌘Z of a pin");
        let i = id.clone();
        self.act_ws("settle", move |ws, cx| ws.toggle_settled(&i, cx)).await;
        self.sleep(300).await;
        self.expect_flags(&id, (start.0, !start.1), "after settle");
        self.dispatch("undo-settle", undo()).await;
        self.sleep(300).await;
        self.expect_flags(&id, start, "after ⌘Z of a settle");
        let i = id.clone();
        self.act_ws("settle", move |ws, cx| ws.toggle_settled(&i, cx)).await;
        self.sleep(300).await;
        let i = id.clone();
        self.act_ws("unsettle", move |ws, cx| ws.toggle_settled(&i, cx)).await;
        self.sleep(300).await;
        self.expect_flags(&id, start, "after settle and unsettle");
        if let Some(project) = project {
            for (kind, on) in [("collapse", true), ("uncollapse", false)] {
                let p = project.clone();
                self.act(kind, move |r, _, cx| r.read(cx).sidebar.clone().update(cx, |s, cx| s.set_collapsed(&p, on, cx))).await;
                self.sleep(300).await;
            }
            // Its row is back once its project is open again (behind Show More at worst).
            let shown = self.read(|r, _, cx| {
                let s = r.sidebar.read(cx);
                s.row_index(&id, cx).is_some() || s.more_index(&project, cx).is_some()
            });
            if !shown {
                self.wrong(&format!("{id} has no sidebar row after its project was collapsed and opened again"));
            }
        }
        // None of this moves the selection off a thread that wasn't touched.
        let now = self.selected();
        if selected.as_deref() != Some(id.as_str()) && now != selected {
            self.wrong(&format!("pinning and settling another thread moved the selection from {selected:?} to {now:?}"));
        }
    }

    /// (pinned, settled) of a thread.
    fn flags(&mut self, id: &str) -> Option<(bool, bool)> {
        self.read(|_, ws, cx| ws.thread(id).map(|t| (t.read(cx).meta.pinned, t.read(cx).meta.settled_at.is_some())))
    }

    fn expect_flags(&mut self, id: &str, want: (bool, bool), when: &str) {
        let got = self.flags(id);
        if got != Some(want) {
            self.wrong(&format!("{id} {when}: (pinned, settled) {got:?}, expected {want:?}"));
        }
    }

    /// The app in a state it shouldn't be in after what was done.
    fn wrong(&self, what: &str) {
        let action = ACTION.with(|a| a.borrow().clone());
        log(&format!("WRONG: {what} (after {action})"));
    }

    /// What must hold after any action, once it has settled: checked after each chaos step.
    async fn check_state(&mut self) {
        self.painted().await;
        let (selected, exists, page, view, find_open, find_query, spotlight) = self.read(|r, ws, cx| {
            let sel = ws.selected.clone();
            let exists = sel.as_ref().is_none_or(|s| ws.thread(s).is_some());
            let view = r.transcript_view(cx).is_some();
            let fq = ws.selected_thread().and_then(|t| t.read(cx).find_query.clone());
            (sel, exists, ws.page.clone(), view, r.find.read(cx).open, fq, r.spotlight.read(cx).open)
        });
        if !exists {
            self.wrong(&format!("the selection {selected:?} is no thread"));
        }
        if selected.is_some() && page.is_none() && !view {
            self.wrong(&format!("{selected:?} is open but no transcript is shown for it"));
        }
        if !find_open && find_query.is_some() {
            self.wrong(&format!("Find is closed but the thread still marks {find_query:?}"));
        }
        if spotlight {
            self.wrong("Spotlight is still open");
        }
        // The keyboard is on something drawn: left on what's gone, shortcuts reach nothing.
        if !self.with(|r, window, cx| r.read(cx).keyboard_reaches_actions(window, cx)) {
            self.wrong("the keyboard is on nothing drawn: shortcuts would reach nothing");
        }
        if crate::ui::settings::is_open() {
            self.wrong("Settings is still open");
        }
    }

    async fn back_to(&mut self, id: &str) {
        let want = id.to_string();
        let id = want.clone();
        self.act_until("back-to-thread", move |r, _, cx| ws_of(r, cx).update(cx, |ws, cx| ws.select(&id, cx)), |_, ws, _| ws.page.is_none(), 2000).await;
        let (page, selected) = self.read(|_, ws, _| (ws.page.clone(), ws.selected.clone()));
        if page.is_some() || selected.as_deref() != Some(want.as_str()) {
            self.wrong(&format!("choosing {want} left page {page:?}, selection {selected:?}"));
        }
    }

    async fn project_page(&mut self) {
        let Some(t) = self.selected_thread() else { return };
        let project = self.read(|_, _, cx| t.read(cx).meta.project_id.clone());
        self.act_until("project-page", move |r, _, cx| ws_of(r, cx).update(cx, |ws, cx| ws.open_project(&project, cx)), |_, ws, _| ws.page.is_some(), 2000).await;
    }

    async fn g_pages(&mut self) {
        let Some(id) = self.selected() else { return };
        self.project_page().await;
        self.sleep(600).await;
        self.back_to(&id).await;
        self.sleep(400).await;
        self.act_until("memory-page", |_, window, cx| window.dispatch_action(Box::new(crate::ui::root::OpenMemory), cx), |_, ws, _| ws.page.is_some(), 2000).await;
        self.sleep(600).await;
        self.back_to(&id).await;
        self.sleep(400).await;
        self.act_until("usage-open", |_, window, cx| window.dispatch_action(Box::new(crate::ui::root::ToggleUsage), cx), |_, ws, _| ws.page.is_some(), 2000).await;
        self.sleep(1000).await;
        self.act_until("usage-close", |_, window, cx| window.dispatch_action(Box::new(crate::ui::root::ToggleUsage), cx), |_, ws, _| ws.page.is_none(), 2000).await;
    }

    /// Settings' window opened (⌘,; it opens behind, never taking focus) and closed.
    async fn g_settings(&mut self) {
        let open = |_: &RootView, _: &Workspace, _: &gpui_kit::App| crate::ui::settings::is_open();
        self.press("settings-open", (",", 43, false), Box::new(crate::ui::root::OpenSettings), Some(&open)).await;
        self.sleep(800).await;
        self.act_until("settings-close", |_, _, cx| crate::ui::settings::close_window(cx), |_, _, _| !crate::ui::settings::is_open(), 2000).await;
    }

    async fn g_new_thread(&mut self, back: Option<Entity<ThreadState>>) {
        let was = self.selected();
        let project = self.read(|r, _, cx| r.current_project(cx));
        // A new blank (in memory only), or the project's unsent one again.
        let p = project.clone();
        let landed = move |_: &RootView, ws: &Workspace, cx: &gpui_kit::App| {
            ws.page.is_none() && ws.selected_thread().is_some_and(|t| t.read(cx).items.is_empty() && Some(&t.read(cx).meta.project_id) == p.as_ref())
        };
        self.press("new-thread", ("n", 45, false), Box::new(crate::ui::root::NewThread), Some(&landed)).await;
        self.sleep(500).await;
        let (now, now_project, page, empty) = self.read(|_, ws, cx| {
            let t = ws.selected_thread();
            (ws.selected.clone(), t.as_ref().map(|t| t.read(cx).meta.project_id.clone()), ws.page.clone(), t.is_some_and(|t| t.read(cx).items.is_empty()))
        });
        if now_project != project || page.is_some() || !empty {
            self.wrong(&format!("⌘N in {project:?}: selection {was:?} → {now:?} in {now_project:?}, page {page:?}, empty {empty}"));
        }
        if let Some(back) = back {
            self.switch_to(&back).await;
        }
    }

    /// The open thread's first running job in the strip above the composer, opened and closed
    /// by clicking it.
    async fn g_jobs(&mut self) {
        let Some(t) = self.selected_thread() else { return };
        if self.page_open() || self.read(|_, _, cx| t.read(cx).run.jobs.running.is_empty()) {
            return;
        }
        self.painted().await;
        for kind in ["job-expand", "job-collapse"] {
            self.note(kind);
            let Some(sent) = self.click_key("job").await else { return };
            let shown = self.painted().await;
            self.record(kind, sent, shown, shown);
            self.sleep(500).await;
        }
    }

    /// A live resize: the window a little narrower each frame, then back.
    async fn g_resize(&mut self, steps: &[i32]) {
        let Some(frame) = self.with(|_, w, _| platform::frame_of(w)) else { return };
        for &step in steps {
            let f = [frame[0], frame[1], frame[2] - step as f64 * 24.0, frame[3]];
            self.act("resize-step", move |_, w, _| platform::set_frame(w, f)).await;
        }
        self.with(move |_, w, _| platform::set_frame(w, frame));
        self.painted().await;
    }

    async fn g_sidebar_toggle(&mut self) {
        for _ in 0..2 {
            self.dispatch("sidebar-toggle", Box::new(crate::ui::root::ToggleSidebar)).await;
            self.sleep(400).await;
        }
    }

    /// The things you click most and everything else you do in a day, in turn, while twenty
    /// threads stream: each group once a round, a thread switched to between rounds.
    async fn everything(&mut self, list: &[Entity<ThreadState>], ordinary: &[String], until: Instant) {
        let only: Option<Vec<String>> = std::env::var("YH_PERF_ONLY").ok().filter(|v| !v.is_empty()).map(|v| v.split(',').map(str::to_string).collect());
        let wants = |g: &str| only.as_ref().is_none_or(|o| o.iter().any(|n| n == g));
        let mut dice = Dice(0x9E3779B97F4A7C15);
        let mut index = 0;
        let mut rounds = 0;
        while Instant::now() < until && !list.is_empty() {
            index = (index + 1) % list.len();
            self.note("switch");
            self.switch_to(&list[index]).await;
            self.sleep(400).await;
            for group in GROUPS {
                if Instant::now() >= until {
                    break;
                }
                if !wants(group) {
                    continue;
                }
                log(&format!("everything: {group}"));
                match group {
                    "type" => self.g_type().await,
                    "draft" => self.g_draft().await,
                    "tabs" => self.g_tabs(list, index).await,
                    "files" => {
                        self.g_files().await;
                        self.inspector(false).await;
                    }
                    "inspector-resize" => {
                        self.g_inspector_resize().await;
                        self.inspector(false).await;
                    }
                    "spotlight" => self.g_spotlight().await,
                    "find" => self.g_find().await,
                    "jump" => self.g_jump().await,
                    "work" => self.g_work().await,
                    "earlier" => self.g_earlier().await,
                    "sidebar" => self.g_sidebar(ordinary, &mut dice).await,
                    "pages" => self.g_pages().await,
                    "settings" => self.g_settings().await,
                    "new-thread" => self.g_new_thread(Some(list[index].clone())).await,
                    "jobs" => self.g_jobs().await,
                    "resize" => self.g_resize(&(1..=15).chain((0..15).rev()).collect::<Vec<_>>()).await,
                    "sidebar-toggle" => self.g_sidebar_toggle().await,
                    _ => {}
                }
                self.check_state().await;
                self.sleep(400).await;
            }
            rounds += 1;
        }
        log(&format!("everything done, {rounds} rounds"));
    }

    /// Anything, in any order, as fast as a hand could: what a list of clicks doesn't think
    /// of. Each action is logged before it's done, so a crash says what led to it, and
    /// `YH_PERF_CHAOS_SEED=<n>` plays a run again.
    async fn chaos(&mut self, list: &[Entity<ThreadState>], ordinary: &[String], until: Instant) {
        let seed = std::env::var("YH_PERF_CHAOS_SEED").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or_else(|| {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(1)
        });
        log(&format!("chaos seed {seed}"));
        let mut dice = Dice(seed | 1);
        let total: u64 = CHAOS.iter().map(|a| a.1).sum();
        let mut count = 0;
        while Instant::now() < until && !list.is_empty() {
            let mut roll = dice.next() % total;
            let Some(&(name, _)) = CHAOS.iter().find(|a| {
                if roll < a.1 {
                    return true;
                }
                roll -= a.1;
                false
            }) else {
                continue;
            };
            count += 1;
            let title = self.read(|_, ws, cx| match &ws.page {
                Some(page) => format!("{page:?}"),
                None => ws.selected_thread().map(|t| t.read(cx).title.clone()).unwrap_or_else(|| "nothing".into()),
            });
            log(&format!("chaos {count} {name} in {title}"));
            self.note(&format!("chaos {count} {name}"));
            // Nothing open (a thread deleted, a project collapsed): back to a huge one.
            if self.selected().is_none() {
                let t = list[dice.below(list.len())].clone();
                self.switch_to(&t).await;
            }
            match name {
                "switch" => {
                    let t = list[dice.below(list.len())].clone();
                    self.switch_to(&t).await;
                }
                "switch-ordinary" => {
                    let id = ordinary.get(dice.below(ordinary.len())).cloned();
                    if let Some(t) = id.and_then(|id| self.read(|_, ws, _| ws.thread(&id))) {
                        self.switch_to(&t).await;
                    }
                }
                "jump-top" => self.jump("top").await,
                "jump-bottom" => self.jump("bottom").await,
                "jump-previous" => self.jump("previous").await,
                "jump-next" => self.jump("next").await,
                "wheel-up" => self.wheel_transcript(30, 60).await,
                "wheel-down" => self.wheel_transcript(30, -60).await,
                "wheel-fling" => self.wheel_transcript(40, 400).await,
                "work" => {
                    self.toggle_work().await;
                }
                "big-tool" => self.big_tool().await,
                "earlier-turns" => self.g_earlier().await,
                "inspector" => {
                    let shows = self.read(|_, ws, _| ws.shows_inspector);
                    self.inspector(!shows).await;
                }
                "tab" => {
                    let panel = crate::state::workspace::Panel::ALL[dice.below(crate::state::workspace::Panel::ALL.len())];
                    self.tab(panel).await;
                }
                "inspector-resize" => self.g_inspector_resize().await,
                "files" => self.g_files().await,
                "spotlight" => self.g_spotlight().await,
                "find" => self.g_find().await,
                "type" => {
                    if self.focus_composer().await {
                        self.type_keys("type-key", "go on", 30).await;
                        self.set_composer("clear-draft", String::new()).await;
                    }
                }
                "draft" => self.g_draft().await,
                "new-thread" => self.g_new_thread(None).await,
                "project-page" => self.project_page().await,
                "memory" => {
                    self.act_until("memory-page", |_, window, cx| window.dispatch_action(Box::new(crate::ui::root::OpenMemory), cx), |_, ws, _| ws.page.is_some(), 2000).await;
                }
                "usage" => self.dispatch("usage-toggle", Box::new(crate::ui::root::ToggleUsage)).await,
                "pages-back" => {
                    if let Some(id) = self.selected() {
                        self.back_to(&id).await;
                    }
                }
                "settings" => self.g_settings().await,
                "sidebar-order" => self.g_sidebar(ordinary, &mut dice).await,
                "undo" => self.dispatch("undo", Box::new(crate::ui::root::UndoThreadAction)).await,
                "resize" => self.g_resize(&[1, 3, 5, 7, 5, 3, 1, 0]).await,
                "jobs" => self.g_jobs().await,
                "sidebar-toggle" => self.dispatch("sidebar-toggle", Box::new(crate::ui::root::ToggleSidebar)).await,
                "stop" => self.dispatch("stop", Box::new(crate::ui::root::Stop)).await,
                "send" => {
                    if let Some(t) = self.selected_thread() {
                        self.act_ws("send", move |ws, cx| ws.send(&t, "PERF seconds=8 rate=1", false, cx)).await;
                    }
                }
                _ => {}
            }
            self.check_state().await;
            let pause = dice.below(400) as u64;
            self.sleep(pause).await;
        }
        // Left with a thread open and Settings closed.
        let _ = self.cx.update(crate::ui::settings::close_window);
        if self.page_open() || self.selected().is_none() {
            let t = list[0].clone();
            self.switch_to(&t).await;
        }
        log(&format!("chaos done, {count} actions"));
    }

    /// Per kind of click in this phase: how many, median and worst, and any over 100 ms.
    fn click_summary(&self, from: usize) {
        let mut by: std::collections::BTreeMap<String, Vec<f64>> = Default::default();
        for c in &self.clicks[from..] {
            if let (Some(kind), Some(ms)) = (c["click"].as_str(), c["doneMs"].as_f64()) {
                by.entry(kind.to_string()).or_default().push(ms);
            }
        }
        for (kind, mut times) in by {
            times.sort_by(f64::total_cmp);
            log(&format!("click {kind}: {} · median {:.0} ms · worst {:.0} ms", times.len(), times[times.len() / 2], times[times.len() - 1]));
        }
        for c in &self.clicks[from..] {
            if c["doneMs"].as_f64().is_some_and(|ms| ms > 100.0) {
                log(&format!("slow click {} {:.0} ms (answered {:.0} ms) at {:.1} s", c["click"].as_str().unwrap_or(""), c["doneMs"].as_f64().unwrap_or(0.0), c["answeredMs"].as_f64().unwrap_or(0.0), c["at"].as_f64().unwrap_or(0.0)));
            }
        }
    }
}

pub fn start(handle: AnyWindowHandle, root: Entity<RootView>, folder: String, cx: &mut gpui_kit::App) {
    ENABLED.with(|e| e.set(true));
    let _ = std::fs::create_dir_all(&folder);
    cx.spawn(async move |cx| {
        let mut p = Probe {
            handle,
            root,
            cx: cx.clone(),
            started: Instant::now(),
            folder,
            phases: vec![],
            clicks: vec![],
            phase: String::new(),
            real_events: true,
            file: String::new(),
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
                let ws = p.root.read(cx).workspace.clone();
                ws.update(cx, |ws, cx| ws.prepare_for_quit(cx));
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

    let mut threads: Vec<(String, Entity<ThreadState>)> = p.read(|_, ws, cx| {
        ws.threads.iter().map(|t| (t.read(cx).title.clone(), t.clone())).filter(|(title, _)| title.starts_with("Huge")).collect()
    });
    threads.sort_by(|a, b| a.0.cmp(&b.0));
    let threads: Vec<Entity<ThreadState>> = threads.into_iter().map(|(_, t)| t).collect();
    if threads.is_empty() {
        log("FAIL: no threads named Huge");
        return;
    }
    // Threads that aren't huge, for pinning and settling; and a long file for the Files tab.
    let ordinary: Vec<String> = p.read(|_, ws, cx| {
        ws.threads
            .iter()
            .map(|t| t.read(cx))
            .filter(|t| !t.title.starts_with("Huge") && t.meta.settled_at.is_none() && !t.meta.pinned)
            .map(|t| t.id().to_string())
            .take(20)
            .collect()
    });
    p.file = std::path::Path::new(&p.folder).join("perf-file.rs").to_string_lossy().into_owned();
    let lines: String = (0..20_000).map(|i| format!("fn step_{i}() -> usize {{ /* the parser drops the timezone when the migration runs twice */ {i} }}\n")).collect();
    let _ = std::fs::write(&p.file, lines);

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
                    // Not while a drag holds the button: a move without it ends the drag.
                    if let (Ok((ns, size)), false) = (target, DRAGGING.with(|d| d.get())) {
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
        "stream-all-everything",
        "stream-all-chaos",
    ];
    for name in names {
        if only.as_ref().is_some_and(|o| !o.iter().any(|n| n == name)) {
            continue;
        }
        p.phase = name.to_string();
        PHASE.with(|ph| *ph.borrow_mut() = name.to_string());
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
                    p.ws_update(move |ws, cx| ws.ensure_loaded(&t2, cx));
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
            "stream-all-everything" => {
                let end = p.start_turns(&threads, seconds, &rate).await;
                p.everything(&threads, &ordinary, end).await;
            }
            "stream-all-chaos" => {
                let end = p.start_turns(&threads, seconds, &rate).await;
                p.chaos(&threads, &ordinary, end).await;
            }
            _ => {}
        }

        p.note("");
        let deltas = stop_frames(p);
        sampling.set(false);
        p.click_summary(clicks_before);
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
