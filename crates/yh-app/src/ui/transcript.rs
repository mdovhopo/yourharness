//! A thread's transcript: GPUI's `list`, bottom-aligned so it follows the end. It redraws when
//! its thread changes; a row is measured again only when the thread splices it.

use crate::probe;
use crate::state::jobs;
use crate::state::thread::ThreadState;
use crate::state::workspace::Workspace;
use crate::theme::Theme;
use crate::ui::rows;
use gpui_kit::{div, list, prelude::*, px, Context, Entity, Subscription, Task, Window};
use std::sync::Arc;
use std::time::Duration;
use yh_core::links::{Linker, Repo};

/// How long a thread may take to open before it says it's loading: one that opens at once
/// shouldn't flash it.
const SLOW_LOAD: Duration = Duration::from_millis(120);

pub struct TranscriptView {
    pub thread: Entity<ThreadState>,
    workspace: Entity<Workspace>,
    /// The thread was running when last looked: its files are read again when it stops.
    was_running: bool,
    /// Still loading after `SLOW_LOAD`: "Loading conversation…".
    slow_load: bool,
    /// Reads what running foreground commands print, once a second, while there are any.
    job_ticker: Option<Task<()>>,
    /// `YH_SELECT_PROBE` has run.
    probed: bool,
    _observe: Subscription,
}

impl TranscriptView {
    pub fn new(thread: Entity<ThreadState>, workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        // Rows' Show Browser and Follow in Sidebar open the right sidebar.
        if !cx.has_global::<rows::ShowPanel>() {
            let ws = workspace.downgrade();
            cx.set_global(rows::ShowPanel(std::rc::Rc::new(move |panel, cx| {
                let _ = ws.update(cx, |ws, cx| ws.show_panel(panel, cx));
            })));
        }
        let observe = cx.observe(&thread, |this, thread, cx| {
            let running = thread.read(cx).running;
            if this.was_running && !running {
                this.load_links(cx);
            }
            this.was_running = running;
            this.follow_jobs(cx);
            cx.notify()
        });
        let was_running = thread.read(cx).running;
        if !thread.read(cx).loaded {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SLOW_LOAD).await;
                let _ = this.update(cx, |this, cx| {
                    if !this.thread.read(cx).loaded {
                        this.slow_load = true;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        let mut view = TranscriptView { thread, workspace, was_running, slow_load: false, job_ticker: None, probed: false, _observe: observe };
        view.load_links(cx);
        view.follow_jobs(cx);
        // The toolbar's PR count, read aside when the thread's transcript is first made.
        {
            let (ws, thread) = (view.workspace.clone(), view.thread.clone());
            cx.defer(move |cx| ws.update(cx, |ws, cx| ws.count_pulls(&thread, cx)));
        }
        view
    }

    /// Reads what the thread's replies link against, off the main thread: its folder's files
    /// (a turn may have made some) and its repo.
    fn load_links(&mut self, cx: &mut Context<Self>) {
        let folder = self.workspace.read(cx).folder_of(self.thread.read(cx));
        let thread = self.thread.downgrade();
        cx.spawn(async move |_, cx| {
            let linker = cx
                .background_executor()
                .spawn(async move {
                    let path = std::path::Path::new(&folder);
                    let files = yh_core::files::list(path);
                    let repo = yh_core::forge::origin(path).map(|(host, path, gitlab)| Repo { host, path, gitlab });
                    Arc::new(Linker::new(&folder, repo, &files))
                })
                .await;
            let _ = thread.update(cx, |t, cx| {
                if t.linker.as_ref().map(|l| l.revision) != Some(linker.revision) {
                    t.linker = Some(linker);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// (tool call, its output file) for each foreground command running; empty when none.
    fn foreground_files(&self, cx: &gpui_kit::App) -> Vec<(String, String)> {
        let t = self.thread.read(cx);
        if !t.jobs.running.iter().any(|j| !j.background) {
            return vec![];
        }
        // A foreground command's file is never told: where the session's others go.
        let folder = t.jobs.folder.clone().or_else(|| t.session_id.as_ref().map(|s| jobs::output_folder(&self.workspace.read(cx).folder_of(t), s)));
        t.jobs
            .running
            .iter()
            .filter(|j| !j.background)
            .filter_map(|j| {
                let tool = j.tool_id.clone()?;
                let path = j.output_file.clone().or_else(|| folder.as_ref().map(|f| format!("{f}/{}.output", j.id)))?;
                Some((tool, path))
            })
            .collect()
    }

    /// A foreground command that's taking a while shows what it printed last under its step:
    /// read once a second, off the main thread, and only a row whose line changed is redrawn.
    /// Also, once a second while jobs run: their steps' clocks ("In background 1:12") and what an
    /// open job step shows of its output. Each redraws only the rows that changed.
    fn follow_jobs(&mut self, cx: &mut Context<Self>) {
        let (running, unread_open) = {
            let t = self.thread.read(cx);
            (!t.jobs.running.is_empty(), t.open_job_files().iter().any(|(tool, _)| !t.job_outputs.contains_key(tool)))
        };
        if !running && !unread_open {
            self.job_ticker = None;
            return;
        }
        if self.job_ticker.is_some() {
            return;
        }
        self.job_ticker = Some(cx.spawn(async move |this, cx| loop {
            let Ok((files, open)) = this.update(cx, |this, cx| (this.foreground_files(cx), this.thread.read(cx).open_job_files())) else { break };
            let lines = cx
                .background_executor()
                .spawn(async move {
                    let lines = files
                        .into_iter()
                        .filter_map(|(tool, path)| {
                            let tail = jobs::tail(&path, 1024)?;
                            let last = tail.lines().rev().find(|l| !l.trim().is_empty())?.trim_end().to_string();
                            Some((tool, last))
                        })
                        .collect::<Vec<_>>();
                    // The last 40 lines, as the Swift app's `JobOutput`.
                    let outputs = open
                        .into_iter()
                        .map(|(tool, path)| {
                            let text = jobs::tail(&path, 16 * 1024).map(|t| {
                                let all: Vec<&str> = t.lines().collect();
                                all[all.len().saturating_sub(40)..].join("\n")
                            });
                            (tool, match text {
                                Some(t) if t.is_empty() => "No output yet".to_string(),
                                Some(t) => t,
                                None => "…".to_string(),
                            })
                        })
                        .collect::<Vec<_>>();
                    (lines, outputs)
                })
                .await;
            let Ok(thread) = this.update(cx, |this, _| this.thread.clone()) else { break };
            let still = thread.update(cx, |t, cx| {
                let (lines, outputs) = lines;
                let mut changed = t.set_job_lines(lines);
                changed |= t.set_job_outputs(outputs);
                changed |= t.tick_job_clocks();
                if changed {
                    cx.notify();
                }
                !t.jobs.running.is_empty()
            });
            // None left: done, so the next job starts it again.
            if !still {
                let _ = this.update(cx, |this, _| this.job_ticker = None);
                break;
            }
            cx.background_executor().timer(Duration::from_secs(1)).await;
        }));
    }
}

/// `YH_SELECT_PROBE=1` (test runs): drags across the last reply with the mouse, through the
/// window's own event path, a moment after the thread shows (`YH_SELECT_PROBE_DELAY` seconds,
/// 2 by default); prints what's selected (what ⌘C copies), then clicks off it and prints that
/// the selection went. `=keep` leaves it selected, for a snapshot.
fn select_probe(thread: Entity<ThreadState>, window: &mut Window, cx: &mut Context<TranscriptView>) {
    use gpui_kit::{point, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput};
    cx.spawn_in(window, async move |_, cx| {
        let delay = std::env::var("YH_SELECT_PROBE_DELAY").ok().and_then(|d| d.parse().ok()).unwrap_or(2.0);
        cx.background_executor().timer(Duration::from_secs_f64(delay)).await;
        let _ = cx.update(|window, cx| {
            let t = thread.read(cx);
            let Some(item) = t.items.iter().rev().find(|i| i.ty == crate::state::thread::ItemType::Text) else { return eprintln!("SELECT FAIL: no reply") };
            // From the start of the reply's first paragraph to the end of its last, where they
            // were laid out.
            let md = yh_core::markdown::parse(&item.entries[0].borrow().data.text);
            let paragraphs: Vec<String> = md.blocks.iter().filter_map(|b| if let yh_core::markdown::MdBlock::Paragraph(s) = b { Some(s.text.clone()) } else { None }).collect();
            let (Some(first), Some(last)) = (paragraphs.first().and_then(|p| crate::ui::selectable::place_of(p)), paragraphs.last().and_then(|p| crate::ui::selectable::place_of(p))) else {
                return eprintln!("SELECT FAIL: the reply isn't laid out");
            };
            let from = point(first.left() + px(1.), first.top() + px(6.));
            let to = point(last.right() - px(1.), last.bottom() - px(6.));
            let m = Modifiers::default();
            window.dispatch_event(PlatformInput::MouseMove(MouseMoveEvent { position: from, pressed_button: None, modifiers: m }), cx);
            window.dispatch_event(PlatformInput::MouseDown(MouseDownEvent { button: MouseButton::Left, position: from, modifiers: m, click_count: 1, first_mouse: false }), cx);
            for step in 1..=8 {
                let f = step as f32 / 8.;
                let at = point(from.x + (to.x - from.x) * f, from.y + (to.y - from.y) * f);
                window.dispatch_event(PlatformInput::MouseMove(MouseMoveEvent { position: at, pressed_button: Some(MouseButton::Left), modifiers: m }), cx);
            }
            window.dispatch_event(PlatformInput::MouseUp(MouseUpEvent { button: MouseButton::Left, position: to, modifiers: m, click_count: 1 }), cx);
            window.refresh();
        });
        cx.background_executor().timer(Duration::from_millis(300)).await;
        let _ = cx.update(|window, cx| {
            let text = gpui_kit::base::TextSelection::selected_text(window, cx);
            let shown: String = text.chars().take(160).collect();
            eprintln!("SELECT {} ({} chars): {shown:?}", if text.trim().is_empty() { "FAIL" } else { "PASS" }, text.chars().count());
        });
        if std::env::var("YH_SELECT_PROBE").as_deref() == Ok("keep") {
            return;
        }
        // A click elsewhere ends it.
        cx.background_executor().timer(Duration::from_millis(1500)).await;
        let _ = cx.update(|window, cx| {
            let t = thread.read(cx);
            let Some(b) = t.items.iter().rposition(|i| i.ty == crate::state::thread::ItemType::Text).and_then(|ix| t.list.bounds_for_item(ix)) else { return };
            let at = point(b.left() + px(4.), b.top() + px(4.));
            let m = Modifiers::default();
            window.dispatch_event(PlatformInput::MouseDown(MouseDownEvent { button: MouseButton::Left, position: at, modifiers: m, click_count: 1, first_mouse: false }), cx);
            window.dispatch_event(PlatformInput::MouseUp(MouseUpEvent { button: MouseButton::Left, position: at, modifiers: m, click_count: 1 }), cx);
        });
        cx.background_executor().timer(Duration::from_millis(300)).await;
        let _ = cx.update(|window, cx| {
            let text = gpui_kit::base::TextSelection::selected_text(window, cx);
            eprintln!("SELECT cleared: {}", if text.trim().is_empty() { "PASS" } else { "FAIL" });
        });
    })
    .detach();
}

impl Render for TranscriptView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.probed && self.thread.read(cx).loaded {
            self.probed = true;
            if std::env::var("YH_SELECT_PROBE").is_ok() {
                select_probe(self.thread.clone(), window, cx);
            }
            // `YH_OPEN_STEP_FILE=<tool name>` (test runs): the newest such step's file opens in
            // the Files tab, as clicking its path does, once the thread's files are read.
            if let Ok(tool) = std::env::var("YH_OPEN_STEP_FILE") {
                let thread = self.thread.clone();
                cx.spawn(async move |_, cx| {
                    cx.background_executor().timer(Duration::from_millis(1500)).await;
                    let _ = cx.update(|cx| match rows::step_file_target(thread.read(cx), &tool) {
                        Some((path, line, written)) => {
                            eprintln!("STEP FILE {path} line {line:?} written {}", written.is_some());
                            rows::open_target(path, line, written, cx);
                        }
                        None => eprintln!("STEP FILE FAIL: no {tool} step with a file in the thread's folder"),
                    });
                })
                .detach();
            }
            if let Ok(file) = std::env::var("YH_OPEN_DIAGRAM") {
                crate::ui::diagram::open_file(&file, window, cx);
            }
            // `YH_SCROLL_TO=<row>` (test runs): the transcript opens with that row at the top
            // (negative: counted from the end), for snapshots of what's above the end.
            if let Some(row) = std::env::var("YH_SCROLL_TO").ok().and_then(|r| r.parse::<i64>().ok()) {
                let thread = self.thread.clone();
                cx.defer(move |cx| {
                    thread.update(cx, |t, cx| {
                        let count = t.items.len() as i64;
                        let ix = if row < 0 { count + row } else { row }.clamp(0, (count - 1).max(0));
                        t.list.scroll_to(gpui_kit::ListOffset { item_ix: ix as usize, offset_in_item: px(0.) });
                        cx.notify();
                    })
                });
            }
        }
        let theme = Theme::get(cx);
        let t = self.thread.read(cx);
        if !t.loaded {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(crate::theme::text::FOOTNOTE))
                .text_color(theme.secondary)
                .when(self.slow_load, |d| d.child("Loading conversation…"))
                .into_any_element();
        }
        let thread = self.thread.clone();
        let mut root = div().size_full().relative();
        if probe::enabled() {
            root = root.child(probe::tracker("transcript".into()));
        }
        // While Claude works, room at the end for the working indicator laid over it (its own
        // view, `ui::working`, so its clock never redraws the list).
        let bottom = if t.running { 40. } else { 16. };
        root.child(list(t.list.clone(), move |ix, window, cx| rows::render_item(&thread, ix, window, cx)).size_full().pb(px(bottom)))
            .into_any_element()
    }
}
