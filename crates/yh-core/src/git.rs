//! Git as the workspace sees it: where a repo's `origin` lives (`GitRemote`), a checkout's
//! branch and working-tree state (`RepoStatus`), its worktrees (`Worktree`), and the plain
//! `git` commands behind them (`Git`). Everything here runs the official `git` binary and
//! blocks until it answers (each command has a 15 s limit): call it off the main thread.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// MARK: Running a CLI

/// What a command said and how it ended.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandOutput {
    /// Exit code; None when it couldn't start, was killed or timed out.
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub launch_failed: bool,
}

impl CommandOutput {
    pub fn succeeded(&self) -> bool {
        self.status == Some(0) && !self.timed_out
    }
}

/// An official CLI (`git`, `gh`, `glab`) and the environment to run it in: the PATH to find it
/// (and what it runs) on, and variables on top of ours.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cli {
    /// A name looked up on `path_env`, or an absolute path.
    pub program: String,
    pub path_env: Option<String>,
    pub env: Vec<(String, String)>,
}

impl Cli {
    pub fn new(program: &str, path_env: Option<&str>) -> Cli {
        Cli { program: program.to_string(), path_env: path_env.map(str::to_string), env: vec![] }
    }

    /// The CLI, when it's on the PATH (`path_env`, else ours).
    pub fn find(program: &str, path_env: Option<&str>) -> Option<Cli> {
        which(program, path_env).map(|_| Cli::new(program, path_env))
    }

    pub fn with_env(mut self, key: &str, value: &str) -> Cli {
        self.env.push((key.to_string(), value.to_string()));
        self
    }

    /// Runs it to the end or `timeout`, whichever comes first (then it's killed). Blocks.
    pub fn run(&self, args: &[String], cwd: Option<&Path>, timeout: Duration, extra: &[(&str, &str)]) -> CommandOutput {
        let mut command = Command::new(&self.program);
        command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        command.current_dir(cwd.map(Path::to_path_buf).unwrap_or_else(std::env::temp_dir));
        // The login shell's PATH once read: an app opened from the Dock has only launchd's.
        if let Some(p) = self.path_env.clone().or_else(crate::shell_env::loaded_path) {
            command.env("PATH", p);
        }
        for (k, v) in &self.env {
            command.env(k, v);
        }
        for (k, v) in extra {
            command.env(k, v);
        }
        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => return CommandOutput { launch_failed: true, stderr: format!("{}: {e}", self.program), ..Default::default() },
        };
        let collect = |pipe: Option<Box<dyn Read + Send>>| {
            let buffer = Arc::new(Mutex::new(Vec::new()));
            let (sink, done) = (buffer.clone(), flume::bounded::<()>(1));
            if let Some(mut pipe) = pipe {
                std::thread::spawn(move || {
                    let mut chunk = [0u8; 64 * 1024];
                    while let Ok(n) = pipe.read(&mut chunk) {
                        if n == 0 {
                            break;
                        }
                        sink.lock().unwrap().extend_from_slice(&chunk[..n]);
                    }
                    let _ = done.0.send(());
                });
            } else {
                let _ = done.0.send(());
            }
            (buffer, done.1)
        };
        let (out, out_done) = collect(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
        let (err, err_done) = collect(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
        let start = Instant::now();
        let (status, timed_out) = loop {
            match child.try_wait() {
                Ok(Some(s)) => break (s.code(), false),
                Ok(None) if start.elapsed() >= timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break (None, true);
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => break (None, false),
            }
        };
        // Something the command started may hold its pipes open: don't wait on it for long.
        let _ = out_done.recv_timeout(Duration::from_secs(1));
        let _ = err_done.recv_timeout(Duration::from_secs(1));
        let text = |b: &Arc<Mutex<Vec<u8>>>| String::from_utf8_lossy(&b.lock().unwrap()).into_owned();
        CommandOutput { status, stdout: text(&out), stderr: text(&err), timed_out, launch_failed: false }
    }
}

/// Where `program` is on `path_env` (else our PATH).
pub fn which(program: &str, path_env: Option<&str>) -> Option<PathBuf> {
    if program.contains('/') {
        return Path::new(program).is_file().then(|| PathBuf::from(program));
    }
    let path = path_env.map(str::to_string).or_else(crate::shell_env::loaded_path).or_else(|| std::env::var("PATH").ok())?;
    path.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join(program)).find(|p| p.is_file())
}

// MARK: Remotes

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Forge {
    GitHub,
    GitLab,
}

impl Forge {
    /// The official CLI we drive for it.
    pub fn cli_name(self) -> &'static str {
        match self {
            Forge::GitHub => "gh",
            Forge::GitLab => "glab",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Forge::GitHub => "GitHub",
            Forge::GitLab => "GitLab",
        }
    }

    pub fn install_url(self) -> &'static str {
        match self {
            Forge::GitHub => "https://cli.github.com",
            Forge::GitLab => "https://gitlab.com/gitlab-org/cli",
        }
    }

    /// What the forge calls a pull request.
    pub fn request_noun(self) -> &'static str {
        match self {
            Forge::GitHub => "pull request",
            Forge::GitLab => "merge request",
        }
    }

    /// Where its CLI goes when no `--hostname` is given.
    pub fn default_host(self) -> &'static str {
        match self {
            Forge::GitHub => "github.com",
            Forge::GitLab => "gitlab.com",
        }
    }
}

/// Where a repo's `origin` lives. Credentials in the URL are dropped on parse, never kept.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GitRemote {
    /// Lowercased.
    pub host: String,
    /// `owner/name`, or `group/subgroup/name` on GitLab.
    pub path: String,
    pub forge: Option<Forge>,
}

impl GitRemote {
    pub fn new(host: &str, path: &str, forge: Option<Forge>) -> GitRemote {
        GitRemote { host: host.to_lowercase(), path: path.to_string(), forge }
    }

    /// `git@host:owner/repo.git`, `ssh://git@host:22/owner/repo`, `https://user:token@host/owner/repo.git`.
    /// None for local paths and anything without an owner.
    pub fn parse(raw: &str) -> Option<GitRemote> {
        let text = raw.trim();
        let (host, path) = if let Some((_, rest)) = text.split_once("://") {
            let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            let authority = &rest[..end];
            let host_port = authority.rsplit('@').next().unwrap_or(authority);
            let host = if host_port.starts_with('[') {
                host_port.split(']').next().unwrap_or("").trim_start_matches('[')
            } else {
                host_port.split(':').next().unwrap_or("")
            };
            let path = &rest[end..];
            let path = path.split(['?', '#']).next().unwrap_or("");
            (host.to_string(), path.to_string())
        } else {
            let colon = text.find(':')?;
            let authority = &text[..colon];
            if authority.contains('/') {
                return None;
            }
            (authority.rsplit('@').next().unwrap_or(authority).to_string(), text[colon + 1..].to_string())
        };
        let mut path = path.trim_matches('/').to_string();
        if path.ends_with(".git") {
            path.truncate(path.len() - 4);
        }
        if host.is_empty() || !path.contains('/') {
            return None;
        }
        let host = host.to_lowercase();
        let forge = if host.contains("github") {
            Some(Forge::GitHub)
        } else if host.contains("gitlab") {
            Some(Forge::GitLab)
        } else {
            None
        };
        Some(GitRemote { host, path, forge })
    }

    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    pub fn owner(&self) -> &str {
        self.path.rsplit_once('/').map(|(o, _)| o).unwrap_or("")
    }

    pub fn web_url(&self) -> String {
        format!("https://{}/{}", self.host, self.path)
    }

    /// The same repo, whatever the case of its path: a PR's link may not spell it as `origin` does.
    pub fn same_repo(&self, host: &str, path: &str) -> bool {
        self.host.eq_ignore_ascii_case(host) && self.path.eq_ignore_ascii_case(path)
    }
}

// MARK: Status

/// A checkout's branch and working-tree state, from `git status --porcelain=v2 --branch`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoStatus {
    /// None when HEAD is detached.
    pub branch: Option<String>,
    pub head_oid: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub staged: u32,
    pub modified: u32,
    pub untracked: u32,
    pub conflicted: u32,
    pub stashes: u32,
    /// It tracks a remote branch that's been deleted: usually its PR was merged.
    pub upstream_gone: bool,
}

impl RepoStatus {
    pub fn changed_files(&self) -> u32 {
        self.staged + self.modified + self.untracked + self.conflicted
    }

    pub fn is_dirty(&self) -> bool {
        self.changed_files() > 0
    }

    pub fn parse(output: &str) -> RepoStatus {
        let mut s = RepoStatus::default();
        let mut saw_ab = false;
        for line in output.split('\n').filter(|l| !l.is_empty()) {
            if let Some(header) = line.strip_prefix("# ") {
                let Some((key, value)) = header.split_once(' ') else { continue };
                match key {
                    "branch.oid" => s.head_oid = (value != "(initial)").then(|| value.to_string()),
                    "branch.head" => s.branch = (value != "(detached)").then(|| value.to_string()),
                    "branch.upstream" => s.upstream = Some(value.to_string()),
                    "branch.ab" => {
                        saw_ab = true;
                        for v in value.split(' ') {
                            if let Some(n) = v.strip_prefix('+') {
                                s.ahead = n.parse().unwrap_or(0);
                            }
                            if let Some(n) = v.strip_prefix('-') {
                                s.behind = n.parse().unwrap_or(0);
                            }
                        }
                    }
                    "stash" => s.stashes = value.parse().unwrap_or(0),
                    _ => {}
                }
                continue;
            }
            match line.as_bytes()[0] {
                b'1' | b'2' => {
                    // "1 XY …": X the index, Y the worktree; "." is unchanged.
                    let xy: Vec<char> = line.chars().skip(2).take(2).collect();
                    if xy.len() != 2 {
                        continue;
                    }
                    if xy[0] != '.' {
                        s.staged += 1;
                    }
                    if xy[1] != '.' {
                        s.modified += 1;
                    }
                }
                b'u' => s.conflicted += 1,
                b'?' => s.untracked += 1,
                _ => {}
            }
        }
        // Tracking something, with nothing to compare against: the remote branch is gone.
        s.upstream_gone = s.upstream.is_some() && !saw_ab;
        s
    }
}

// MARK: Worktrees

/// One checkout of a repo: its main working tree, or a linked one (`git worktree add`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Worktree {
    pub path: String,
    pub head_oid: Option<String>,
    /// Short name; None when HEAD is detached (or the repo is bare).
    pub branch: Option<String>,
    /// The repo's own checkout, the one the others hang off.
    pub is_main: bool,
    pub is_bare: bool,
    /// `git worktree lock`: git won't prune or remove it. The reason, if one was given.
    pub lock_reason: Option<String>,
    pub is_locked: bool,
    /// Its folder is gone; `git worktree prune` would drop it.
    pub is_missing: bool,
}

impl Worktree {
    pub fn is_detached(&self) -> bool {
        self.branch.is_none() && !self.is_bare
    }

    /// `git worktree list --porcelain`: blocks of `key value` lines, the main one first.
    pub fn parse(output: &str) -> Vec<Worktree> {
        let mut all: Vec<Worktree> = vec![];
        let mut current: Option<Worktree> = None;
        for line in output.split('\n') {
            let (key, value) = match line.split_once(' ') {
                Some((k, v)) => (k, Some(v)),
                None => (line, None),
            };
            if key.is_empty() {
                all.extend(current.take());
                continue;
            }
            match key {
                "worktree" => {
                    all.extend(current.take());
                    current = Some(Worktree { path: value.unwrap_or("").to_string(), is_main: all.is_empty(), ..Default::default() });
                }
                _ => {
                    let Some(w) = current.as_mut() else { continue };
                    match key {
                        "HEAD" => w.head_oid = value.map(str::to_string),
                        "branch" => w.branch = value.map(|v| v.strip_prefix("refs/heads/").unwrap_or(v).to_string()),
                        "bare" => w.is_bare = true,
                        "locked" => {
                            w.is_locked = true;
                            w.lock_reason = value.map(str::to_string);
                        }
                        "prunable" => w.is_missing = true,
                        _ => {}
                    }
                }
            }
        }
        all.extend(current);
        all
    }
}

// MARK: Commands

/// Runs `git`. Reads pass `--no-optional-locks`, so we never touch the index while the user
/// (or an agent) works in the repo; writes (worktree add/remove) take git's locks as usual.
/// Every call blocks, 15 s at most.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Git {
    pub cli: Cli,
}

const GIT_TIMEOUT: Duration = Duration::from_secs(15);

impl Git {
    pub fn new(path_env: Option<&str>) -> Git {
        Git { cli: Cli::new("git", path_env) }
    }

    pub fn run(&self, args: &[&str], repo: &Path, writes: bool) -> CommandOutput {
        self.run_with(args, repo, writes, &[])
    }

    /// `extra`: variables on top of the usual ones (`GIT_INDEX_FILE`, say).
    pub fn run_with(&self, args: &[&str], repo: &Path, writes: bool, extra: &[(&str, &str)]) -> CommandOutput {
        let mut all: Vec<String> = vec![];
        if !writes {
            all.push("--no-optional-locks".into());
        }
        all.push("-C".into());
        all.push(repo.to_string_lossy().into_owned());
        all.extend(args.iter().map(|a| a.to_string()));
        self.cli.run(&all, Some(repo), GIT_TIMEOUT, extra)
    }

    pub fn status(&self, repo: &Path) -> Option<RepoStatus> {
        let out = self.run(&["status", "--porcelain=v2", "--branch", "--show-stash", "--untracked-files=normal"], repo, false);
        out.succeeded().then(|| RepoStatus::parse(&out.stdout))
    }

    pub fn origin_url(&self, repo: &Path) -> Option<String> {
        let out = self.run(&["config", "--get", "remote.origin.url"], repo, false);
        let url = out.stdout.trim();
        (out.succeeded() && !url.is_empty()).then(|| url.to_string())
    }

    pub fn origin(&self, repo: &Path) -> Option<GitRemote> {
        self.origin_url(repo).and_then(|u| GitRemote::parse(&u))
    }

    /// Every checkout of the repo, the main one first.
    pub fn worktrees(&self, repo: &Path) -> Option<Vec<Worktree>> {
        let out = self.run(&["worktree", "list", "--porcelain"], repo, false);
        out.succeeded().then(|| Worktree::parse(&out.stdout))
    }

    /// The branch work lands in: the one `origin/HEAD` names (local `main` when you have it,
    /// else `origin/main`), or `main` or `master` without a remote.
    pub fn default_branch(&self, repo: &Path) -> Option<String> {
        let remote = self.run(&["symbolic-ref", "--short", "refs/remotes/origin/HEAD"], repo, false);
        if remote.succeeded() {
            let name = remote.stdout.trim();
            let local = name.strip_prefix("origin/").unwrap_or(name);
            if !local.is_empty() && self.branch_exists(local, repo) {
                return Some(local.to_string());
            }
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
        ["main", "master"].into_iter().find(|n| self.branch_exists(n, repo)).map(str::to_string)
    }

    /// The commits on `base`'s own line (first parents only), up to `limit`. A branch
    /// pointing at one of these has no commits of its own; a merged branch's commits came in
    /// through a merge's second parent, so they aren't here.
    pub fn main_line(&self, base: &str, repo: &Path, limit: usize) -> HashSet<String> {
        let max = format!("--max-count={limit}");
        let out = self.run(&["rev-list", "--first-parent", &max, base], repo, false);
        if !out.succeeded() {
            return HashSet::new();
        }
        out.stdout.lines().map(str::to_string).collect()
    }

    /// Whether a branch, remote branch or tag holds `oid`: if not, a detached worktree's
    /// commits are only reachable from its HEAD, and removing it loses them.
    pub fn is_reachable(&self, oid: &str, repo: &Path) -> bool {
        let out = self.run(&["for-each-ref", "--contains", oid, "--count=1", "--format=%(refname)", "refs/heads", "refs/remotes", "refs/tags"], repo, false);
        out.succeeded() && !out.stdout.trim().is_empty()
    }

    /// Local branches, most recently committed to first.
    pub fn local_branches(&self, repo: &Path) -> Vec<String> {
        let out = self.run(&["for-each-ref", "--sort=-committerdate", "--format=%(refname:short)", "refs/heads"], repo, false);
        if !out.succeeded() {
            return vec![];
        }
        out.stdout.lines().map(str::to_string).collect()
    }

    pub fn branch_exists(&self, branch: &str, repo: &Path) -> bool {
        self.run(&["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")], repo, false).succeeded()
    }

    /// Local branches whose tip is already in `base` (by merge or fast-forward; a squash
    /// merge doesn't show here, a deleted remote branch usually does).
    pub fn branches_merged(&self, base: &str, repo: &Path) -> HashSet<String> {
        let out = self.run(&["branch", "--merged", base, "--format=%(refname:short)"], repo, false);
        if !out.succeeded() {
            return HashSet::new();
        }
        out.stdout.lines().map(str::to_string).collect()
    }

    /// `git worktree add`: a new checkout at `path`, on a new branch from `base`, or on an
    /// existing branch. Git's error when it fails.
    pub fn add_worktree(&self, repo: &Path, path: &Path, new_branch: Option<&str>, base: Option<&str>, existing_branch: Option<&str>) -> Result<(), String> {
        let path = path.to_string_lossy();
        let mut args = vec!["worktree", "add"];
        if let Some(b) = new_branch {
            args.extend(["-b", b]);
        }
        args.push(&path);
        if let Some(b) = existing_branch.or(base) {
            args.push(b);
        }
        Self::result(self.run(&args, repo, true))
    }

    /// `git worktree remove`; `force` also drops uncommitted changes.
    pub fn remove_worktree(&self, worktree: &Worktree, repo: &Path, force: bool) -> Result<(), String> {
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(&worktree.path);
        Self::result(self.run(&args, repo, true))
    }

    /// Forgets worktrees whose folders are gone.
    pub fn prune_worktrees(&self, repo: &Path) -> Result<(), String> {
        Self::result(self.run(&["worktree", "prune"], repo, true))
    }

    fn result(out: CommandOutput) -> Result<(), String> {
        if out.succeeded() {
            return Ok(());
        }
        let text = out.stderr.trim();
        Err(if text.is_empty() { "git exited with an error.".into() } else { text.replace("fatal: ", "") })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn parses_remotes() {
        for url in ["git@github.com:acme/api.git", "https://github.com/acme/api.git", "https://ghp_secret@github.com/acme/api", "ssh://git@github.com:22/acme/api.git"] {
            let r = GitRemote::parse(url).unwrap_or_else(|| panic!("{url}"));
            assert_eq!((r.host.as_str(), r.path.as_str(), r.forge), ("github.com", "acme/api", Some(Forge::GitHub)), "{url}");
            assert_eq!((r.owner(), r.name()), ("acme", "api"));
        }
        let r = GitRemote::parse("https://user:token123@gitlab.example.com/group/sub/repo.git").unwrap();
        assert_eq!(r.forge, Some(Forge::GitLab));
        assert_eq!((r.path.as_str(), r.owner()), ("group/sub/repo", "group/sub"));
        assert!(!format!("{r:?}").contains("token123"));
        assert_eq!(GitRemote::parse("/Users/me/repos/api"), None);
        assert_eq!(GitRemote::parse("../api"), None);
        assert_eq!(GitRemote::parse("https://Git.Example.com/a/b").unwrap().forge, None);
    }

    #[test]
    fn parses_status() {
        let output = "# branch.oid 1234abcd\n# branch.head feature/x\n# branch.upstream origin/feature/x\n# branch.ab +2 -1\n# stash 3\n\
            1 M. N... 100644 100644 100644 aaa bbb Sources/A.swift\n1 .M N... 100644 100644 100644 aaa bbb Sources/B.swift\n\
            1 MM N... 100644 100644 100644 aaa bbb Sources/C.swift\n2 R. N... 100644 100644 100644 aaa bbb R100 New.swift\tOld.swift\n\
            u UU N... 100644 100644 100644 100644 aaa bbb ccc Conflict.swift\n? notes.txt\n? scratch/\n";
        let s = RepoStatus::parse(output);
        assert_eq!(s.branch.as_deref(), Some("feature/x"));
        assert_eq!(s.upstream.as_deref(), Some("origin/feature/x"));
        assert_eq!((s.ahead, s.behind, s.staged, s.modified, s.conflicted, s.untracked, s.stashes), (2, 1, 3, 2, 1, 2, 3));
        assert!(s.is_dirty() && !s.upstream_gone);
        let d = RepoStatus::parse("# branch.oid abc\n# branch.head (detached)\n");
        assert_eq!((d.branch.as_deref(), d.head_oid.as_deref(), d.is_dirty()), (None, Some("abc"), false));
        assert!(RepoStatus::parse("# branch.oid abc\n# branch.head feat\n# branch.upstream origin/feat\n").upstream_gone);
        assert!(!RepoStatus::parse("# branch.oid abc\n# branch.head local\n").upstream_gone);
    }

    #[test]
    fn parses_worktree_list() {
        let output = "worktree /code/api\nHEAD 1111111111111111111111111111111111111111\nbranch refs/heads/main\n\n\
            worktree /code/api-login\nHEAD 2222222222222222222222222222222222222222\nbranch refs/heads/feature/login\nlocked moving disks\n\n\
            worktree /code/api-spike\nHEAD 3333333333333333333333333333333333333333\ndetached\n\n\
            worktree /code/api-gone\nHEAD 4444444444444444444444444444444444444444\nbranch refs/heads/old\nprunable gitdir file points to non-existent location\n\n";
        let w = Worktree::parse(output);
        assert_eq!(w.iter().map(|w| w.path.as_str()).collect::<Vec<_>>(), ["/code/api", "/code/api-login", "/code/api-spike", "/code/api-gone"]);
        assert!(w[0].is_main && w[0].branch.as_deref() == Some("main"));
        assert!(!w[1].is_main && w[1].branch.as_deref() == Some("feature/login"));
        assert!(w[1].is_locked && w[1].lock_reason.as_deref() == Some("moving disks"));
        assert!(w[2].is_detached() && w[2].head_oid.as_deref().unwrap().starts_with("3333"));
        assert!(w[3].is_missing);
    }

    pub(crate) fn test_git() -> Git {
        let mut git = Git::new(Some("/usr/bin:/bin:/opt/homebrew/bin"));
        for (k, v) in [("GIT_AUTHOR_NAME", "t"), ("GIT_AUTHOR_EMAIL", "t@t"), ("GIT_COMMITTER_NAME", "t"), ("GIT_COMMITTER_EMAIL", "t@t"), ("GIT_CONFIG_GLOBAL", "/dev/null"), ("GIT_CONFIG_NOSYSTEM", "1")] {
            git.cli = git.cli.with_env(k, v);
        }
        git
    }

    pub(crate) fn scratch(name: &str) -> PathBuf {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = crate::footprint::resolve(&std::env::temp_dir().join(format!("yh-{name}-{n}")).to_string_lossy(), "/").unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        PathBuf::from(dir)
    }

    #[test]
    fn real_worktrees() {
        let root = scratch("wt");
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = test_git();
        assert!(git.run(&["init", "-q", "-b", "main"], &repo, true).succeeded());
        assert!(git.run(&["commit", "-q", "--allow-empty", "-m", "one"], &repo, true).succeeded());
        git.add_worktree(&repo, &root.join("feature"), Some("feature"), Some("main"), None).unwrap();
        let spike = root.join("spike");
        assert!(git.run(&["worktree", "add", "-q", "--detach", &spike.to_string_lossy()], &repo, true).succeeded());
        let list = git.worktrees(&repo).unwrap();
        assert_eq!(list.len(), 3);
        assert!(list.iter().any(|w| w.is_detached()));
        assert_eq!(git.default_branch(&repo).as_deref(), Some("main"));
        assert!(git.branches_merged("main", &repo).contains("feature"));
        assert_eq!(git.status(&repo).unwrap().branch.as_deref(), Some("main"));
        let feature = list.iter().find(|w| w.branch.as_deref() == Some("feature")).unwrap();
        git.remove_worktree(feature, &repo, false).unwrap();
        assert_eq!(git.worktrees(&repo).unwrap().len(), 2);
        assert!(git.remove_worktree(feature, &repo, false).is_err());

        // A fresh branch from main isn't work; a branch merged with a merge commit is merged.
        assert!(git.run(&["branch", "fresh"], &repo, true).succeeded());
        assert!(git.run(&["checkout", "-q", "-b", "work"], &repo, true).succeeded());
        assert!(git.run(&["commit", "-q", "--allow-empty", "-m", "work"], &repo, true).succeeded());
        assert!(git.run(&["checkout", "-q", "main"], &repo, true).succeeded());
        assert!(git.run(&["merge", "-q", "--no-ff", "work", "-m", "merge"], &repo, true).succeeded());
        let line = git.main_line("main", &repo, 5000);
        let rev = |b: &str| git.run(&["rev-parse", b], &repo, false).stdout.trim().to_string();
        assert!(git.branches_merged("main", &repo).is_superset(&["fresh".to_string(), "work".to_string()].into()));
        assert!(line.contains(&rev("fresh")) && !line.contains(&rev("work")));
        assert!(git.is_reachable(&rev("work"), &repo));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn times_out() {
        let out = Cli::new("/bin/sleep", None).run(&["5".into()], None, Duration::from_millis(100), &[]);
        assert!(out.timed_out && !out.succeeded());
        assert!(Cli::new("/no/such/thing", None).run(&[], None, GIT_TIMEOUT, &[]).launch_failed);
    }
}
