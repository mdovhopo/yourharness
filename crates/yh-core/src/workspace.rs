//! A project's repos, worktrees and open pull requests: what the project overview and a
//! thread's checkouts and PRs are drawn from. Everything is discovered; nothing is written to
//! the project but what the user asks for (worktree add/remove, stack commands).
//!
//! `scan` finds the repos in a folder (`WorkspaceLayout`). `Workspace` holds what's known
//! about them and refreshes it: `refresh` (scan when needed, then both of the others),
//! `refresh_status` (branches, changes, worktrees, merged branches; many `git` calls, 8 at a
//! time, usually well under a second, 15 s at worst per call) and `refresh_pulls` (one GraphQL
//! query per 20 repos per forge host, 3 at a time; a second or two, 30 s at worst). They
//! block: the app runs them on a background thread, on a copy or behind a lock, and they
//! throttle themselves, so calling them often is cheap. The pieces also exist as plain
//! functions (`read_remotes`, `read_status`, `read_pulls`) taking what they need and
//! returning what they found, for a caller that keeps its own state.

pub use crate::footprint::real_path;
use crate::footprint;
use crate::forge::PullRequest;
use crate::git::{Cli, Forge, Git, GitRemote, RepoStatus, Worktree};
use crate::github::{ForgeSnapshot, GitHubClient, GitHubStacks, LocalStack};
use crate::gitlab::GitLabClient;
use crate::stacks::PullRequestStack;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

// MARK: Scanning

/// A repo inside a project folder: the project's own repo, a nested clone, or one the
/// workspace references (compose `build.context`) but hasn't cloned yet.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WorkspaceRepo {
    /// Relative to the project root; empty for the root repo itself.
    pub relative_path: String,
    pub path: PathBuf,
    pub is_cloned: bool,
}

impl WorkspaceRepo {
    pub fn id(&self) -> &str {
        &self.relative_path
    }

    pub fn is_root(&self) -> bool {
        self.relative_path.is_empty()
    }

    pub fn name(&self) -> String {
        if self.is_root() {
            self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            self.relative_path.clone()
        }
    }

    /// Its folder as git reports it (`/tmp` is `/private/tmp`).
    pub fn real_path(&self) -> String {
        real_path(&self.path.to_string_lossy())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutKind {
    /// No git anywhere we looked.
    Plain,
    /// One repo, nothing nested.
    Repo,
    /// A plain folder holding repos.
    Folder,
    /// A repo whose job is to hold other repos (gitignored inside it).
    Shell,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceLayout {
    pub root: PathBuf,
    pub kind: LayoutKind,
    /// Root repo first (if any), then nested repos by path, then uncloned ones.
    pub repos: Vec<WorkspaceRepo>,
}

impl WorkspaceLayout {
    pub fn is_workspace(&self) -> bool {
        matches!(self.kind, LayoutKind::Folder | LayoutKind::Shell)
    }

    pub fn cloned_repos(&self) -> Vec<&WorkspaceRepo> {
        self.repos.iter().filter(|r| r.is_cloned).collect()
    }
}

/// Folders that never contain repos worth showing, and can be enormous.
const SKIPPED: &[&str] = &[
    "node_modules", "vendor", "Pods", "Carthage", "DerivedData", "build", "dist", "out", "target", "venv", "__pycache__", "bower_components", "Library",
    "Applications",
];

/// Folders macOS shows as one file (bundles): never looked into.
const PACKAGES: &[&str] = &["app", "bundle", "framework", "xcodeproj", "xcworkspace", "playground", "photoslibrary", "plugin", "kext", "pkg", "rtfd", "xcassets"];

pub const MAX_DEPTH: usize = 4;
pub const MAX_DIRECTORIES: usize = 5_000;

/// Walks `root` looking for nested repos, `MAX_DEPTH` folders down and `MAX_DIRECTORIES`
/// folders at most, so pointing it at a huge tree stays cheap. Blocks on the file system.
pub fn scan(root: &Path) -> WorkspaceLayout {
    scan_bounded(root, MAX_DEPTH, MAX_DIRECTORIES)
}

pub fn scan_bounded(root: &Path, max_depth: usize, max_directories: usize) -> WorkspaceLayout {
    let root = PathBuf::from(footprint::resolve(&root.to_string_lossy(), "/").unwrap_or_else(|| root.to_string_lossy().into_owned()));
    let root_is_repo = is_repo(&root);
    let mut nested: Vec<WorkspaceRepo> = vec![];
    let mut queue: std::collections::VecDeque<(PathBuf, usize)> = [(root.clone(), 0)].into();
    let mut visited = 0;
    while let Some((dir, depth)) = queue.pop_front() {
        if visited >= max_directories {
            break;
        }
        visited += 1;
        if depth >= max_depth {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut children: Vec<(String, PathBuf)> = entries
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let kind = e.file_type().ok()?;
                // Hidden, links, files, bundles and the enormous folders aren't looked in.
                let package = name.rsplit_once('.').is_some_and(|(_, ext)| PACKAGES.contains(&ext));
                (!name.starts_with('.') && kind.is_dir() && !kind.is_symlink() && !package && !SKIPPED.contains(&name.as_str())).then(|| (name, e.path()))
            })
            .collect();
        children.sort();
        for (_, child) in children {
            if is_repo(&child) {
                nested.push(WorkspaceRepo { relative_path: relative_path(&child, &root), path: child, is_cloned: true });
            } else {
                queue.push_back((child, depth + 1));
            }
        }
    }

    let cloned: Vec<&str> = nested.iter().map(|r| r.relative_path.as_str()).collect();
    let mut uncloned: Vec<WorkspaceRepo> = compose_build_contexts(&root)
        .into_iter()
        // Skip contexts inside a cloned repo, or folders the shell owns.
        .filter(|p| !cloned.iter().any(|c| p == c || p.starts_with(&format!("{c}/"))) && is_empty_or_missing(&root.join(p)))
        .map(|p| WorkspaceRepo { path: root.join(&p), relative_path: p, is_cloned: false })
        .collect();

    let kind = match (root_is_repo, nested.is_empty()) {
        (true, true) => LayoutKind::Repo,
        (true, false) => LayoutKind::Shell,
        (false, false) => LayoutKind::Folder,
        (false, true) => LayoutKind::Plain,
    };
    let mut repos = vec![];
    if kind != LayoutKind::Plain {
        if root_is_repo {
            repos.push(WorkspaceRepo { relative_path: String::new(), path: root.clone(), is_cloned: true });
        }
        nested.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        uncloned.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        repos.extend(nested);
        repos.extend(uncloned);
    }
    WorkspaceLayout { root, kind, repos }
}

/// `.git` is a folder in a normal clone and a file in a worktree or submodule.
pub fn is_repo(dir: &Path) -> bool {
    dir.join(".git").exists()
}

fn relative_path(path: &Path, root: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) => rel.to_string_lossy().trim_matches('/').to_string(),
        Err(_) => path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
    }
}

fn is_empty_or_missing(dir: &Path) -> bool {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries.filter_map(Result::ok).all(|e| e.file_name().to_string_lossy().starts_with('.')),
        Err(_) => true,
    }
}

const COMPOSE_FILES: &[&str] = &["compose.yaml", "compose.yml", "docker-compose.yaml", "docker-compose.yml"];

/// Local build contexts from compose files at the root, sorted, without repeats.
pub fn compose_build_contexts(root: &Path) -> Vec<String> {
    let mut all: Vec<String> = COMPOSE_FILES.iter().filter_map(|n| std::fs::read_to_string(root.join(n)).ok()).flat_map(|t| parse_build_contexts(&t)).collect();
    all.sort();
    all.dedup();
    all
}

/// Line-based on purpose: a YAML parser isn't worth a dependency, and a missed entry only
/// hides an uncloned repo.
pub fn parse_build_contexts(compose: &str) -> Vec<String> {
    compose
        .lines()
        .filter_map(|raw| {
            let line = raw.trim();
            if line.starts_with('#') {
                return None;
            }
            let value = line.strip_prefix("context:").or_else(|| line.strip_prefix("build:"))?;
            let path = value.split('#').next().unwrap_or("").trim().trim_matches(['"', '\'']);
            normalize(path)
        })
        .collect()
}

/// Keeps relative paths that stay inside the root; drops URLs, variables and `.`.
pub fn normalize(path: &str) -> Option<String> {
    if path.is_empty() || path.starts_with('/') || path.contains("://") || path.contains('$') || path.starts_with('{') {
        return None;
    }
    let mut parts = vec![];
    for part in path.split('/').filter(|p| !p.is_empty()) {
        match part {
            "." => {}
            ".." => return None,
            p => parts.push(p),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

// MARK: Tools

/// The CLIs a refresh drives, each only when it's on the PATH.
#[derive(Clone, Debug, Default)]
pub struct WorkspaceTools {
    pub git: Option<Git>,
    pub github: Option<GitHubClient>,
    pub gitlab: Option<GitLabClient>,
    /// `gh stack`, when gh is there; whether the extension is installed is asked separately.
    pub stacks: Option<GitHubStacks>,
}

impl WorkspaceTools {
    /// Looks for `git`, `gh` and `glab` on `path_env` (else our PATH).
    pub fn find(path_env: Option<&str>) -> WorkspaceTools {
        let gh = Cli::find("gh", path_env);
        WorkspaceTools {
            git: Cli::find("git", path_env).map(|cli| Git { cli }),
            github: gh.clone().map(|cli| GitHubClient { cli }),
            gitlab: Cli::find("glab", path_env).map(|cli| GitLabClient { cli }),
            stacks: gh.map(|cli| GitHubStacks { cli }),
        }
    }
}

// MARK: Reading

/// Each cloned repo's `origin`, by repo ID. Blocks: a `git config` per repo, 8 at a time.
pub fn read_remotes(layout: &WorkspaceLayout, git: &Git) -> HashMap<String, GitRemote> {
    let cloned: Vec<&WorkspaceRepo> = layout.cloned_repos();
    par_map(&cloned, 8, |r| (r.id().to_string(), git.origin(&r.path))).into_iter().filter_map(|(id, r)| r.map(|r| (id, r))).collect()
}

/// What git says about a workspace's checkouts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatusSnapshot {
    /// By repo ID.
    pub statuses: HashMap<String, RepoStatus>,
    /// Each repo's linked worktrees (not its main checkout), by path, by repo ID.
    pub worktrees: HashMap<String, Vec<Worktree>>,
    /// By worktree path.
    pub worktree_statuses: HashMap<String, RepoStatus>,
    /// Per repo: its default branch, the local branches already in it, and its own line of
    /// commits (a branch pointing at one of them has no work of its own).
    pub default_branches: HashMap<String, String>,
    pub merged_branches: HashMap<String, HashSet<String>>,
    pub main_lines: HashMap<String, HashSet<String>>,
    /// Detached worktrees, by path: whether some branch also holds their commit.
    pub detached_reachable: HashMap<String, bool>,
    /// The stacks `gh stack` keeps in each repo, by repo ID: branches not submitted yet show too.
    pub local_stacks: HashMap<String, Vec<LocalStack>>,
}

/// Reads every cloned repo's status, worktrees and merged branches. A status that doesn't
/// come back (git busy, slow) keeps `previous`'s: dropped, a repo's branch and its PRs would
/// vanish until the next refresh. Blocks: several `git` calls per repo and worktree, 8 at a time.
pub fn read_status(layout: &WorkspaceLayout, git: &Git, previous: &StatusSnapshot) -> StatusSnapshot {
    let cloned: Vec<&WorkspaceRepo> = layout.cloned_repos();
    let mut snap = StatusSnapshot::default();
    for (id, status) in par_map(&cloned, 8, |r| (r.id().to_string(), git.status(&r.path))) {
        if let Some(s) = status.or_else(|| previous.statuses.get(&id).cloned()) {
            snap.statuses.insert(id, s);
        }
    }

    // Worktrees: each repo's list, the status of each one, and which branches are merged.
    let lists = par_map(&cloned, 8, |r| {
        let linked: Vec<Worktree> = git.worktrees(&r.path).unwrap_or_default().into_iter().filter(|w| !w.is_main && !w.is_bare).collect();
        (r.id().to_string(), r.real_path(), linked, git.default_branch(&r.path))
    });
    // A repo listed as a linked worktree of another (it's nested in the project) is that
    // one's worktree: only its main repo lists it.
    let linked_paths: HashSet<String> = lists.iter().flat_map(|(_, _, l, _)| l.iter().map(|w| w.path.clone())).collect();
    for (id, real, mut linked, base) in lists {
        if linked_paths.contains(&real) {
            continue;
        }
        if !linked.is_empty() {
            linked.sort_by(|a, b| a.path.cmp(&b.path));
            snap.worktrees.insert(id.clone(), linked);
        }
        if let Some(b) = base {
            snap.default_branches.insert(id, b);
        }
    }
    let path_of = |id: &str| cloned.iter().find(|r| r.id() == id).map(|r| r.path.clone()).unwrap_or_default();
    let bases: Vec<(String, String)> = snap.default_branches.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (id, merged, line) in par_map(&bases, 8, |(id, base)| {
        let path = path_of(id);
        (id.clone(), git.branches_merged(base, &path), git.main_line(base, &path, 5000))
    }) {
        snap.merged_branches.insert(id.clone(), merged);
        snap.main_lines.insert(id, line);
    }
    let present: Vec<Worktree> = snap.worktrees.values().flatten().filter(|w| !w.is_missing).cloned().collect();
    for (path, status) in par_map(&present, 8, |w| (w.path.clone(), git.status(Path::new(&w.path)))) {
        if let Some(s) = status.or_else(|| previous.worktree_statuses.get(&path).cloned()) {
            snap.worktree_statuses.insert(path, s);
        }
    }
    let detached: Vec<(String, Worktree)> =
        snap.worktrees.iter().flat_map(|(id, list)| list.iter().filter(|w| w.is_detached() && !w.is_missing).map(|w| (id.clone(), w.clone()))).collect();
    snap.detached_reachable = par_map(&detached, 8, |(id, w)| (w.path.clone(), w.head_oid.as_deref().is_some_and(|oid| git.is_reachable(oid, &path_of(id))))).into_iter().collect();
    snap.local_stacks = cloned.iter().map(|r| (r.id().to_string(), LocalStack::read(&r.path))).filter(|(_, s)| !s.is_empty()).collect();
    snap
}

/// What the forges said about a workspace's repos.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PullsSnapshot {
    /// Open PRs, most recently updated first.
    pub pull_requests: Vec<PullRequest>,
    pub errors: BTreeMap<Forge, String>,
    /// The signed-in user per forge, as its CLI said.
    pub accounts: BTreeMap<Forge, String>,
    pub unreachable: HashSet<GitRemote>,
}

/// Every repo's open PRs, one batched GraphQL query per forge host (GitHub and GitLab at once).
/// A forge that fails outright (offline) keeps `previous`'s PRs, rather than blanking the
/// list. `YH_MOCK_PULLS` answers from its file instead (the open PRs of these repos). Blocks:
/// a second or two, 30 s at worst. None when there's nothing to ask.
pub fn read_pulls(remotes: &HashMap<String, GitRemote>, tools: &WorkspaceTools, previous: &[PullRequest]) -> Option<PullsSnapshot> {
    if remotes.is_empty() {
        return None;
    }
    if let Some(mock) = crate::forge::mock_pulls() {
        let mut pulls: Vec<PullRequest> = mock.unwrap_or_default().into_iter().filter(|p| p.is_open() && remotes.values().any(|r| p.is_in(r))).collect();
        pulls.sort_by(|a, b| b.updated_seconds().total_cmp(&a.updated_seconds()));
        return Some(PullsSnapshot { pull_requests: pulls, ..Default::default() });
    }
    let mut by_forge: BTreeMap<Forge, Vec<GitRemote>> = BTreeMap::new();
    for remote in remotes.values() {
        if let Some(f) = remote.forge {
            by_forge.entry(f).or_default().push(remote.clone());
        }
    }
    if by_forge.is_empty() {
        return None;
    }
    let missing = |forge: Forge| {
        let mut s = ForgeSnapshot::new(forge);
        s.error = Some(format!("Install `{}` to see {}s from {}.", forge.cli_name(), forge.request_noun(), forge.display_name()));
        s
    };
    let (github, gitlab) = std::thread::scope(|scope| {
        let gh = by_forge.get(&Forge::GitHub).map(|r| scope.spawn(|| tools.github.as_ref().map(|c| c.fetch(r)).unwrap_or_else(|| missing(Forge::GitHub))));
        let gl = by_forge.get(&Forge::GitLab).map(|r| scope.spawn(|| tools.gitlab.as_ref().map(|c| c.fetch(r)).unwrap_or_else(|| missing(Forge::GitLab))));
        (gh.and_then(|h| h.join().ok()), gl.and_then(|h| h.join().ok()))
    });
    let snapshots: Vec<ForgeSnapshot> = [github, gitlab].into_iter().flatten().collect();
    let mut out = PullsSnapshot::default();
    for s in &snapshots {
        if let Some(e) = &s.error {
            out.errors.insert(s.forge, e.clone());
        }
        if let Some(a) = s.accounts.values().next() {
            out.accounts.insert(s.forge, a.clone());
        }
        out.pull_requests.extend(s.pull_requests.iter().cloned());
        out.unreachable.extend(s.unreachable.iter().cloned());
    }
    let failed: HashSet<Forge> = out.errors.keys().copied().filter(|f| !snapshots.iter().any(|s| s.forge == *f && !s.pull_requests.is_empty())).collect();
    out.pull_requests.extend(previous.iter().filter(|p| failed.contains(&if p.link.gitlab { Forge::GitLab } else { Forge::GitHub })).cloned());
    out.pull_requests.sort_by(|a, b| b.updated_seconds().total_cmp(&a.updated_seconds()));
    Some(out)
}

// MARK: The workspace

/// A checkout a thread works in: a repo's main one (worktree None) or a linked worktree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkout {
    pub repo: WorkspaceRepo,
    pub worktree: Option<Worktree>,
}

impl Checkout {
    pub fn id(&self) -> String {
        self.worktree.as_ref().map(|w| w.path.clone()).unwrap_or_else(|| self.repo.id().to_string())
    }

    pub fn path(&self) -> String {
        self.worktree.as_ref().map(|w| w.path.clone()).unwrap_or_else(|| self.repo.real_path())
    }
}

/// A thread as far as checkouts go: the folders it touched (`footprint::folders`), the ones
/// it changed something in (`footprint::written_folders`), where it runs, and when it was
/// last active (for ordering, newest first).
#[derive(Clone, Copy, Debug)]
pub struct ThreadFolders<'a> {
    pub id: &'a str,
    pub footprint: &'a [String],
    pub written: &'a [String],
    pub working_directory: Option<&'a str>,
    pub last_active: f64,
}

/// What removing a worktree would cost: nothing (just do it), work that stays on its branch
/// (ask), or work that's lost (ask, and say so).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemovalRisk {
    None,
    KeepsWork(String),
    LosesWork(String),
}

/// PR stacks split into what needs you, what's yours, and everything else.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PullSections {
    pub needs_review: Vec<PullRequestStack>,
    pub mine: Vec<PullRequestStack>,
    pub others: Vec<PullRequestStack>,
}

/// One project's repos and their open PRs, and what's worked out from them.
#[derive(Clone, Debug)]
pub struct Workspace {
    pub root: PathBuf,
    pub layout: Option<WorkspaceLayout>,
    /// By repo ID.
    pub remotes: HashMap<String, GitRemote>,
    pub status: StatusSnapshot,
    pub pulls: PullsSnapshot,
    pub status_updated_at: Option<Instant>,
    pub pulls_updated_at: Option<Instant>,
    /// The last scan knew where git is, so it read the repos' remotes.
    remotes_loaded: bool,
}

impl Workspace {
    /// `root` is resolved first: git names worktrees by their real paths, so a project reached
    /// through a symlink would match none of them.
    pub fn new(root: &Path) -> Workspace {
        Workspace {
            root: std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()),
            layout: None,
            remotes: HashMap::new(),
            status: StatusSnapshot::default(),
            pulls: PullsSnapshot::default(),
            status_updated_at: None,
            pulls_updated_at: None,
            remotes_loaded: false,
        }
    }

    /// Rescans the folder when asked or on first use, then refreshes status and, when older
    /// than `pulls_max_age`, PRs (both at once). Blocks; each part throttles itself.
    pub fn refresh(&mut self, tools: &WorkspaceTools, rescan: bool, pulls_max_age: Duration) {
        // Also when an early scan ran before git was found: its repos have no remotes yet.
        if self.layout.is_none() || rescan || (!self.remotes_loaded && tools.git.is_some()) {
            self.scan(tools);
        }
        let (layout, remotes) = (self.layout.clone(), self.remotes.clone());
        let status_due = tools.git.is_some() && self.status_updated_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(3));
        let pulls_due = self.pulls_updated_at.is_none_or(|t| t.elapsed() >= pulls_max_age);
        let (previous_status, previous_pulls) = (&self.status, &self.pulls.pull_requests);
        let (status, pulls) = std::thread::scope(|scope| {
            let status = status_due.then(|| scope.spawn(|| Some(read_status(layout.as_ref()?, tools.git.as_ref()?, previous_status))));
            let pulls = if pulls_due { read_pulls(&remotes, tools, previous_pulls) } else { None };
            (status.and_then(|h| h.join().ok().flatten()), pulls)
        });
        if let Some(s) = status {
            self.status = s;
            self.status_updated_at = Some(Instant::now());
        }
        // Nothing to match until the repos' remotes are known: then it's asked again.
        if let Some(p) = pulls {
            self.pulls = p;
            self.pulls_updated_at = Some(Instant::now());
        }
    }

    /// Finds the repos, and their remotes when git is there. Blocks.
    pub fn scan(&mut self, tools: &WorkspaceTools) {
        let layout = scan(&self.root);
        if let Some(git) = &tools.git {
            self.remotes = read_remotes(&layout, git);
            self.remotes_loaded = true;
        }
        self.layout = Some(layout);
    }

    /// Status, worktrees and merged branches, unless read less than `min_interval` ago.
    pub fn refresh_status(&mut self, tools: &WorkspaceTools, min_interval: Duration) {
        let Some(git) = &tools.git else { return };
        if self.status_updated_at.is_some_and(|t| t.elapsed() < min_interval) {
            return;
        }
        let Some(layout) = &self.layout else { return };
        self.status = read_status(layout, git, &self.status);
        self.status_updated_at = Some(Instant::now());
    }

    /// Open PRs, unless read less than `max_age` ago (`Duration::ZERO`: now).
    pub fn refresh_pulls(&mut self, tools: &WorkspaceTools, max_age: Duration) {
        if self.pulls_updated_at.is_some_and(|t| t.elapsed() < max_age) {
            return;
        }
        // Nothing to match until the repos' remotes are known.
        if let Some(p) = read_pulls(&self.remotes, tools, &self.pulls.pull_requests) {
            self.pulls = p;
            self.pulls_updated_at = Some(Instant::now());
        }
    }

    pub fn repos(&self) -> &[WorkspaceRepo] {
        self.layout.as_ref().map(|l| l.repos.as_slice()).unwrap_or(&[])
    }

    /// Cloned repos; linked worktrees inside the project are listed under their repo, not as repos.
    pub fn cloned_repos(&self) -> Vec<&WorkspaceRepo> {
        let linked: HashSet<&str> = self.status.worktrees.values().flatten().map(|w| w.path.as_str()).collect();
        self.repos().iter().filter(|r| r.is_cloned && !linked.contains(r.real_path().as_str())).collect()
    }

    pub fn uncloned_repos(&self) -> Vec<&WorkspaceRepo> {
        self.repos().iter().filter(|r| !r.is_cloned).collect()
    }

    pub fn all_worktrees(&self) -> Vec<&Worktree> {
        self.cloned_repos().into_iter().flat_map(|r| self.worktrees_of(r)).collect()
    }

    pub fn worktrees_of(&self, repo: &WorkspaceRepo) -> &[Worktree] {
        self.status.worktrees.get(repo.id()).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn has_forge_remotes(&self) -> bool {
        self.remotes.values().any(|r| r.forge.is_some())
    }

    pub fn pull_requests(&self) -> &[PullRequest] {
        &self.pulls.pull_requests
    }

    pub fn pull_requests_for(&self, repo: &WorkspaceRepo) -> Vec<&PullRequest> {
        let Some(remote) = self.remotes.get(repo.id()) else { return vec![] };
        self.pulls.pull_requests.iter().filter(|p| p.is_in(remote)).collect()
    }

    pub fn repo_for(&self, pull: &PullRequest) -> Option<&WorkspaceRepo> {
        self.repos().iter().find(|r| self.remotes.get(r.id()).is_some_and(|remote| pull.is_in(remote)))
    }

    /// The open PR from a worktree's branch.
    pub fn pull_request_for(&self, worktree: &Worktree, repo: &WorkspaceRepo) -> Option<&PullRequest> {
        let branch = worktree.branch.as_deref()?;
        self.pull_requests_for(repo).into_iter().find(|p| p.head == branch)
    }

    /// Its branch points at a commit of the default branch's own: nothing done in it yet.
    pub fn has_no_work(&self, worktree: &Worktree, repo: &WorkspaceRepo) -> bool {
        let (Some(_), Some(head)) = (&worktree.branch, &worktree.head_oid) else { return false };
        self.status.main_lines.get(repo.id()).is_some_and(|l| l.contains(head))
    }

    /// Done with: nothing uncommitted, no open PR, and its work has landed: its remote branch
    /// was deleted (what a merged PR usually does), or its own commits are in the default
    /// branch. A branch still where it started has no work yet, so it isn't done.
    pub fn is_finished(&self, worktree: &Worktree, repo: &WorkspaceRepo) -> bool {
        let Some(branch) = &worktree.branch else { return false };
        if worktree.is_missing || self.pull_request_for(worktree, repo).is_some() {
            return false;
        }
        let Some(status) = self.status.worktree_statuses.get(&worktree.path) else { return false };
        if status.is_dirty() {
            return false;
        }
        if status.upstream_gone {
            return true;
        }
        self.status.merged_branches.get(repo.id()).is_some_and(|m| m.contains(branch)) && !self.has_no_work(worktree, repo)
    }

    pub fn removal_risk(&self, worktree: &Worktree, repo: &WorkspaceRepo) -> RemovalRisk {
        if worktree.is_missing {
            return RemovalRisk::None;
        }
        let Some(status) = self.status.worktree_statuses.get(&worktree.path) else {
            return RemovalRisk::KeepsWork("Its state hasn't been read yet.".into());
        };
        if status.is_dirty() {
            let n = status.changed_files();
            return RemovalRisk::LosesWork(format!("It has {n} uncommitted change{}, which will be lost.", if n == 1 { "" } else { "s" }));
        }
        if worktree.is_detached() {
            return if self.status.detached_reachable.get(&worktree.path) == Some(&true) {
                RemovalRisk::None
            } else {
                RemovalRisk::LosesWork(
                    "It isn't on a branch, and its commits aren't on any branch either: they'd be lost. Make a branch in it first to keep them.".into(),
                )
            };
        }
        let branch = worktree.branch.clone().unwrap_or_default();
        if let Some(pr) = self.pull_request_for(worktree, repo) {
            return RemovalRisk::KeepsWork(format!("#{} is still open. The branch {branch} keeps its commits, so you can check it out again.", pr.number()));
        }
        if self.is_finished(worktree, repo) || self.has_no_work(worktree, repo) {
            return RemovalRisk::None;
        }
        RemovalRisk::KeepsWork(format!("{branch} has commits that aren't merged yet. The branch keeps them, so you can check it out again."))
    }

    /// A worktree git just removed, dropped from what's known at once: branches, merges and
    /// PRs don't change with it, so nothing is read again.
    pub fn forget_worktree(&mut self, worktree: &Worktree, repo: &WorkspaceRepo) {
        if let Some(list) = self.status.worktrees.get_mut(repo.id()) {
            list.retain(|w| w.path != worktree.path);
            if list.is_empty() {
                self.status.worktrees.remove(repo.id());
            }
        }
        self.status.worktree_statuses.remove(&worktree.path);
        self.status.detached_reachable.remove(&worktree.path);
        // One inside the project was also found by the scan, as a repo of its own.
        let Some(layout) = &mut self.layout else { return };
        let gone: Vec<String> = layout.repos.iter().filter(|r| r.real_path() == worktree.path).map(|r| r.id().to_string()).collect();
        layout.repos.retain(|r| r.real_path() != worktree.path);
        for id in gone {
            self.status.statuses.remove(&id);
            self.remotes.remove(&id);
        }
    }

    /// `git worktree prune` dropped the repo's worktrees whose folders are gone.
    pub fn forget_missing_worktrees(&mut self, repo: &WorkspaceRepo) {
        for w in self.worktrees_of(repo).iter().filter(|w| w.is_missing).cloned().collect::<Vec<_>>() {
            self.forget_worktree(&w, repo);
        }
    }

    fn all_checkouts(&self) -> Vec<Checkout> {
        self.cloned_repos()
            .into_iter()
            .flat_map(|r| {
                std::iter::once(Checkout { repo: r.clone(), worktree: None })
                    .chain(self.worktrees_of(r).iter().map(|w| Checkout { repo: r.clone(), worktree: Some(w.clone()) }))
            })
            .collect()
    }

    /// Checkout roots, longest first: the innermost holding a folder comes first.
    fn checkout_roots(&self) -> Vec<String> {
        let mut roots: Vec<String> = self.cloned_repos().iter().map(|r| r.real_path()).chain(self.all_worktrees().iter().map(|w| w.path.clone())).collect();
        roots.sort_by_key(|r| std::cmp::Reverse(r.len()));
        roots
    }

    /// Which threads work in which checkout, by checkout path, each list most recently active
    /// first: every folder a thread touched counts for the innermost checkout holding it, so a
    /// worktree nested in its repo's folder doesn't also count for the repo. `written`: only
    /// folders it changed something in.
    pub fn threads_by_checkout(&self, threads: &[ThreadFolders], written: bool) -> HashMap<String, Vec<String>> {
        let roots = self.checkout_roots();
        let mut result: HashMap<String, Vec<(f64, String)>> = HashMap::new();
        for t in threads {
            let folders: Vec<String> =
                if written { t.written.to_vec() } else { t.footprint.iter().cloned().chain(t.working_directory.map(real_path)).collect() };
            let mut hit: HashSet<&String> = HashSet::new();
            for folder in &folders {
                if let Some(root) = roots.iter().find(|r| footprint::touches(std::slice::from_ref(folder), r)) {
                    hit.insert(root);
                }
            }
            for root in hit {
                result.entry(root.clone()).or_default().push((t.last_active, t.id.to_string()));
            }
        }
        result
            .into_iter()
            .map(|(k, mut v)| {
                v.sort_by(|a, b| b.0.total_cmp(&a.0));
                (k, v.into_iter().map(|(_, id)| id).collect())
            })
            .collect()
    }

    /// For each thread that works here, the repos (main checkout paths) it works in.
    pub fn checkouts_by_thread(&self, threads: &[ThreadFolders]) -> HashMap<String, Vec<String>> {
        let mut repo_of: HashMap<String, String> = HashMap::new();
        for r in self.cloned_repos() {
            repo_of.entry(r.real_path()).or_insert_with(|| r.real_path());
            for w in self.worktrees_of(r) {
                repo_of.entry(w.path.clone()).or_insert_with(|| r.real_path());
            }
        }
        let mut result: HashMap<String, Vec<String>> = HashMap::new();
        for (checkout, users) in self.threads_by_checkout(threads, false) {
            let Some(repo) = repo_of.get(&checkout) else { continue };
            for id in users {
                result.entry(id).or_default().push(repo.clone());
            }
        }
        result
    }

    /// The checkouts one thread works in, the one it runs in first, then by path.
    pub fn checkouts_for(&self, thread: &ThreadFolders) -> Vec<Checkout> {
        let used = self.threads_by_checkout(std::slice::from_ref(thread), false);
        let running = thread.working_directory.map(real_path);
        let mut all: Vec<Checkout> = self.all_checkouts().into_iter().filter(|c| used.contains_key(&c.path()) || Some(c.path()) == running).collect();
        all.sort_by_key(|c| (Some(c.path()) != running, c.path()));
        all
    }

    /// The checkouts a thread changed something in: edited files, made commits, pushed.
    pub fn written_checkouts(&self, thread: &ThreadFolders) -> Vec<Checkout> {
        let wrote = self.threads_by_checkout(std::slice::from_ref(thread), true);
        self.all_checkouts().into_iter().filter(|c| wrote.contains_key(&c.path())).collect()
    }

    /// The checkout a folder is in: the innermost repo or worktree holding it.
    pub fn checkout_containing(&self, folder: &str) -> Option<Checkout> {
        let path = real_path(folder);
        self.all_checkouts().into_iter().filter(|c| footprint::touches(std::slice::from_ref(&path), &c.path())).max_by_key(|c| c.path().len())
    }

    /// A checkout's branch: the worktree's, or the main checkout's.
    pub fn branch_of(&self, checkout: &Checkout) -> Option<String> {
        match &checkout.worktree {
            Some(w) => w.branch.clone(),
            None => self.status.statuses.get(checkout.repo.id()).and_then(|s| s.branch.clone()),
        }
    }

    /// The open PRs, grouped into stacks and split into what needs you, what's yours, and the rest.
    pub fn pull_request_sections(&self) -> PullSections {
        let mut sections = PullSections::default();
        for stack in PullRequestStack::build(&self.pulls.pull_requests) {
            let members = stack.all();
            if members.iter().any(|p| p.review_requested_from_me) {
                sections.needs_review.push(stack);
            } else if members.iter().any(|p| p.is_mine) {
                sections.mine.push(stack);
            } else {
                sections.others.push(stack);
            }
        }
        sections
    }

    /// The local stack GitHub numbers `number` in the repo `remote` points at.
    pub fn local_stack(&self, number: u64, remote: &GitRemote) -> Option<&LocalStack> {
        let repo = self.repos().iter().find(|r| self.remotes.get(r.id()) == Some(remote))?;
        self.status.local_stacks.get(repo.id())?.iter().find(|s| s.number == Some(number))
    }

    /// Stacks made with `gh stack` that have a branch without a PR yet, by repo.
    pub fn unsubmitted_stacks(&self) -> Vec<(&WorkspaceRepo, &LocalStack)> {
        self.cloned_repos()
            .into_iter()
            .flat_map(|r| {
                self.status.local_stacks.get(r.id()).into_iter().flatten().filter(|s| s.layers.iter().any(|l| l.pull_request_number.is_none())).map(move |s| (r, s))
            })
            .collect()
    }

    /// A checkout (main or worktree) of the repo that's on one of `branches`: where `gh stack`
    /// knows which stack is meant.
    pub fn checkout_on_any_of(&self, repo: &WorkspaceRepo, branches: &[String]) -> Option<PathBuf> {
        if self.status.statuses.get(repo.id()).and_then(|s| s.branch.as_ref()).is_some_and(|b| branches.contains(b)) {
            return Some(repo.path.clone());
        }
        self.worktrees_of(repo).iter().find(|w| !w.is_missing && w.branch.as_ref().is_some_and(|b| branches.contains(b))).map(|w| PathBuf::from(&w.path))
    }
}

/// `f` over `items` on up to `limit` threads at once, results in the items' order. Blocks.
pub fn par_map<T: Sync, R: Send>(items: &[T], limit: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    if items.len() <= 1 {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<R>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    std::thread::scope(|scope| {
        for _ in 0..limit.max(1).min(items.len()) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(i) else { break };
                    let r = f(item);
                    results.lock().unwrap()[i] = Some(r);
                }
            });
        }
    });
    results.into_inner().unwrap().into_iter().map(|r| r.expect("every item ran")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::tests::{scratch, test_git};

    fn make_repo(path: &Path) {
        std::fs::create_dir_all(path.join(".git")).unwrap();
    }

    #[test]
    fn plain_folder_and_shell() {
        let root = scratch("scan");
        assert_eq!(scan(&root).kind, LayoutKind::Plain);
        make_repo(&root.join("repos/api"));
        make_repo(&root.join("group/billing/web"));
        make_repo(&root.join("node_modules/pkg"));
        make_repo(&root.join("repos/api/vendor/x"));
        make_repo(&root.join("Thing.app/inner"));
        let folder = scan(&root);
        assert_eq!(folder.kind, LayoutKind::Folder);
        assert_eq!(folder.repos.iter().map(|r| r.relative_path.as_str()).collect::<Vec<_>>(), ["group/billing/web", "repos/api"]);
        make_repo(&root);
        let shell = scan(&root);
        assert_eq!(shell.kind, LayoutKind::Shell);
        assert!(shell.repos[0].is_root() && shell.repos.len() == 3);
        let _ = std::fs::remove_dir_all(&root);

        let single = scratch("single");
        make_repo(&single);
        let layout = scan(&single);
        assert!(layout.kind == LayoutKind::Repo && layout.repos.len() == 1 && !layout.is_workspace());
        let _ = std::fs::remove_dir_all(&single);
    }

    #[test]
    fn compose_contexts_become_uncloned_repos() {
        let root = scratch("compose");
        make_repo(&root);
        make_repo(&root.join("services/api"));
        std::fs::create_dir_all(root.join("services/web")).unwrap();
        std::fs::create_dir_all(root.join("storage/mysql")).unwrap();
        std::fs::write(root.join("storage/mysql/Dockerfile"), "x").unwrap();
        std::fs::write(
            root.join("docker-compose.yml"),
            "services:\n  api:\n    build:\n      context: ./services/api/docker   # inside a cloned repo\n  web:\n    build:\n      context: ./services/web\n      target: develop\n  worker:\n    build: services/worker\n  mysql:\n    build: ./storage/mysql\n  remote:\n    build: https://github.com/acme/thing.git\n  templated:\n    build: ${CONTEXT}\n",
        )
        .unwrap();
        let layout = scan(&root);
        assert_eq!(layout.kind, LayoutKind::Shell);
        assert_eq!(layout.repos.iter().filter(|r| !r.is_cloned).map(|r| r.relative_path.as_str()).collect::<Vec<_>>(), ["services/web", "services/worker"]);
        assert_eq!(normalize("./services/a/").as_deref(), Some("services/a"));
        assert_eq!((normalize("../outside"), normalize("."), normalize("/abs")), (None, None, None));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Real git: a shell with a nested clone, a worktree nested inside the project and one
    /// outside, then which threads work where and what removing each would cost.
    #[test]
    fn a_real_workspace() {
        let root = scratch("ws");
        let git = test_git();
        let run = |args: &[&str], dir: &Path| assert!(git.run(args, dir, true).succeeded(), "{args:?}");
        run(&["init", "-q", "-b", "main"], &root);
        std::fs::write(root.join(".gitignore"), "repos/\n").unwrap();
        run(&["add", "."], &root);
        run(&["commit", "-q", "-m", "shell"], &root);
        let api = root.join("repos/api");
        std::fs::create_dir_all(&api).unwrap();
        run(&["init", "-q", "-b", "main"], &api);
        run(&["remote", "add", "origin", "git@github.com:acme/api.git"], &api);
        run(&["commit", "-q", "--allow-empty", "-m", "one"], &api);
        // A worktree nested in the project (found by the scan too) and one outside it.
        git.add_worktree(&api, &root.join("repos/api-login"), Some("login"), Some("main"), None).unwrap();
        let outside = root.parent().unwrap().join(format!("{}-spike", root.file_name().unwrap().to_string_lossy()));
        git.add_worktree(&api, &outside, Some("spike"), Some("main"), None).unwrap();
        std::fs::write(outside.join("new.txt"), "x").unwrap();

        let tools = WorkspaceTools { git: Some(git.clone()), ..Default::default() };
        let mut ws = Workspace::new(&root);
        ws.scan(&tools);
        assert_eq!(ws.layout.as_ref().unwrap().kind, LayoutKind::Shell);
        assert_eq!(ws.remotes.get("repos/api").map(|r| r.path.as_str()), Some("acme/api"));
        ws.refresh_status(&tools, Duration::ZERO);
        let names: Vec<String> = ws.cloned_repos().iter().map(|r| r.name()).collect();
        assert_eq!(names.len(), 2, "the nested worktree isn't a repo of its own: {names:?}");
        let repo = ws.cloned_repos().into_iter().find(|r| r.id() == "repos/api").unwrap().clone();
        let worktrees = ws.worktrees_of(&repo).to_vec();
        assert_eq!(worktrees.len(), 2);
        assert_eq!(ws.status.default_branches.get("repos/api").map(String::as_str), Some("main"));
        let login = worktrees.iter().find(|w| w.branch.as_deref() == Some("login")).unwrap();
        let spike = worktrees.iter().find(|w| w.branch.as_deref() == Some("spike")).unwrap();
        // Fresh branches: nothing done yet, so nothing to lose; uncommitted changes are.
        assert!(ws.has_no_work(login, &repo) && !ws.is_finished(login, &repo));
        assert_eq!(ws.removal_risk(login, &repo), RemovalRisk::None);
        assert!(matches!(ws.removal_risk(spike, &repo), RemovalRisk::LosesWork(_)));

        let footprint = vec![format!("{}/src", login.path), api.to_string_lossy().into_owned()];
        let threads = [ThreadFolders { id: "t1", footprint: &footprint, written: &[], working_directory: None, last_active: 1.0 }];
        let by = ws.threads_by_checkout(&threads, false);
        assert_eq!(by.get(&login.path).map(Vec::len), Some(1));
        assert_eq!(by.get(&repo.real_path()).map(Vec::len), Some(1));
        assert_eq!(ws.checkouts_for(&threads[0]).len(), 2);
        assert_eq!(ws.checkout_containing(&format!("{}/deep/er", login.path)).and_then(|c| c.worktree).map(|w| w.path), Some(login.path.clone()));
        assert_eq!(ws.checkouts_by_thread(&threads).get("t1").map(Vec::len), Some(2));

        git.remove_worktree(login, &api, false).unwrap();
        ws.forget_worktree(login, &repo);
        assert_eq!(ws.worktrees_of(&repo).len(), 1);
        assert!(ws.layout.as_ref().unwrap().repos.iter().all(|r| r.real_path() != login.path));
        let _ = std::fs::remove_dir_all(&outside);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pulls_from_the_mock_file_and_sections() {
        let dir = scratch("mock");
        let file = dir.join("pulls.json");
        std::fs::write(
            &file,
            r#"[{"repo": "github.com/acme/api", "number": 1, "head": "a", "base": "main", "mine": true, "ageMinutes": 5},
                {"repo": "github.com/acme/api", "number": 2, "head": "b", "base": "a", "ageMinutes": 1},
                {"repo": "github.com/acme/api", "number": 3, "head": "c", "base": "main", "reviewRequested": true, "ageMinutes": 9},
                {"repo": "github.com/acme/api", "number": 4, "head": "d", "state": "merged"},
                {"repo": "github.com/other/repo", "number": 5, "head": "e"}]"#,
        )
        .unwrap();
        // SAFETY: the only test that sets it; the others that read it run without it set.
        unsafe { std::env::set_var("YH_MOCK_PULLS", &file) };
        let remotes: HashMap<String, GitRemote> = [("".to_string(), GitRemote::parse("git@github.com:Acme/API.git").unwrap())].into();
        let found = read_pulls(&remotes, &WorkspaceTools::default(), &[]);
        unsafe { std::env::remove_var("YH_MOCK_PULLS") };
        let pulls = found.unwrap();
        assert_eq!(pulls.pull_requests.iter().map(|p| p.number()).collect::<Vec<_>>(), [2, 1, 3]);
        let mut ws = Workspace::new(&dir);
        ws.pulls = pulls;
        let s = ws.pull_request_sections();
        assert_eq!((s.needs_review.len(), s.mine.len(), s.others.len()), (1, 1, 0));
        assert_eq!(s.mine[0].all().len(), 2);
        assert!(read_pulls(&HashMap::new(), &WorkspaceTools::default(), &[]).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn par_map_keeps_order() {
        let items: Vec<u32> = (0..50).collect();
        assert_eq!(par_map(&items, 8, |x| x * 2), items.iter().map(|x| x * 2).collect::<Vec<_>>());
    }
}
