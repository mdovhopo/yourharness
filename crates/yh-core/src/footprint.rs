//! Where a thread has been working: the folders its tool calls touched, from which the repos
//! and worktrees it works in are worked out. Discovered, never declared: a thread can work in
//! any number of them.

use serde_json::Value;
use std::path::Path;

/// Kept per thread, newest last.
pub const LIMIT: usize = 300;

/// Absolute folders one tool call touched. Files count as their folder; shell commands give
/// whatever path-like words they contain, resolved against `working_directory`.
pub fn folders(tool_name: &str, input_json: &str, working_directory: &str) -> Vec<String> {
    let Ok(Value::Object(input)) = serde_json::from_str::<Value>(input_json) else { return vec![] };
    let mut found = vec![];
    for key in ["file_path", "notebook_path"] {
        if let Some(abs) = input.get(key).and_then(Value::as_str).and_then(|p| resolve(p, working_directory)) {
            found.push(parent(&abs));
        }
    }
    if let Some(abs) = input.get("path").and_then(Value::as_str).and_then(|p| resolve(p, working_directory)) {
        found.push(abs);
    }
    if tool_name == "Bash" {
        if let Some(command) = input.get("command").and_then(Value::as_str) {
            found.extend(path_words(command).into_iter().filter(|w| is_path(w, working_directory)).filter_map(|w| resolve(&w, working_directory)));
        }
    }
    unique(found)
}

/// Folders one tool call changed something in: files it edited or wrote, and where a shell
/// command made commits or pushed (`git commit`, `push`, `merge`, …). Reading doesn't count:
/// this is what makes a branch's PR the thread's own work.
pub fn written_folders(tool_name: &str, input_json: &str, working_directory: &str) -> Vec<String> {
    if ["Edit", "MultiEdit", "Write", "NotebookEdit"].contains(&tool_name) {
        return folders(tool_name, input_json, working_directory);
    }
    if tool_name != "Bash" {
        return vec![];
    }
    let Ok(v) = serde_json::from_str::<Value>(input_json) else { return vec![] };
    let Some(command) = v["command"].as_str() else { return vec![] };
    if !writes_git(command) {
        return vec![];
    }
    let mut all = folders(tool_name, input_json, working_directory);
    all.push(working_directory.to_string());
    unique(all)
}

const GIT_WRITES: &[&str] = &["commit", "push", "merge", "rebase", "cherry-pick", "revert", "am", "apply", "pull"];

/// Whether a command runs a git subcommand that adds commits or publishes them.
pub fn writes_git(command: &str) -> bool {
    let words: Vec<&str> = command.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        if w.rsplit('/').next() != Some("git") {
            continue;
        }
        // `git -C path commit`: the subcommand is the first word that isn't an option or its value.
        let mut next = i + 1;
        while next < words.len() && words[next].starts_with('-') {
            next += if ["-C", "-c", "--git-dir", "--work-tree"].contains(&words[next]) { 2 } else { 1 };
        }
        if next < words.len() && GIT_WRITES.contains(&words[next]) {
            return true;
        }
    }
    false
}

/// Absolute paths in a tool's result: where `EnterWorktree` put the session, say.
pub fn folders_in_result(result: &str) -> Vec<String> {
    unique(path_words(result).into_iter().filter(|w| w.starts_with('/')).filter_map(|w| resolve(w.trim_matches(['.', ':']), "/")).collect())
}

/// Adds `new` to `existing`, most recent last, without repeats, within the limit.
pub fn merge(new: &[String], existing: &[String]) -> Vec<String> {
    if new.is_empty() {
        return existing.to_vec();
    }
    let mut kept: Vec<String> = existing.iter().filter(|e| !new.contains(e)).cloned().collect();
    kept.extend(new.iter().cloned());
    let skip = kept.len().saturating_sub(LIMIT);
    kept.split_off(skip)
}

/// Whether any of `folders` is `root` or inside it.
pub fn touches(folders: &[String], root: &str) -> bool {
    let prefix = if root.ends_with('/') { root.to_string() } else { format!("{root}/") };
    folders.iter().any(|f| f == root || f.starts_with(&prefix))
}

fn path_words(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || ";|&()<>\"'`,".contains(c))
        .filter_map(|word| {
            let mut word = word;
            // --prefix=/x, cwd=…
            if let Some(eq) = word.find('=') {
                if word.starts_with('-') || word[..eq].chars().all(|c| c.is_alphabetic() || c == '_') {
                    word = &word[eq + 1..];
                }
            }
            let ok = word.contains('/') && !word.starts_with('-') && !word.contains("://") && !word.contains('$') && !word.contains('*') && word.chars().count() < 400;
            ok.then(|| word.to_string())
        })
        .take(40)
        .collect()
}

/// In a command, `feature/login` is as likely a branch as a folder. A word counts when it is
/// clearly a path (`/…`, `~/…`, `./…`, `../…`) or starts with something that's there.
fn is_path(word: &str, working_directory: &str) -> bool {
    if ["/", "~", "./", "../"].iter().any(|p| word.starts_with(p)) {
        return true;
    }
    match word.split('/').find(|s| !s.is_empty()) {
        Some(first) => Path::new(working_directory).join(first).exists(),
        None => false,
    }
}

/// Absolute and standardized (no `.`, `..` or trailing slash), with `/tmp` and `/var` as their
/// real `/private` paths, as git reports them. None for empty paths and `/`.
pub fn resolve(path: &str, working_directory: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let expanded = if path == "~" || path.starts_with("~/") {
        format!("{}{}", std::env::var("HOME").unwrap_or_default(), &path[1..])
    } else {
        path.to_string()
    };
    let absolute = if expanded.starts_with('/') { expanded } else { format!("{}/{}", working_directory.trim_end_matches('/'), expanded) };
    let mut parts: Vec<&str> = vec![];
    for part in absolute.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    let mut standardized = format!("/{}", parts.join("/"));
    for link in ["/tmp", "/var"] {
        if standardized == link || standardized.starts_with(&format!("{link}/")) {
            standardized = format!("/private{standardized}");
        }
    }
    (standardized.len() > 1).then_some(standardized)
}

/// git reports real paths: `/tmp` is `/private/tmp`.
pub fn real_path(path: &str) -> String {
    resolve(path, "/").unwrap_or_else(|| path.to_string())
}

fn parent(path: &str) -> String {
    match path.rfind('/') {
        Some(0) => "/".into(),
        Some(i) => path[..i].to_string(),
        None => path.to_string(),
    }
}

fn unique(paths: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    paths.into_iter().filter(|p| seen.insert(p.clone())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_from_tool_calls() {
        let cwd = "/work/acme";
        assert_eq!(folders("Edit", r#"{"file_path":"/work/acme-login/src/app.ts"}"#, cwd), ["/work/acme-login/src"]);
        assert_eq!(folders("Grep", r#"{"pattern":"x","path":"repos/api"}"#, cwd), ["/work/acme/repos/api"]);
        // A relative word counts only if its first part is there: `repos` is, `feature` isn't.
        let root = crate::git::tests::scratch("fp");
        std::fs::create_dir_all(root.join("repos/web")).unwrap();
        let base = root.to_string_lossy().to_string();
        let bash = folders(
            "Bash",
            r#"{"command":"cd ../acme-spike && git -C repos/web status && git worktree add -b feature/login x; npm --prefix=/opt/x i https://x.dev/a"}"#,
            &base,
        );
        assert_eq!(bash, [format!("{}/acme-spike", parent(&base)), format!("{base}/repos/web"), "/opt/x".into()]);
        assert!(folders("Bash", r#"{"command":"ls"}"#, cwd).is_empty());
        assert_eq!(folders_in_result("Created worktree at /work/acme/.claude/worktrees/fix."), ["/work/acme/.claude/worktrees/fix"]);
        assert_eq!(resolve("/tmp/x/", "/").as_deref(), Some("/private/tmp/x"));
        assert!(writes_git("cd x && git -C repos/web commit -m hi"));
        assert!(!writes_git("git -C repos/web status"));
        assert_eq!(written_folders("Bash", r#"{"command":"git push"}"#, "/work/acme"), ["/work/acme"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn merges_and_matches() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let f = merge(&s(&["/work/acme-login/src", "/work/acme"]), &s(&["/work/acme", "/old"]));
        assert_eq!(f, s(&["/old", "/work/acme-login/src", "/work/acme"]));
        assert!(touches(&f, "/work/acme-login"));
        assert!(touches(&f, "/work/acme"));
        assert!(!touches(&f, "/work/acme-spike"));
        assert!(!touches(&s(&["/work/acme-loginx"]), "/work/acme-login"));
    }
}
