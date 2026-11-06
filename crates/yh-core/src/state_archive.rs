//! Everything the app keeps, as one file to hand over: for reproducing a bug with someone's
//! exact state, or moving it to another Mac. The Swift app's `.yhstate` format, so either app
//! reads the other's: a zip (made and opened with `ditto`, as Finder does) laid out as
//!
//! ```text
//! manifest.json   what made it, on what Mac, and what's inside
//! data/           the app's items in its data folder: harness.sqlite and usage.sqlite
//!                 (snapshots), attachments/, …
//! ```
//!
//! (The Swift app also wrote `defaults.plist` and `browser-cookies.json`; read here they're
//! left alone.) Importing replaces the app's items at the next launch, before the database
//! opens: `stage` puts the file's contents in the data folder's `pending-import`, and
//! `apply_pending` swaps them in, in steps that pick up where they stopped after a crash.

use crate::db::Database;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const FORMAT: &str = "yourharness-state";
pub const FORMAT_VERSION: i64 = 1;
pub const PENDING: &str = "pending-import";

/// What the app keeps in its data folder: what an export takes and an import replaces.
pub const ITEMS: &[&str] = &["harness.sqlite", "usage.sqlite", "attachments", "memory-imports", "state.json", "threads"];
const DATABASES: &[&str] = &["harness.sqlite", "usage.sqlite"];
/// Never copied (the snapshots hold what they had); moved aside on import, so an old WAL is
/// never played over a new database.
const COMPANIONS: &[&str] = &["harness.sqlite-wal", "harness.sqlite-shm", "usage.sqlite-wal", "usage.sqlite-shm"];

fn temp(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{prefix}-{}", crate::library::new_uuid()))
}

fn ditto(args: &[&str], failure: &str) -> Result<(), String> {
    let out = Command::new("/usr/bin/ditto").args(args).stdin(Stdio::null()).output().map_err(|e| format!("{failure}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if detail.is_empty() { format!("{failure}.") } else { format!("{failure}: {detail}") })
    }
}

/// `sysctl` by name, for the manifest.
fn sysctl(name: &str) -> Option<String> {
    let out = Command::new("/usr/sbin/sysctl").args(["-n", name]).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn tool(name: &str, path: Option<PathBuf>) -> Value {
    let version = path.as_ref().and_then(|p| Command::new(p).arg("--version").output().ok()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|v| !v.is_empty());
    json!({ "name": name, "path": path.map(|p| p.to_string_lossy().into_owned()), "version": version })
}

/// What a data folder holds (the live one, or an unpacked file's), to say before replacing it.
pub fn contents(folder: &Path) -> Value {
    let mut projects: Vec<String> = vec![];
    let (mut threads, mut entries, mut memories) = (0i64, 0i64, 0i64);
    if let Ok(c) = Connection::open_with_flags(folder.join("harness.sqlite"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) {
        if let Ok(mut s) = c.prepare("SELECT json FROM projects ORDER BY position") {
            if let Ok(rows) = s.query_map([], |r| r.get::<_, Vec<u8>>(0)) {
                for json in rows.flatten() {
                    let v: Value = serde_json::from_slice(&json).unwrap_or_default();
                    projects.push(v["name"].as_str().unwrap_or("Unnamed").to_string());
                }
            }
        }
        let count = |sql: &str| c.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0);
        threads = count("SELECT count(*) FROM threads");
        entries = count("SELECT count(*) FROM entries");
        memories = count("SELECT count(*) FROM memories");
    }
    let attachments = std::fs::read_dir(folder.join("attachments")).map(|d| d.flatten().filter(|e| !e.file_name().to_string_lossy().starts_with('.')).count()).unwrap_or(0);
    let bytes: u64 = ITEMS.iter().chain(COMPANIONS).map(|n| size(&folder.join(n))).sum();
    json!({ "projects": projects, "threads": threads, "entries": entries, "memories": memories, "attachments": attachments, "bytes": bytes, "hasDefaults": false })
}

fn size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if !meta.is_dir() {
        return meta.len();
    }
    std::fs::read_dir(path).map(|d| d.flatten().map(|e| size(&e.path())).sum()).unwrap_or(0)
}

/// Writes what the app keeps, the databases as of now, to `file`; returns the manifest.
/// Blocks: off the main thread.
pub fn export(db: &Database, data_folder: &Path, app: Value, file: &Path) -> Result<Value, String> {
    let work = temp("yourharness-export");
    let data = work.join("data");
    let result = (|| {
        std::fs::create_dir_all(&data).map_err(|e| e.to_string())?;
        db.flush();
        let target = data.join("harness.sqlite").to_string_lossy().into_owned();
        // A connection of its own, outside the writer's transactions: in WAL mode `VACUUM INTO`
        // only reads, and makes a consistent copy with nothing left in a WAL.
        Connection::open(db.path())
            .and_then(|c| c.execute("VACUUM INTO ?1", [&target]))
            .map_err(|e| format!("Couldn't snapshot the database: {e}"))?;
        let usage = data_folder.join("usage.sqlite");
        if usage.exists() {
            let c = Connection::open(&usage).map_err(|e| e.to_string())?;
            c.execute("VACUUM INTO ?1", [data.join("usage.sqlite").to_string_lossy()]).map_err(|e| format!("Couldn't snapshot usage: {e}"))?;
        }
        for name in ITEMS.iter().filter(|n| !DATABASES.contains(n)) {
            let source = data_folder.join(name);
            if source.exists() {
                // A clone on APFS: instant, and no more space until one of them changes.
                let ok = Command::new("/bin/cp").args(["-c", "-R"]).arg(&source).arg(data.join(name)).status().map(|s| s.success()).unwrap_or(false);
                if !ok {
                    return Err(format!("Couldn't copy {name}."));
                }
            }
        }
        let schema: i64 = Connection::open(data.join("harness.sqlite")).and_then(|c| c.query_row("PRAGMA user_version", [], |r| r.get(0))).unwrap_or(0);
        let manifest = json!({
            "format": FORMAT,
            "formatVersion": FORMAT_VERSION,
            "exportedAt": crate::library::iso_now(),
            "app": app,
            "system": {
                "os": format!("macOS {}", sysctl("kern.osproductversion").unwrap_or_default()),
                "model": sysctl("hw.model"),
                "chip": sysctl("machdep.cpu.brand_string"),
                "memoryBytes": sysctl("hw.memsize").and_then(|m| m.parse::<u64>().ok()).unwrap_or(0),
                "processors": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
                "locale": std::env::var("LANG").unwrap_or_default(),
                "timeZone": std::fs::read_link("/etc/localtime").ok().and_then(|p| p.to_string_lossy().split("zoneinfo/").nth(1).map(str::to_string)).unwrap_or_default(),
            },
            "tools": [tool("claude", crate::claude::find_claude()), tool("git", Some(PathBuf::from("/usr/bin/git")))],
            "schemaVersion": schema,
            "dataFolder": data_folder.to_string_lossy(),
            "contents": contents(&data),
        });
        std::fs::write(work.join("manifest.json"), serde_json::to_vec_pretty(&manifest).unwrap_or_default()).map_err(|e| e.to_string())?;
        let zip = temp("yourharness-export").with_extension("zip");
        ditto(&["-c", "-k", "--norsrc", "--noextattr", "--noqtn", &work.to_string_lossy(), &zip.to_string_lossy()], "Couldn't write the file")?;
        let _ = std::fs::remove_file(file);
        std::fs::rename(&zip, file).or_else(|_| std::fs::copy(&zip, file).map(|_| ())).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&zip);
        Ok(manifest)
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// A state file unpacked and checked, waiting to be staged or thrown away.
pub struct Opened {
    pub folder: PathBuf,
    pub manifest: Value,
}

impl Opened {
    pub fn discard(self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// Unpacks `file` somewhere temporary and checks it's a state this app can import.
pub fn open(file: &Path) -> Result<Opened, String> {
    let folder = temp("yourharness-import");
    let result = (|| {
        ditto(&["-x", "-k", &file.to_string_lossy(), &folder.to_string_lossy()], "This isn't a YourHarness state file")?;
        let manifest = read_manifest(&folder)?;
        let db = folder.join("data/harness.sqlite");
        if !db.exists() {
            return Err("The file has no database in it.".to_string());
        }
        let check: String = Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).and_then(|c| c.query_row("PRAGMA quick_check", [], |r| r.get(0))).unwrap_or_default();
        if check != "ok" {
            return Err("The file is damaged: its database doesn't pass SQLite's check.".to_string());
        }
        Ok(manifest)
    })();
    match result {
        Ok(manifest) => Ok(Opened { folder, manifest }),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&folder);
            Err(e)
        }
    }
}

fn read_manifest(folder: &Path) -> Result<Value, String> {
    let v: Value = std::fs::read(folder.join("manifest.json")).ok().and_then(|d| serde_json::from_slice(&d).ok()).ok_or("This isn't a YourHarness state file.")?;
    if v["format"].as_str() != Some(FORMAT) {
        return Err("This isn't a YourHarness state file.".into());
    }
    let version = v["formatVersion"].as_i64().ok_or("This isn't a YourHarness state file.")?;
    if version > FORMAT_VERSION {
        return Err(format!("This file is from a newer YourHarness (state format {version}; this version reads up to {FORMAT_VERSION}). Update YourHarness, then import it."));
    }
    Ok(v)
}

/// Moves what `opened` holds into the data folder, to replace its state at the next launch.
pub fn stage(opened: Opened, data_folder: &Path) -> Result<(), String> {
    let pending = data_folder.join(PENDING);
    std::fs::create_dir_all(data_folder).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&pending);
    if std::fs::rename(&opened.folder, &pending).is_err() {
        // Across volumes: copy, then let go of the original.
        let ok = Command::new("/bin/cp").args(["-c", "-R"]).arg(&opened.folder).arg(&pending).status().map(|s| s.success()).unwrap_or(false);
        let _ = std::fs::remove_dir_all(&opened.folder);
        if !ok {
            return Err("Couldn't put the state in place.".into());
        }
    }
    Ok(())
}

/// At launch, before the database opens: swaps in a staged import, if there is one, and
/// returns its manifest. The app's items move aside first, then a marker is written, then
/// the imported ones move in: each step can run again after a crash without losing either.
pub fn apply_pending(data_folder: &Path) -> Option<Value> {
    let pending = data_folder.join(PENDING);
    if !pending.exists() {
        return None;
    }
    let data = pending.join("data");
    let replaced = pending.join("replaced");
    let cleared = pending.join("cleared");
    let swap = || -> std::io::Result<()> {
        if !data.exists() {
            return Ok(());
        }
        if !cleared.exists() {
            std::fs::create_dir_all(&replaced)?;
            let moved: std::io::Result<()> = (|| {
                for name in ITEMS.iter().chain(COMPANIONS) {
                    let item = data_folder.join(name);
                    if item.exists() {
                        let aside = replaced.join(name);
                        let _ = std::fs::remove_dir_all(&aside);
                        let _ = std::fs::remove_file(&aside);
                        std::fs::rename(&item, &aside)?;
                    }
                }
                std::fs::write(&cleared, b"")
            })();
            if let Err(e) = moved {
                // Back as it was, so the app opens on the state it had.
                for e2 in std::fs::read_dir(&replaced).into_iter().flatten().flatten() {
                    let _ = std::fs::rename(e2.path(), data_folder.join(e2.file_name()));
                }
                return Err(e);
            }
        }
        for e in std::fs::read_dir(&data)?.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !ITEMS.contains(&name.as_str()) {
                continue;
            }
            let destination = data_folder.join(&name);
            if destination.is_dir() {
                std::fs::remove_dir_all(&destination)?;
            } else if destination.exists() {
                std::fs::remove_file(&destination)?;
            }
            std::fs::rename(e.path(), destination)?;
        }
        std::fs::remove_dir_all(&data)
    };
    if let Err(e) = swap() {
        eprintln!("Importing the state didn't finish ({e}); it's tried again at the next launch");
        return None;
    }
    let _ = std::fs::remove_file(&cleared);
    let manifest = read_manifest(&pending).ok();
    let _ = std::fs::remove_dir_all(&pending);
    manifest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let root = temp("yh-state-test");
        let (a, b) = (root.join("a"), root.join("b"));
        std::fs::create_dir_all(a.join("attachments")).unwrap();
        std::fs::write(a.join("attachments/x.png"), b"png").unwrap();
        let db = Database::open(&a).unwrap();
        db.flush();
        let file = root.join("s.yhstate");
        let manifest = export(&db, &a, json!({ "name": "test" }), &file).unwrap();
        assert_eq!(manifest["contents"]["attachments"], 1);
        // Into another folder that has a state of its own: replaced at "launch".
        std::fs::create_dir_all(b.join("attachments")).unwrap();
        std::fs::write(b.join("attachments/old.png"), b"old").unwrap();
        let opened = open(&file).unwrap();
        stage(opened, &b).unwrap();
        let applied = apply_pending(&b).unwrap();
        assert_eq!(applied["format"], FORMAT);
        assert!(b.join("attachments/x.png").exists());
        assert!(!b.join("attachments/old.png").exists());
        assert!(b.join("harness.sqlite").exists());
        assert!(!b.join(PENDING).exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
