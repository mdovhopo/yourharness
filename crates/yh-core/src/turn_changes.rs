//! Which files a turn changed, as git sees them: whoever changed them, Claude's edit tools, a
//! shell command, a script it ran, a commit (the Swift app's `TurnChanges`). Read without
//! touching the repo's index or files: tracked files go through a copy of the index into a tree
//! object; untracked ones are compared by size and modification time rather than added (a big
//! untracked file would be copied into the repo's objects on every turn). Blocking: aside.

use crate::git::Cli;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// More untracked files than this (a build folder nobody ignored) and they're left out.
const UNTRACKED_LIMIT: usize = 20_000;

/// A working folder as it stood at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// The repo's top folder.
    pub top: PathBuf,
    /// Tracked files, as git would commit them.
    tree: String,
    /// Untracked files that aren't ignored, relative to `top`: (size, modified); None when
    /// there were too many to keep.
    untracked: Option<HashMap<String, (u64, f64)>>,
}

fn git(path_env: Option<&str>) -> Cli {
    Cli::new("git", path_env)
}

fn run(cli: &Cli, args: &[&str], cwd: &Path, env: &[(&str, &str)]) -> Option<String> {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let out = cli.run(&args, Some(cwd), Duration::from_secs(30), env);
    out.succeeded().then_some(out.stdout)
}

/// Where `folder`'s repo stands now; None outside a repo or when git fails.
pub fn snapshot(folder: &Path, path_env: Option<&str>) -> Option<Snapshot> {
    let cli = git(path_env);
    let top = PathBuf::from(run(&cli, &["rev-parse", "--show-toplevel"], folder, &[])?.trim());
    let tree = tracked_tree(&cli, &top)?;
    Some(Snapshot { untracked: untracked_stamps(&cli, &top), top, tree })
}

/// Absolute paths of the files that changed (added, edited, removed) since `start`; None
/// when git fails.
pub fn changes(start: &Snapshot, path_env: Option<&str>) -> Option<Vec<String>> {
    let cli = git(path_env);
    let tree = tracked_tree(&cli, &start.top)?;
    let diff = run(&cli, &["diff-tree", "-r", "--name-only", "-z", "--no-renames", &start.tree, &tree], &start.top, &[])?;
    let mut changed: std::collections::BTreeSet<String> = diff.split('\0').filter(|p| !p.is_empty()).map(str::to_string).collect();
    if let (Some(before), Some(after)) = (&start.untracked, untracked_stamps(&cli, &start.top)) {
        for (path, stamp) in &after {
            if before.get(path) != Some(stamp) {
                changed.insert(path.clone());
            }
        }
        for path in before.keys() {
            if !after.contains_key(path) {
                changed.insert(path.clone());
            }
        }
    }
    Some(changed.into_iter().map(|p| start.top.join(p).to_string_lossy().into_owned()).collect())
}

/// The tracked files as a tree object, through a copy of the index: `add -u` on it only reads
/// files whose size or time changed.
fn tracked_tree(cli: &Cli, root: &Path) -> Option<String> {
    let index = run(cli, &["rev-parse", "--git-path", "index"], root, &[])?;
    let index = index.trim();
    let index = if index.starts_with('/') { PathBuf::from(index) } else { root.join(index) };
    let copy = std::env::temp_dir().join(format!("yourharness-index-{}", crate::library::new_uuid()));
    // A repo with nothing added yet has no index: an empty one is right.
    let _ = std::fs::copy(&index, &copy);
    let copy_str = copy.to_string_lossy().into_owned();
    let env = [("GIT_INDEX_FILE", copy_str.as_str())];
    let tree = run(cli, &["add", "-u"], root, &env).and_then(|_| run(cli, &["write-tree"], root, &env));
    let _ = std::fs::remove_file(&copy);
    let _ = std::fs::remove_file(copy.with_extension("lock"));
    tree.map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

fn untracked_stamps(cli: &Cli, root: &Path) -> Option<HashMap<String, (u64, f64)>> {
    let listed = run(cli, &["ls-files", "-z", "--others", "--exclude-standard"], root, &[])?;
    let paths: Vec<&str> = listed.split('\0').filter(|p| !p.is_empty()).collect();
    if paths.len() > UNTRACKED_LIMIT {
        return None;
    }
    Some(
        paths
            .into_iter()
            .filter_map(|p| {
                let meta = std::fs::metadata(root.join(p)).ok()?;
                let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64();
                Some((p.to_string(), (meta.len(), modified)))
            })
            .collect(),
    )
}

/// With another thread working in the same folder, git can't tell whose a change was: only the
/// files this turn's tool calls name count (by path, path in the repo, or file name).
pub fn named_only(changed: Vec<String>, top: &Path, named: &str) -> Vec<String> {
    let top = format!("{}/", top.to_string_lossy());
    changed
        .into_iter()
        .filter(|path| {
            let relative = path.strip_prefix(&top).unwrap_or(path);
            let name = path.rsplit('/').next().unwrap_or(path);
            named.contains(path.as_str()) || named.contains(relative) || named.contains(name)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git").args(args).current_dir(dir).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t").output().unwrap();
        assert!(ok.status.success(), "{:?}", String::from_utf8_lossy(&ok.stderr));
    }

    #[test]
    fn sees_what_changed_whoever_changed_it() {
        let dir = std::env::temp_dir().join(format!("yh-turn-{}", crate::library::new_uuid()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        sh(&dir, &["init", "-q"]);
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        std::fs::write(dir.join("keep.txt"), "k").unwrap();
        sh(&dir, &["add", "."]);
        sh(&dir, &["commit", "-qm", "one"]);
        std::fs::write(dir.join("gone.txt"), "g").unwrap();
        let start = snapshot(&dir, None).unwrap();
        // An edit, a new file, a deleted untracked one: as a shell command would.
        std::fs::write(dir.join("a.txt"), "changed").unwrap();
        std::fs::write(dir.join("new.txt"), "n").unwrap();
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        let changed = changes(&start, None).unwrap();
        let names: Vec<&str> = changed.iter().map(|p| p.rsplit('/').next().unwrap()).collect();
        assert_eq!(names, ["a.txt", "gone.txt", "new.txt"]);
        // The index is untouched.
        let status = std::process::Command::new("git").args(["diff", "--cached", "--name-only"]).current_dir(&dir).output().unwrap();
        assert!(status.stdout.is_empty());
        assert_eq!(named_only(changed, &dir, r#"{"file_path":"a.txt"}"#).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(snapshot(Path::new("/"), None).is_none());
    }
}
