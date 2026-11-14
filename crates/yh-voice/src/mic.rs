//! The microphone, through CoreAudio (cpal), on a thread of its own: samples gathered as they
//! come, mono, until `stop` hands them over at 16 kHz. macOS asks once for the microphone, the
//! first time a recording starts (the app's `NSMicrophoneUsageDescription` says why).

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use objc::runtime::{Object, BOOL, YES};
use objc::{class, msg_send, sel, sel_impl};
use std::sync::{Arc, Mutex};

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {}

pub use crate::apple::Permission;

/// AVMediaTypeAudio.
fn audio_type() -> *mut Object {
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c"soun".as_ptr()] }
}

/// Whether the microphone is allowed, without asking. Denied, CoreAudio records silence, so this
/// is asked first.
pub fn permission() -> Permission {
    let status: isize = unsafe { msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: audio_type()] };
    match status {
        0 => Permission::NotAsked,
        3 => Permission::Allowed,
        _ => Permission::Denied,
    }
}

/// Asks macOS (once: it remembers) and waits for the answer. Never on the main thread.
pub fn ask() -> Permission {
    let (tx, rx) = flume::bounded::<bool>(1);
    let handler = block::ConcreteBlock::new(move |granted: BOOL| {
        let _ = tx.send(granted == YES);
    })
    .copy();
    unsafe {
        let _: () = msg_send![class!(AVCaptureDevice), requestAccessForMediaType: audio_type() completionHandler: &*handler];
    }
    match rx.recv_timeout(std::time::Duration::from_secs(300)) {
        Ok(true) => Permission::Allowed,
        _ => Permission::Denied,
    }
}

pub struct Recording {
    samples: Arc<Mutex<Vec<f32>>>,
    rate: u32,
    stop: flume::Sender<()>,
    done: Option<std::thread::JoinHandle<()>>,
}

impl Recording {
    /// Starts listening on the default input. Err says why it can't (no microphone, refused).
    pub fn start() -> Result<Recording, String> {
        let samples = Arc::new(Mutex::new(Vec::with_capacity(48_000 * 60)));
        let (stop, stopped) = flume::bounded::<()>(1);
        let (ready, started) = flume::bounded::<Result<u32, String>>(1);
        let gathered = samples.clone();
        // A cpal stream isn't Send: it lives and ends on this thread.
        let done = std::thread::Builder::new()
            .name("yh-voice-mic".into())
            .spawn(move || {
                let stream = match open(gathered) {
                    Ok((stream, rate)) => {
                        let _ = ready.send(Ok(rate));
                        stream
                    }
                    Err(e) => {
                        let _ = ready.send(Err(e));
                        return;
                    }
                };
                let _ = stopped.recv();
                drop(stream);
            })
            .map_err(|e| e.to_string())?;
        let rate = started.recv().map_err(|_| "The microphone couldn't start.".to_string())??;
        Ok(Recording { samples, rate, stop, done: Some(done) })
    }

    /// Seconds heard so far.
    pub fn seconds(&self) -> f32 {
        self.samples.lock().map(|s| s.len()).unwrap_or(0) as f32 / self.rate as f32
    }

    /// Stops listening; what was heard, at 16 kHz.
    pub fn stop(mut self) -> Vec<f32> {
        let _ = self.stop.send(());
        if let Some(t) = self.done.take() {
            let _ = t.join();
        }
        let heard = std::mem::take(&mut *self.samples.lock().unwrap_or_else(|e| e.into_inner()));
        crate::audio::resample(&heard, self.rate)
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

fn open(into: Arc<Mutex<Vec<f32>>>) -> Result<(cpal::Stream, u32), String> {
    let device = cpal::default_host().default_input_device().ok_or("No microphone is connected.")?;
    let supported = device.default_input_config().map_err(|e| format!("The microphone isn't available: {e}"))?;
    let (rate, channels) = (supported.sample_rate(), supported.channels() as usize);
    let config = supported.config();
    let on_error = |e: cpal::Error| eprintln!("VOICE microphone: {e}");
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(config, move |data: &[f32], _: &_| push(&into, data.iter().copied(), channels), on_error, None),
        cpal::SampleFormat::I16 => device.build_input_stream(config, move |data: &[i16], _: &_| push(&into, data.iter().map(|s| *s as f32 / 32768.), channels), on_error, None),
        other => return Err(format!("The microphone gives {other} samples, which YourHarness can't read.")),
    }
    .map_err(|e| format!("The microphone couldn't start: {e}"))?;
    stream.play().map_err(|e| format!("The microphone couldn't start: {e}"))?;
    Ok((stream, rate))
}

/// Interleaved frames, averaged to mono.
fn push(into: &Mutex<Vec<f32>>, data: impl Iterator<Item = f32>, channels: usize) {
    let Ok(mut out) = into.lock() else { return };
    if channels <= 1 {
        out.extend(data);
        return;
    }
    let mut sum = 0.;
    for (i, s) in data.enumerate() {
        sum += s;
        if i % channels == channels - 1 {
            out.push(sum / channels as f32);
            sum = 0.;
        }
    }
}
