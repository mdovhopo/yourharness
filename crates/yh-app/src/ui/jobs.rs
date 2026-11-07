//! Above the composer while Claude has jobs running: what each is, how long it's been going
//! and the last thing it printed, so it's clear whether Claude is waiting on something or
//! done. Claude is woken when one ends; you can type meanwhile. Open a job for its live
//! output; Stop ends it (`stop_task`, as the Swift app's Stop).
//!
//! A view of its own: its clock and the output it reads (off the main thread, once a second,
//! only while jobs run) redraw it alone, never the transcript. Also asks before quitting
//! with jobs running.

use crate::state::jobs::{self, Job};
use crate::state::thread::ThreadState;
use crate::state::workspace::Workspace;
use crate::theme::{Theme, MONO};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::Sizable;
use gpui_kit::{
    div, prelude::*, px, AnyWindowHandle, App, Context, Entity, FontWeight, Global, PromptLevel, SharedString, Subscription, Task,
    Window,
};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

/// Lines of a job's output shown when it's opened.
const OUTPUT_LINES: usize = 40;

pub struct JobsStrip {
    workspace: Entity<Workspace>,
    thread: Option<Entity<ThreadState>>,
    /// The thread's `jobs.version` last drawn: anything else the thread does is ignored.
    seen: u64,
    expanded: HashSet<String>,
    /// What each job printed last, by task ID, and the end of it for opened ones.
    last_lines: HashMap<String, String>,
    outputs: HashMap<String, String>,
    /// Once a second while jobs run: the clock, and their output read again.
    ticker: Option<Task<()>>,
    _observe: Option<Subscription>,
}

impl JobsStrip {
    pub fn new(workspace: Entity<Workspace>) -> Self {
        JobsStrip { workspace, thread: None, seen: 0, expanded: HashSet::new(), last_lines: HashMap::new(), outputs: HashMap::new(), ticker: None, _observe: None }
    }

    /// The open thread's jobs from now on.
    pub fn switch_to(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(thread) = self.workspace.read(cx).thread(id) else { return };
        if self.thread.as_ref().is_some_and(|t| t == &thread) {
            return;
        }
        self.expanded.clear();
        self.last_lines.clear();
        self.outputs.clear();
        self.seen = thread.read(cx).jobs.version;
        self._observe = Some(cx.observe(&thread, |this, thread, cx| {
            let version = thread.read(cx).jobs.version;
            if version != this.seen {
                this.seen = version;
                this.follow(cx);
                cx.notify();
            }
        }));
        self.thread = Some(thread);
        self.follow(cx);
        cx.notify();
    }

    /// Ticks while the thread has jobs running, and stops when it has none.
    fn follow(&mut self, cx: &mut Context<Self>) {
        // `YH_JOBS_EXPAND=1`: every job drawn open, for checking its live output in a snapshot.
        if std::env::var("YH_JOBS_EXPAND").as_deref() == Ok("1") {
            if let Some(t) = &self.thread {
                self.expanded.extend(t.read(cx).jobs.running.iter().map(|j| j.id.clone()));
            }
        }
        let running = self.thread.as_ref().is_some_and(|t| !t.read(cx).jobs.running.is_empty());
        if !running {
            self.ticker = None;
            return;
        }
        if self.ticker.is_some() {
            return;
        }
        self.ticker = Some(cx.spawn(async move |this, cx| loop {
            let Ok(Some(files)) = this.update(cx, |this, cx| this.output_files(cx)) else { break };
            let read = cx
                .background_executor()
                .spawn(async move {
                    files
                        .into_iter()
                        .filter_map(|(id, path, open)| {
                            let tail = jobs::tail(&path, if open { 8192 } else { 1024 })?;
                            let last = tail.lines().rev().find(|l| !l.trim().is_empty()).map(str::to_string);
                            let output = open.then(|| {
                                let lines: Vec<&str> = tail.lines().collect();
                                lines[lines.len().saturating_sub(OUTPUT_LINES)..].join("\n")
                            });
                            Some((id, last, output))
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let alive = this
                .update(cx, |this, cx| {
                    for (id, last, output) in read {
                        if let Some(last) = last {
                            this.last_lines.insert(id.clone(), last);
                        }
                        if let Some(output) = output {
                            this.outputs.insert(id, output);
                        }
                    }
                    // The clock moves: this strip alone redraws.
                    cx.notify();
                })
                .is_ok();
            if !alive {
                break;
            }
            cx.background_executor().timer(Duration::from_secs(1)).await;
        }));
    }

    /// (task id, its output file, opened) for the running jobs; `None` once there are none.
    fn output_files(&self, cx: &App) -> Option<Vec<(String, String, bool)>> {
        let thread = self.thread.as_ref()?.read(cx);
        if thread.jobs.running.is_empty() {
            return None;
        }
        // A foreground command's file is never told: where the session's others go.
        let folder = thread.jobs.folder.clone().or_else(|| thread.session_id.as_ref().map(|s| jobs::output_folder(&self.workspace.read(cx).folder_of(thread), s)));
        Some(
            thread
                .jobs
                .running
                .iter()
                .filter_map(|j| {
                    let path = j.output_file.clone().or_else(|| folder.as_ref().map(|f| format!("{f}/{}.output", j.id)))?;
                    Some((j.id.clone(), path, self.expanded.contains(&j.id)))
                })
                .collect(),
        )
    }

    fn job_row(&self, job: &Job, thread: &Entity<ThreadState>, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.expanded.contains(&job.id);
        let last = job.last_line.clone().or_else(|| self.last_lines.get(&job.id).cloned()).filter(|l| !l.is_empty());
        let elapsed = jobs::clock(job.started.elapsed().as_secs());
        let toggle_id = job.id.clone();
        let stop_id = job.id.clone();
        let (ws, thread) = (self.workspace.clone(), thread.clone());
        let description = if job.description.is_empty() { "Command".to_string() } else { job.description.clone() };
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(10.))
            .py(px(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .id(SharedString::from(format!("job-{}", job.id)))
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .cursor_pointer()
                            .child(div().flex_none().w(px(16.)).text_color(theme.secondary).font_family(MONO).text_size(px(11.)).child(if job.watch { "◉" } else { ">_" }))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(div().font_weight(FontWeight::MEDIUM).overflow_hidden().whitespace_nowrap().text_ellipsis().child(description))
                                    .when_some(last.filter(|_| !open), |d, line| {
                                        d.child(
                                            div()
                                                .font_family(MONO)
                                                .text_size(px(11.))
                                                .line_height(px(15.))
                                                .text_color(theme.secondary)
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis()
                                                .child(line),
                                        )
                                    }),
                            )
                            .child(div().flex_none().text_color(theme.secondary).child(elapsed))
                            .child(div().flex_none().w(px(10.)).text_color(theme.secondary).child(if open { "⌄" } else { "›" }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.expanded.remove(&toggle_id) {
                                    this.expanded.insert(toggle_id.clone());
                                }
                                cx.notify();
                            })),
                    )
                    // A foreground command stops with the turn (Stop ⌘.).
                    .when(job.background, |d| {
                        d.child(
                            Button::new(SharedString::from(format!("job-stop-{}", job.id)))
                                .ghost()
                                .xsmall()
                                .label("Stop")
                                .tooltip(if job.watch { "Stop watching" } else { "Stop this command" })
                                .on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.stop_job(&thread, &stop_id, cx))),
                        )
                    }),
            )
            .when(open, |d| {
                let text = match self.outputs.get(&job.id) {
                    Some(t) if t.is_empty() => "No output yet".to_string(),
                    Some(t) => t.clone(),
                    None => "Its output shows once it prints".to_string(),
                };
                d.child(
                    div()
                        .id(SharedString::from(format!("job-output-{}", job.id)))
                        .max_h(px(220.))
                        .overflow_y_scroll()
                        .font_family(MONO)
                        .text_size(px(11.))
                        .line_height(px(15.))
                        .bg(theme.code)
                        .rounded(px(6.))
                        .px(px(8.))
                        .py(px(6.))
                        .text_color(theme.text)
                        .child(text),
                )
            })
    }
}

impl Render for JobsStrip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(thread) = self.thread.clone() else { return div() };
        let theme = Theme::get(cx);
        let t = thread.read(cx);
        if t.jobs.running.is_empty() {
            return div();
        }
        let header = if t.jobs.waiting() > 0 && !t.running {
            "Claude is waiting on"
        } else if t.jobs.background().next().is_some() {
            "Running in the background"
        } else {
            "Running"
        };
        let jobs: Vec<Job> = t.jobs.running.clone();
        let mut strip = div()
            .w_full()
            .max_w(px(828.))
            .mb(px(6.))
            .pb(px(3.))
            .rounded(px(10.))
            .bg(theme.card)
            .border_1()
            .border_color(theme.separator)
            .text_size(px(12.))
            .flex()
            .flex_col()
            .child(div().px(px(10.)).pt(px(7.)).pb(px(2.)).text_size(px(11.)).text_color(theme.secondary).child(header));
        for job in &jobs {
            strip = strip.child(self.job_row(job, &thread, &theme, cx));
        }
        div().w_full().flex().justify_center().px(px(20.)).child(strip)
    }
}

/// What Quit does: asks first while background jobs run, as the Swift app did (they stop
/// with the app). Set up once the window is there; until then Quit just quits.
#[derive(Clone)]
pub struct QuitGuard(Rc<dyn Fn(&mut App)>);

impl Global for QuitGuard {}

pub fn quit(cx: &mut App) {
    match cx.try_global::<QuitGuard>().cloned() {
        Some(guard) => (guard.0)(cx),
        None => cx.quit(),
    }
}

/// `YH_QUIT_ANSWER=quit|cancel` answers the question without showing it, printing it
/// (`QUIT asked: …`); `YH_QUIT_AFTER=<seconds>` chooses Quit that long after launch.
pub fn guard_quit(workspace: Entity<Workspace>, window: AnyWindowHandle, cx: &mut App) {
    cx.set_global(QuitGuard(Rc::new(move |cx| {
        let Some((title, detail)) = workspace.read(cx).quit_question(cx) else { return cx.quit() };
        if let Ok(answer) = std::env::var("YH_QUIT_ANSWER") {
            eprintln!("QUIT asked: {title} / {detail} -> {answer}");
            if answer == "quit" {
                cx.quit();
            }
            return;
        }
        let Ok(answer) = window.update(cx, |_, window, cx| window.prompt(PromptLevel::Warning, &title, Some(&detail), &["Quit", "Cancel"], cx)) else {
            return cx.quit();
        };
        cx.spawn(async move |cx| {
            if answer.await == Ok(0) {
                let _ = cx.update(|cx| cx.quit());
            }
        })
        .detach();
    })));
    if let Some(seconds) = std::env::var("YH_QUIT_AFTER").ok().and_then(|s| s.parse::<f64>().ok()) {
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_secs_f64(seconds)).await;
            let _ = cx.update(quit);
        })
        .detach();
    }
}
