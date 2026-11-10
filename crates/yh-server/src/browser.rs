//! A thread's browser on the phone (protocol 7): where the app and its servers meet. Clients say
//! which thread's browser they watch; the app reads that (`watched`), captures those browsers'
//! shown tabs and hands the pictures and tabs in (`publish_frame`, `publish_state`); each server
//! sends them on to its clients, a frame only once the client said it drew the last
//! (`browser_ack`), so a slow phone gets fewer pictures instead of a queue of old ones.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use yh_proto::{BrowserState, ToApp};

/// The newest picture of a thread's browser, as the message that carries it (made once, sent
/// to every client watching).
pub struct Frame {
    pub seq: u64,
    pub text: Arc<String>,
}

#[derive(Default)]
pub struct Hub {
    /// Who watches what: a client's key (its server's base plus its id), the thread, and the
    /// most pixels across it can use.
    watchers: Mutex<HashMap<u64, (String, u32)>>,
    frames: Mutex<HashMap<String, (u64, Arc<Frame>)>>,
    states: Mutex<HashMap<String, BrowserState>>,
    /// When the phone last did something in a thread's browser: pictures come faster after.
    poked: Mutex<HashMap<String, Instant>>,
    /// Each server's listener: told the thread whose browser changed.
    listeners: Mutex<Vec<flume::Sender<String>>>,
    seq: AtomicU64,
    bases: AtomicU64,
}

impl Hub {
    pub fn new() -> Arc<Hub> {
        Arc::new(Hub::default())
    }

    /// The threads whose browsers are watched, each with the most pixels across any of its
    /// watchers can use.
    pub fn watched(&self) -> Vec<(String, u32)> {
        let mut out: Vec<(String, u32)> = vec![];
        for (thread, width) in self.watchers.lock().unwrap().values() {
            match out.iter_mut().find(|(t, _)| t == thread) {
                Some(w) => w.1 = w.1.max(*width),
                None => out.push((thread.clone(), *width)),
            }
        }
        out
    }

    /// A new picture of a thread's browser (a JPEG); false when it's the same as the last, which
    /// isn't sent again.
    pub fn publish_frame(&self, thread: &str, tab: u32, width: u32, height: u32, jpeg: &[u8]) -> bool {
        let hash = fnv(jpeg) ^ u64::from(tab) << 48;
        if self.frames.lock().unwrap().get(thread).is_some_and(|(h, _)| *h == hash) {
            return false;
        }
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let message = ToApp::BrowserFrame { thread: thread.to_string(), tab, seq, width, height, data: yh_core::attachments::base64(jpeg) };
        let frame = Arc::new(Frame { seq, text: Arc::new(serde_json::to_string(&message).expect("messages serialize")) });
        self.frames.lock().unwrap().insert(thread.to_string(), (hash, frame));
        self.tell(thread);
        true
    }

    /// A thread's browser's tabs and page as they are now; told on only when they changed.
    pub fn publish_state(&self, thread: &str, state: BrowserState) {
        let changed = self.states.lock().unwrap().insert(thread.to_string(), state.clone()).as_ref() != Some(&state);
        if changed {
            self.tell(thread);
        }
    }

    /// A browser nobody watches any more: what's held of it goes.
    pub fn forget(&self, thread: &str) {
        self.frames.lock().unwrap().remove(thread);
        self.states.lock().unwrap().remove(thread);
    }

    /// The phone did something in this thread's browser.
    pub fn poke(&self, thread: &str) {
        self.poked.lock().unwrap().insert(thread.to_string(), Instant::now());
    }

    pub fn poked_at(&self, thread: &str) -> Option<Instant> {
        self.poked.lock().unwrap().get(thread).copied()
    }

    pub fn frame(&self, thread: &str) -> Option<Arc<Frame>> {
        self.frames.lock().unwrap().get(thread).map(|(_, f)| f.clone())
    }

    pub fn state(&self, thread: &str) -> Option<BrowserState> {
        self.states.lock().unwrap().get(thread).cloned()
    }

    fn tell(&self, thread: &str) {
        self.listeners.lock().unwrap().retain(|l| l.send(thread.to_string()).is_ok());
    }

    pub(crate) fn subscribe(&self) -> flume::Receiver<String> {
        let (tx, rx) = flume::unbounded();
        self.listeners.lock().unwrap().push(tx);
        rx
    }

    /// Where a server's client keys start, so two servers' clients never share one.
    pub(crate) fn base(&self) -> u64 {
        (self.bases.fetch_add(1, Ordering::SeqCst) + 1) << 32
    }

    pub(crate) fn watch(&self, key: u64, thread: &str, width: u32) {
        self.watchers.lock().unwrap().insert(key, (thread.to_string(), width));
    }

    pub(crate) fn unwatch(&self, key: u64) {
        let mut watchers = self.watchers.lock().unwrap();
        let Some((thread, _)) = watchers.remove(&key) else { return };
        if !watchers.values().any(|(t, _)| *t == thread) {
            drop(watchers);
            self.forget(&thread);
        }
    }
}

/// FNV-1a over a sample of the bytes: enough to tell two pictures apart.
fn fnv(bytes: &[u8]) -> u64 {
    let step = (bytes.len() / 4096).max(1);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ bytes.len() as u64;
    for b in bytes.iter().step_by(step) {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// A client's watch: the thread, and how far its pictures got.
#[derive(Clone, Debug)]
pub(crate) struct Watch {
    pub thread: String,
    pub sent: u64,
    pub acked: u64,
    pub sent_at: Option<Instant>,
    pub state: Option<BrowserState>,
}

impl Watch {
    /// The frame to send now: newer than the last sent, the last one drawn (or lost: five
    /// seconds without its ack).
    pub fn due(&self, frame: &Frame) -> bool {
        frame.seq > self.sent && (self.acked >= self.sent || self.sent_at.is_none_or(|t| t.elapsed().as_secs() >= 5))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_wait_for_the_last_ack_and_repeats_are_dropped() {
        let hub = Hub::new();
        let told = hub.subscribe();
        let key = hub.base() + 1;
        hub.watch(key, "t", 800);
        hub.watch(hub.base() + 1, "t", 1200);
        assert_eq!(hub.watched(), vec![("t".to_string(), 1200)]);
        assert!(hub.publish_frame("t", 1, 10, 10, b"one"));
        assert!(!hub.publish_frame("t", 1, 10, 10, b"one"));
        assert_eq!(told.try_iter().count(), 1);
        let mut w = Watch { thread: "t".into(), sent: 0, acked: 0, sent_at: None, state: None };
        let first = hub.frame("t").unwrap();
        assert!(w.due(&first));
        (w.sent, w.sent_at) = (first.seq, Some(Instant::now()));
        assert!(hub.publish_frame("t", 1, 10, 10, b"two"));
        let second = hub.frame("t").unwrap();
        assert!(!w.due(&second), "not before the first is drawn");
        w.acked = first.seq;
        assert!(w.due(&second));
        assert!(second.text.contains(r#""type":"browser_frame""#));
        // Watched no more: what's held goes.
        hub.unwatch(key);
        assert!(hub.frame("t").is_some(), "the other client still watches");
        hub.unwatch((2u64 << 32) + 1);
        assert!(hub.frame("t").is_none());
        assert!(hub.watched().is_empty());
    }
}
