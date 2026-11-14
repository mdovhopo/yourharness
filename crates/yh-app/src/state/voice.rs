//! Voice input: what you say, written into the composer (never sent). The speech models
//! Settings → Voice offers, downloaded when you ask, into `<data>/voice-models` (`YH_VOICE_MODELS`
//! elsewhere); the one picked as default, saved in the `voice` setting; and the recording: the mic
//! toggled on, off, then the text emitted as `VoiceEvent::Heard` for the thread it was started in.
//! The model runs on `yh_voice`'s own thread, loaded when a recording starts and let go after a
//! few idle minutes.
//!
//! `YH_VOICE_FILE=<audio>` stands in for the microphone in test runs (a real one would ask for
//! permission): a recording "hears" that file, whatever its length. `YH_VOICE_USE=<id>` picks a
//! model at launch and `YH_VOICE_DOWNLOAD=<id>` presses its Download (scratch data only).

use gpui_kit::{App, AppContext, Context, Entity, EventEmitter, Global};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use yh_core::library::Library;
use yh_voice::{Engine, Model};

/// A recording stops by itself after this long.
const LONGEST: Duration = Duration::from_secs(10 * 60);

pub enum Phase {
    Idle,
    /// Asking macOS for the microphone, or opening it.
    Starting { thread: String },
    Recording { thread: String, since: Instant, source: Source },
    Transcribing { thread: String },
}

pub enum Source {
    Mic(yh_voice::mic::Recording),
    /// `YH_VOICE_FILE`'s audio.
    File(Vec<f32>),
}

pub enum VoiceEvent {
    /// What was said in `thread`'s composer (empty: nothing heard).
    Heard { thread: String, text: String },
}

struct Download {
    cancel: Arc<AtomicBool>,
    /// 0…1.
    progress: f32,
}

pub struct Voice {
    library: Arc<Library>,
    models: PathBuf,
    worker: yh_voice::Worker,
    installed: HashSet<&'static str>,
    downloads: HashMap<&'static str, Download>,
    /// Why a model's download or the last recording failed, by model id ("" for the recording).
    pub errors: HashMap<&'static str, String>,
    pub phase: Phase,
}

impl EventEmitter<VoiceEvent> for Voice {}

/// The one `Voice`, for the composer and Settings.
pub struct VoiceLink(pub Entity<Voice>);

impl Global for VoiceLink {}

pub fn get(cx: &App) -> Option<Entity<Voice>> {
    cx.try_global::<VoiceLink>().map(|l| l.0.clone())
}

impl Voice {
    pub fn new(library: Arc<Library>) -> Voice {
        let models = std::env::var("YH_VOICE_MODELS").map(PathBuf::from).unwrap_or_else(|_| yh_core::library::data_directory().join("voice-models"));
        let installed = yh_voice::MODELS.iter().filter(|m| yh_voice::download::installed(&models, m)).map(|m| m.id).collect();
        let worker = yh_voice::Worker::start(models.clone());
        Voice { library, models, worker, installed, downloads: HashMap::new(), errors: HashMap::new(), phase: Phase::Idle }
    }

    pub fn start_global(library: Arc<Library>, cx: &mut App) {
        let voice = cx.new(|_| Voice::new(library));
        voice.update(cx, |v, cx| {
            if let Some(model) = std::env::var("YH_VOICE_USE").ok().and_then(|id| yh_voice::find(&id)) {
                v.set_default(model, cx);
            }
            if let Some(model) = std::env::var("YH_VOICE_DOWNLOAD").ok().and_then(|id| yh_voice::find(&id)) {
                v.download(model, cx);
            }
        });
        cx.set_global(VoiceLink(voice));
    }

    /// Let go of the model before the process exits (whisper.cpp aborts at exit otherwise).
    pub fn shutdown(&self) {
        self.worker.shutdown();
    }

    pub fn is_installed(&self, model: &Model) -> bool {
        self.installed.contains(model.id)
    }

    /// 0…1 while it downloads.
    pub fn download_progress(&self, model: &Model) -> Option<f32> {
        self.downloads.get(model.id).map(|d| d.progress)
    }

    /// The model picked in Settings, if it's still there.
    pub fn default_model(&self) -> Option<&'static Model> {
        let id = self.library.setting("voice").and_then(|v| v["model"].as_str().map(str::to_string))?;
        yh_voice::find(&id).filter(|m| self.is_installed(m))
    }

    /// Voice input is ready: a model picked and on disk. Until then the mic leads to Settings.
    pub fn ready(&self) -> bool {
        self.default_model().is_some()
    }

    pub fn set_default(&mut self, model: &'static Model, cx: &mut Context<Self>) {
        let mut v = self.library.setting("voice").filter(Value::is_object).unwrap_or(json!({}));
        v["model"] = json!(model.id);
        self.library.set_setting("voice", &v);
        cx.notify();
    }

    /// Downloads `model`'s files; picked as default when it's the first one there.
    pub fn download(&mut self, model: &'static Model, cx: &mut Context<Self>) {
        if self.downloads.contains_key(model.id) || self.is_installed(model) {
            return;
        }
        self.errors.remove(model.id);
        let cancel = Arc::new(AtomicBool::new(false));
        self.downloads.insert(model.id, Download { cancel: cancel.clone(), progress: yh_voice::download::on_disk(&self.models, model) as f32 / model.bytes() as f32 });
        cx.notify();
        let models = self.models.clone();
        log(format!("download {} started", model.id));
        cx.spawn(async move |this, cx| {
            let (dir, stop) = (models.clone(), cancel.clone());
            let task = cx.background_executor().spawn(async move { yh_voice::download::download(&dir, model, &stop) });
            // Progress: what's on disk, a few times a second.
            let watch = {
                let this = this.clone();
                cx.spawn(async move |cx| loop {
                    cx.background_executor().timer(Duration::from_millis(250)).await;
                    let got = yh_voice::download::on_disk(&models, model) as f32 / model.bytes() as f32;
                    let more = this.update(cx, |v, cx| match v.downloads.get_mut(model.id) {
                        Some(d) => {
                            d.progress = got.min(1.);
                            cx.notify();
                            true
                        }
                        None => false,
                    });
                    if !more.unwrap_or(false) {
                        break;
                    }
                })
            };
            let result = task.await;
            drop(watch);
            let _ = this.update(cx, |v, cx| {
                v.downloads.remove(model.id);
                match result {
                    Ok(()) => {
                        log(format!("download {} done", model.id));
                        v.installed.insert(model.id);
                        if v.default_model().is_none() {
                            v.set_default(model, cx);
                        }
                    }
                    Err(e) if e == "Cancelled" => log(format!("download {} cancelled", model.id)),
                    Err(e) => {
                        log(format!("download {} failed: {e}", model.id));
                        v.errors.insert(model.id, e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn cancel_download(&mut self, model: &Model) {
        if let Some(d) = self.downloads.get(model.id) {
            d.cancel.store(true, Ordering::SeqCst);
        }
    }

    /// The model's files, gone (and it let go of first, if loaded).
    pub fn remove(&mut self, model: &'static Model, cx: &mut Context<Self>) {
        self.worker.unload(model.id);
        if let Err(e) = yh_voice::download::remove(&self.models, model) {
            self.errors.insert(model.id, e);
        }
        if model.needs_download() {
            self.installed.remove(model.id);
        }
        log(format!("removed {}", model.id));
        cx.notify();
    }

    /// The thread a recording is for, while one starts, runs or is being written out.
    pub fn busy_thread(&self) -> Option<&str> {
        match &self.phase {
            Phase::Idle => None,
            Phase::Starting { thread } | Phase::Recording { thread, .. } | Phase::Transcribing { thread } => Some(thread),
        }
    }

    /// Seconds recorded so far.
    pub fn recorded_seconds(&self) -> Option<u64> {
        match &self.phase {
            Phase::Recording { since, .. } => Some(since.elapsed().as_secs()),
            _ => None,
        }
    }

    /// The mic pressed: starts recording for `thread`, or stops and writes out what was heard.
    /// `glossary`: words to expect (the project's name).
    pub fn toggle(&mut self, thread: &str, glossary: String, cx: &mut Context<Self>) {
        match &self.phase {
            Phase::Idle => self.start(thread, cx),
            Phase::Recording { .. } => self.finish(glossary, cx),
            Phase::Starting { .. } | Phase::Transcribing { .. } => {}
        }
    }

    /// ⎋ while recording: nothing written.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if matches!(self.phase, Phase::Recording { .. }) {
            self.phase = Phase::Idle;
            log("recording cancelled".into());
            cx.notify();
        }
    }

    fn start(&mut self, thread: &str, cx: &mut Context<Self>) {
        let Some(model) = self.default_model() else { return };
        self.errors.remove("");
        self.phase = Phase::Starting { thread: thread.to_string() };
        cx.notify();
        // Ready by the time you stop.
        if model.engine != Engine::Apple {
            self.worker.preload(model);
        }
        let file = std::env::var("YH_VOICE_FILE").ok();
        let thread = thread.to_string();
        cx.spawn(async move |this, cx| {
            let opened: Result<Source, String> = cx
                .background_executor()
                .spawn(async move {
                    if let Some(file) = file {
                        return yh_voice::audio::read_file(std::path::Path::new(&file)).map(Source::File);
                    }
                    let allowed = match yh_voice::mic::permission() {
                        yh_voice::mic::Permission::NotAsked => yh_voice::mic::ask(),
                        p => p,
                    };
                    if allowed != yh_voice::mic::Permission::Allowed {
                        return Err("Allow the microphone for YourHarness in System Settings → Privacy & Security → Microphone.".into());
                    }
                    yh_voice::mic::Recording::start().map(Source::Mic)
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                if !matches!(&v.phase, Phase::Starting { thread: t } if *t == thread) {
                    return;
                }
                match opened {
                    Ok(source) => {
                        log(format!("recording for {thread}"));
                        v.phase = Phase::Recording { thread: thread.clone(), since: Instant::now(), source };
                        v.tick(cx);
                    }
                    Err(e) => {
                        log(format!("couldn't record: {e}"));
                        v.errors.insert("", e);
                        v.phase = Phase::Idle;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Once a second while recording: the timer redrawn, and the longest recording stopped.
    fn tick(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let more = this.update(cx, |v, cx| match &v.phase {
                Phase::Recording { since, .. } => {
                    if since.elapsed() >= LONGEST {
                        v.finish(String::new(), cx);
                        return false;
                    }
                    cx.notify();
                    true
                }
                _ => false,
            });
            if !more.unwrap_or(false) {
                break;
            }
        })
        .detach();
    }

    fn finish(&mut self, glossary: String, cx: &mut Context<Self>) {
        let Phase::Recording { thread, source, .. } = std::mem::replace(&mut self.phase, Phase::Idle) else { return };
        let Some(model) = self.default_model() else { return };
        self.phase = Phase::Transcribing { thread: thread.clone() };
        cx.notify();
        let worker = self.worker.clone();
        cx.spawn(async move |this, cx| {
            // Stopping joins the mic's thread and resamples: off the main thread.
            let audio = cx
                .background_executor()
                .spawn(async move {
                    match source {
                        Source::Mic(r) => r.stop(),
                        Source::File(a) => a,
                    }
                })
                .await;
            let answer = worker.transcribe(model, audio, yh_voice::Hints { glossary, language: None }).recv_async().await.unwrap_or_else(|_| Err("The voice thread stopped.".into()));
            let _ = this.update(cx, |v, cx| {
                v.phase = Phase::Idle;
                match answer {
                    Ok(t) => {
                        log(format!("heard {:.1} s with {} (load {:.2} s, took {:.2} s): {}", t.audio_s, model.id, t.load_s, t.took_s, t.text));
                        cx.emit(VoiceEvent::Heard { thread, text: t.text });
                    }
                    Err(e) => {
                        log(format!("transcription failed: {e}"));
                        v.errors.insert("", e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Why the last recording failed, until the next starts.
    pub fn recording_error(&self) -> Option<&str> {
        self.errors.get("").map(String::as_str)
    }
}

/// `VOICE …` lines in development runs (`YH_DATA_DIR` set), for tests to read.
fn log(line: String) {
    if std::env::var("YH_DATA_DIR").is_ok() {
        eprintln!("VOICE {line}");
    }
}
