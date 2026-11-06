//! Pull requests (GitHub) and merge requests (GitLab) as a thread knows them: links found in
//! what was said, what its tool calls did to them (opened, merged, commented on, as against
//! looked at), and what the forge's own CLI says about each now. Never the forge's API with
//! our own credentials: `gh` and `glab` do the talking.

use serde_json::Value;
use std::path::Path;
use std::process::Command;

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

    fn parse(url: &str) -> Option<Link> {
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
    /// What GitHub says is in the way: `BLOCKED`, `BEHIND`, `DIRTY`…
    pub merge_status: Option<String>,
    pub auto_merge: bool,
    pub updated_at: Option<String>,
}

const FIELDS: &str = "number,title,url,author,isDraft,headRefName,baseRefName,updatedAt,reviewDecision,statusCheckRollup,state,mergeStateStatus,autoMergeRequest";

/// A PR as `gh pr view` (or `glab mr view`) tells it now; an error when the CLI can't.
/// `YH_MOCK_PULLS=<file.json>` reads them from a file instead: `{"<url>": {…gh's JSON…}}`.
pub fn look_up(link: &Link, path_env: Option<&str>) -> Result<PullRequest, String> {
    if let Ok(file) = std::env::var("YH_MOCK_PULLS") {
        let all: Value = serde_json::from_str(&std::fs::read_to_string(&file).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let v = all.get(&link.url).ok_or("not in the mock file")?;
        return Ok(from_github(link, v));
    }
    let (tool, args): (&str, Vec<String>) = if link.gitlab {
        ("glab", vec!["mr".into(), "view".into(), link.url.clone(), "--output".into(), "json".into()])
    } else {
        ("gh", vec!["pr".into(), "view".into(), link.url.clone(), "--json".into(), FIELDS.into()])
    };
    let mut command = Command::new(tool);
    command.args(&args).current_dir(std::env::temp_dir());
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
    }
}

/// The repo a folder's `origin` points at, as `host`, `owner/repo` and whether it's GitLab.
pub fn origin(folder: &Path) -> Option<(String, String, bool)> {
    let out = Command::new("git").args(["remote", "get-url", "origin"]).current_dir(folder).output().ok()?;
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let rest = url
        .strip_prefix("git@")
        .map(|r| r.replacen(':', "/", 1))
        .or_else(|| url.strip_prefix("https://").map(str::to_string))
        .or_else(|| url.strip_prefix("ssh://git@").map(str::to_string))?;
    let (host, path) = rest.split_once('/')?;
    let path = path.trim_end_matches(".git").to_string();
    Some((host.to_lowercase(), path, host.contains("gitlab")))
}

/// The current branch of a folder's checkout.
pub fn branch(folder: &Path) -> Option<String> {
    let out = Command::new("git").args(["rev-parse", "--abbrev-ref", "HEAD"]).current_dir(folder).output().ok()?;
    let b = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!b.is_empty() && b != "HEAD").then_some(b)
}

#[cfg(test)]
mod tests {
    use super::*;

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
