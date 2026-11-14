//! A model's files into `<models>/<id>/`: `/usr/bin/curl` as the updater uses it, picking up
//! where a stopped download left off (`<name>.part`), each file checked against its SHA-256
//! before it takes its name. A model counts as installed once every file has its name.

use crate::catalog::Model;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

pub fn folder(models: &Path, model: &Model) -> PathBuf {
    models.join(model.id)
}

pub fn installed(models: &Path, model: &Model) -> bool {
    !model.needs_download() || model.files.iter().all(|f| folder(models, model).join(f.name).is_file())
}

/// Bytes on disk so far, finished files and `.part`s: the progress.
pub fn on_disk(models: &Path, model: &Model) -> u64 {
    let dir = folder(models, model);
    model
        .files
        .iter()
        .map(|f| {
            let done = std::fs::metadata(dir.join(f.name)).map(|m| m.len()).ok();
            done.unwrap_or_else(|| std::fs::metadata(dir.join(format!("{}.part", f.name))).map(|m| m.len()).unwrap_or(0))
        })
        .sum()
}

/// Downloads what's missing, stopping when `cancel` is set (Err("Cancelled"), the `.part`s kept
/// for next time). Blocks: run it off the main thread.
pub fn download(models: &Path, model: &Model, cancel: &AtomicBool) -> Result<(), String> {
    let dir = folder(models, model);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't make {}: {e}", dir.display()))?;
    for file in model.files {
        let done = dir.join(file.name);
        if done.is_file() {
            continue;
        }
        let part = dir.join(format!("{}.part", file.name));
        // A part bigger than the file is from something else.
        if std::fs::metadata(&part).map(|m| m.len() > file.bytes).unwrap_or(false) {
            let _ = std::fs::remove_file(&part);
        }
        if std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0) < file.bytes {
            curl(file.url, &part, cancel)?;
        }
        if sha256(&part)? != file.sha256 {
            let _ = std::fs::remove_file(&part);
            return Err(format!("{} didn't match its checksum, so it was thrown away. Try again.", file.name));
        }
        std::fs::rename(&part, &done).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The model's folder, gone.
pub fn remove(models: &Path, model: &Model) -> Result<(), String> {
    let dir = folder(models, model);
    match std::fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
        _ => Ok(()),
    }
}

fn curl(url: &str, to: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let mut child = Command::new("/usr/bin/curl")
        .args(["-fsSL", "--retry", "2", "-C", "-", "-o"])
        .arg(to)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    loop {
        if cancel.load(Ordering::SeqCst) {
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
                let e = e.trim().trim_start_matches("curl: ").to_string();
                return Err(if e.contains("resolve host") { "You're offline.".into() } else if e.is_empty() { "The download failed.".into() } else { e });
            }
            None => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
}

fn sha256(path: &Path) -> Result<String, String> {
    let out = Command::new("/usr/bin/shasum").args(["-a", "256"]).arg(path).stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
    String::from_utf8_lossy(&out.stdout).split_whitespace().next().map(str::to_lowercase).ok_or_else(|| "shasum failed".into())
}
