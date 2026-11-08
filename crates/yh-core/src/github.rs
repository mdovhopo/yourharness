//! GitHub through `gh`: every repo's open PRs in one GraphQL query per batch of 20 repos
//! (`GitHubClient::fetch`), specific PRs by link in any state (`fetch_links`), and GitHub's
//! own stacks (`LocalStack`, what `gh stack` keeps in a repo; `GitHubStacks`, its commands).
//! Uses whatever account `gh` is signed in with; never its credentials. All of it blocks.

use crate::forge::{Link, PullRequest, StackPlace};
use crate::git::{Cli, CommandOutput, Forge, GitRemote};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Everything one forge reported in a refresh.
#[derive(Clone, Debug, PartialEq)]
pub struct ForgeSnapshot {
    pub forge: Forge,
    /// The signed-in user, per host.
    pub accounts: BTreeMap<String, String>,
    pub pull_requests: Vec<PullRequest>,
    /// Repos the forge couldn't resolve (no access, renamed, deleted).
    pub unreachable: HashSet<GitRemote>,
    /// Set when a whole request failed: CLI missing, signed out, offline.
    pub error: Option<String>,
}

impl ForgeSnapshot {
    pub fn new(forge: Forge) -> ForgeSnapshot {
        ForgeSnapshot { forge, accounts: BTreeMap::new(), pull_requests: vec![], unreachable: HashSet::new(), error: None }
    }
}

/// One batch's answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub login: Option<String>,
    pub pull_requests: Vec<PullRequest>,
    pub unreachable: HashSet<GitRemote>,
}

pub const BATCH_SIZE: usize = 20;
const PULLS_PER_REPO: usize = 20;
/// Batches in flight at once.
const CONCURRENCY: usize = 3;

#[derive(Clone, Debug)]
pub struct GitHubClient {
    pub cli: Cli,
}

impl GitHubClient {
    pub fn new(path_env: Option<&str>) -> GitHubClient {
        GitHubClient { cli: Cli::new("gh", path_env) }
    }

    /// Open PRs of every repo, a GraphQL query per 20 repos of a host, 3 at a time. Up to 30 s
    /// per query (a second or two usually).
    pub fn fetch(&self, remotes: &[GitRemote]) -> ForgeSnapshot {
        fetch_batches(Forge::GitHub, remotes, &self.cli, query, parse)
    }

    /// Specific PRs a thread links to, whatever their state: one query per host, up to 20 s.
    /// `YH_MOCK_PULLS` answers from its file instead.
    pub fn fetch_links(&self, links: &[Link]) -> Vec<PullRequest> {
        if let Some(mock) = crate::forge::mock_pulls() {
            let all = mock.unwrap_or_default();
            return links.iter().filter_map(|l| all.iter().find(|p| l.matches(p)).cloned()).collect();
        }
        let mut groups: BTreeMap<String, Vec<Link>> = BTreeMap::new();
        for link in links.iter().filter(|l| !l.gitlab) {
            groups.entry(link.host.clone()).or_default().push(link.clone());
        }
        let mut found = vec![];
        for (host, group) in groups {
            let mut args = vec!["api".to_string(), "graphql".into(), "-f".into(), format!("query={}", query_links(&group))];
            if host != "github.com" {
                args.extend(["--hostname".into(), host]);
            }
            let out = self.cli.run(&args, None, Duration::from_secs(20), &[]);
            // One PR or repo that doesn't resolve makes `gh` exit non-zero, but it still
            // prints the rest: read them.
            found.extend(parse_links(&out.stdout, &group));
        }
        found
    }
}

/// A PR's place in a GitHub stack. Only github.com has it: an Enterprise server without the
/// fields would fail the whole query.
fn stack_fields(host: Option<&str>) -> &'static str {
    if host == Some("github.com") { "\n      stack { number size } stackEntry { position }" } else { "" }
}

/// The open PRs of a batch of repos (all on one host), `r0`, `r1`… in order.
pub fn query(batch: &[GitRemote]) -> String {
    let stack = stack_fields(batch.first().map(|r| r.host.as_str()));
    let repos: Vec<String> = batch
        .iter()
        .enumerate()
        .map(|(i, r)| {
            format!(
                "r{i}: repository(owner: {}, name: {}) {{
  pullRequests(states: OPEN, first: {PULLS_PER_REPO}, orderBy: {{field: UPDATED_AT, direction: DESC}}) {{
    nodes {{
      number title url isDraft headRefName baseRefName updatedAt reviewDecision mergeStateStatus{stack}
      author {{ login }}
      reviewRequests(first: 20) {{ nodes {{ requestedReviewer {{ ... on User {{ login }} }} }} }}
      commits(last: 1) {{ nodes {{ commit {{ statusCheckRollup {{ state }} }} }} }}
    }}
  }}
}}",
                quoted(r.owner()),
                quoted(r.name())
            )
        })
        .collect();
    format!("query {{\n  viewer {{ login }}\n{}\n}}", repos.join("\n"))
}

/// `gh api graphql`'s answer for `query(batch)`; None when it isn't one (an error message).
pub fn parse(json: &str, batch: &[GitRemote]) -> Option<Page> {
    let v: Value = serde_json::from_str(json).ok()?;
    let data = v.get("data")?.as_object()?;
    let mut page = Page { login: data.get("viewer").and_then(|u| u["login"].as_str()).map(str::to_string), ..Default::default() };
    for (i, remote) in batch.iter().enumerate() {
        let Some(repo) = data.get(&format!("r{i}")).filter(|r| r.is_object()) else {
            page.unreachable.insert(remote.clone());
            continue;
        };
        let nodes = repo["pullRequests"]["nodes"].as_array().cloned().unwrap_or_default();
        page.pull_requests.extend(nodes.iter().filter_map(|n| pull_request(n, remote, page.login.as_deref())));
    }
    Some(page)
}

/// One PR node of either query.
pub fn pull_request(node: &Value, remote: &GitRemote, login: Option<&str>) -> Option<PullRequest> {
    let number = node["number"].as_u64()?;
    let url = node["url"].as_str()?;
    let (head, base) = (node["headRefName"].as_str()?, node["baseRefName"].as_str()?);
    let author = node["author"]["login"].as_str().map(str::to_string);
    let reviewers: Vec<&str> =
        node["reviewRequests"]["nodes"].as_array().map(|a| a.iter().filter_map(|n| n["requestedReviewer"]["login"].as_str()).collect()).unwrap_or_default();
    let rollup = node["commits"]["nodes"].get(0).map(|n| &n["commit"]["statusCheckRollup"]["state"]).and_then(Value::as_str);
    let mut pull = PullRequest::new(Link::to(remote, number, Some(url)), head, base);
    pull.title = node["title"].as_str().unwrap_or("").to_string();
    pull.is_mine = login.is_some() && author.as_deref() == login;
    pull.author = author;
    pull.draft = node["isDraft"].as_bool().unwrap_or(false);
    pull.updated_at = node["updatedAt"].as_str().map(str::to_string);
    pull.review = review(node["reviewDecision"].as_str()).into();
    pull.checks = checks(rollup).into();
    pull.review_requested_from_me = login.is_some_and(|l| reviewers.contains(&l));
    pull.merge_status = node["mergeStateStatus"].as_str().map(str::to_string);
    if let (Some(n), Some(position)) = (node["stack"]["number"].as_u64(), node["stackEntry"]["position"].as_u64()) {
        pull.stack = Some(StackPlace { number: n, size: node["stack"]["size"].as_u64().unwrap_or(position), position });
    }
    Some(pull)
}

/// Specific PRs (all on one host), `p0`, `p1`… in order, whatever their state.
pub fn query_links(links: &[Link]) -> String {
    let stack = stack_fields(links.first().map(|l| l.host.as_str()));
    let parts: Vec<String> = links
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let remote = l.remote();
            format!(
                "p{i}: repository(owner: {}, name: {}) {{
  pullRequest(number: {}) {{
    number title url isDraft headRefName baseRefName updatedAt reviewDecision state
    mergedAt mergeStateStatus isInMergeQueue autoMergeRequest {{ enabledAt }}{stack}
    author {{ login }}
    commits(last: 1) {{ nodes {{ commit {{ statusCheckRollup {{ state }} }} }} }}
  }}
}}",
                quoted(remote.owner()),
                quoted(remote.name()),
                l.number
            )
        })
        .collect();
    format!("query {{\n  viewer {{ login }}\n{}\n}}", parts.join("\n"))
}

pub fn parse_links(json: &str, links: &[Link]) -> Vec<PullRequest> {
    let Ok(v) = serde_json::from_str::<Value>(json) else { return vec![] };
    let Some(data) = v.get("data").and_then(Value::as_object) else { return vec![] };
    let login = data.get("viewer").and_then(|u| u["login"].as_str());
    links
        .iter()
        .enumerate()
        .filter_map(|(i, link)| {
            let node = data.get(&format!("p{i}")).map(|r| &r["pullRequest"]).filter(|n| n.is_object())?;
            let mut pull = pull_request(node, &link.remote(), login)?;
            pull.state = node["state"].as_str().unwrap_or("OPEN").to_lowercase();
            if !["open", "merged", "closed"].contains(&pull.state.as_str()) {
                pull.state = "open".into();
            }
            pull.auto_merge = node["autoMergeRequest"].is_object();
            pull.in_merge_queue = node["isInMergeQueue"].as_bool().unwrap_or(false);
            pull.merged_at = node["mergedAt"].as_str().map(str::to_string);
            Some(pull)
        })
        .collect()
}

pub fn review(decision: Option<&str>) -> &'static str {
    match decision {
        Some("APPROVED") => "approved",
        Some("CHANGES_REQUESTED") => "changesRequested",
        Some("REVIEW_REQUIRED") => "required",
        _ => "",
    }
}

pub fn checks(state: Option<&str>) -> &'static str {
    match state {
        Some("SUCCESS") => "passing",
        Some("FAILURE" | "ERROR") => "failing",
        Some("PENDING" | "EXPECTED") => "pending",
        _ => "",
    }
}

/// What a failed forge CLI call means, in words.
pub fn describe_failure(out: &CommandOutput, tool: &str, service: &str) -> String {
    if out.timed_out {
        return format!("{service} didn't respond in time.");
    }
    let message = out.stderr.trim();
    if message.contains("auth login") || message.contains("not logged") {
        return format!("{tool} isn't signed in. Run `{tool} auth login`.");
    }
    if out.launch_failed {
        return format!("{tool} couldn't be started.");
    }
    message.lines().next().map(str::to_string).unwrap_or_else(|| format!("{tool} exited with code {}.", out.status.unwrap_or(-1)))
}

/// A GraphQL string literal.
pub fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Runs the batched queries through a forge CLI, a few at a time, and merges the pages.
pub(crate) fn fetch_batches(
    forge: Forge,
    remotes: &[GitRemote],
    cli: &Cli,
    query: fn(&[GitRemote]) -> String,
    parse: fn(&str, &[GitRemote]) -> Option<Page>,
) -> ForgeSnapshot {
    let mut unique: Vec<GitRemote> = remotes.iter().cloned().collect::<HashSet<_>>().into_iter().collect();
    unique.sort_by(|a, b| (&a.host, &a.path).cmp(&(&b.host, &b.path)));
    let mut batches: Vec<(String, Vec<GitRemote>)> = vec![];
    for remote in unique {
        match batches.last_mut() {
            Some((host, list)) if *host == remote.host && list.len() < BATCH_SIZE => list.push(remote),
            _ => batches.push((remote.host.clone(), vec![remote])),
        }
    }
    let results = crate::workspace::par_map(&batches, CONCURRENCY, |(host, batch)| {
        let mut args = vec!["api".to_string(), "graphql".into(), "-f".into(), format!("query={}", query(batch))];
        if host != forge.default_host() {
            args.extend(["--hostname".into(), host.clone()]);
        }
        let out = cli.run(&args, None, Duration::from_secs(30), &[]);
        // The CLI exits non-zero when any repo fails to resolve, but still prints the rest.
        (host.clone(), parse(&out.stdout, batch), out)
    });
    let mut snapshot = ForgeSnapshot::new(forge);
    for (host, page, out) in results {
        let Some(page) = page else {
            snapshot.error = Some(describe_failure(&out, forge.cli_name(), forge.display_name()));
            continue;
        };
        if let Some(login) = page.login {
            snapshot.accounts.insert(host, login);
        }
        snapshot.pull_requests.extend(page.pull_requests);
        snapshot.unreachable.extend(page.unreachable);
    }
    snapshot
}

// MARK: gh stack

/// A stack `gh stack` keeps in a repo: its branches bottom to top and, once submitted, each
/// one's PR and the stack's number on GitHub. Read from gh-stack's own file in the repo's git
/// folder (`gh-stack`), leniently: it's the extension's format, not ours.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LocalStack {
    /// GitHub's number for it; None until it's submitted.
    pub number: Option<u64>,
    /// The branch the bottom layer goes into.
    pub trunk: String,
    pub layers: Vec<StackLayer>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StackLayer {
    pub branch: String,
    /// Set once `gh stack submit` opened its PR.
    pub pull_request_number: Option<u64>,
}

impl LocalStack {
    pub fn id(&self) -> String {
        match self.number {
            Some(n) => format!("#{n}"),
            None => format!("{}:{}", self.trunk, self.layers.iter().map(|l| l.branch.as_str()).collect::<Vec<_>>().join(",")),
        }
    }

    /// The stacks in a repo's checkout (a linked worktree reads its main repo's).
    pub fn read(repo: &Path) -> Vec<LocalStack> {
        git_folder(repo).and_then(|f| std::fs::read_to_string(f.join("gh-stack")).ok()).map(|t| LocalStack::parse(&t)).unwrap_or_default()
    }

    pub fn parse(text: &str) -> Vec<LocalStack> {
        let Ok(v) = serde_json::from_str::<Value>(text) else { return vec![] };
        let Some(stacks) = v["stacks"].as_array() else { return vec![] };
        stacks
            .iter()
            .filter_map(|s| {
                let trunk = s["trunk"]["branch"].as_str().unwrap_or("main").to_string();
                let layers: Vec<StackLayer> = s["branches"]
                    .as_array()
                    .map(|bs| {
                        bs.iter()
                            .filter_map(|b| Some(StackLayer { branch: b["branch"].as_str()?.to_string(), pull_request_number: b["pullRequest"]["number"].as_u64() }))
                            .collect()
                    })
                    .unwrap_or_default();
                (!layers.is_empty()).then(|| LocalStack { number: s["number"].as_u64(), trunk, layers })
            })
            .collect()
    }
}

/// The repo's shared git folder: `.git`, or for a linked worktree the main repo's.
pub fn git_folder(repo: &Path) -> Option<PathBuf> {
    let dot_git = repo.join(".git");
    let meta = std::fs::metadata(&dot_git).ok()?;
    if meta.is_dir() {
        return Some(dot_git);
    }
    // A worktree's `.git` file points at its own folder, which says where the shared one is.
    let pointer = std::fs::read_to_string(&dot_git).ok()?;
    let own = pointer.lines().find_map(|l| l.strip_prefix("gitdir:")).map(str::trim)?;
    let own = normalize(&repo.join(own));
    match std::fs::read_to_string(own.join("commondir")) {
        Ok(common) => Some(normalize(&own.join(common.trim()))),
        Err(_) => Some(own),
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

/// GitHub's stacked pull requests, through the official `gh stack` extension. Each command
/// runs in a checkout of the repo, never waits on a prompt, and blocks until it's done (up to
/// five minutes for merge, submit and sync, which push and wait on GitHub); Err says what
/// went wrong in words.
#[derive(Clone, Debug)]
pub struct GitHubStacks {
    pub cli: Cli,
}

pub const STACK_INSTALL_COMMAND: &str = "gh extension install github/gh-stack";

/// Whether gh-stack is in gh's extensions folder (`$GH_DATA_DIR`, `$XDG_DATA_HOME/gh` or
/// `~/.local/share/gh`, then `extensions/gh-stack`): a quick look for before `gh extension list`
/// has answered, so a thread started at launch isn't told less than the next one.
pub fn stack_extension_on_disk() -> bool {
    let data = std::env::var_os("GH_DATA_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(|d| std::path::PathBuf::from(d).join("gh")))
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share/gh")));
    data.is_some_and(|d| d.join("extensions").join("gh-stack").exists())
}

impl GitHubStacks {
    pub fn new(path_env: Option<&str>) -> GitHubStacks {
        GitHubStacks { cli: Cli::new("gh", path_env) }
    }

    /// Whether `gh stack` is there to run.
    pub fn is_installed(&self) -> bool {
        self.cli.run(&["extension".into(), "list".into()], None, Duration::from_secs(15), &[]).stdout.contains("gh-stack")
    }

    pub fn install(&self) -> Result<(), String> {
        let out = self.cli.run(&["extension".into(), "install".into(), "github/gh-stack".into()], None, Duration::from_secs(120), &[]);
        if out.succeeded() { Ok(()) } else { Err(out.stderr.trim().to_string()) }
    }

    /// Merges a stack, or the layers up to and including `pull_request`, squashed. All or
    /// nothing; GitHub rebases and retargets what stays open.
    pub fn merge(&self, stack: Option<u64>, up_to: Option<u64>, repo: &Path) -> Result<(), String> {
        let mut args = vec!["stack".to_string(), "merge".into()];
        args.extend(up_to.or(stack).map(|n| n.to_string()));
        args.extend(["--squash".into(), "--yes".into()]);
        self.run(args, repo, 300)
    }

    /// Pushes the stack's branches and opens or updates their PRs, ready for review.
    pub fn submit(&self, checkout: &Path) -> Result<(), String> {
        self.run(vec!["stack".into(), "submit".into(), "--auto".into(), "--open".into()], checkout, 300)
    }

    /// Makes existing branches a stack, bottom first, onto `base` (the repo's default branch
    /// when None). Nothing goes to GitHub until it's submitted.
    pub fn create(&self, branches: &[String], base: Option<&str>, repo: &Path) -> Result<(), String> {
        let mut args = vec!["stack".to_string(), "init".into()];
        if let Some(b) = base {
            args.extend(["--base".into(), b.into()]);
        }
        args.extend(branches.iter().cloned());
        self.run(args, repo, 60)
    }

    /// A new branch on top of the stack, at HEAD of `checkout` (on the stack's top branch).
    pub fn add(&self, branch: &str, checkout: &Path) -> Result<(), String> {
        self.run(vec!["stack".into(), "add".into(), branch.into()], checkout, 60)
    }

    /// Makes PRs that already exist one GitHub stack, bottom first. Branches and PRs stay as
    /// they are; GitHub only learns they're a stack.
    pub fn link(&self, urls: &[String], repo: &Path) -> Result<(), String> {
        let mut args = vec!["stack".to_string(), "link".into()];
        args.extend(urls.iter().cloned());
        self.run(args, repo, 120)
    }

    /// Fetches, rebases what needs it, pushes and brings PR state up to date.
    pub fn sync(&self, checkout: &Path) -> Result<(), String> {
        self.run(vec!["stack".into(), "sync".into()], checkout, 300)
    }

    fn run(&self, args: Vec<String>, folder: &Path, seconds: u64) -> Result<(), String> {
        let out = self.cli.run(&args, Some(folder), Duration::from_secs(seconds), &[("GH_PROMPT_DISABLED", "1")]);
        if out.succeeded() { Ok(()) } else { Err(describe_stack_failure(&out)) }
    }
}

/// What a failed `gh stack` command means, by its documented exit codes.
pub fn describe_stack_failure(out: &CommandOutput) -> String {
    let said = out.stderr.trim();
    let detail = said.lines().last().unwrap_or("");
    if out.timed_out {
        return "gh stack didn't finish in time.".into();
    }
    if out.launch_failed {
        return "gh couldn't be started.".into();
    }
    if said.contains("official extension") || said.contains("unknown command \"stack\"") {
        return format!("gh stack isn't installed. Run `{STACK_INSTALL_COMMAND}`.");
    }
    match out.status {
        Some(2) => "That isn't a stack, or the stack wasn't found.".into(),
        Some(3) => "A rebase hit conflicts. Resolve them in the repo (or ask Claude), then run `gh stack rebase --continue`.".into(),
        Some(4) => format!("GitHub didn't accept it: {detail}"),
        Some(6) => "A branch is in more than one stack.".into(),
        Some(7) => "A rebase is already in progress in this repo.".into(),
        Some(8) => "Another gh stack command is running in this repo.".into(),
        Some(9) => "Stacked pull requests aren't turned on for this repository.".into(),
        code if detail.is_empty() => format!("gh stack exited with code {}.", code.unwrap_or(-1)),
        _ => detail.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api() -> GitRemote {
        GitRemote::new("github.com", "acme/api", Some(Forge::GitHub))
    }

    #[test]
    fn parses_a_batch_with_a_missing_repo() {
        let gone = GitRemote::new("github.com", "acme/gone", Some(Forge::GitHub));
        let page = parse(include_str!("../fixtures/github-pulls.json"), &[api(), gone.clone()]).unwrap();
        assert_eq!(page.login.as_deref(), Some("me"));
        assert_eq!(page.unreachable, [gone].into());
        assert_eq!(page.pull_requests.iter().map(|p| p.number()).collect::<Vec<_>>(), [12, 13, 9]);
        let first = &page.pull_requests[0];
        assert!(first.is_mine && first.review == "approved" && first.checks == "passing" && first.updated_at.is_some());
        assert!(first.is_in(&api()) && first.link.url == "https://github.com/acme/api/pull/12");
        assert!(page.pull_requests[1].draft && page.pull_requests[1].checks == "failing");
        let theirs = &page.pull_requests[2];
        assert!(!theirs.is_mine && theirs.review_requested_from_me && theirs.checks.is_empty());
        assert!(parse("gh: not logged in", &[api()]).is_none());
    }

    #[test]
    fn queries() {
        let q = query(&[GitRemote::new("github.com", "acme/we\"ird", Some(Forge::GitHub))]);
        assert!(q.contains(r#"name: "we\"ird""#) && q.contains("viewer { login }"));
        assert!(query(&[api()]).contains("stackEntry { position }"));
        assert!(!query(&[GitRemote::new("github.acme.com", "acme/api", Some(Forge::GitHub))]).contains("stackEntry"));
        let links = Link::find("https://github.com/acme/api/pull/5");
        assert!(query_links(&links).contains("pullRequest(number: 5)"));
    }

    #[test]
    fn reads_a_place_in_a_stack_and_linked_pulls() {
        let node = serde_json::json!({
            "number": 2, "title": "Layer B", "url": "https://github.com/acme/api/pull/2", "isDraft": false,
            "headRefName": "stack-b", "baseRefName": "stack-a", "mergeStateStatus": "CLEAN",
            "stack": {"number": 4, "size": 3}, "stackEntry": {"position": 2}, "brandNewField": {"x": 1}
        });
        let pull = pull_request(&node, &api(), Some("me")).unwrap();
        assert_eq!(pull.stack, Some(StackPlace { number: 4, size: 3, position: 2 }));
        assert_eq!(pull.merge_status.as_deref(), Some("CLEAN"));
        let mut alone = node.clone();
        alone["stack"] = Value::Null;
        alone["stackEntry"] = Value::Null;
        assert_eq!(pull_request(&alone, &api(), None).unwrap().stack, None);

        let links = Link::find("https://github.com/acme/api/pull/2 https://github.com/acme/api/pull/404");
        let mut merged = node.clone();
        merged["state"] = "MERGED".into();
        merged["mergedAt"] = "2026-10-17T15:26:56Z".into();
        merged["autoMergeRequest"] = serde_json::json!({"enabledAt": "x"});
        let answer = serde_json::json!({"data": {"viewer": {"login": "me"}, "p0": {"pullRequest": merged}, "p1": {"pullRequest": null}}}).to_string();
        let pulls = parse_links(&answer, &links);
        assert_eq!(pulls.len(), 1);
        assert!(pulls[0].state == "merged" && pulls[0].auto_merge && pulls[0].merged_at.is_some());
    }

    #[test]
    fn reads_gh_stack_files() {
        let text = r#"{"stacks": [
            {"number": 4, "trunk": {"branch": "main"}, "branches": [{"branch": "a", "pullRequest": {"number": 1}}, {"branch": "b"}]},
            {"branches": []}
        ], "version": 9}"#;
        let stacks = LocalStack::parse(text);
        assert_eq!(stacks.len(), 1);
        assert_eq!(stacks[0].id(), "#4");
        assert_eq!(stacks[0].layers[1], StackLayer { branch: "b".into(), pull_request_number: None });
        assert!(LocalStack::parse("nope").is_empty());
    }

    #[test]
    fn describes_stack_failures() {
        let out = CommandOutput { status: Some(3), ..Default::default() };
        assert!(describe_stack_failure(&out).contains("conflicts"));
        let out = CommandOutput { status: Some(1), stderr: "unknown command \"stack\" for \"gh\"".into(), ..Default::default() };
        assert!(describe_stack_failure(&out).contains("isn't installed"));
    }
}
