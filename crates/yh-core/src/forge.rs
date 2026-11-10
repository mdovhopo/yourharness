//! Pull requests (GitHub) and merge requests (GitLab) as a thread knows them: links found in
//! what was said, what its tool calls did to them (opened, merged, commented on, as against
//! looked at), and what the forge's own CLI says about each now. Never the forge's API with
//! our own credentials: `gh` and `glab` do the talking.

use crate::git::{Forge, GitRemote};
use serde_json::Value;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Link {
    pub host: String,
    /// `owner/repo` (GitLab: `group/sub/repo`).
    pub path: String,
    pub number: u64,
    pub url: String,
    pub gitlab: bool,
}

impl Link {
    pub fn id(&self) -> String {
        format!("{}/{}#{}", self.host, self.path.to_lowercase(), self.number)
    }

    /// GitHub's `…/owner/repo/pull/12` and GitLab's `…/group/repo/-/merge_requests/12`.
    pub fn find(text: &str) -> Vec<Link> {
        if !text.contains("/pull/") && !text.contains("/merge_requests/") {
            return vec![];
        }
        let mut links: Vec<Link> = vec![];
        let mut rest = text;
        while let Some(at) = rest.find("http") {
            let tail = &rest[at..];
            let end = tail.find(|c: char| c.is_whitespace() || "\"'<>)]`,".contains(c)).unwrap_or(tail.len());
            let candidate = &tail[..end];
            if let Some(link) = Link::parse(candidate) {
                if !links.iter().any(|l| l.id() == link.id()) {
                    links.push(link);
                }
            }
            rest = &tail[end.max(4)..];
        }
        links
    }

    pub fn parse(url: &str) -> Option<Link> {
        let after = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
        let (host, path) = after.split_once('/')?;
        if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
            return None;
        }
        let (repo, number, gitlab) = if let Some((repo, n)) = path.split_once("/-/merge_requests/") {
            (repo, n, true)
        } else {
            let (repo, n) = path.split_once("/pull/")?;
            (repo, n, false)
        };
        let digits: String = number.chars().take_while(|c| c.is_ascii_digit()).collect();
        let number: u64 = digits.parse().ok()?;
        let ok = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c));
        let parts: Vec<&str> = repo.split('/').collect();
        if parts.len() < 2 || !parts.iter().all(|p| ok(p)) {
            return None;
        }
        let kind = if gitlab { "/-/merge_requests/" } else { "/pull/" };
        Some(Link { host: host.to_lowercase(), path: repo.to_string(), number, url: format!("https://{host}/{repo}{kind}{number}"), gitlab })
    }
}

/// What a tool call did to a PR it named.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Named by URL, or linked in its output (`gh pr create` prints it).
    Link(Link),
    /// `gh pr merge 57`, with `-R owner/repo` when given; else the repo it ran in.
    Number(u64, Option<String>),
    /// No PR named: the one of the branch it ran on.
    CurrentBranch,
}

const GH_WRITES: &[&str] = &["create", "merge", "edit", "ready", "close", "reopen", "review", "comment", "update-branch", "lock", "unlock"];
const STACK_WRITES: &[&str] = &["submit", "merge", "sync", "push", "link", "rebase", "unstack"];
const GLAB_WRITES: &[&str] = &["create", "merge", "update", "close", "reopen", "approve", "unapprove", "note", "rebase", "revoke"];

/// The PRs a tool call changed; None when it changed none (looking at one isn't working on it).
pub fn activity(tool: &str, input: &str, result: &str) -> Option<Vec<Target>> {
    let v: Value = serde_json::from_str(input).unwrap_or_default();
    if tool == "Bash" {
        let command = v["command"].as_str()?;
        let mut targets = command_targets(command)?;
        let linked: Vec<Target> = Link::find(result).into_iter().map(Target::Link).collect();
        if !linked.is_empty() && command.contains("gh stack") {
            targets.retain(|t| *t != Target::CurrentBranch);
        }
        targets.extend(linked);
        return Some(unique(targets));
    }
    let name = tool.to_lowercase();
    let writes = ["create", "merge", "update", "review", "comment", "close", "edit", "request"];
    if !name.starts_with("mcp__")
        || !(name.contains("pull_request") || name.contains("merge_request"))
        || !writes.iter().any(|w| name.contains(w))
        || name.contains("get_")
        || name.contains("list_")
        || name.contains("search_")
    {
        return None;
    }
    let mut targets: Vec<Target> = Link::find(&format!("{input}\n{result}")).into_iter().map(Target::Link).collect();
    let number = ["pullNumber", "pull_number", "pr_number", "number", "merge_request_iid", "iid"]
        .iter()
        .find_map(|k| v[*k].as_u64().or_else(|| v[*k].as_str().and_then(|s| s.parse().ok())));
    if let Some(n) = number {
        let repo = match (v["owner"].as_str(), v["repo"].as_str()) {
            (Some(o), Some(r)) => Some(format!("{o}/{r}")),
            _ => v["project_id"].as_str().map(str::to_string),
        };
        targets.push(Target::Number(n, repo));
    }
    Some(unique(targets))
}

fn unique(targets: Vec<Target>) -> Vec<Target> {
    let mut seen: Vec<Target> = vec![];
    for t in targets {
        if !seen.contains(&t) {
            seen.push(t);
        }
    }
    seen
}

fn command_targets(command: &str) -> Option<Vec<Target>> {
    let mut targets = vec![];
    let mut changes = false;
    for segment in command.split(|c| ";|&\n".contains(c)).filter(|s| !s.trim().is_empty()) {
        let words = shell_words(segment);
        for start in 0..words.len() {
            let tool = words[start].rsplit('/').next().unwrap_or("");
            let rest = &words[start + 1..];
            let verb = rest.get(1).map(String::as_str).unwrap_or("");
            let first = rest.first().map(String::as_str);
            if tool == "gh" && first == Some("pr") && GH_WRITES.contains(&verb) {
                changes = true;
                targets.extend(named(&rest[2..]));
            } else if tool == "gh" && first == Some("stack") && STACK_WRITES.contains(&verb) {
                changes = true;
                targets.push(Target::CurrentBranch);
            } else if tool == "glab" && first == Some("mr") && GLAB_WRITES.contains(&verb) {
                changes = true;
                targets.extend(named(&rest[2..]));
            } else if tool == "gh" && first == Some("api") {
                match api_target(&rest[1..]) {
                    Some(t) => {
                        changes = true;
                        targets.push(t);
                    }
                    None => continue,
                }
            } else {
                continue;
            }
            break;
        }
    }
    changes.then_some(targets)
}

fn named(args: &[String]) -> Vec<Target> {
    let mut repo = None;
    for (i, a) in args.iter().enumerate() {
        if (a == "-R" || a == "--repo") && i + 1 < args.len() {
            repo = Some(args[i + 1].clone());
        }
        if let Some(r) = a.strip_prefix("--repo=") {
            repo = Some(r.to_string());
        }
    }
    for a in args.iter().filter(|a| !a.starts_with('-')) {
        if let Some(link) = Link::find(a).into_iter().next() {
            return vec![Target::Link(link)];
        }
        let digits = a.strip_prefix('#').or_else(|| a.strip_prefix('!')).unwrap_or(a);
        if let Ok(n) = digits.parse::<u64>() {
            if n > 0 {
                return vec![Target::Number(n, repo)];
            }
        }
    }
    vec![Target::CurrentBranch]
}

fn api_target(args: &[String]) -> Option<Target> {
    let path = args.iter().find(|a| a.contains("/pulls/"))?;
    let after = path.split("repos/").nth(1)?;
    let mut parts = after.split('/');
    let (owner, repo, pulls, number) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    if pulls != "pulls" {
        return None;
    }
    let number: u64 = number.parse().ok()?;
    let mut method = "GET".to_string();
    for (i, a) in args.iter().enumerate() {
        if (a == "-X" || a == "--method") && i + 1 < args.len() {
            method = args[i + 1].to_uppercase();
        }
        if let Some(m) = a.strip_prefix("--method=") {
            method = m.to_uppercase();
        }
        if ["-f", "-F", "--field", "--raw-field", "--input"].contains(&a.as_str()) && method == "GET" {
            method = "POST".into();
        }
    }
    (method != "GET").then(|| Target::Number(number, Some(format!("{owner}/{repo}"))))
}

/// Words of a command line, quotes taken off: enough to read `gh` arguments.
pub fn shell_words(text: &str) -> Vec<String> {
    let (mut words, mut current, mut quote, mut in_word) = (vec![], String::new(), None::<char>, false);
    for c in text.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                current.push(c);
            }
        } else if c == '"' || c == '\'' {
            quote = Some(c);
            in_word = true;
        } else if c.is_whitespace() {
            if in_word {
                words.push(std::mem::take(&mut current));
            }
            in_word = false;
        } else {
            current.push(c);
            in_word = true;
        }
    }
    if in_word {
        words.push(current);
    }
    words
}

// MARK: Looking them up

#[derive(Clone, Debug, PartialEq)]
pub struct PullRequest {
    /// Its repo (host and path as the repo was asked for, or as its URL spells them), number and URL.
    pub link: Link,
    pub title: String,
    pub author: Option<String>,
    pub draft: bool,
    pub head: String,
    pub base: String,
    /// `open`, `merged`, `closed`.
    pub state: String,
    /// `approved`, `changesRequested`, `required` or empty.
    pub review: String,
    /// `passing`, `failing`, `pending` or empty.
    pub checks: String,
    /// What GitHub says stands between it and merging: `CLEAN`, `BLOCKED`, `BEHIND`, `DIRTY`
    /// (conflicts), `UNSTABLE` (failing checks that aren't required), `DRAFT`.
    pub merge_status: Option<String>,
    /// Auto-merge is on: it merges by itself once checks and reviews allow.
    pub auto_merge: bool,
    /// ISO 8601, as the forge gave it.
    pub updated_at: Option<String>,
    /// Opened by the account the forge's CLI is signed in with.
    pub is_mine: bool,
    pub review_requested_from_me: bool,
    pub in_merge_queue: bool,
    pub merged_at: Option<String>,
    /// Its place in one of GitHub's own stacks (`gh stack`), when it's in one.
    pub stack: Option<StackPlace>,
}

/// Which of a repo's GitHub stacks a PR is in, how many PRs it has, and where this one sits:
/// 1 is the bottom, on the stack's base branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StackPlace {
    pub number: u64,
    pub size: u64,
    pub position: u64,
}

impl PullRequest {
    /// A PR with only what identifies it; the rest empty, open.
    pub fn new(link: Link, head: &str, base: &str) -> PullRequest {
        PullRequest {
            link,
            title: String::new(),
            author: None,
            draft: false,
            head: head.into(),
            base: base.into(),
            state: "open".into(),
            review: String::new(),
            checks: String::new(),
            merge_status: None,
            auto_merge: false,
            updated_at: None,
            is_mine: false,
            review_requested_from_me: false,
            in_merge_queue: false,
            merged_at: None,
            stack: None,
        }
    }

    /// `host/owner/repo#12`, the path lowercased.
    pub fn id(&self) -> String {
        self.link.id()
    }

    pub fn number(&self) -> u64 {
        self.link.number
    }

    pub fn is_open(&self) -> bool {
        self.state == "open"
    }

    pub fn remote(&self) -> GitRemote {
        self.link.remote()
    }

    /// Whether it's in the repo `remote` points at (case aside).
    pub fn is_in(&self, remote: &GitRemote) -> bool {
        remote.same_repo(&self.link.host, &self.link.path)
    }

    /// Seconds since 1970 it was last updated, for sorting.
    pub fn updated_seconds(&self) -> f64 {
        self.updated_at.as_deref().and_then(crate::library::parse_iso).unwrap_or(f64::MIN)
    }
}

impl Link {
    /// The link to PR `number` in `remote`; its URL made up unless given.
    pub fn to(remote: &GitRemote, number: u64, url: Option<&str>) -> Link {
        let gitlab = remote.forge == Some(Forge::GitLab);
        let kind = if gitlab { "/-/merge_requests/" } else { "/pull/" };
        Link {
            host: remote.host.clone(),
            path: remote.path.clone(),
            number,
            url: url.map(str::to_string).unwrap_or_else(|| format!("https://{}/{}{kind}{number}", remote.host, remote.path)),
            gitlab,
        }
    }

    pub fn remote(&self) -> GitRemote {
        GitRemote::new(&self.host, &self.path, Some(if self.gitlab { Forge::GitLab } else { Forge::GitHub }))
    }

    /// Whether `pull` is the PR this links to.
    pub fn matches(&self, pull: &PullRequest) -> bool {
        pull.link.number == self.number && self.remote().same_repo(&pull.link.host, &pull.link.path)
    }
}

const FIELDS: &str = "number,title,url,author,isDraft,headRefName,baseRefName,updatedAt,reviewDecision,statusCheckRollup,state,mergeStateStatus,autoMergeRequest";

/// A PR as `gh pr view` (or `glab mr view`) tells it now; an error when the CLI can't.
/// `YH_MOCK_PULLS=<file.json>` reads them from a file instead (see `mock_pulls`).
pub fn look_up(link: &Link, path_env: Option<&str>) -> Result<PullRequest, String> {
    if let Some(mock) = mock_pulls() {
        return mock?.into_iter().find(|p| link.matches(p)).ok_or_else(|| "not in the mock file".to_string());
    }
    let (tool, args): (&str, Vec<String>) = if link.gitlab {
        ("glab", vec!["mr".into(), "view".into(), link.url.clone(), "--output".into(), "json".into()])
    } else {
        ("gh", vec!["pr".into(), "view".into(), link.url.clone(), "--json".into(), FIELDS.into()])
    };
    let mut command = Command::new(tool);
    command.args(&args).current_dir(std::env::temp_dir());
    // The login shell's environment: opened from the Dock the app's own PATH has no Homebrew,
    // so `gh` and `glab` aren't found (and their tokens' variables are missing).
    crate::shell_env::apply(&mut command);
    if let Some(p) = path_env {
        command.env("PATH", p);
    }
    let output = command.output().map_err(|e| format!("{tool}: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let v: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    Ok(if link.gitlab { from_gitlab(link, &v) } else { from_github(link, &v) })
}

fn from_github(link: &Link, v: &Value) -> PullRequest {
    let checks = v["statusCheckRollup"].as_array().map(|runs| {
        let states: Vec<String> =
            runs.iter().map(|r| r["conclusion"].as_str().filter(|c| !c.is_empty()).or(r["state"].as_str()).or(r["status"].as_str()).unwrap_or("").to_uppercase()).collect();
        if states.is_empty() {
            ""
        } else if states.iter().any(|s| ["FAILURE", "ERROR", "CANCELLED", "TIMED_OUT", "ACTION_REQUIRED"].contains(&s.as_str())) {
            "failing"
        } else if states.iter().any(|s| ["PENDING", "IN_PROGRESS", "QUEUED", "EXPECTED", ""].contains(&s.as_str())) {
            "pending"
        } else {
            "passing"
        }
    });
    PullRequest {
        link: link.clone(),
        title: v["title"].as_str().unwrap_or("").to_string(),
        author: v["author"]["login"].as_str().map(str::to_string),
        draft: v["isDraft"].as_bool().unwrap_or(false),
        head: v["headRefName"].as_str().unwrap_or("").to_string(),
        base: v["baseRefName"].as_str().unwrap_or("").to_string(),
        state: v["state"].as_str().unwrap_or("OPEN").to_lowercase(),
        review: match v["reviewDecision"].as_str() {
            Some("APPROVED") => "approved",
            Some("CHANGES_REQUESTED") => "changesRequested",
            Some("REVIEW_REQUIRED") => "required",
            _ => "",
        }
        .into(),
        checks: checks.unwrap_or("").into(),
        merge_status: v["mergeStateStatus"].as_str().map(str::to_string),
        auto_merge: !v["autoMergeRequest"].is_null(),
        updated_at: v["updatedAt"].as_str().map(str::to_string),
        is_mine: false,
        review_requested_from_me: false,
        in_merge_queue: v["isInMergeQueue"].as_bool().unwrap_or(false),
        merged_at: v["mergedAt"].as_str().map(str::to_string),
        stack: None,
    }
}

fn from_gitlab(link: &Link, v: &Value) -> PullRequest {
    PullRequest {
        link: link.clone(),
        title: v["title"].as_str().unwrap_or("").to_string(),
        author: v["author"]["username"].as_str().map(str::to_string),
        draft: v["draft"].as_bool().or(v["work_in_progress"].as_bool()).unwrap_or(false),
        head: v["source_branch"].as_str().unwrap_or("").to_string(),
        base: v["target_branch"].as_str().unwrap_or("").to_string(),
        state: match v["state"].as_str() {
            Some("merged") => "merged",
            Some("closed") => "closed",
            _ => "open",
        }
        .into(),
        review: String::new(),
        checks: String::new(),
        merge_status: None,
        auto_merge: false,
        updated_at: v["updated_at"].as_str().map(str::to_string),
        is_mine: false,
        review_requested_from_me: false,
        in_merge_queue: false,
        merged_at: v["merged_at"].as_str().map(str::to_string),
        stack: None,
    }
}

/// The repo a folder's `origin` points at, as `host`, `owner/repo` and whether it's GitLab.
pub fn origin(folder: &Path) -> Option<(String, String, bool)> {
    let out = Command::new("git").args(["remote", "get-url", "origin"]).current_dir(folder).output().ok()?;
    let remote = GitRemote::parse(&String::from_utf8_lossy(&out.stdout))?;
    let gitlab = remote.host.contains("gitlab");
    Some((remote.host, remote.path, gitlab))
}

/// The current branch of a folder's checkout.
pub fn branch(folder: &Path) -> Option<String> {
    let out = Command::new("git").args(["rev-parse", "--abbrev-ref", "HEAD"]).current_dir(folder).output().ok()?;
    let b = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!b.is_empty() && b != "HEAD").then_some(b)
}

// MARK: The mock file

/// Development: `YH_MOCK_PULLS=<file.json>` serves pull requests from a file instead of
/// asking `gh`/`glab`, both for looking one up (`look_up`, `github::GitHubClient::fetch_links`)
/// and for a workspace's open PRs (`workspace::read_pulls`, which keeps the open ones whose
/// repo is one of the workspace's). The file is read again whenever it changes, so editing it
/// (a PR merged, one gone) shows at the next refresh. Two shapes:
///
/// - 0.0.x's list, one entry per PR:
///   `[{"repo": "github.com/acme/api", "number": 57, "title": "…", "head": "feature/login",
///   "base": "main", "draft": false, "review": "approved", "checks": "passing",
///   "state": "open", "author": "you", "mine": true}]`, optionally with `"reviewRequested"`,
///   `"mergeStatus": "BLOCKED"`, `"autoMerge"`, `"mergeQueue"`, `"ageMinutes": 90` and
///   `"stack": {"number": 4, "size": 3, "position": 1}` for a GitHub stack. `review` is
///   `approved|changesRequested|required|none`, `checks` `passing|failing|pending|none`,
///   `state` `open|merged|closed`; a `repo` whose host has `gitlab` in it is a GitLab project.
/// - An object of PR URLs to what `gh pr view --json …` prints: `{"<url>": {…}}`.
///
/// None when it isn't set; an error when the file can't be read or is neither shape.
pub fn mock_pulls() -> Option<Result<Vec<PullRequest>, String>> {
    let path = std::env::var("YH_MOCK_PULLS").ok().filter(|p| !p.is_empty())?;
    type Cached = Option<(String, Option<SystemTime>, Vec<PullRequest>)>;
    static CACHE: Mutex<Cached> = Mutex::new(None);
    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    if let Some((p, m, pulls)) = CACHE.lock().unwrap().as_ref() {
        if *p == path && *m == modified && modified.is_some() {
            return Some(Ok(pulls.clone()));
        }
    }
    let read = std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|text| parse_mock(&text));
    if let Ok(pulls) = &read {
        *CACHE.lock().unwrap() = Some((path, modified, pulls.clone()));
    }
    Some(read)
}

/// The mock file's contents (see `mock_pulls`). Unknown fields are ignored, and so are entries
/// without a repo and number.
pub fn parse_mock(text: &str) -> Result<Vec<PullRequest>, String> {
    let now = SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    match serde_json::from_str::<Value>(text).map_err(|e| e.to_string())? {
        Value::Array(items) => Ok(items.iter().filter_map(|item| mock_item(item, now)).collect()),
        Value::Object(map) => Ok(map
            .iter()
            .filter_map(|(url, v)| {
                let link = Link::parse(url)?;
                Some(if link.gitlab { from_gitlab(&link, v) } else { from_github(&link, v) })
            })
            .collect()),
        _ => Err("the mock file is neither a list of PRs nor an object of URLs".into()),
    }
}

fn mock_item(item: &Value, now: f64) -> Option<PullRequest> {
    let repo = item["repo"].as_str()?;
    let number = item["number"].as_u64()?;
    let (host, path) = repo.split_once('/')?;
    let forge = if host.contains("gitlab") { Forge::GitLab } else { Forge::GitHub };
    let remote = GitRemote::new(host, path, Some(forge));
    let mut pull = PullRequest::new(Link::to(&remote, number, None), item["head"].as_str().unwrap_or(""), item["base"].as_str().unwrap_or("main"));
    let text = |k: &str| item[k].as_str().map(str::to_string);
    let flag = |k: &str| item[k].as_bool().unwrap_or(false);
    pull.title = text("title").unwrap_or_default();
    pull.author = text("author");
    pull.draft = flag("draft");
    pull.review = text("review").filter(|r| ["approved", "changesRequested", "required"].contains(&r.as_str())).unwrap_or_default();
    pull.checks = text("checks").filter(|c| ["passing", "failing", "pending"].contains(&c.as_str())).unwrap_or_default();
    pull.state = text("state").filter(|s| ["open", "merged", "closed"].contains(&s.as_str())).unwrap_or_else(|| "open".into());
    pull.is_mine = flag("mine");
    pull.review_requested_from_me = flag("reviewRequested");
    pull.merge_status = text("mergeStatus");
    pull.auto_merge = flag("autoMerge");
    pull.in_merge_queue = flag("mergeQueue");
    let age = item["ageMinutes"].as_f64().unwrap_or(0.0);
    pull.updated_at = Some(crate::library::iso((now - age * 60.0) as i64));
    if pull.state == "merged" {
        pull.merged_at = pull.updated_at.clone();
    }
    let stack = &item["stack"];
    if let (Some(n), Some(position)) = (stack["number"].as_u64(), stack["position"].as_u64()) {
        pull.stack = Some(StackPlace { number: n, size: stack["size"].as_u64().unwrap_or(position), position });
    }
    Some(pull)
}

/// A shell command (a `Bash` call's input) that changes or reads pull requests: the right
/// sidebar looks the thread's PRs up again after it.
pub fn touches_pull_requests(input_json: &str) -> bool {
    let v: serde_json::Value = serde_json::from_str(input_json).unwrap_or_default();
    let command = v["command"].as_str().unwrap_or("");
    ["gh pr", "gh stack", "glab mr", "git push", "gh api"].iter().any(|c| command.contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_mock_file() {
        let list = r#"[
            {"repo": "github.com/acme/api", "number": 57, "title": "Login", "head": "feature/login", "base": "main",
             "draft": true, "review": "approved", "checks": "passing", "state": "open", "author": "you", "mine": true,
             "ageMinutes": 90, "stack": {"number": 4, "size": 3, "position": 1}, "somethingNew": [1, 2]},
            {"repo": "gitlab.com/g/sub/hooks", "number": 3, "head": "x", "state": "merged", "review": "bogus"},
            {"repo": "nohost", "number": 1}
        ]"#;
        let pulls = parse_mock(list).unwrap();
        assert_eq!(pulls.len(), 2);
        let p = &pulls[0];
        assert_eq!((p.link.url.as_str(), p.head.as_str(), p.base.as_str()), ("https://github.com/acme/api/pull/57", "feature/login", "main"));
        assert!(p.draft && p.is_mine && p.is_open());
        assert_eq!((p.review.as_str(), p.checks.as_str()), ("approved", "passing"));
        assert_eq!(p.stack, Some(StackPlace { number: 4, size: 3, position: 1 }));
        assert!(p.updated_at.is_some());
        let m = &pulls[1];
        assert!(m.link.gitlab && m.link.url.ends_with("/g/sub/hooks/-/merge_requests/3"));
        assert_eq!((m.state.as_str(), m.review.as_str(), m.base.as_str()), ("merged", "", "main"));
        assert!(m.merged_at.is_some());

        let object = r#"{"https://github.com/o/r/pull/9": {"title": "T", "headRefName": "h", "baseRefName": "main", "state": "MERGED", "isDraft": false}}"#;
        let pulls = parse_mock(object).unwrap();
        assert_eq!((pulls[0].link.number, pulls[0].state.as_str(), pulls[0].head.as_str()), (9, "merged", "h"));
        assert!(Link::find("https://github.com/O/R/pull/9")[0].matches(&pulls[0]));
        assert!(parse_mock("3").is_err());
    }

    #[test]
    fn finds_links() {
        let links = Link::find("see https://github.com/o/r/pull/12, and https://gitlab.com/g/s/r/-/merge_requests/3).");
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].path, "o/r");
        assert_eq!(links[0].number, 12);
        assert!(links[1].gitlab);
        assert_eq!(links[1].path, "g/s/r");
    }

    #[test]
    fn reads_commands() {
        let input = |c: &str| serde_json::json!({ "command": c }).to_string();
        assert_eq!(activity("Bash", &input("gh pr view 3"), ""), None);
        assert_eq!(activity("Bash", &input("gh pr merge 57 -R o/r --squash"), ""), Some(vec![Target::Number(57, Some("o/r".into()))]));
        assert_eq!(activity("Bash", &input("git push && gh pr create --fill"), "https://github.com/o/r/pull/9\n").map(|t| t.len()), Some(2));
        assert_eq!(activity("Bash", &input("gh api repos/o/r/pulls/5/merge -X PUT"), ""), Some(vec![Target::Number(5, Some("o/r".into()))]));
        assert_eq!(activity("Bash", &input("gh api repos/o/r/pulls/5"), ""), None);
        assert_eq!(activity("mcp__github__merge_pull_request", r#"{"owner":"o","repo":"r","pullNumber":4}"#, ""), Some(vec![Target::Number(4, Some("o/r".into()))]));
    }
}
