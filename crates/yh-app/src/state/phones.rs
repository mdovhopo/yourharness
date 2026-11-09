//! Phones on this network (Settings → Phone): the server listening on the Mac's network
//! addresses while it's on, a pairing code for the next phone, and the phones paired. Each
//! phone pairs once with the code (a QR code on the Mac) and then has a token of its own; the
//! list keeps only their hashes (`phoneDevices`), and forgetting one cuts it off.
//!
//! The traffic isn't encrypted on the network yet (plain WebSocket): for networks you trust,
//! until Tailscale carries it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::Value;
use yh_server::pairing::{Device, Pairing};

pub const DEFAULT_PORT: u16 = 7878;

pub struct Phones {
    pub pairing: Arc<Pairing>,
    /// Serving now: set to stop it.
    stop: Option<Arc<AtomicBool>>,
    pub port: u16,
    /// Why it couldn't start (the port taken…).
    pub error: Option<String>,
    /// The Mac's addresses on its networks, the likeliest first.
    pub addresses: Vec<String>,
}

impl Phones {
    /// The paired phones as saved; `changed` hears each change to the list (to save it).
    pub fn new(saved: Option<Value>, port: Option<u16>, changed: flume::Sender<Vec<Device>>) -> Phones {
        let devices = saved.and_then(|v| v.as_array().map(|a| a.iter().filter_map(Device::from_json).collect())).unwrap_or_default();
        let pairing = Arc::new(Pairing::new(devices, Box::new(move |d| {
            let _ = changed.send(d.to_vec());
        })));
        Phones { pairing, stop: None, port: port.unwrap_or(DEFAULT_PORT), error: None, addresses: vec![] }
    }

    pub fn running(&self) -> bool {
        self.stop.is_some()
    }

    /// Listens on every network address (`YH_PHONES_BIND` for tests: 127.0.0.1, so no firewall
    /// question), its actions to `actions`. A pairing code is ready once it's on.
    pub fn start(&mut self, data_dir: std::path::PathBuf, name: String, links: yh_server::Links) {
        if self.running() {
            return;
        }
        let bind = std::env::var("YH_PHONES_BIND").unwrap_or_else(|_| "0.0.0.0".into());
        let listener = match std::net::TcpListener::bind((bind.as_str(), self.port)) {
            Ok(l) => l,
            Err(e) => {
                self.error = Some(if e.kind() == std::io::ErrorKind::AddrInUse { format!("Port {} is taken by another app.", self.port) } else { e.to_string() });
                eprintln!("PHONES can't listen on {bind}:{}: {e}", self.port);
                return;
            }
        };
        self.error = None;
        self.addresses = addresses();
        let stop = Arc::new(AtomicBool::new(false));
        self.stop = Some(stop.clone());
        let options = yh_server::Options { token: None, name, pairing: Some(self.pairing.clone()), stop: Some(stop) };
        eprintln!("PHONES listening on {bind}:{} ({})", self.port, self.addresses.join(", "));
        let _ = std::thread::Builder::new().name("phones".into()).spawn(move || {
            if let Err(e) = yh_server::serve(listener, data_dir, options, Some(links)) {
                eprintln!("PHONES stopped: {e}");
            }
        });
        self.pairing.new_code();
    }

    /// Stops listening; phones connected are let go.
    pub fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop.store(true, Ordering::Relaxed);
            eprintln!("PHONES stopped");
        }
    }

    /// What the QR code holds: the app's link with where to connect and a code that works once.
    /// None while off. Made anew when the code is used or out of date.
    pub fn pairing_link(&self, name: &str) -> Option<String> {
        if !self.running() {
            return None;
        }
        let code = self.pairing.code().unwrap_or_else(|| self.pairing.new_code());
        let host = self.addresses.first()?;
        Some(format!("yourharness://pair?host={host}&port={}&code={code}&name={}", self.port, encode(name)))
    }
}

/// The Mac's IPv4 addresses on its networks: Wi‑Fi and Ethernet first, then others (a VPN's),
/// never loopback or link-local.
pub fn addresses() -> Vec<String> {
    let mut found: Vec<(bool, String)> = vec![];
    unsafe {
        let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut list) != 0 {
            return vec![];
        }
        let mut at = list;
        while !at.is_null() {
            let ifa = &*at;
            at = ifa.ifa_next;
            if ifa.ifa_addr.is_null() || (*ifa.ifa_addr).sa_family as i32 != libc::AF_INET || ifa.ifa_flags & libc::IFF_UP as u32 == 0 {
                continue;
            }
            let addr = &*(ifa.ifa_addr as *const libc::sockaddr_in);
            let ip = std::net::Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr));
            if ip.is_loopback() || ip.is_link_local() {
                continue;
            }
            let name = std::ffi::CStr::from_ptr(ifa.ifa_name).to_string_lossy();
            found.push((name.starts_with("en"), ip.to_string()));
        }
        libc::freeifaddrs(list);
    }
    found.sort_by_key(|(preferred, _)| !preferred);
    let mut out: Vec<String> = vec![];
    for (_, ip) in found {
        if !out.contains(&ip) {
            out.push(ip);
        }
    }
    out
}

/// A name in a link: letters and digits as they are, the rest `%XX`.
fn encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_link_holds_where_to_connect_and_a_code() {
        let (tx, _rx) = flume::unbounded();
        let mut p = super::Phones::new(None, Some(7979), tx);
        assert!(p.pairing_link("Mac").is_none(), "off: no link");
        p.stop = Some(Default::default());
        p.addresses = vec!["192.168.1.20".into()];
        let link = p.pairing_link("Max's Mac").unwrap();
        assert!(link.starts_with("yourharness://pair?host=192.168.1.20&port=7979&code="), "{link}");
        assert!(link.ends_with("&name=Max%27s%20Mac"), "{link}");
        assert_eq!(p.pairing_link("Max's Mac").unwrap(), link, "the same code until it's used");
    }
}
