//! What the app knows and does, with no drawing: projects, threads, which one is open, each
//! thread's `claude`, and every request the UI makes (send, stop, allow, pin, rename…). Views
//! read it and call it; it tells them what changed with `WorkspaceEvent`s and `notify`.
//! The port of the web rewrite's `Backend` and `ThreadRuntime` (legacy/Sources/HarnessBackend).

use crate::probe;
use crate::state::thread::ThreadState;
use gpui_kit::{App, AppContext, Context, Entity, EventEmitter, Task, WeakEntity};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use yh_core::claude::{ClaudeProcess, Decision, Event, LaunchOptions, ModelInfo, SlashCommand};
use yh_core::library::{cli_mode, control_mode, data_directory, iso_now, Library, ProjectMeta, ThreadMeta};

pub const PAGE: usize = 160;
/// A thread's `claude` is let go of after this long without a turn, and resumed on the next.
const IDLE_LIMIT: Duration = Duration::from_secs(600);

pub enum WorkspaceEvent {
    /// A thread was opened.
    Selected(String),
    /// A thread went away.
    Removed(String),
}

/// One thread's `claude`, between turns too.
struct Runtime {
    process: ClaudeProcess,
    /// Events from an older process are ignored.
    generation: u64,
    /// Effort is a launch option: a different one starts a new process.
    effort: Option<String>,
    idle: Option<Task<()>>,
}

pub struct Workspace {
    pub library: Arc<Library>,
    pub data_dir: PathBuf,
    pub projects: Vec<ProjectMeta>,
    pub threads: Vec<Entity<ThreadState>>,
    by_id: HashMap<String, Entity<ThreadState>>,
    pub selected: Option<String>,
    runtimes: HashMap<String, Runtime>,
    generation: u64,
    /// The CLI's models, as it last listed them (saved as `claudeModels`).
    pub models: Vec<ModelInfo>,
    /// Usage limits as Claude last reported them.
    pub limits: Option<Value>,
    /// Each project's slash commands and skills, as the CLI last listed them (`slashCommands`).
    commands: HashMap<String, Vec<SlashCommand>>,
    /// A folder's files for `@` mentions, kept 30 seconds.
    files: HashMap<String, (Instant, Arc<Vec<String>>)>,
    new_count: usize,
}

impl EventEmitter<WorkspaceEvent> for Workspace {}

impl Workspace {
    pub fn new(library: Arc<Library>, cx: &mut Context<Self>) -> Self {
        let weak = cx.entity().downgrade();
        let threads: Vec<Entity<ThreadState>> =
            library.threads.iter().map(|meta| make_thread(meta.clone(), weak.clone(), cx)).collect();
        let by_id = threads.iter().map(|t| (t.read(cx).id().to_string(), t.clone())).collect();
        let models = library
            .setting("claudeModels")
            .and_then(|v| v.as_array().map(|a| a.iter().filter_map(ModelInfo::from_json).collect()))
            .unwrap_or_default();
        let mut workspace = Workspace {
            projects: library.projects.clone(),
            data_dir: data_directory(),
            library,
            threads,
            by_id,
            selected: None,
            runtimes: HashMap::new(),
            generation: 0,
            models,
            limits: None,
            commands: HashMap::new(),
            files: HashMap::new(),
            new_count: 0,
        };
        if let Some(Value::Object(all)) = workspace.library.setting("slashCommands") {
            for (project, list) in all {
                let list = list.as_array().map(|a| a.iter().filter_map(SlashCommand::from_json).collect()).unwrap_or_default();
                workspace.commands.insert(project, list);
            }
        }
        workspace.recover_cut_off_turns(cx);
        workspace
    }

    pub fn thread(&self, id: &str) -> Option<Entity<ThreadState>> {
        self.by_id.get(id).cloned()
    }

    pub fn selected_thread(&self) -> Option<Entity<ThreadState>> {
        self.selected.as_deref().and_then(|id| self.thread(id))
    }

    pub fn project(&self, id: &str) -> Option<&ProjectMeta> {
        self.projects.iter().find(|p| p.id == id)
    }

    /// The Home project's ID, shown first and named Home.
    pub fn home_project(&self) -> Option<String> {
        self.library.setting("homeProjectID").and_then(|v| v.as_str().map(str::to_string))
    }

    pub fn running_count(&self, cx: &App) -> usize {
        self.threads.iter().filter(|t| t.read(cx).running).count()
    }

    /// The default model or effort from Settings (`YH_MODEL`/`YH_EFFORT` in test runs).
    fn default_setting(&self, key: &str) -> Option<String> {
        std::env::var(format!("YH_{}", key.to_uppercase()))
            .ok()
            .or_else(|| self.library.setting("defaults").and_then(|d| d[key].as_str().map(str::to_string)))
            .filter(|s| !s.is_empty())
    }

    /// The thread's model: its own, else the default.
    pub fn model_of(&self, thread: &ThreadState) -> Option<String> {
        thread.meta.model.clone().or_else(|| self.default_setting("model"))
    }

    pub fn effort_of(&self, thread: &ThreadState) -> Option<String> {
        thread.meta.effort.clone().or_else(|| self.default_setting("effort"))
    }

    // MARK: Opening and reading

    /// The click is answered in the next frame (selection, title, a held transcript); reading a
    /// history that isn't held follows, off the main thread.
    pub fn select(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(thread) = self.thread(id) else { return };
        if self.selected.as_deref() == Some(id) {
            return;
        }
        if let Some(old) = self.selected_thread() {
            // A draft stays with its thread; the composer saves it on the way out.
            old.update(cx, |t, _| t.unread = false);
        }
        self.selected = Some(id.to_string());
        thread.update(cx, |t, cx| {
            if t.unread {
                t.unread = false;
                cx.notify();
            }
        });
        self.ensure_loaded(&thread, cx);
        cx.emit(WorkspaceEvent::Selected(id.to_string()));
        cx.notify();
    }

    pub fn ensure_loaded(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let (id, needed) = {
            let t = thread.read(cx);
            (t.id().to_string(), !t.loaded && !t.loading)
        };
        if !needed {
            return;
        }
        if id.starts_with("new:") {
            thread.update(cx, |t, _| t.loaded = true);
            return;
        }
        // Rows held from a live turn are newer than the history: read from before them.
        let before = thread.update(cx, |t, _| {
            t.loading = true;
            t.first_seq()
        });
        let library = self.library.clone();
        let weak = thread.downgrade();
        cx.spawn(async move |_, cx| {
            let page = cx.background_executor().spawn(async move { library.page(&id, before, PAGE) }).await;
            let _ = weak.update(cx, |t, cx| {
                if !t.loading {
                    return; // let go of meanwhile
                }
                if t.total.is_none() {
                    t.total = Some(page.total);
                }
                t.prepend(page.entries, page.from);
                t.loaded = true;
                t.loading = false;
                cx.notify();
            });
        })
        .detach();
    }

    pub fn load_earlier(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let (id, before) = {
            let t = thread.read(cx);
            if !t.loaded || t.loading_earlier || t.before == 0 {
                return;
            }
            (t.id().to_string(), t.before)
        };
        thread.update(cx, |t, _| t.loading_earlier = true);
        let library = self.library.clone();
        let weak = thread.downgrade();
        cx.spawn(async move |_, cx| {
            let page = cx.background_executor().spawn(async move { library.page(&id, Some(before), PAGE) }).await;
            let _ = weak.update(cx, |t, cx| {
                t.loading_earlier = false;
                if t.loaded && t.before == before {
                    t.prepend(page.entries, page.from);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The transcript was let go of on screen: save its rows and keep only what a running turn
    /// needs.
    pub fn release(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        self.save_rows(thread, cx);
        thread.update(cx, |t, cx| {
            t.in_deck = false;
            if t.running {
                t.trim(200);
            } else {
                t.unload();
            }
            cx.notify();
        });
    }

    // MARK: Threads and projects

    pub fn new_thread(&mut self, project_id: &str, cx: &mut Context<Self>) -> String {
        self.new_count += 1;
        // Saved in harness.sqlite right away, as the Swift app does; `new:` only without one.
        let meta = if self.library.db.is_some() && !project_id.starts_with("dir:") {
            self.library.add_thread(project_id, "New thread")
        } else {
            ThreadMeta::new(format!("new:{}", self.new_count), project_id.into(), "New thread".into())
        };
        let thread = make_thread(meta, cx.entity().downgrade(), cx);
        let id = thread.read(cx).id().to_string();
        thread.update(cx, |t, _| t.loaded = true);
        self.threads.insert(0, thread.clone());
        self.by_id.insert(id.clone(), thread);
        self.select(&id, cx);
        id
    }

    pub fn add_project(&mut self, path: &str, cx: &mut Context<Self>) -> String {
        let project = self.library.add_project(path);
        let id = project.id.clone();
        if self.project(&id).is_none() {
            self.projects.push(project);
        }
        cx.notify();
        id
    }

    fn update_meta(&mut self, id: &str, cx: &mut Context<Self>, change: impl FnOnce(&mut ThreadMeta, &mut serde_json::Map<String, Value>)) {
        let Some(thread) = self.thread(id) else { return };
        let mut record_change: Option<serde_json::Map<String, Value>> = None;
        thread.update(cx, |t, cx| {
            let mut patch = serde_json::Map::new();
            change(&mut t.meta, &mut patch);
            t.title = t.meta.title.clone();
            record_change = Some(patch);
            cx.notify();
        });
        if let Some(patch) = record_change {
            self.library.update_thread(id, |record| {
                for (k, v) in patch {
                    if v.is_null() {
                        record.remove(&k);
                    } else {
                        record.insert(k, v);
                    }
                }
            });
        }
        cx.notify();
    }

    pub fn rename(&mut self, id: &str, title: &str, cx: &mut Context<Self>) {
        let title = title.trim().to_string();
        if title.is_empty() {
            return;
        }
        self.update_meta(id, cx, |m, r| {
            m.title = title.clone();
            m.title_chosen = true;
            r.insert("title".into(), title.into());
            r.insert("titleChosen".into(), true.into());
        });
    }

    pub fn set_pinned(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        self.update_meta(id, cx, |m, r| {
            m.pinned = on;
            r.insert("isPinned".into(), if on { true.into() } else { Value::Null });
        });
    }

    /// On its project's Settled shelf, its `claude` ended, history kept; or brought back.
    pub fn set_settled(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        if on {
            self.end_runtime(id, cx);
        }
        let now = iso_now();
        self.update_meta(id, cx, |m, r| {
            if on {
                m.settled_at = Some(yh_core::library::now_ms());
                r.insert("settledAt".into(), now.into());
            } else {
                m.settled_at = None;
                m.updated_at = yh_core::library::now_ms();
                r.insert("settledAt".into(), Value::Null);
                r.insert("reactivatedAt".into(), now.into());
            }
        });
    }

    pub fn delete(&mut self, id: &str, cx: &mut Context<Self>) {
        self.end_runtime(id, cx);
        self.threads.retain(|t| t.read(cx).id() != id);
        self.by_id.remove(id);
        self.library.remove_thread(id);
        if self.selected.as_deref() == Some(id) {
            self.selected = None;
        }
        cx.emit(WorkspaceEvent::Removed(id.to_string()));
        cx.notify();
    }

    /// The composer's text, kept with the thread until sent.
    pub fn save_draft(&mut self, id: &str, text: &str, cx: &mut Context<Self>) {
        let Some(thread) = self.thread(id) else { return };
        let draft = (!text.trim().is_empty()).then(|| text.to_string());
        if thread.read(cx).meta.draft == draft {
            return;
        }
        thread.update(cx, |t, _| t.meta.draft = draft.clone());
        self.library.update_thread(id, |r| match draft {
            Some(d) => {
                r.insert("draft".into(), d.into());
            }
            None => {
                r.remove("draft");
            }
        });
    }

    pub fn set_model(&mut self, id: &str, model: Option<String>, cx: &mut Context<Self>) {
        let value = model.clone();
        self.update_meta(id, cx, |m, r| {
            m.model = value.clone();
            r.insert("model".into(), value.map(Value::from).unwrap_or(Value::Null));
        });
        if let Some(rt) = self.runtimes.get_mut(id) {
            rt.process.set_model(model.as_deref().unwrap_or(""));
        }
    }

    /// Applies from the next start of its `claude` (effort is a launch option).
    pub fn set_effort(&mut self, id: &str, effort: Option<String>, cx: &mut Context<Self>) {
        self.update_meta(id, cx, |m, r| {
            m.effort = effort.clone();
            r.insert("effort".into(), effort.map(Value::from).unwrap_or(Value::Null));
        });
    }

    /// The project's permission mode; running threads switch at once.
    pub fn set_mode(&mut self, project_id: &str, mode: &str, cx: &mut Context<Self>) {
        self.library.update_project(project_id, |p| {
            p.insert("permissionMode".into(), mode.into());
        });
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == project_id) {
            p.mode = mode.to_string();
        }
        let ids: Vec<String> =
            self.threads.iter().filter(|t| t.read(cx).meta.project_id == project_id).map(|t| t.read(cx).id().to_string()).collect();
        for id in ids {
            if let Some(rt) = self.runtimes.get_mut(&id) {
                rt.process.set_mode(control_mode(mode));
            }
        }
        cx.notify();
    }

    // MARK: Turns

    /// Sends your message. While Claude works it waits for the current step (steering), or
    /// with `now` stops the turn for it.
    pub fn send(&mut self, thread: &Entity<ThreadState>, text: &str, now: bool, cx: &mut Context<Self>) {
        self.send_with_images(thread, text, vec![], now, cx)
    }

    /// With images already saved in `attachments/` (by name).
    pub fn send_with_images(&mut self, thread: &Entity<ThreadState>, text: &str, images: Vec<String>, now: bool, cx: &mut Context<Self>) {
        let text = text.trim().to_string();
        if text.is_empty() && images.is_empty() {
            return;
        }
        let (id, running, title, chosen) = {
            let t = thread.read(cx);
            (t.id().to_string(), t.running, t.title.clone(), t.meta.title_chosen)
        };
        // The next row's number: its rows so far, before the first new one.
        if thread.read(cx).total.is_none() {
            let total = self.library.db.as_ref().map_or(0, |db| db.transcript_count(&id));
            thread.update(cx, |t, _| t.total = Some(total));
        }
        let first_message = thread.read(cx).total == Some(0);
        let message_id = yh_core::library::new_uuid();
        let delivery = running.then_some(if now { "interrupted" } else { "steered" });
        thread.update(cx, |t, cx| {
            t.add_user(&text, &images, &message_id, running && !now, delivery);
            t.running = true;
            t.meta.turn_in_progress = true;
            t.meta.draft = None;
            if title == "New thread" && !chosen {
                // Named after its first message until a title is generated or chosen.
                t.title = text.lines().next().unwrap_or("").trim().chars().take(60).collect();
                t.meta.title = t.title.clone();
            }
            cx.notify();
        });
        let new_title = thread.read(cx).title.clone();
        self.library.update_thread(&id, |r| {
            r.remove("draft");
            r.remove("settledAt");
            r.insert("title".into(), new_title.into());
            r.insert("turnInProgress".into(), true.into());
            r.insert("updatedAt".into(), iso_now().into());
        });
        if first_message && title == "New thread" && !chosen && !text.is_empty() {
            self.generate_title(thread, &text, cx);
        }
        if let Err(error) = self.ensure_runtime(thread, cx) {
            thread.update(cx, |t, cx| {
                t.notice(&format!("Couldn't send: {error}"));
                t.running = false;
                cx.notify();
            });
            self.turn_ended(thread, cx);
            return;
        }
        if let Some(rt) = self.runtimes.get_mut(&id) {
            rt.idle = None;
            let blocks: Vec<(String, String)> = images
                .iter()
                .filter_map(|name| {
                    let bytes = std::fs::read(yh_core::attachments::path(&self.data_dir, name)).ok()?;
                    Some((yh_core::attachments::media_type(name).to_string(), yh_core::attachments::base64(&bytes)))
                })
                .collect();
            rt.process.send(&text, &blocks, &message_id, now.then_some("now"));
        }
        self.save_rows(thread, cx);
        cx.notify();
    }

    fn ensure_runtime(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) -> Result<(), String> {
        let t = thread.read(cx);
        let id = t.id().to_string();
        let effort = self.effort_of(t);
        if let Some(rt) = self.runtimes.get(&id) {
            if rt.effort == effort {
                return Ok(());
            }
        }
        self.end_runtime(&id, cx);
        let t = thread.read(cx);
        let project = self.project(&t.meta.project_id).cloned();
        let cwd = self.folder_of(t);
        let attachments = self.data_dir.join("attachments");
        let mut env = vec![];
        // Claude Code's task tools: what the plan checklist shows.
        if self.library.setting("planChecklist").and_then(|v| v.as_bool()).unwrap_or(true) {
            env.push(("CLAUDE_CODE_ENABLE_TODO_TOOLS".to_string(), "1".to_string()));
        }
        let options = LaunchOptions {
            cwd,
            session_id: t.session_id.clone(),
            model: self.model_of(t),
            mode: cli_mode(project.as_ref().map_or("auto", |p| p.mode.as_str())).map(str::to_string),
            effort: effort.clone(),
            add_dirs: vec![attachments.to_string_lossy().into_owned()],
            env,
        };
        let (tx, rx) = flume::unbounded();
        let mut process = ClaudeProcess::spawn(options, tx)?;
        process.initialize();
        self.generation += 1;
        let generation = self.generation;
        self.runtimes.insert(id.clone(), Runtime { process, generation, effort, idle: None });
        self.pump(thread.clone(), id, generation, rx, cx);
        Ok(())
    }

    /// Ends a thread's `claude`; the next message resumes its session.
    fn end_runtime(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(rt) = self.runtimes.remove(id) else { return };
        rt.process.terminate();
        if let Some(thread) = self.thread(id) {
            if thread.read(cx).running {
                thread.update(cx, |t, cx| {
                    t.apply(Event::Exit { code: 0, tail: String::new() }, cx);
                    cx.notify();
                });
                self.turn_ended(&thread, cx);
            }
        }
    }

    /// Claude's events into the thread: what's arrived, at most once a frame, deltas joined.
    fn pump(&mut self, thread: Entity<ThreadState>, id: String, generation: u64, rx: flume::Receiver<Event>, cx: &mut Context<Self>) {
        let log = std::env::var("YH_EVENT_LOG").as_deref() == Ok("1");
        cx.spawn(async move |this, cx| {
            let mut saved_at = Instant::now();
            while let Ok(first) = rx.recv_async().await {
                let current = this.update(cx, |ws, _| ws.runtimes.get(&id).is_some_and(|r| r.generation == generation)).unwrap_or(false);
                if !current {
                    break;
                }
                let started = Instant::now();
                let mut batch: Vec<Event> = vec![first];
                while let Ok(next) = rx.try_recv() {
                    if !batch.last_mut().unwrap().merge(&next) {
                        batch.push(next);
                    }
                }
                if log {
                    for e in &batch {
                        eprintln!("event {id}: {e:?}");
                    }
                }
                let ended = batch.iter().any(|e| matches!(e, Event::Done { .. } | Event::Exit { .. }));
                let exited = batch.iter().any(|e| matches!(e, Event::Exit { .. }));
                let count = batch.len();
                // What the workspace keeps (catalog, limits, session) before the rows take the rest.
                let _ = this.update(cx, |ws, cx| {
                    for e in &batch {
                        match e {
                            Event::Catalog { models, commands } => {
                                let project = thread.read(cx).meta.project_id.clone();
                                ws.note_catalog(models, commands, &project, cx);
                            }
                            Event::Limits(info) => ws.limits = Some(info.clone()),
                            Event::Session { id: sid } => {
                                let sid = sid.clone();
                                ws.library.update_thread(&id, |r| {
                                    r.insert("sessionID".into(), sid.into());
                                });
                            }
                            _ => {}
                        }
                    }
                });
                let chrome = thread.update(cx, |t, cx| {
                    let mut chrome = false;
                    for event in batch {
                        chrome |= t.apply(event, cx);
                    }
                    cx.notify();
                    chrome
                });
                probe::note_batch(started.elapsed().as_secs_f64() * 1000.0, count);
                let thread2 = thread.clone();
                let id2 = id.clone();
                let _ = this.update(cx, |ws, cx| {
                    // Saved at most once a second while it streams, and when anything settles.
                    if chrome || ended || saved_at.elapsed() > Duration::from_secs(1) {
                        saved_at = Instant::now();
                        ws.save_rows(&thread2, cx);
                    }
                    if exited {
                        ws.runtimes.remove(&id2);
                    }
                    if ended {
                        ws.turn_ended(&thread2, cx);
                    }
                    if chrome {
                        cx.notify();
                        ws.answer_for_tests(&thread2, cx);
                    }
                });
                if exited {
                    break;
                }
                cx.background_executor().timer(Duration::from_millis(8)).await;
            }
        })
        .detach();
    }

    /// The turn is over: saved, unread if you were elsewhere, its `claude` let go of later.
    fn turn_ended(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let id = thread.read(cx).id().to_string();
        let selected = self.selected.as_deref() == Some(id.as_str());
        let (title, session) = thread.update(cx, |t, _| {
            t.running = false;
            t.meta.turn_in_progress = false;
            t.meta.updated_at = yh_core::library::now_ms();
            if !selected {
                t.unread = true;
            }
            (t.title.clone(), t.session_id.clone())
        });
        self.library.update_thread(&id, |r| {
            r.remove("turnInProgress");
            r.insert("title".into(), title.into());
            if let Some(s) = session {
                r.insert("sessionID".into(), s.into());
            }
            r.insert("updatedAt".into(), iso_now().into());
        });
        self.save_rows(thread, cx);
        let weak = thread.downgrade();
        if let Some(rt) = self.runtimes.get_mut(&id) {
            rt.idle = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(IDLE_LIMIT).await;
                let _ = this.update(cx, |ws, cx| {
                    let waiting = weak.upgrade().is_some_and(|t| t.read(cx).running || t.read(cx).permission.is_some());
                    if !waiting {
                        ws.end_runtime(&id, cx);
                    }
                });
            }));
        }
        cx.notify();
    }

    /// After the first message: a title written by a one-shot request with no tools, on the
    /// model chosen in Settings (Haiku by default), off the main thread. One you chose
    /// meanwhile stays.
    fn generate_title(&mut self, thread: &Entity<ThreadState>, message: &str, cx: &mut Context<Self>) {
        let settings = self.library.setting("titleGeneration").unwrap_or(serde_json::json!({"provider": "claude", "model": "haiku"}));
        if settings["provider"].as_str() != Some("claude") {
            return;
        }
        let model = settings["model"].as_str().unwrap_or("haiku").to_string();
        let rules = self.library.setting("titleRules").and_then(|r| r.as_str().map(str::to_string));
        let prompt = yh_core::titles::initial_prompt(message, rules.as_deref());
        let Some(claude) = yh_core::claude::find_claude() else { return };
        let path = std::env::var("YH_PATH").ok();
        let weak = thread.downgrade();
        cx.spawn(async move |this, cx| {
            let mut result = Err(String::new());
            for attempt in 0..2 {
                if attempt > 0 {
                    cx.background_executor().timer(Duration::from_secs(2)).await;
                }
                let (claude, path, model, prompt) = (claude.clone(), path.clone(), model.clone(), prompt.clone());
                result = cx.background_executor().spawn(async move { yh_core::titles::generate(&claude, path.as_deref(), &model, &prompt) }).await;
                if result.is_ok() {
                    break;
                }
            }
            let Ok(title) = result else { return };
            let _ = this.update(cx, |ws, cx| {
                let Some(thread) = weak.upgrade() else { return };
                if thread.read(cx).meta.title_chosen {
                    return;
                }
                let id = thread.read(cx).id().to_string();
                thread.update(cx, |t, cx| {
                    t.title = title.clone();
                    t.meta.title = title.clone();
                    cx.notify();
                });
                ws.library.update_thread(&id, |r| {
                    r.insert("title".into(), title.into());
                });
                cx.notify();
            });
        })
        .detach();
    }

    pub fn interrupt(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let id = thread.read(cx).id().to_string();
        if let Some(rt) = self.runtimes.get_mut(&id) {
            rt.process.interrupt();
        }
    }

    /// Settles the thread's pending permission request (a tool, a question, a plan).
    pub fn respond(&mut self, thread: &Entity<ThreadState>, decision: Decision, cx: &mut Context<Self>) {
        let (id, permission) = {
            let t = thread.read(cx);
            (t.id().to_string(), t.permission.clone())
        };
        let Some(permission) = permission else { return };
        if let Some(rt) = self.runtimes.get(&id) {
            rt.process.respond(&permission.id, &permission.input, &decision);
        }
        thread.update(cx, |t, cx| {
            t.permission = None;
            cx.notify();
        });
        cx.notify();
    }

    /// Approves a plan into a permission mode, which becomes the project's.
    pub fn approve_plan(&mut self, thread: &Entity<ThreadState>, mode: &str, cx: &mut Context<Self>) {
        let project = thread.read(cx).meta.project_id.clone();
        self.set_mode(&project, mode, cx);
        self.respond(thread, Decision::Allow { updated_input: None, mode: Some(control_mode(mode).into()) }, cx);
    }

    /// `YH_ANSWER=allow|deny`: permission prompts answered by themselves, for tests.
    fn answer_for_tests(&mut self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let Ok(answer) = std::env::var("YH_ANSWER") else { return };
        if let Some(p) = thread.read(cx).permission.clone() {
            eprintln!("answering {} for {}: {answer}", p.id, p.tool);
            let decision = if answer == "allow" { Decision::allow() } else { Decision::deny() };
            self.respond(thread, decision, cx);
        }
    }

    /// The project's slash commands and skills.
    pub fn commands(&self, project_id: &str) -> &[SlashCommand] {
        self.commands.get(project_id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Where a thread's `claude` runs: its folder, else its project's, else the data dir's `home`.
    pub fn folder_of(&self, thread: &ThreadState) -> String {
        thread
            .meta
            .working_directory
            .clone()
            .or_else(|| self.project(&thread.meta.project_id).map(|p| p.path.clone()))
            .filter(|p| std::path::Path::new(p).is_dir())
            .unwrap_or_else(|| {
                // A project that's gone runs in a folder of the data dir, never your home folder.
                let home = self.data_dir.join("home");
                let _ = std::fs::create_dir_all(&home);
                home.to_string_lossy().into_owned()
            })
    }

    /// The thread's folder's files matching `query`, best first; listed off the main thread.
    pub fn mention_files(&mut self, thread: &Entity<ThreadState>, query: String, cx: &mut Context<Self>) -> Task<Vec<String>> {
        let folder = self.folder_of(thread.read(cx));
        let cached = self.files.get(&folder).filter(|(at, _)| at.elapsed() < Duration::from_secs(30)).map(|(_, f)| f.clone());
        cx.spawn(async move |this, cx| {
            let files = match cached {
                Some(files) => files,
                None => {
                    let path = folder.clone();
                    let files = Arc::new(cx.background_executor().spawn(async move { yh_core::files::list(std::path::Path::new(&path)) }).await);
                    let _ = this.update(cx, |ws, _| {
                        ws.files.insert(folder, (Instant::now(), files.clone()));
                    });
                    files
                }
            };
            cx.background_executor().spawn(async move { yh_core::files::matches(&files, &query, 12) }).await
        })
    }

    fn note_catalog(&mut self, models: &Value, commands: &Value, project_id: &str, cx: &mut Context<Self>) {
        if let Some(list) = commands.as_array() {
            let parsed: Vec<SlashCommand> = list.iter().filter_map(SlashCommand::from_json).collect();
            if !parsed.is_empty() {
                self.commands.insert(project_id.to_string(), parsed);
                let mut all = match self.library.setting("slashCommands") {
                    Some(Value::Object(m)) => m,
                    _ => serde_json::Map::new(),
                };
                // In the Swift app's shape (it decodes its own `SlashCommand`).
                let swift: Vec<Value> = list
                    .iter()
                    .filter(|c| c["name"].as_str().is_some_and(|n| !n.is_empty()))
                    .map(|c| {
                        serde_json::json!({
                            "name": c["name"], "description": c["description"].as_str().unwrap_or(""),
                            "argumentHint": c["argumentHint"].as_str().unwrap_or(""),
                            "aliases": c["aliases"].as_array().cloned().unwrap_or_default(),
                            "isBuiltin": c["builtin"].as_bool().unwrap_or(false),
                        })
                    })
                    .collect();
                all.insert(project_id.to_string(), Value::Array(swift));
                self.library.set_setting("slashCommands", &Value::Object(all));
            }
        }
        let parsed: Vec<ModelInfo> = models.as_array().map(|a| a.iter().filter_map(ModelInfo::from_json).collect()).unwrap_or_default();
        if parsed.is_empty() || parsed == self.models {
            return;
        }
        self.models = parsed;
        self.library.set_setting("claudeModels", models);
        cx.notify();
    }

    /// The thread's changed rows into harness.sqlite (in the background).
    pub fn save_rows(&self, thread: &Entity<ThreadState>, cx: &mut Context<Self>) {
        let Some(db) = self.library.db.clone() else { return };
        let (id, total, rows) = thread.update(cx, |t, _| (t.id().to_string(), t.total, t.take_changes()));
        if rows.is_empty() || id.starts_with("new:") || id.starts_with("cc:") {
            return;
        }
        if let Some(total) = total {
            db.save_rows(&id, total, rows);
        }
    }

    /// Quitting: everything saved, every `claude` ended.
    pub fn prepare_for_quit(&mut self, cx: &mut Context<Self>) {
        for t in self.threads.clone() {
            self.save_rows(&t, cx);
        }
        for (_, rt) in self.runtimes.drain() {
            rt.process.terminate();
        }
        self.library.save();
        if let Some(db) = &self.library.db {
            db.flush();
        }
    }

    // MARK: Recovery

    /// Turns a quit or crash cut off (`turnInProgress` still set at launch): finished from
    /// Claude Code's session file (it writes as it goes), with a note, in the background.
    fn recover_cut_off_turns(&mut self, cx: &mut Context<Self>) {
        let Some(db) = self.library.db.clone() else { return };
        let cut: Vec<Entity<ThreadState>> = self.threads.iter().filter(|t| t.read(cx).meta.turn_in_progress).cloned().collect();
        for thread in cut {
            let (id, session) = {
                let t = thread.read(cx);
                (t.id().to_string(), t.session_id.clone())
            };
            let db = db.clone();
            let weak = thread.downgrade();
            cx.spawn(async move |this, cx| {
                let id2 = id.clone();
                let count = cx
                    .background_executor()
                    .spawn(async move { yh_core::recovery::recover(&db, &id2, session.as_deref()) })
                    .await;
                let _ = this.update(cx, |ws, cx| {
                    ws.library.update_thread(&id, |r| {
                        r.remove("turnInProgress");
                    });
                    let Some(thread) = weak.upgrade() else { return };
                    // Talked to meanwhile: what's there now counts.
                    if thread.read(cx).running {
                        return;
                    }
                    let open = ws.selected.as_deref() == Some(id.as_str());
                    thread.update(cx, |t, cx| {
                        t.meta.turn_in_progress = false;
                        t.unload();
                        t.total = Some(count);
                        cx.notify();
                    });
                    if open {
                        ws.ensure_loaded(&thread, cx);
                    }
                });
            })
            .detach();
        }
    }
}

fn make_thread(meta: ThreadMeta, workspace: WeakEntity<Workspace>, cx: &mut Context<Workspace>) -> Entity<ThreadState> {
    let thread = cx.new(|_| ThreadState::new(meta));
    // Near the top of what's held: read the page before it.
    let weak = thread.downgrade();
    thread.read(cx).list.set_scroll_handler(move |event, _, cx| {
        if event.visible_range.start > 4 {
            return;
        }
        let (ws, weak) = (workspace.clone(), weak.clone());
        cx.defer(move |cx| {
            if let (Some(ws), Some(thread)) = (ws.upgrade(), weak.upgrade()) {
                ws.update(cx, |ws, cx| ws.load_earlier(&thread, cx));
            }
        });
    });
    thread
}
