//! Voice input: what you say, as text for the composer. Speech models that run on the Mac
//! (whisper.cpp on the GPU, Parakeet through sherpa-onnx, or macOS's own recognizer), downloaded
//! when you ask for them; the microphone; one thread that holds the model in use. No GPUI.

// `objc` 0.2's `msg_send!` checks a `cargo-clippy` cfg that newer compilers don't know.
#[allow(unexpected_cfgs)]
pub mod apple;
pub mod audio;
pub mod catalog;
pub mod download;
pub mod engine;
#[allow(unexpected_cfgs)]
pub mod mic;
pub mod worker;

pub use catalog::{find, Engine, Model, MODELS};
pub use engine::Hints;
pub use worker::{Transcript, Worker};
