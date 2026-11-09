//! Phones paired with this Mac. A phone pairs once with a short-lived code (shown on the Mac as a
//! QR code, used once) and gets a token of its own; from then on it connects with that token.
//! Tokens are kept only as hashes, so the saved list can't be used to connect; forgetting a
//! phone ends its connection and its token with it.

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// How long a pairing code works.
pub const CODE_LIFETIME: Duration = Duration::from_secs(600);

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// Hex SHA-256 of its token.
    pub token_hash: String,
    /// Seconds since 1970.
    pub paired_at: f64,
    pub last_seen: Option<f64>,
    /// The phone's own lasting ID: pairing it again replaces this entry rather than adding one.
    pub phone_id: Option<String>,
}

impl Device {
    pub fn to_json(&self) -> Value {
        json!({ "id": self.id, "name": self.name, "tokenHash": self.token_hash, "pairedAt": self.paired_at, "lastSeen": self.last_seen, "phoneID": self.phone_id })
    }

    pub fn from_json(v: &Value) -> Option<Device> {
        Some(Device {
            id: v["id"].as_str()?.to_string(),
            name: v["name"].as_str().unwrap_or("Phone").to_string(),
            token_hash: v["tokenHash"].as_str()?.to_string(),
            paired_at: v["pairedAt"].as_f64().unwrap_or(0.),
            last_seen: v["lastSeen"].as_f64(),
            phone_id: v["phoneID"].as_str().map(str::to_string),
        })
    }
}

/// Called with the whole list whenever it changes (paired, forgotten, seen again), to save it.
pub type OnChange = Box<dyn Fn(&[Device]) + Send + Sync>;

pub struct Pairing {
    state: Mutex<State>,
    on_change: OnChange,
}

struct State {
    devices: Vec<Device>,
    code: Option<(String, Instant)>,
}

impl Pairing {
    pub fn new(devices: Vec<Device>, on_change: OnChange) -> Pairing {
        Pairing { state: Mutex::new(State { devices, code: None }), on_change }
    }

    /// A fresh code, replacing any other; it works once, for `CODE_LIFETIME`.
    pub fn new_code(&self) -> String {
        let code = random_hex(16);
        self.state.lock().unwrap().code = Some((code.clone(), Instant::now()));
        code
    }

    /// The code that works now, if any.
    pub fn code(&self) -> Option<String> {
        let state = self.state.lock().unwrap();
        state.code.as_ref().filter(|(_, at)| at.elapsed() < CODE_LIFETIME).map(|(c, _)| c.clone())
    }

    /// A phone pairing with `code`: its id and new token, or None for a wrong or used code. A
    /// phone paired before (same `phone_id`) is replaced: its old token stops working.
    pub fn pair(&self, code: &str, name: &str, phone_id: Option<&str>) -> Option<(String, String)> {
        let (devices, id, token) = {
            let mut state = self.state.lock().unwrap();
            let valid = state.code.as_ref().is_some_and(|(c, at)| at.elapsed() < CODE_LIFETIME && same(c, code));
            if !valid {
                return None;
            }
            state.code = None;
            let token = random_hex(32);
            let id = random_hex(8);
            let name = name.trim().chars().take(60).collect::<String>();
            let now = now();
            let phone_id = phone_id.map(|p| p.chars().take(64).collect::<String>()).filter(|p| !p.is_empty());
            if let Some(p) = &phone_id {
                state.devices.retain(|d| d.phone_id.as_ref() != Some(p));
            }
            state.devices.push(Device { id: id.clone(), name: if name.is_empty() { "Phone".into() } else { name }, token_hash: hash(&token), paired_at: now, last_seen: Some(now), phone_id });
            (state.devices.clone(), id, token)
        };
        (self.on_change)(&devices);
        Some((id, token))
    }

    /// The device a token belongs to; marks it seen.
    pub fn check(&self, token: &str) -> Option<String> {
        let h = hash(token);
        let (id, devices) = {
            let mut state = self.state.lock().unwrap();
            let d = state.devices.iter_mut().find(|d| same(&d.token_hash, &h))?;
            d.last_seen = Some(now());
            (d.id.clone(), state.devices.clone())
        };
        (self.on_change)(&devices);
        Some(id)
    }

    /// Still paired (a connection asks as it goes, so forgetting ends it).
    pub fn has(&self, id: &str) -> bool {
        self.state.lock().unwrap().devices.iter().any(|d| d.id == id)
    }

    pub fn forget(&self, id: &str) {
        let devices = {
            let mut state = self.state.lock().unwrap();
            state.devices.retain(|d| d.id != id);
            state.devices.clone()
        };
        (self.on_change)(&devices);
    }

    pub fn devices(&self) -> Vec<Device> {
        self.state.lock().unwrap().devices.clone()
    }
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.)
}

fn hash(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).expect("the system's randomness");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compared in constant time, so timing doesn't tell how much of a guess was right.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn a_code_pairs_once_and_its_token_works_until_forgotten() {
        let saved: Arc<Mutex<Vec<Device>>> = Arc::default();
        let s = saved.clone();
        let p = Pairing::new(vec![], Box::new(move |d| *s.lock().unwrap() = d.to_vec()));
        assert!(p.pair("nope", "x", None).is_none(), "no code yet");
        let code = p.new_code();
        assert!(p.pair("0000", "x", None).is_none(), "a wrong code");
        let (first, first_token) = p.pair(&code, "Max's iPhone", Some("phone-1")).unwrap();
        assert!(p.pair(&code, "again", None).is_none(), "used once");
        // The same phone pairing again replaces itself, its old token gone.
        let (id, token) = p.pair(&p.new_code(), "Max's iPhone", Some("phone-1")).unwrap();
        assert!(p.check(&first_token).is_none() && !p.has(&first) && p.devices().len() == 1);
        assert_eq!(p.check(&token).as_deref(), Some(id.as_str()));
        assert!(p.check("not a token").is_none());
        let kept = saved.lock().unwrap().clone();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "Max's iPhone");
        assert!(!kept[0].to_json().to_string().contains(&token), "only the hash is saved");
        assert_eq!(Device::from_json(&kept[0].to_json()).as_ref(), Some(&kept[0]));
        p.forget(&id);
        assert!(p.check(&token).is_none() && !p.has(&id));
        assert!(saved.lock().unwrap().is_empty());
    }
}
