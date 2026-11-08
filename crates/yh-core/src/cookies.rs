//! Bringing sign-ins over from your other browsers into a project's browser: you pick the
//! browser and the sites, and only those cookies are copied. Nothing is read until you pick
//! a browser, and macOS asks first (Full Disk Access for Safari, the keychain for the key a
//! Chromium browser encrypts its cookies with).

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Cookie {
    pub domain: String,
    pub name: String,
    pub value: String,
    pub path: String,
    /// Seconds since 1970.
    pub expires: Option<f64>,
    pub secure: bool,
    pub http_only: bool,
    /// `lax`, `strict`, `none`.
    pub same_site: Option<String>,
}

impl Cookie {
    /// The site it's for, without the leading dot of a domain cookie.
    pub fn site(&self) -> &str {
        self.domain.trim_start_matches('.')
    }

    pub fn is_expired(&self) -> bool {
        self.expires.is_some_and(|e| e < now())
    }
}

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.)
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

// MARK: Safari

pub fn safari_files() -> Vec<PathBuf> {
    vec![home().join("Library/Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies"), home().join("Library/Cookies/Cookies.binarycookies")]
}

pub const SAFARI_ACCESS: &str = "macOS keeps Safari's cookies private. To import them, turn on Full Disk Access for YourHarness in System Settings → Privacy & Security, then try again.";

pub fn read_safari() -> Result<Vec<Cookie>, String> {
    let mut last = None;
    for f in safari_files() {
        match std::fs::read(&f) {
            Ok(data) => return parse_safari(&data),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => last = Some(SAFARI_ACCESS.to_string()),
            Err(e) => last = Some(e.to_string()),
        }
    }
    Err(last.unwrap_or_else(|| "Safari has no cookies saved.".into()))
}

/// Safari's `Cookies.binarycookies`: big-endian page sizes, little-endian records.
pub fn parse_safari(bytes: &[u8]) -> Result<Vec<Cookie>, String> {
    if bytes.len() < 8 || &bytes[..4] != b"cook" {
        return Err("This isn't a Safari cookie file.".into());
    }
    let be = |at: usize| u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
    let pages = be(4);
    let mut offset = 8;
    let mut sizes = vec![];
    for _ in 0..pages {
        if offset + 4 > bytes.len() {
            return Err("Safari's cookie file is cut short.".into());
        }
        sizes.push(be(offset));
        offset += 4;
    }
    let mut cookies = vec![];
    for size in sizes {
        if offset + size > bytes.len() || size < 8 {
            return Err("Safari's cookie file is cut short.".into());
        }
        cookies.extend(safari_page(&bytes[offset..offset + size]));
        offset += size;
    }
    Ok(cookies)
}

fn le(b: &[u8], at: usize) -> u32 {
    if at + 4 > b.len() {
        return 0;
    }
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn safari_page(page: &[u8]) -> Vec<Cookie> {
    let count = le(page, 4) as usize;
    let mut out = vec![];
    for i in 0..count {
        let at = 8 + i * 4;
        if at + 4 > page.len() {
            break;
        }
        let start = le(page, at) as usize;
        if start + 56 > page.len() {
            continue;
        }
        let size = le(page, start) as usize;
        if size < 56 || start + size > page.len() {
            continue;
        }
        let r = &page[start..start + size];
        let flags = le(r, 8);
        let string = |field: usize| -> String {
            let pos = le(r, field) as usize;
            if pos >= r.len() {
                return String::new();
            }
            let end = r[pos..].iter().position(|b| *b == 0).map_or(r.len(), |e| pos + e);
            String::from_utf8_lossy(&r[pos..end]).into_owned()
        };
        let expiry = f64::from_le_bytes(r[40..48].try_into().unwrap_or([0; 8]));
        out.push(Cookie {
            domain: string(16),
            name: string(20),
            value: string(28),
            path: string(24),
            // Seconds since 2001.
            expires: (expiry > 0.).then_some(expiry + 978_307_200.),
            secure: flags & 1 != 0,
            http_only: flags & 4 != 0,
            same_site: None,
        });
    }
    out
}

/// A Safari-format cookie file of `cookies`, for tests (and the inverse of `parse_safari`).
pub fn write_safari(cookies: &[Cookie]) -> Vec<u8> {
    let mut records: Vec<Vec<u8>> = vec![];
    for c in cookies {
        let mut strings = vec![];
        let mut offsets = [0u32; 4];
        let base = 56usize;
        for (i, s) in [&c.domain, &c.name, &c.path, &c.value].iter().enumerate() {
            offsets[i] = (base + strings.len()) as u32;
            strings.extend_from_slice(s.as_bytes());
            strings.push(0);
        }
        let size = base + strings.len();
        let mut r = vec![0u8; base];
        r[0..4].copy_from_slice(&(size as u32).to_le_bytes());
        let flags = (c.secure as u32) | ((c.http_only as u32) << 2);
        r[8..12].copy_from_slice(&flags.to_le_bytes());
        r[16..20].copy_from_slice(&offsets[0].to_le_bytes());
        r[20..24].copy_from_slice(&offsets[1].to_le_bytes());
        r[24..28].copy_from_slice(&offsets[2].to_le_bytes());
        r[28..32].copy_from_slice(&offsets[3].to_le_bytes());
        let expiry = c.expires.map_or(0., |e| e - 978_307_200.);
        r[40..48].copy_from_slice(&expiry.to_le_bytes());
        r.extend(strings);
        records.push(r);
    }
    let header = 8 + records.len() * 4 + 4;
    let mut page = vec![0u8; header];
    page[0..4].copy_from_slice(&[0, 0, 1, 0]);
    page[4..8].copy_from_slice(&(records.len() as u32).to_le_bytes());
    let mut at = header;
    for (i, r) in records.iter().enumerate() {
        page[8 + i * 4..12 + i * 4].copy_from_slice(&(at as u32).to_le_bytes());
        at += r.len();
    }
    for r in &records {
        page.extend_from_slice(r);
    }
    let mut out = b"cook".to_vec();
    out.extend_from_slice(&1u32.to_be_bytes());
    out.extend_from_slice(&(page.len() as u32).to_be_bytes());
    out.extend(page);
    out
}

// MARK: Chromium browsers

pub struct Chromium {
    pub name: &'static str,
    folder: &'static str,
    service: &'static str,
    account: &'static str,
}

pub const CHROMIUM: &[Chromium] = &[
    Chromium { name: "Chrome", folder: "Google/Chrome", service: "Chrome Safe Storage", account: "Chrome" },
    Chromium { name: "Arc", folder: "Arc/User Data", service: "Arc Safe Storage", account: "Arc" },
    Chromium { name: "Brave", folder: "BraveSoftware/Brave-Browser", service: "Brave Safe Storage", account: "Brave" },
    Chromium { name: "Edge", folder: "Microsoft Edge", service: "Microsoft Edge Safe Storage", account: "Microsoft Edge" },
    Chromium { name: "Vivaldi", folder: "Vivaldi", service: "Vivaldi Safe Storage", account: "Vivaldi" },
    Chromium { name: "Chromium", folder: "Chromium", service: "Chromium Safe Storage", account: "Chromium" },
];

/// Where to import from: Safari always, and every Chromium profile with cookies.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Safari,
    /// (browser index in `CHROMIUM`, profile name, its cookie database)
    Chromium(usize, String, PathBuf),
}

impl Source {
    pub fn label(&self) -> String {
        match self {
            Source::Safari => "Safari".into(),
            Source::Chromium(i, profile, _) => {
                let b = &CHROMIUM[*i];
                if chromium_profiles(*i).len() > 1 { format!("{} — {profile}", b.name) } else { b.name.to_string() }
            }
        }
    }
}

fn chromium_root(i: usize) -> PathBuf {
    home().join("Library/Application Support").join(CHROMIUM[i].folder)
}

fn chromium_profiles(i: usize) -> Vec<(String, PathBuf)> {
    let root = chromium_root(i);
    let Ok(entries) = std::fs::read_dir(&root) else { return vec![] };
    let names: serde_json::Value = std::fs::read(root.join("Local State")).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default();
    let mut folders: Vec<String> = entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|f| f == "Default" || f.starts_with("Profile ")).collect();
    folders.sort_by_key(|f| (f != "Default", f.clone()));
    folders
        .into_iter()
        .filter_map(|f| {
            let base = root.join(&f);
            let db = [base.join("Cookies"), base.join("Network/Cookies")].into_iter().find(|p| p.exists())?;
            let name = names["profile"]["info_cache"][&f]["name"].as_str().unwrap_or(&f).to_string();
            Some((name, db))
        })
        .collect()
}

pub fn sources() -> Vec<Source> {
    let mut all = vec![Source::Safari];
    for i in 0..CHROMIUM.len() {
        all.extend(chromium_profiles(i).into_iter().map(|(n, p)| Source::Chromium(i, n, p)));
    }
    all
}

/// The source's cookies, unexpired. Blocks (the keychain asks): off the main thread.
pub fn read(source: &Source) -> Result<Vec<Cookie>, String> {
    let cookies = match source {
        Source::Safari => read_safari()?,
        Source::Chromium(i, _, db) => read_chromium(db, &keychain_password(*i)?)?,
    };
    Ok(cookies.into_iter().filter(|c| !c.is_expired() && !c.name.is_empty()).collect())
}

/// The key the browser encrypts its cookies with, from your keychain (macOS asks first).
fn keychain_password(i: usize) -> Result<String, String> {
    let b = &CHROMIUM[i];
    let out = std::process::Command::new("/usr/bin/security").args(["find-generic-password", "-w", "-s", b.service, "-a", b.account]).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("Access to {}'s cookie key was refused, or it isn't in your keychain, so its cookies can't be read.", b.name));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[link(name = "System")]
unsafe extern "C" {
    fn CCKeyDerivationPBKDF(algorithm: u32, password: *const u8, password_len: usize, salt: *const u8, salt_len: usize, prf: u32, rounds: u32, derived: *mut u8, derived_len: usize) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn CCCrypt(op: u32, alg: u32, options: u32, key: *const u8, key_len: usize, iv: *const u8, data_in: *const u8, in_len: usize, data_out: *mut u8, out_avail: usize, out_moved: *mut usize) -> i32;
}

fn derived_key(password: &str) -> [u8; 16] {
    let mut key = [0u8; 16];
    let salt = b"saltysalt";
    // kCCPBKDF2 = 2, kCCPRFHmacAlgSHA1 = 1
    unsafe { CCKeyDerivationPBKDF(2, password.as_ptr(), password.len(), salt.as_ptr(), salt.len(), 1, 1003, key.as_mut_ptr(), 16) };
    key
}

/// AES-128-CBC with a space IV, PKCS7 padding: `v10` and the rest.
fn crypt(encrypt: bool, input: &[u8], key: &[u8; 16]) -> Option<Vec<u8>> {
    let iv = [0x20u8; 16];
    let mut out = vec![0u8; input.len() + 16];
    let mut moved = 0usize;
    // kCCEncrypt 0 / kCCDecrypt 1, kCCAlgorithmAES 0, kCCOptionPKCS7Padding 1
    let status = unsafe { CCCrypt(if encrypt { 0 } else { 1 }, 0, 1, key.as_ptr(), 16, iv.as_ptr(), input.as_ptr(), input.len(), out.as_mut_ptr(), out.len(), &mut moved) };
    (status == 0).then(|| {
        out.truncate(moved);
        out
    })
}

pub fn read_chromium(db: &Path, password: &str) -> Result<Vec<Cookie>, String> {
    // A copy: the browser may have it open.
    let copy = std::env::temp_dir().join(format!("yh-cookies-{}", crate::library::new_uuid()));
    std::fs::copy(db, &copy).map_err(|e| format!("Couldn't read the cookie database: {e}"))?;
    for suffix in ["-wal", "-journal"] {
        let side = PathBuf::from(format!("{}{suffix}", db.display()));
        if side.exists() {
            let _ = std::fs::copy(&side, format!("{}{suffix}", copy.display()));
        }
    }
    let result = (|| {
        let c = rusqlite::Connection::open(&copy).map_err(|_| "Couldn't open the cookie database.".to_string())?;
        let version: i64 = c.query_row("SELECT value FROM meta WHERE key = 'version'", [], |r| r.get::<_, String>(0)).ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        let key = derived_key(password);
        let mut s = c
            .prepare("SELECT host_key, name, value, encrypted_value, path, expires_utc, is_secure, is_httponly, samesite FROM cookies")
            .map_err(|_| "The cookie database isn't in a format YourHarness knows.".to_string())?;
        let rows = s
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Vec<u8>>(3).unwrap_or_default(),
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, i64>(8).unwrap_or(-1),
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut out = vec![];
        for (host, name, value, encrypted, path, expires, secure, http_only, same_site) in rows.flatten() {
            let mut value = value;
            if value.is_empty() && encrypted.len() > 3 {
                if &encrypted[..3] != b"v10" {
                    continue;
                }
                let Some(mut plain) = crypt(false, &encrypted[3..], &key) else { continue };
                // Since database version 24 the plaintext starts with a hash of the domain.
                if version >= 24 && plain.len() >= 32 {
                    plain.drain(..32);
                }
                value = String::from_utf8_lossy(&plain).into_owned();
            }
            out.push(Cookie {
                domain: host,
                name,
                value,
                path,
                expires: (expires != 0).then(|| expires as f64 / 1_000_000. - 11_644_473_600.),
                secure: secure != 0,
                http_only: http_only != 0,
                same_site: match same_site {
                    0 => Some("none".into()),
                    1 => Some("lax".into()),
                    2 => Some("strict".into()),
                    _ => None,
                },
            });
        }
        Ok(out)
    })();
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", copy.display()));
    }
    result
}

/// The sites in a set of cookies, with how many each has, the most first.
pub fn sites(cookies: &[Cookie]) -> Vec<(String, usize)> {
    let mut by: std::collections::HashMap<String, usize> = Default::default();
    for c in cookies {
        *by.entry(c.site().to_string()).or_default() += 1;
    }
    let mut v: Vec<(String, usize)> = by.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safari_round_trip() {
        let c = Cookie { domain: ".example.com".into(), name: "sid".into(), value: "abc".into(), path: "/".into(), expires: Some(2_000_000_000.), secure: true, http_only: true, same_site: None };
        let parsed = parse_safari(&write_safari(std::slice::from_ref(&c))).unwrap();
        assert_eq!(parsed, vec![c]);
        assert!(parse_safari(b"nope").is_err());
    }

    #[test]
    fn chromium_crypto_round_trip() {
        let key = derived_key("peanuts");
        let sealed = crypt(true, b"hello", &key).unwrap();
        assert_eq!(crypt(false, &sealed, &key).unwrap(), b"hello");
    }
}
