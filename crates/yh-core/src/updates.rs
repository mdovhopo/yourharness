//! Keeping the app current from GitHub Releases, as the Swift app does: the latest release's
//! disk image downloaded when you ask, its checksum and signature checked, the new app put
//! next to this one, and swapped in on Restart. Blocking steps for a background thread;
//! `/usr/bin/curl`, `hdiutil`, `ditto` and `codesign` do the work.
//!
//! `YH_UPDATE_FEED=<release.json>` reads the release from a file (or a list, for several
//! releases' notes) shaped like GitHub's `releases/latest`; its disk image may be `file://`.

use serde_json::Value;
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const GITHUB_LATEST: &str = "https://api.github.com/repos/mdovhopo/yourharness/releases/latest";
pub const GITHUB_RELEASES: &str = "https://api.github.com/repos/mdovhopo/yourharness/releases?per_page=50";
/// Every release, for "All Releases on GitHub".
pub const RELEASES_PAGE: &str = "https://github.com/mdovhopo/yourharness/releases";

/// One release's notes, as GitHub has them.
#[derive(Clone, Debug, PartialEq)]
pub struct Notes {
    pub version: Version,
    pub text: String,
    pub page: Option<String>,
    /// When it was published (seconds since 1970).
    pub published_at: Option<f64>,
}

/// Every release's notes, newest first: from `feed` (a file or URL, one release or a list), or
/// GitHub's list. Drafts and pre-releases are left out, as the updater never offers them.
pub fn fetch_notes(feed: Option<&str>, current: &str) -> Result<Vec<Notes>, String> {
    let data = match feed {
        Some(f) if !f.contains("://") => std::fs::read(f).map_err(|e| e.to_string())?,
        Some(url) => curl(&[url])?,
        None => curl(&["-H", "Accept: application/vnd.github+json", "-A", &format!("YourHarness/{current}"), GITHUB_RELEASES])?,
    };
    let v: Value = serde_json::from_slice(&data).map_err(|_| "GitHub's answer couldn't be read.".to_string())?;
    let list = match v {
        Value::Array(list) => list,
        other => vec![other],
    };
    let mut notes: Vec<Notes> = list.iter().filter_map(notes_of).collect();
    notes.sort_by(|a, b| b.version.cmp(&a.version));
    Ok(notes)
}

fn notes_of(v: &Value) -> Option<Notes> {
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) {
        return None;
    }
    Some(Notes {
        version: Version::parse(v["tag_name"].as_str()?)?,
        text: whats_new(v["body"].as_str().unwrap_or("")),
        page: v["html_url"].as_str().map(str::to_string),
        published_at: v["published_at"].as_str().and_then(crate::library::parse_iso),
    })
}

/// The notes of the releases after `after` up to and including `through`, newest first;
/// without `after`, just `through`'s.
pub fn notes_between(all: &[Notes], after: Option<&Version>, through: &Version) -> Vec<Notes> {
    all.iter().filter(|n| n.version <= *through && after.map_or(n.version == *through, |a| n.version > *a)).cloned().collect()
}

#[derive(Clone, Debug)]
pub struct Version {
    pub numbers: Vec<u64>,
    pub prerelease: Option<String>,
}

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim().trim_start_matches(['v', 'V']);
        let (core, pre) = match text.split_once('-') {
            Some((c, p)) => (c, Some(p.to_string())),
            None => (text, None),
        };
        let numbers: Option<Vec<u64>> = core.split('.').map(|n| n.parse().ok()).collect();
        let numbers = numbers.filter(|n| !n.is_empty())?;
        Some(Version { numbers, prerelease: pre })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.numbers.iter().map(u64::to_string).collect::<Vec<_>>().join("."))?;
        if let Some(p) = &self.prerelease {
            write!(f, "-{p}")?;
        }
        Ok(())
    }
}

/// 1.0 is 1.0.0.
impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Version {}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, b: &Self) -> Ordering {
        for i in 0..self.numbers.len().max(b.numbers.len()) {
            let (x, y) = (self.numbers.get(i).copied().unwrap_or(0), b.numbers.get(i).copied().unwrap_or(0));
            if x != y {
                return x.cmp(&y);
            }
        }
        // A pre-release comes before its release.
        match (&self.prerelease, &b.prerelease) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: Version,
    pub title: String,
    pub notes: String,
    pub page: Option<String>,
    pub disk_image: String,
    pub disk_image_size: Option<u64>,
    pub checksum: Option<String>,
}

/// GitHub's `releases/latest`; pre-releases and drafts aren't offered.
pub fn parse_release(v: &Value) -> Result<Release, String> {
    let tag = v["tag_name"].as_str().ok_or("The release feed didn't describe a release.")?;
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) {
        return Err("The release feed didn't describe a release.".into());
    }
    let version = Version::parse(tag).ok_or_else(|| format!("The latest release, {tag}, doesn't have a version number."))?;
    let assets = v["assets"].as_array().cloned().unwrap_or_default();
    let image = assets
        .iter()
        .find(|a| a["name"].as_str().is_some_and(|n| n.to_lowercase().ends_with(".dmg")))
        .ok_or_else(|| format!("The latest release, {tag}, has no disk image to download."))?;
    let image_name = image["name"].as_str().unwrap_or("");
    let checksum = assets.iter().find(|a| a["name"].as_str() == Some(&format!("{image_name}.sha256"))).and_then(|a| a["browser_download_url"].as_str().map(str::to_string));
    Ok(Release {
        title: v["name"].as_str().filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| format!("YourHarness {version}")),
        version,
        notes: v["body"].as_str().unwrap_or("").to_string(),
        page: v["html_url"].as_str().map(str::to_string),
        disk_image: image["browser_download_url"].as_str().unwrap_or("").to_string(),
        disk_image_size: image["size"].as_u64(),
        checksum,
    })
}

/// The release notes without the install instructions the release page adds below them.
pub fn whats_new(notes: &str) -> String {
    let lower = notes.to_lowercase();
    let cut = ["\n## install", "\n### install"].iter().filter_map(|m| lower.find(m)).min().or_else(|| (lower.starts_with("## install") || lower.starts_with("### install")).then_some(0));
    notes[..cut.unwrap_or(notes.len())].trim().to_string()
}

fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("/usr/bin/curl").args(["-fsSL", "--max-time", "600"]).args(args).stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        let e = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if e.contains("403") || e.contains("429") { "GitHub is limiting requests right now. Try again in a while.".into() } else if e.contains("resolve host") { "You're offline.".into() } else { e })
    }
}

/// The newest release offered: from `feed` (a file or URL; a list offers its newest), or GitHub.
pub fn fetch_latest(feed: Option<&str>, current: &str) -> Result<Release, String> {
    let data = match feed {
        Some(f) if !f.contains("://") => std::fs::read(f).map_err(|e| e.to_string())?,
        Some(url) => curl(&[url])?,
        None => curl(&["-H", "Accept: application/vnd.github+json", "-A", &format!("YourHarness/{current}"), GITHUB_LATEST])?,
    };
    let v: Value = serde_json::from_slice(&data).map_err(|_| "The release feed didn't describe a release.".to_string())?;
    match v.as_array() {
        Some(list) => list.iter().filter_map(|r| parse_release(r).ok()).max_by(|a, b| a.version.cmp(&b.version)).ok_or_else(|| "The release feed didn't describe a release.".into()),
        None => parse_release(&v),
    }
}

/// Downloads to `to` (curl writes as it goes, so its size is the progress).
pub fn download(url: &str, to: &Path) -> Result<(), String> {
    curl(&["-o", &to.to_string_lossy(), url]).map(|_| ())
}

/// As `download`, stopped (curl killed) once `cancel` is set: Err("Cancelled").
pub fn download_cancellable(url: &str, to: &Path, cancel: &std::sync::atomic::AtomicBool) -> Result<(), String> {
    let mut child = Command::new("/usr/bin/curl")
        .args(["-fsSL", "--max-time", "600", "-o"])
        .arg(to)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    loop {
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Cancelled".into());
        }
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) if status.success() => return Ok(()),
            Some(_) => {
                let mut e = String::new();
                if let Some(mut err) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut err, &mut e);
                }
                let e = e.trim().to_string();
                return Err(if e.contains("resolve host") { "You're offline.".into() } else if e.is_empty() { "The download failed.".into() } else { e });
            }
            None => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
}

/// The hex digest from a `shasum` line (`<digest>  <file name>`).
pub fn digest_in(text: &str) -> Option<String> {
    let word = text.split_whitespace().next()?.to_lowercase();
    (word.len() == 64 && word.chars().all(|c| c.is_ascii_hexdigit())).then_some(word)
}

pub fn verify_checksum(image: &Path, checksum_url: &str) -> Result<(), String> {
    let expected = digest_in(&String::from_utf8_lossy(&curl(&[checksum_url])?)).ok_or("The release's checksum file couldn't be read.")?;
    let out = Command::new("/usr/bin/shasum").args(["-a", "256"]).arg(image).output().map_err(|e| e.to_string())?;
    if digest_in(&String::from_utf8_lossy(&out.stdout)).as_deref() == Some(expected.as_str()) {
        Ok(())
    } else {
        Err("The download doesn't match the release's checksum, so it wasn't installed.".into())
    }
}

/// An app bundle's ID and version, from its Info.plist.
pub fn bundle_info(app: &Path) -> Option<(String, Version)> {
    let read = |key: &str| -> Option<String> {
        let out = Command::new("/usr/bin/plutil").args(["-extract", key, "raw", "-o", "-"]).arg(app.join("Contents/Info.plist")).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    Some((read("CFBundleIdentifier")?, Version::parse(&read("CFBundleShortVersionString")?)?))
}

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program).args(args).stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Ok(text)
    } else {
        Err(text.trim().to_string())
    }
}

/// What a build signed with an identity requires of itself; None when signed ad hoc.
fn designated_requirement(app: &Path) -> Option<String> {
    let text = run("/usr/bin/codesign", &["-d", "-r-", "--verbose=2", &app.to_string_lossy()]).ok()?;
    if text.contains("Signature=adhoc") {
        return None;
    }
    text.lines().find_map(|l| l.strip_prefix("designated => ").map(str::to_string))
}

/// Opens the disk image, checks the app in it is `version` of this app (same bundle ID,
/// signed the way this one is), and copies it to `staged`, next to this one.
pub fn stage(image: &Path, version: &Version, bundle_id: &str, current_app: &Path, staged: &Path, scratch: &Path) -> Result<(), String> {
    let mount = scratch.join("mount");
    std::fs::create_dir_all(&mount).map_err(|e| e.to_string())?;
    run("/usr/bin/hdiutil", &["attach", "-nobrowse", "-readonly", "-noautoopen", "-noverify", "-mountpoint", &mount.to_string_lossy(), &image.to_string_lossy()])
        .map_err(|e| format!("The disk image wouldn't open: {e}"))?;
    let result = (|| {
        let app = std::fs::read_dir(&mount)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "app") && bundle_info(p).is_some_and(|(id, _)| id == bundle_id))
            .ok_or("The disk image has no YourHarness in it.")?;
        if bundle_info(&app).map(|(_, v)| v).as_ref() != Some(version) {
            return Err(format!("The app in the disk image isn't version {version}."));
        }
        let _ = std::fs::remove_dir_all(staged);
        run("/usr/bin/ditto", &[&app.to_string_lossy(), &staged.to_string_lossy()]).map_err(|e| format!("The new version couldn't be copied: {e}"))?;
        run("/usr/bin/codesign", &["--verify", "--deep", "--strict", &staged.to_string_lossy()]).map_err(|_| "The new version's signature doesn't check out, so it wasn't installed.".to_string())?;
        // Signed with an identity (not ad hoc): only an update signed the same way.
        if let Some(requirement) = designated_requirement(current_app) {
            run("/usr/bin/codesign", &["--verify", "--deep", "--strict", "-R", &format!("={requirement}"), &staged.to_string_lossy()])
                .map_err(|_| "The new version isn't signed the way this one is, so it wasn't installed.".to_string())?;
        }
        Ok(())
    })();
    let _ = run("/usr/bin/hdiutil", &["detach", &mount.to_string_lossy(), "-force"]);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(staged);
    }
    result
}

/// Where a downloaded update waits: `/Applications/.YourHarness.app.update`.
pub fn staged_path(app: &Path) -> PathBuf {
    app.with_file_name(format!(".{}.update", app.file_name().unwrap_or_default().to_string_lossy()))
}

/// Once this process is gone, swaps the staged app in for `app` and opens it, with
/// `open_args` (a development run's `-g --env …`). The old copy comes back if the swap fails.
pub fn swap_after_quit(app: &Path, open_args: &[String]) -> Result<(), String> {
    let script = r#"pid=$1 app=$2 staged=$3 old=$4; shift 4
i=0
while kill -0 "$pid" 2>/dev/null; do
  sleep 0.1; i=$((i+1)); [ $i -gt 600 ] && exit 1
done
if mv "$app" "$old"; then
  if mv "$staged" "$app"; then rm -rf "$old"; else mv "$old" "$app"; fi
fi
xattr -dr com.apple.quarantine "$app" 2>/dev/null
open "$@" "$app""#;
    let old = app.with_file_name(format!(".{}.old", app.file_name().unwrap_or_default().to_string_lossy()));
    let mut args: Vec<String> = vec!["-c".into(), script.into(), "sh".into(), std::process::id().to_string(), app.to_string_lossy().into(), staged_path(app).to_string_lossy().into(), old.to_string_lossy().into()];
    args.extend(open_args.iter().cloned());
    Command::new("/bin/sh").args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(|_| ()).map_err(|e| format!("Couldn't restart: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn versions() {
        let v = |s| Version::parse(s).unwrap();
        assert!(v("0.0.10") > v("0.0.9"));
        assert!(v("v1.0") == v("1.0.0"));
        assert!(v("1.0.0-beta.2") < v("1.0.0"));
        assert!(Version::parse("next").is_none());
    }

    #[test]
    fn releases() {
        let r = parse_release(&json!({
            "tag_name": "v0.1.0", "name": "", "body": "Faster.\n\n## Install\nDrag it.",
            "assets": [{ "name": "YourHarness-0.1.0.dmg", "browser_download_url": "file:///tmp/a.dmg", "size": 10 },
                       { "name": "YourHarness-0.1.0.dmg.sha256", "browser_download_url": "file:///tmp/a.sha" }]
        }))
        .unwrap();
        assert_eq!(r.version.to_string(), "0.1.0");
        assert_eq!(r.title, "YourHarness 0.1.0");
        assert_eq!(r.checksum.as_deref(), Some("file:///tmp/a.sha"));
        assert_eq!(whats_new(&r.notes), "Faster.");
        assert!(parse_release(&json!({ "tag_name": "v1", "assets": [] })).is_err());
        assert_eq!(digest_in(&format!("{}  x.dmg", "a".repeat(64))), Some("a".repeat(64)));
    }

    #[test]
    fn notes_for_a_range() {
        let dir = std::env::temp_dir().join(format!("yh-notes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let feed = dir.join("releases.json");
        std::fs::write(&feed, json!([
            {"tag_name": "v0.1.0", "body": "B\n\n## Install\nx", "published_at": "2026-11-01T00:00:00Z"},
            {"tag_name": "v0.0.16", "body": "A"},
            {"tag_name": "v0.2.0-beta", "prerelease": true, "body": "no"},
            {"tag_name": "v0.0.15", "body": "old"}
        ]).to_string()).unwrap();
        let all = fetch_notes(Some(feed.to_str().unwrap()), "0.0.16").unwrap();
        assert_eq!(all.iter().map(|n| n.version.to_string()).collect::<Vec<_>>(), ["0.1.0", "0.0.16", "0.0.15"]);
        assert_eq!(all[0].text, "B");
        let v = |s| Version::parse(s).unwrap();
        assert_eq!(notes_between(&all, Some(&v("0.0.15")), &v("0.1.0")).len(), 2);
        assert_eq!(notes_between(&all, None, &v("0.0.16"))[0].text, "A");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
