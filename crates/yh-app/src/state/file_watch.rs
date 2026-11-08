//! A folder watched for changes (FSEvents, through `notify`), for the Files tab: the tree
//! reads a folder again when something in it is created, deleted or renamed, and an open
//! file reloads when it's saved. Events arrive on FSEvents' own thread and are passed on in
//! batches, a moment after they stop (an editor's save or `git checkout` is many events), so
//! the main thread hears once per burst.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long after the last event a batch goes out.
pub const SETTLE: Duration = Duration::from_millis(250);

/// Watching while it's kept; dropping it stops.
pub struct FolderWatch {
    _watcher: ::notify::RecommendedWatcher,
}

impl FolderWatch {
    /// Watches `root` and everything under it. The receiver gets the paths that changed (as
    /// FSEvents names them, resolved), each burst once.
    pub fn start(root: &Path) -> Option<(FolderWatch, flume::Receiver<Vec<PathBuf>>)> {
        use ::notify::Watcher;
        let (raw_tx, raw_rx) = flume::unbounded::<PathBuf>();
        let mut watcher = ::notify::recommended_watcher(move |event: ::notify::Result<::notify::Event>| {
            if let Ok(event) = event {
                // Reading a file (an access) isn't a change.
                if matches!(event.kind, ::notify::EventKind::Access(_)) {
                    return;
                }
                for p in event.paths {
                    let _ = raw_tx.send(p);
                }
            }
        })
        .ok()?;
        watcher.watch(root, ::notify::RecursiveMode::Recursive).ok()?;
        let (tx, rx) = flume::unbounded();
        // Batches: the first path, then whatever comes until things stay quiet for `SETTLE`.
        std::thread::Builder::new()
            .name("files watch".into())
            .spawn(move || {
                while let Ok(first) = raw_rx.recv() {
                    let mut batch: BTreeSet<PathBuf> = BTreeSet::from([first]);
                    while let Ok(p) = raw_rx.recv_timeout(SETTLE) {
                        batch.insert(p);
                        if batch.len() > 10_000 {
                            break;
                        }
                    }
                    if tx.send(batch.into_iter().collect()).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some((FolderWatch { _watcher: watcher }, rx))
    }
}

/// `path` relative to `root` (both resolved), or None when it's outside, or in `.git`.
pub fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let rel = rel.to_string_lossy().trim_end_matches('/').to_string();
    let in_git = rel == ".git" || rel.starts_with(".git/") || rel.contains("/.git/") || rel.ends_with("/.git");
    (!in_git).then_some(rel)
}

/// The folder a changed path is in, relative to the root ("" for the root itself).
pub fn parent_of(relative: &str) -> String {
    relative.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hears_a_file_made_and_removed() {
        let dir = std::env::temp_dir().join(format!("yh-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let root = std::fs::canonicalize(&dir).unwrap();
        let (_watch, rx) = FolderWatch::start(&root).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(root.join("sub/new.txt"), "x").unwrap();
        // FSEvents may first report the folder made before the watch began: read batches
        // until the file's shows up.
        let until = std::time::Instant::now() + Duration::from_secs(10);
        let mut names: Vec<String> = vec![];
        while !names.iter().any(|n| n == "sub/new.txt") {
            let left = until.saturating_duration_since(std::time::Instant::now());
            let batch = rx.recv_timeout(left).unwrap_or_else(|_| panic!("no batch with the file: {names:?}"));
            names.extend(batch.iter().filter_map(|p| relative(&root, p)));
        }
        assert_eq!(parent_of("sub/new.txt"), "sub");
        assert_eq!(parent_of("top.txt"), "");
        assert_eq!(relative(&root, &root.join(".git/index")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
