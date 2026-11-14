//! macOS's own speech recognition (SFSpeechRecognizer), on the device where it can be, in the
//! Mac's language. The audio goes through a WAV file in the temporary folder. macOS asks once
//! to allow Speech Recognition (the app's `NSSpeechRecognitionUsageDescription` says why).

use objc::runtime::{Object, BOOL, NO, YES};
use objc::{class, msg_send, sel, sel_impl};
use std::ffi::{CStr, CString};

#[link(name = "Speech", kind = "framework")]
unsafe extern "C" {}

type Id = *mut Object;

pub struct Apple;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    NotAsked,
    Denied,
    Allowed,
}

/// Whether Speech Recognition is allowed, without asking.
pub fn permission() -> Permission {
    let status: isize = unsafe { msg_send![class!(SFSpeechRecognizer), authorizationStatus] };
    match status {
        0 => Permission::NotAsked,
        3 => Permission::Allowed,
        _ => Permission::Denied,
    }
}

/// Asks macOS (once: it remembers) and waits for the answer. Never on the main thread.
fn ask() -> Permission {
    let (tx, rx) = flume::bounded::<isize>(1);
    let handler = block::ConcreteBlock::new(move |status: isize| {
        let _ = tx.send(status);
    })
    .copy();
    unsafe {
        let _: () = msg_send![class!(SFSpeechRecognizer), requestAuthorization: &*handler];
    }
    match rx.recv_timeout(std::time::Duration::from_secs(300)) {
        Ok(3) => Permission::Allowed,
        _ => Permission::Denied,
    }
}

impl crate::engine::Transcriber for Apple {
    fn transcribe(&mut self, audio: &[f32], _: &crate::engine::Hints) -> Result<String, String> {
        if permission() == Permission::NotAsked && ask() != Permission::Allowed || permission() == Permission::Denied {
            return Err("Allow Speech Recognition for YourHarness in System Settings → Privacy & Security.".into());
        }
        let wav = std::env::temp_dir().join(format!("yh-voice-apple-{}.wav", std::process::id()));
        std::fs::write(&wav, crate::audio::wav16(audio)).map_err(|e| e.to_string())?;
        let result = objc::rc::autoreleasepool(|| recognize(&wav));
        let _ = std::fs::remove_file(&wav);
        result
    }
}

fn recognize(wav: &std::path::Path) -> Result<String, String> {
    let path = CString::new(wav.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
    let (tx, rx) = flume::bounded::<Result<String, String>>(1);
    unsafe {
        let recognizer: Id = msg_send![class!(SFSpeechRecognizer), alloc];
        let recognizer: Id = msg_send![recognizer, init];
        if recognizer.is_null() {
            return Err("Apple's speech recognition doesn't know your Mac's language.".into());
        }
        let available: BOOL = msg_send![recognizer, isAvailable];
        if available == NO {
            let _: () = msg_send![recognizer, release];
            return Err("Apple's speech recognition isn't available right now.".into());
        }
        // Its answers on a queue of their own, never the main thread's.
        let queue: Id = msg_send![class!(NSOperationQueue), new];
        let _: () = msg_send![recognizer, setQueue: queue];
        let string: Id = msg_send![class!(NSString), stringWithUTF8String: path.as_ptr()];
        let url: Id = msg_send![class!(NSURL), fileURLWithPath: string];
        let request: Id = msg_send![class!(SFSpeechURLRecognitionRequest), alloc];
        let request: Id = msg_send![request, initWithURL: url];
        let on_device: BOOL = msg_send![recognizer, supportsOnDeviceRecognition];
        let _: () = msg_send![request, setRequiresOnDeviceRecognition: on_device];
        let _: () = msg_send![request, setAddsPunctuation: YES];
        let handler = block::ConcreteBlock::new(move |result: Id, error: Id| {
            if !error.is_null() {
                let _ = tx.send(Err(describe(error)));
            } else if !result.is_null() {
                let last: BOOL = msg_send![result, isFinal];
                if last == YES {
                    let best: Id = msg_send![result, bestTranscription];
                    let text: Id = msg_send![best, formattedString];
                    let _ = tx.send(Ok(string_of(text)));
                }
            }
        })
        .copy();
        let task: Id = msg_send![recognizer, recognitionTaskWithRequest: request resultHandler: &*handler];
        let answer = rx.recv_timeout(std::time::Duration::from_secs(120)).unwrap_or_else(|_| Err("Apple's speech recognition didn't answer.".into()));
        if !task.is_null() {
            let _: () = msg_send![task, cancel];
        }
        let _: () = msg_send![request, release];
        let _: () = msg_send![recognizer, release];
        let _: () = msg_send![queue, release];
        // "No speech detected" is an empty message, not a failure.
        match answer {
            Err(e) if e.contains("No speech") => Ok(String::new()),
            other => other,
        }
    }
}

unsafe fn string_of(s: Id) -> String {
    if s.is_null() {
        return String::new();
    }
    let utf8: *const std::os::raw::c_char = unsafe { msg_send![s, UTF8String] };
    if utf8.is_null() { String::new() } else { unsafe { CStr::from_ptr(utf8) }.to_string_lossy().into_owned() }
}

unsafe fn describe(error: Id) -> String {
    let text: Id = unsafe { msg_send![error, localizedDescription] };
    unsafe { string_of(text) }
}
