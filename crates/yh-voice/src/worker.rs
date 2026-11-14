//! One thread that holds the model in use: loaded on first use (or when a recording starts, so
//! it's ready by the time you stop), let go after a few idle minutes so its gigabyte isn't held
//! while you type.

use crate::catalog::Model;
use crate::engine::{Hints, Transcriber};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const IDLE: Duration = Duration::from_secs(5 * 60);

/// `YH_VOICE_LOG=1`: each load, unload and transcription printed as `VOICE worker …`.
fn trace(line: String) {
    if std::env::var("YH_VOICE_LOG").as_deref() == Ok("1") {
        eprintln!("VOICE worker {line}");
    }
}

enum Job {
    Preload(&'static Model),
    Transcribe { model: &'static Model, audio: Vec<f32>, hints: Hints, reply: flume::Sender<Result<Transcript, String>> },
    Unload(&'static str),
    /// Told when done; or `None` from the exit handler, which watches `SHUT_DOWN`.
    Shutdown(Option<flume::Sender<()>>),
}

#[derive(Clone, Debug)]
pub struct Transcript {
    pub text: String,
    /// Seconds of audio, and how long loading and transcribing took.
    pub audio_s: f32,
    pub load_s: f32,
    pub took_s: f32,
}

#[derive(Clone)]
pub struct Worker {
    jobs: flume::Sender<Job>,
}

impl Worker {
    pub fn start(models: PathBuf) -> Worker {
        let (jobs, rx) = flume::unbounded();
        std::thread::Builder::new().name("yh-voice".into()).spawn(move || run(models, rx)).expect("the voice thread starts");
        let worker = Worker { jobs };
        let _ = EXIT.set(worker.clone());
        worker
    }

    /// Loads `model` now if it isn't, so the transcription after it doesn't wait.
    pub fn preload(&self, model: &'static Model) {
        let _ = self.jobs.send(Job::Preload(model));
    }

    /// Lets go of `id` if it's the one loaded (before its files are deleted).
    pub fn unload(&self, id: &'static str) {
        let _ = self.jobs.send(Job::Unload(id));
    }

    /// Lets go of the model and ends the thread, waiting up to two seconds. Before the process
    /// exits: whisper.cpp's GPU teardown at exit aborts if a model is still loaded (an `atexit`
    /// handler calls it too, registered on the first load).
    pub fn shutdown(&self) {
        let (done, wait) = flume::bounded(1);
        if self.jobs.send(Job::Shutdown(Some(done))).is_ok() {
            let _ = wait.recv_timeout(Duration::from_secs(2));
        }
    }

    /// The text of `audio` (16 kHz mono); the answer comes on the receiver.
    pub fn transcribe(&self, model: &'static Model, audio: Vec<f32>, hints: Hints) -> flume::Receiver<Result<Transcript, String>> {
        let (reply, answer) = flume::bounded(1);
        let _ = self.jobs.send(Job::Transcribe { model, audio, hints, reply });
        answer
    }
}

unsafe extern "C" {
    fn pthread_set_qos_class_self_np(qos: u32, relative_priority: i32) -> i32;
}

/// QOS_CLASS_USER_INITIATED: you're waiting for the text. It reaches the model's own threads,
/// which inherit it; at the default class macOS puts them on the efficiency cores, and Parakeet
/// takes twice as long.
const USER_INITIATED: u32 = 0x19;

static EXIT: std::sync::OnceLock<Worker> = std::sync::OnceLock::new();
static AT_EXIT: std::sync::Once = std::sync::Once::new();

/// Set once the thread has let go of its model and ended.
static SHUT_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// At exit this thread's locals may be gone already, and a blocking channel wait needs them: so
/// a send (which doesn't block) and a watch on `SHUT_DOWN`, up to two seconds.
extern "C" fn shutdown_at_exit() {
    let Some(w) = EXIT.get() else { return };
    if w.jobs.send(Job::Shutdown(None)).is_err() {
        return;
    }
    for _ in 0..200 {
        if SHUT_DOWN.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn run(models: PathBuf, jobs: flume::Receiver<Job>) {
    unsafe { pthread_set_qos_class_self_np(USER_INITIATED, 0) };
    let mut loaded: Option<(&'static str, Box<dyn Transcriber>)> = None;
    let mut last_used = Instant::now();
    loop {
        let job = match jobs.recv_timeout(IDLE) {
            Ok(job) => job,
            Err(flume::RecvTimeoutError::Timeout) => {
                if loaded.is_some() && last_used.elapsed() >= IDLE {
                    trace("idle: model let go".into());
                    loaded = None;
                }
                continue;
            }
            Err(flume::RecvTimeoutError::Disconnected) => {
                SHUT_DOWN.store(true, std::sync::atomic::Ordering::SeqCst);
                return;
            }
        };
        last_used = Instant::now();
        let mut ensure = |model: &'static Model| -> Result<f32, String> {
            if loaded.as_ref().map(|(id, _)| *id) == Some(model.id) {
                return Ok(0.);
            }
            loaded = None;
            let t = Instant::now();
            trace(format!("loading {}", model.id));
            let engine = crate::engine::load(model, &models)?;
            loaded = Some((model.id, engine));
            trace(format!("loaded {} in {:.2} s", model.id, t.elapsed().as_secs_f32()));
            // Every way out (a quit that skips the app's handler, `exit` from anywhere) lets go of
            // the model first. whisper.cpp makes its GPU device on the first load, registering its
            // destructor then; an `atexit` handler registered after it runs before it.
            AT_EXIT.call_once(|| unsafe {
                libc::atexit(shutdown_at_exit);
            });
            Ok(t.elapsed().as_secs_f32())
        };
        match job {
            Job::Preload(model) => {
                if let Err(e) = ensure(model) {
                    eprintln!("VOICE preload {}: {e}", model.id);
                }
            }
            Job::Shutdown(done) => {
                drop(loaded.take());
                SHUT_DOWN.store(true, std::sync::atomic::Ordering::SeqCst);
                if let Some(done) = done {
                    let _ = done.send(());
                }
                return;
            }
            Job::Unload(id) => {
                if loaded.as_ref().map(|(l, _)| *l) == Some(id) {
                    loaded = None;
                }
            }
            Job::Transcribe { model, audio, hints, reply } => {
                let audio_s = audio.len() as f32 / crate::audio::RATE as f32;
                if crate::audio::is_silent(&audio) {
                    let _ = reply.send(Ok(Transcript { text: String::new(), audio_s, load_s: 0., took_s: 0. }));
                    continue;
                }
                trace(format!("transcribing {audio_s:.1} s"));
                let answer = ensure(model).and_then(|load_s| {
                    let t = Instant::now();
                    let (_, engine) = loaded.as_mut().expect("loaded above");
                    let text = engine.transcribe(&audio, &hints)?;
                    Ok(Transcript { text, audio_s, load_s, took_s: t.elapsed().as_secs_f32() })
                });
                let _ = reply.send(answer);
                last_used = Instant::now();
            }
        }
    }
}
