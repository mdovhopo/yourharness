//! Automations run (yh-core's `automations.rs`): a timer looks for the ones that are due (at
//! their time, and every 20 seconds at least) and runs them, which starts a thread in their project with their prompt, or
//! sends it to their own thread (queued behind a turn that's going). A run that came due while
//! the app was closed runs once, soon after launch. Run Now (the project page, an agent's
//! `run_automation`) goes through the same `run_automation`.

use crate::state::workspace::Workspace;
use gpui_kit::Context;
use std::time::Duration;
use yh_core::automations::{self, Automation};

/// How often the timer looks.
const TICK: Duration = Duration::from_secs(20);
/// The first look after launch: once the threads are up.
const FIRST: Duration = Duration::from_secs(8);
/// At most this many threads working in a project before a run waits for the next one.
const MAX_RUNNING: usize = 8;

fn logs() -> bool {
    std::env::var("YH_AUTOMATION_LOG").is_ok_and(|v| v == "1")
}

/// Starts the timer: not in perf runs, nor with `YH_AUTOMATIONS=0`.
pub fn start(cx: &mut Context<Workspace>) {
    if std::env::var("YH_PERF").is_ok() || std::env::var("YH_AUTOMATIONS").is_ok_and(|v| v == "0") {
        return;
    }
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(FIRST).await;
        loop {
            // Until the next run is due, or TICK at most: a run lands within a second of its
            // time, and one made or changed meanwhile is seen soon.
            let Ok(next) = this.update(cx, |ws, cx| {
                ws.run_due_automations(cx);
                ws.automations().iter().filter(|a| a.enabled).filter_map(|a| a.next_run).fold(f64::INFINITY, f64::min)
            }) else {
                break;
            };
            let wait = (next - automations::now() + 0.2).clamp(0.5, TICK.as_secs_f64());
            cx.background_executor().timer(Duration::from_secs_f64(wait)).await;
        }
    })
    .detach();
}

impl Workspace {
    /// Every automation, as saved.
    pub fn automations(&self) -> Vec<Automation> {
        automations::all(self.library.as_ref())
    }

    /// They changed (an agent's tools, the project page): pages showing them read them again.
    pub fn automations_changed(&mut self, cx: &mut Context<Self>) {
        self.automations_revision += 1;
        cx.notify();
    }

    /// Changes them, saves, and tells the pages.
    pub fn edit_automations<R>(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Vec<Automation>) -> R) -> R {
        let result = automations::edit(self.library.as_ref(), change);
        self.automations_changed(cx);
        result
    }

    pub fn run_due_automations(&mut self, cx: &mut Context<Self>) {
        let now = automations::now();
        let due: Vec<String> = self.automations().into_iter().filter(|a| a.due(now) && self.project(&a.project_id).is_some()).map(|a| a.id).collect();
        for id in due {
            let _ = self.run_automation(&id, false, cx);
        }
    }

    /// Runs one now and records it: the thread it started or sent to, or why it couldn't.
    /// `manual`: Run Now, which leaves the schedule as it was.
    pub fn run_automation(&mut self, id: &str, manual: bool, cx: &mut Context<Self>) -> Result<String, String> {
        let Some(auto) = self.automations().into_iter().find(|a| a.id == id) else { return Err("That automation is gone.".into()) };
        let now = automations::now();
        let result = self.fire(&auto, cx);
        if logs() {
            match &result {
                Ok(thread) => eprintln!("AUTOMATION ran “{}” ({}) in {thread}{}", auto.name, auto.schedule.describe(now), if manual { " by hand" } else { "" }),
                Err(e) => eprintln!("AUTOMATION couldn't run “{}”: {e}", auto.name),
            }
        }
        let (thread, error) = match &result {
            Ok(t) => (Some(t.clone()), None),
            Err(e) => (None, Some(e.clone())),
        };
        self.edit_automations(cx, |list| {
            if let Some(a) = list.iter_mut().find(|a| a.id == id) {
                a.ran(now, thread, error, manual);
            }
        });
        result
    }

    fn fire(&mut self, auto: &Automation, cx: &mut Context<Self>) -> Result<String, String> {
        if self.project(&auto.project_id).is_none() {
            return Err("Its project is gone.".into());
        }
        // Its own thread, when it has one that's still here: queued behind a turn that's going.
        if auto.same_thread {
            if let Some(thread) = auto.thread_id.as_deref().and_then(|t| self.thread(t)) {
                let id = thread.read(cx).id().to_string();
                if thread.read(cx).meta.settled_at.is_some() {
                    self.set_settled(&id, false, cx);
                }
                self.send_message(&thread, &auto.prompt, vec![], vec![], yh_core::claude::Timing::Queue, cx);
                return Ok(id);
            }
        }
        let running = self.threads.iter().filter(|t| {
            let t = t.read(cx);
            t.run.running && t.meta.project_id == auto.project_id
        });
        if running.count() >= MAX_RUNNING {
            return Err(format!("{MAX_RUNNING} threads were already working in the project."));
        }
        let id = self.start_thread(&auto.project_id, Some(&auto.prompt), auto.model.clone(), auto.effort.clone(), None, None, cx)?;
        self.rename(&id, &auto.name, cx);
        let automation = auto.id.clone();
        self.update_meta(&id, cx, |_, record| {
            record.insert("automationID".into(), automation.into());
        });
        Ok(id)
    }
}
