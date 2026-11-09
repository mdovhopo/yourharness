//! Tailscale, through its command-line tool: this Mac's addresses and name on the tailnet, and who
//! a connection from the tailnet belongs to. Phones reach the Mac over the tailnet from anywhere;
//! WireGuard encrypts the way, and only devices logged in as the Mac's own tailnet user get in.
//!
//! Test hooks: `YH_TAILSCALE=0` (as if it weren't installed), `YH_TAILSCALE_STATUS=<file>` (what
//! `tailscale status --json` would say), `YH_TAILSCALE_WHOIS=<file>` (`{"<ip>": <user id>}`).

use std::net::IpAddr;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

/// Where the tool may be: the shim the app installs, Homebrew's, the app itself.
const CLIS: &[&str] = &["/usr/local/bin/tailscale", "/opt/homebrew/bin/tailscale", "/Applications/Tailscale.app/Contents/MacOS/Tailscale"];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// Logged in and connected.
    pub running: bool,
    /// What Tailscale says when it isn't (`Stopped`, `NeedsLogin`…).
    pub state: String,
    /// This Mac's tailnet addresses, IPv4 first.
    pub ips: Vec<String>,
    /// Its MagicDNS name, without the last dot (`mac.tail1234.ts.net`).
    pub dns_name: Option<String>,
    pub user_id: Option<i64>,
    /// Who it's logged in as (an email).
    pub login: Option<String>,
}

impl Status {
    /// The address phones dial: the first IPv4 one.
    pub fn ipv4(&self) -> Option<&str> {
        self.ips.iter().map(String::as_str).find(|ip| !ip.contains(':'))
    }

    pub fn parse(v: &Value) -> Status {
        let state = v["BackendState"].as_str().unwrap_or_default().to_string();
        let me = &v["Self"];
        let mut ips: Vec<String> = me["TailscaleIPs"].as_array().or(v["TailscaleIPs"].as_array()).map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default();
        ips.sort_by_key(|ip| ip.contains(':'));
        let user_id = me["UserID"].as_i64();
        let login = user_id.and_then(|id| v["User"][id.to_string()]["LoginName"].as_str().map(str::to_string));
        let dns_name = me["DNSName"].as_str().map(|s| s.trim_end_matches('.').to_string()).filter(|s| !s.is_empty());
        Status { running: state == "Running" && !ips.is_empty(), state, ips, dns_name, user_id, login }
    }
}

/// Off in tests that say so.
fn disabled() -> bool {
    std::env::var("YH_TAILSCALE").as_deref() == Ok("0")
}

/// The tool, when Tailscale is installed.
pub fn cli() -> Option<PathBuf> {
    if disabled() {
        return None;
    }
    CLIS.iter().map(PathBuf::from).find(|p| p.exists())
}

/// Tailscale's status now; None when it isn't installed or doesn't answer. Runs the tool: call it
/// off the main thread.
pub fn status() -> Option<Status> {
    if disabled() {
        return None;
    }
    if let Ok(file) = std::env::var("YH_TAILSCALE_STATUS") {
        let text = std::fs::read_to_string(file).ok()?;
        return Some(Status::parse(&serde_json::from_str(&text).ok()?));
    }
    let out = Command::new(cli()?).args(["status", "--json"]).output().ok()?;
    // Stopped or logged out, it still prints its state (and exits non-zero).
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    Some(Status::parse(&v))
}

/// The tailnet user a peer at `ip` is logged in as; None when it isn't a tailnet device or
/// Tailscale doesn't know it. Runs the tool: off the main thread.
pub fn whois(ip: IpAddr) -> Option<i64> {
    if disabled() {
        return None;
    }
    if let Ok(file) = std::env::var("YH_TAILSCALE_WHOIS") {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
        return v[ip.to_string()].as_i64();
    }
    let out = Command::new(cli()?).args(["whois", "--json", &ip.to_string()]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    v["UserProfile"]["ID"].as_i64()
}

/// An address Tailscale hands out: 100.64.0.0/10, or its IPv6 range fd7a:115c:a1e0::/48.
pub fn is_tailnet(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 100 && (64..128).contains(&o[1])
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_tailnet(IpAddr::V4(v4));
            }
            let s = v6.segments();
            s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
        }
    }
}

/// Whether a connection from `ip` may go on to its token: anyone off the tailnet (the local
/// network, as before), and on it only devices of the Mac's own tailnet user. `me`: the Mac's
/// user, None when Tailscale isn't running (then no tailnet peer can reach it anyway).
pub fn admit(ip: IpAddr, me: Option<i64>, whois: impl Fn(IpAddr) -> Option<i64>) -> Result<(), &'static str> {
    if !is_tailnet(ip) {
        return Ok(());
    }
    match (me, whois(ip)) {
        (Some(me), Some(them)) if me == them => Ok(()),
        (None, _) => Err("Tailscale isn't running on this Mac."),
        _ => Err("This device isn't logged in to Tailscale as you."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_status() {
        let v = json!({
            "BackendState": "Running",
            "Self": { "DNSName": "mac.tail1.ts.net.", "TailscaleIPs": ["fd7a:115c:a1e0::1", "100.78.55.9"], "UserID": 42 },
            "User": { "42": { "ID": 42, "LoginName": "me@example.com" } },
            "Peer": {}
        });
        let s = Status::parse(&v);
        assert!(s.running);
        assert_eq!(s.ipv4(), Some("100.78.55.9"));
        assert_eq!(s.ips[0], "100.78.55.9", "IPv4 first");
        assert_eq!(s.dns_name.as_deref(), Some("mac.tail1.ts.net"));
        assert_eq!((s.user_id, s.login.as_deref()), (Some(42), Some("me@example.com")));
        let stopped = Status::parse(&json!({ "BackendState": "Stopped", "Self": { "TailscaleIPs": [] } }));
        assert!(!stopped.running && stopped.state == "Stopped" && stopped.ipv4().is_none());
        // Unknown fields and a missing Self are fine.
        assert!(!Status::parse(&json!({ "Nope": 1 })).running);
    }

    #[test]
    fn knows_tailnet_addresses() {
        for ip in ["100.64.0.1", "100.127.255.254", "100.78.55.9", "fd7a:115c:a1e0::c233:370a", "::ffff:100.78.55.9"] {
            assert!(is_tailnet(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["100.63.255.255", "100.128.0.1", "192.168.1.20", "127.0.0.1", "::1", "fd7a:115c:a1e1::1"] {
            assert!(!is_tailnet(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn admits_only_your_own_devices_from_the_tailnet() {
        let whois = |ip: IpAddr| match ip.to_string().as_str() {
            "100.73.33.92" => Some(42),
            "100.90.0.1" => Some(7),
            _ => None,
        };
        let ok = |ip: &str, me| admit(ip.parse().unwrap(), me, whois).is_ok();
        assert!(ok("192.168.1.20", Some(42)) && ok("127.0.0.1", None), "the local network as before");
        assert!(ok("100.73.33.92", Some(42)), "your own device");
        assert!(!ok("100.90.0.1", Some(42)), "someone else's");
        assert!(!ok("100.100.1.1", Some(42)), "unknown to Tailscale");
        assert!(!ok("100.73.33.92", None), "Tailscale not running here");
    }
}
