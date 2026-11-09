//! Phones on this network (Settings → Phone): the server listening on the Mac's network
//! addresses while it's on, a pairing code for the next phone, and the phones paired. Each
//! phone pairs once with the code (a QR code on the Mac) and then has a token of its own; the
//! list keeps only their hashes (`phoneDevices`), and forgetting one cuts it off.
//!
//! With Tailscale on, phones also reach the Mac over the tailnet from anywhere: WireGuard
//! encrypts the way, and only devices logged in as the Mac's own tailnet user get in. On the
//! local network the traffic isn't encrypted (plain WebSocket): for networks you trust.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::Value;
use yh_server::pairing::{Device, Pairing};
use yh_server::tailscale;

/// Unassigned at IANA and no known app's default (7878, the first choice, is registered to
/// Opswise and is Radarr's default); below the ports macOS hands out for outgoing connections.
pub const DEFAULT_PORT: u16 = 47878;
/// What 0.1.3's Settings → Phone listened on: kept for a Mac whose phones paired with it.
pub const FIRST_PORT: u16 = 7878;
/// Ports tried after the chosen one when it's taken.
const TRIES: u16 = 10;

pub struct Phones {
    pub pairing: Arc<Pairing>,
    /// Serving now: set to stop it.
    stop: Option<Arc<AtomicBool>>,
    pub port: u16,
    /// Why it couldn't start (the port taken…).
    pub error: Option<String>,
    /// The Mac's addresses on its networks, the likeliest first.
    pub addresses: Vec<String>,
    /// This Mac's lasting ID, in the pairing link.
    pub mac_id: String,
    /// Tailscale as last read (off the main thread); None when it isn't installed.
    pub tailscale: Option<tailscale::Status>,
    /// Phones over Tailscale only, not the local network (Settings → Phone, saved).
    pub tailscale_only: bool,
    /// The tailnet address it listens on in `tailscale_only`, to start again when it changes.
    pub bound_tailnet: Option<String>,
}

impl Phones {
    /// The paired phones as saved; `changed` hears each change to the list (to save it).
    pub fn new(saved: Option<Value>, port: Option<u16>, changed: flume::Sender<Vec<Device>>) -> Phones {
        let devices = saved.and_then(|v| v.as_array().map(|a| a.iter().filter_map(Device::from_json).collect())).unwrap_or_default();
        let pairing = Arc::new(Pairing::new(devices, Box::new(move |d| {
            let _ = changed.send(d.to_vec());
        })));
        Phones { pairing, stop: None, port: port.unwrap_or(DEFAULT_PORT), error: None, addresses: vec![], mac_id: String::new(), tailscale: None, tailscale_only: false, bound_tailnet: None }
    }

    pub fn running(&self) -> bool {
        self.stop.is_some()
    }

    /// The Mac's tailnet address, when Tailscale is connected.
    pub fn tailnet_ip(&self) -> Option<String> {
        self.tailscale.as_ref().filter(|t| t.running).and_then(|t| t.ipv4()).map(str::to_string)
    }

    /// Where phones dial the Mac on the tailnet: its MagicDNS name, which iOS lets the phone
    /// reach over plain WebSocket (an exception for `ts.net`; never for the 100.x address).
    pub fn tailnet_host(&self) -> Option<String> {
        self.tailscale.as_ref().and_then(tailnet_host)
    }

    /// Where phones may dial, the likeliest first: the local network's addresses, then the
    /// tailnet's (only that in `tailscale_only`).
    pub fn hosts(&self) -> Vec<String> {
        hosts(&self.addresses, self.tailnet_host(), self.tailscale_only)
    }

    /// Listens on every network address, or only the tailnet's in `tailscale_only`
    /// (`YH_PHONES_BIND` for tests: 127.0.0.1, so no firewall question), its actions to
    /// `actions`. A pairing code is ready once it's on. When its port is taken, the next few are
    /// tried; the port it got is `self.port`, to keep. Read `tailscale` first.
    pub fn start(&mut self, data_dir: std::path::PathBuf, name: String, mac_id: String, links: yh_server::Links) {
        if self.running() {
            return;
        }
        self.bound_tailnet = None;
        let bind = match std::env::var("YH_PHONES_BIND") {
            Ok(b) => b,
            Err(_) if self.tailscale_only => match self.tailnet_ip() {
                Some(ip) => {
                    self.bound_tailnet = Some(ip.clone());
                    ip
                }
                None => {
                    self.error = Some(if self.tailscale.is_none() { "Tailscale isn't installed".into() } else { "Tailscale isn't connected".into() });
                    eprintln!("PHONES waiting for Tailscale");
                    return;
                }
            },
            Err(_) => "0.0.0.0".into(),
        };
        let mut last = None;
        let mut got = None;
        for port in self.port..self.port.saturating_add(TRIES) {
            match std::net::TcpListener::bind((bind.as_str(), port)) {
                Ok(l) => {
                    got = Some((l, port));
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                    eprintln!("PHONES port {port} is taken");
                    last = Some(e);
                }
                Err(e) => {
                    last = Some(e);
                    break;
                }
            }
        }
        let Some((listener, port)) = got else {
            let e = last.map(|e| e.to_string()).unwrap_or_default();
            self.error = Some(format!("Couldn't listen on ports {}–{}: {e}", self.port, self.port.saturating_add(TRIES - 1)));
            eprintln!("PHONES can't listen on {bind}:{}: {e}", self.port);
            return;
        };
        self.port = port;
        self.error = None;
        self.addresses = addresses();
        let stop = Arc::new(AtomicBool::new(false));
        self.stop = Some(stop.clone());
        self.mac_id = mac_id.clone();
        // Read again for each phone that connects: Tailscale may have come up since.
        let only = self.tailscale_only;
        let hosts: yh_server::Hosts = Arc::new(move || hosts(&addresses(), tailscale::status().as_ref().and_then(tailnet_host), only));
        let options = yh_server::Options { token: None, name, pairing: Some(self.pairing.clone()), stop: Some(stop), mac_id: Some(mac_id), tailscale: true, tailscale_only: only, hosts: Some(hosts) };
        eprintln!("PHONES listening on {bind}:{} ({})", self.port, self.hosts().join(", "));
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
        let hosts = self.hosts();
        let host = hosts.first()?;
        // `host` for phones from before `hosts`.
        Some(format!("yourharness://pair?host={host}&hosts={}&port={}&code={code}&name={}&id={}", hosts.join(","), self.port, encode(name), encode(&self.mac_id)))
    }
}

/// The MagicDNS name, else the IPv4 address (MagicDNS off), while Tailscale is connected.
fn tailnet_host(t: &tailscale::Status) -> Option<String> {
    t.running.then(|| t.dns_name.clone().or_else(|| t.ipv4().map(str::to_string))).flatten()
}

fn hosts(local: &[String], tailnet: Option<String>, tailscale_only: bool) -> Vec<String> {
    let mut out: Vec<String> = if tailscale_only { vec![] } else { local.to_vec() };
    if let Some(ip) = tailnet {
        if !out.contains(&ip) {
            out.push(ip);
        }
    }
    out
}

/// The Mac's IPv4 addresses on its networks: Wi‑Fi and Ethernet first, then others (a VPN's),
/// never loopback, link-local or the tailnet's (`Phones::hosts` adds that last).
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
            if ip.is_loopback() || ip.is_link_local() || tailscale::is_tailnet(std::net::IpAddr::V4(ip)) {
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
        assert_eq!(super::Phones::new(None, None, flume::unbounded().0).port, super::DEFAULT_PORT);
        assert!(p.pairing_link("Mac").is_none(), "off: no link");
        p.stop = Some(Default::default());
        p.addresses = vec!["192.168.1.20".into()];
        let link = p.pairing_link("Max's Mac").unwrap();
        assert!(link.starts_with("yourharness://pair?host=192.168.1.20&hosts=192.168.1.20&port=7979&code="), "{link}");
        assert!(link.contains("&name=Max%27s%20Mac&id="), "{link}");
        assert_eq!(p.pairing_link("Max's Mac").unwrap(), link, "the same code until it's used");
        // With Tailscale connected, its address comes after the local network's.
        p.tailscale = Some(yh_server::tailscale::Status { running: true, ips: vec!["100.78.55.9".into()], ..Default::default() });
        let link = p.pairing_link("Mac").unwrap();
        assert!(link.contains("?host=192.168.1.20&hosts=192.168.1.20,100.78.55.9&port=7979"), "{link}");
        // Its MagicDNS name rather than the address, when it has one: iOS lets that through.
        p.tailscale.as_mut().unwrap().dns_name = Some("mac.tail1.ts.net".into());
        assert!(p.pairing_link("Mac").unwrap().contains("&hosts=192.168.1.20,mac.tail1.ts.net&"));
        p.tailscale_only = true;
        assert!(p.pairing_link("Mac").unwrap().contains("?host=mac.tail1.ts.net&hosts=mac.tail1.ts.net&"));
        p.tailscale.as_mut().unwrap().running = false;
        assert!(p.pairing_link("Mac").is_none(), "Tailscale only, and it's down: nowhere to dial");
    }
}
