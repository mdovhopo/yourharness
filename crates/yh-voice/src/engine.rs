//! A loaded model that turns 16 kHz audio into text: whisper.cpp, sherpa-onnx (Parakeet), or
//! macOS's speech recognition.

use crate::catalog::{Engine, Model};
use std::path::Path;

/// What helps a model hear you.
#[derive(Clone, Debug, Default)]
pub struct Hints {
    /// Words it should expect (the project's name, its stack), given to Whisper as its prompt.
    pub glossary: String,
    /// A language code ("en", "uk") or None to detect it.
    pub language: Option<String>,
}

pub trait Transcriber: Send {
    fn transcribe(&mut self, audio: &[f32], hints: &Hints) -> Result<String, String>;
}

/// Loads `model` from its folder under `models` (seconds for the big ones: off the main thread).
pub fn load(model: &Model, models: &Path) -> Result<Box<dyn Transcriber>, String> {
    let dir = crate::download::folder(models, model);
    match model.engine {
        Engine::Whisper => Ok(Box::new(Whisper::load(&dir.join(model.files[0].name))?)),
        Engine::Parakeet => Ok(Box::new(Parakeet::load(&dir)?)),
        Engine::Apple => Ok(Box::new(crate::apple::Apple)),
    }
}

/// The performance cores less two, 2 to 6 (measured: on an M1 Max's eight, six beat eight while
/// other things run). `YH_VOICE_THREADS` overrides it.
fn threads() -> i32 {
    if let Some(n) = std::env::var("YH_VOICE_THREADS").ok().and_then(|n| n.parse().ok()) {
        return n;
    }
    let mut cores: i32 = 0;
    let mut len = std::mem::size_of::<i32>();
    let name = c"hw.perflevel0.physicalcpu";
    let ok = unsafe { libc::sysctlbyname(name.as_ptr(), (&mut cores as *mut i32).cast(), &mut len, std::ptr::null_mut(), 0) } == 0;
    if !ok || cores <= 0 {
        cores = std::thread::available_parallelism().map(|n| n.get() as i32).unwrap_or(4);
    }
    (cores - 2).clamp(2, 6)
}

struct Whisper {
    // The state borrows nothing from the context in whisper-rs 0.16 (it holds an Arc), but the
    // context is kept alive beside it all the same.
    _ctx: whisper_rs::WhisperContext,
    state: whisper_rs::WhisperState,
}

impl Whisper {
    fn load(file: &Path) -> Result<Whisper, String> {
        whisper_rs::install_logging_hooks();
        let path = file.to_str().ok_or("The model's path isn't UTF-8.")?;
        // Flash attention, as whisper.cpp itself defaults to (whisper-rs turns it off): a third faster.
        let mut params = whisper_rs::WhisperContextParameters::default();
        params.flash_attn(true);
        let ctx = whisper_rs::WhisperContext::new_with_params(path, params).map_err(|e| format!("The model couldn't load: {e}"))?;
        let state = ctx.create_state().map_err(|e| format!("The model couldn't start: {e}"))?;
        Ok(Whisper { _ctx: ctx, state })
    }
}

impl Transcriber for Whisper {
    fn transcribe(&mut self, audio: &[f32], hints: &Hints) -> Result<String, String> {
        let mut params = whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(threads());
        params.set_language(Some(hints.language.as_deref().unwrap_or("auto")));
        if !hints.glossary.trim().is_empty() {
            params.set_initial_prompt(hints.glossary.trim());
        }
        params.set_no_context(true);
        params.set_suppress_nst(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        self.state.full(params, audio).map_err(|e| format!("Transcription failed: {e}"))?;
        let text: Vec<String> = self.state.as_iter().filter_map(|s| s.to_str_lossy().ok().map(|t| t.trim().to_string())).filter(|t| !t.is_empty()).collect();
        Ok(text.join(" "))
    }
}

struct Parakeet {
    recognizer: sherpa_onnx::OfflineRecognizer,
}

// The recognizer is only ever used from the worker's thread.
unsafe impl Send for Parakeet {}

impl Parakeet {
    fn load(dir: &Path) -> Result<Parakeet, String> {
        let file = |name: &str| Some(dir.join(name).to_string_lossy().into_owned());
        let mut config = sherpa_onnx::OfflineRecognizerConfig::default();
        config.model_config.transducer = sherpa_onnx::OfflineTransducerModelConfig { encoder: file("encoder.int8.onnx"), decoder: file("decoder.int8.onnx"), joiner: file("joiner.int8.onnx") };
        config.model_config.tokens = file("tokens.txt");
        config.model_config.model_type = Some("nemo_transducer".into());
        config.model_config.num_threads = threads();
        let recognizer = sherpa_onnx::OfflineRecognizer::create(&config).ok_or("The model couldn't load.")?;
        Ok(Parakeet { recognizer })
    }
}

impl Transcriber for Parakeet {
    fn transcribe(&mut self, audio: &[f32], _: &Hints) -> Result<String, String> {
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(crate::audio::RATE as i32, audio);
        self.recognizer.decode(&stream);
        Ok(stream.get_result().map(|r| r.text.trim().to_string()).unwrap_or_default())
    }
}
