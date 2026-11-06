//! A project's files for `@` mentions: what git sees in a repo (tracked and untracked, nothing
//! ignored), or a walk of the folder elsewhere, ranked by name, then path, then letters in
//! order. The port of the Swift app's `FileIndex`, simpler: listed in the background, cached by
//! the app for 30 seconds.

use std::path::Path;
use std::process::Command;

/// Folders a walk skips: build output and dependencies nobody mentions.
const SKIP: &[&str] = &[".git", "node_modules", "target", "build", ".build", "dist", "DerivedData", ".venv", "__pycache__"];
const LIMIT: usize = 50_000;

/// Paths relative to `folder`, folders included (with a trailing `/`).
pub fn list(folder: &Path) -> Vec<String> {
    let git = Command::new("/usr/bin/git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard", "-z"])
        .current_dir(folder)
        .output();
    let mut files: Vec<String> = match git {
        // A folder git ignores (inside a repo) lists nothing: walk it instead.
        Ok(out) if out.status.success() && !out.stdout.is_empty() => out
            .stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .take(LIMIT)
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect(),
        _ => walk(folder),
    };
    // The folders the files are in, to mention a folder too.
    let mut folders: Vec<String> = files
        .iter()
        .flat_map(|f| {
            let parts: Vec<&str> = f.split('/').collect();
            (1..parts.len()).map(move |n| format!("{}/", parts[..n].join("/"))).collect::<Vec<_>>()
        })
        .collect();
    folders.sort();
    folders.dedup();
    files.extend(folders);
    files
}

fn walk(folder: &Path) -> Vec<String> {
    let mut out = vec![];
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') && name != ".github" {
                continue;
            }
            let path = e.path();
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_dir() {
                if !SKIP.contains(&name.as_str()) {
                    stack.push(path);
                }
            } else if let Ok(rel) = path.strip_prefix(folder) {
                out.push(rel.to_string_lossy().to_string());
                if out.len() >= LIMIT {
                    return out;
                }
            }
        }
    }
    out
}

/// The best `limit` matches for `query`: name starts with it, name has it, path has it, then
/// its letters in order. Shorter paths first among equals.
pub fn matches(files: &[String], query: &str, limit: usize) -> Vec<String> {
    let q = query.to_lowercase();
    let mut scored: Vec<(u8, usize, &String)> = files
        .iter()
        .filter_map(|f| {
            let lower = f.to_lowercase();
            let name = lower.trim_end_matches('/').rsplit('/').next().unwrap_or(&lower).to_string();
            let score = if q.is_empty() {
                3
            } else if name.starts_with(&q) {
                0
            } else if name.contains(&q) {
                1
            } else if lower.contains(&q) {
                2
            } else if in_order(&lower, &q) {
                3
            } else {
                return None;
            };
            Some((score, f.len(), f))
        })
        .collect();
    scored.sort();
    scored.into_iter().take(limit).map(|(_, _, f)| f.clone()).collect()
}

fn in_order(text: &str, q: &str) -> bool {
    let mut chars = text.chars();
    q.chars().all(|c| chars.any(|t| t == c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_name_then_path_then_letters() {
        let files: Vec<String> =
            ["src/app.rs", "docs/apple.md", "src/rows/application.rs", "README.md", "src/"].iter().map(|s| s.to_string()).collect();
        let m = matches(&files, "app", 10);
        assert_eq!(m[0], "src/app.rs");
        assert!(m.contains(&"docs/apple.md".to_string()));
        assert!(!m.contains(&"README.md".to_string()));
        assert_eq!(matches(&files, "rdm", 10), vec!["README.md".to_string()]);
    }

    #[test]
    fn lists_a_plain_folder() {
        let dir = std::env::temp_dir().join(format!("yh-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/node_modules")).unwrap();
        std::fs::write(dir.join("a/one.txt"), "x").unwrap();
        std::fs::write(dir.join("a/node_modules/skip.js"), "x").unwrap();
        let files = list(&dir);
        assert!(files.contains(&"a/one.txt".to_string()));
        assert!(files.contains(&"a/".to_string()));
        assert!(!files.iter().any(|f| f.contains("skip")));
    }
}
