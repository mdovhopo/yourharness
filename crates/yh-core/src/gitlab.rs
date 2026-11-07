//! GitLab's open merge requests through `glab api graphql`, batched as GitHub's are: one query
//! per 20 projects of a host, 3 at a time, up to 30 s each. Blocks.

use crate::forge::{Link, PullRequest};
use crate::git::{Cli, Forge, GitRemote};
use crate::github::{ForgeSnapshot, Page, fetch_batches, quoted};
use serde_json::Value;

const REQUESTS_PER_PROJECT: usize = 20;

#[derive(Clone, Debug)]
pub struct GitLabClient {
    pub cli: Cli,
}

impl GitLabClient {
    pub fn new(path_env: Option<&str>) -> GitLabClient {
        GitLabClient { cli: Cli::new("glab", path_env) }
    }

    pub fn fetch(&self, remotes: &[GitRemote]) -> ForgeSnapshot {
        fetch_batches(Forge::GitLab, remotes, &self.cli, query, parse)
    }
}

/// The open MRs of a batch of projects (all on one host), `p0`, `p1`… in order.
pub fn query(batch: &[GitRemote]) -> String {
    let projects: Vec<String> = batch
        .iter()
        .enumerate()
        .map(|(i, r)| {
            format!(
                "p{i}: project(fullPath: {}) {{
  mergeRequests(state: opened, first: {REQUESTS_PER_PROJECT}, sort: UPDATED_DESC) {{
    nodes {{
      iid title webUrl draft sourceBranch targetBranch updatedAt approved
      author {{ username }}
      reviewers {{ nodes {{ username }} }}
      headPipeline {{ status }}
    }}
  }}
}}",
                quoted(&r.path)
            )
        })
        .collect();
    format!("query {{\n  currentUser {{ username }}\n{}\n}}", projects.join("\n"))
}

pub fn parse(json: &str, batch: &[GitRemote]) -> Option<Page> {
    let v: Value = serde_json::from_str(json).ok()?;
    let data = v.get("data")?.as_object()?;
    let mut page = Page { login: data.get("currentUser").and_then(|u| u["username"].as_str()).map(str::to_string), ..Default::default() };
    for (i, remote) in batch.iter().enumerate() {
        let Some(project) = data.get(&format!("p{i}")).filter(|p| p.is_object()) else {
            page.unreachable.insert(remote.clone());
            continue;
        };
        let nodes = project["mergeRequests"]["nodes"].as_array().cloned().unwrap_or_default();
        page.pull_requests.extend(nodes.iter().filter_map(|n| merge_request(n, remote, page.login.as_deref())));
    }
    Some(page)
}

pub fn merge_request(node: &Value, remote: &GitRemote, login: Option<&str>) -> Option<PullRequest> {
    // `iid` is a String in GitLab's schema.
    let number = node["iid"].as_str().and_then(|s| s.parse().ok()).or_else(|| node["iid"].as_u64())?;
    let url = node["webUrl"].as_str()?;
    let (head, base) = (node["sourceBranch"].as_str()?, node["targetBranch"].as_str()?);
    let author = node["author"]["username"].as_str().map(str::to_string);
    let reviewers: Vec<&str> = node["reviewers"]["nodes"].as_array().map(|a| a.iter().filter_map(|n| n["username"].as_str()).collect()).unwrap_or_default();
    let mut remote = remote.clone();
    remote.forge = Some(Forge::GitLab);
    let mut pull = PullRequest::new(Link::to(&remote, number, Some(url)), head, base);
    pull.title = node["title"].as_str().unwrap_or("").to_string();
    pull.is_mine = login.is_some() && author.as_deref() == login;
    pull.author = author;
    pull.draft = node["draft"].as_bool().unwrap_or(false);
    pull.updated_at = node["updatedAt"].as_str().map(str::to_string);
    pull.review = if node["approved"].as_bool() == Some(true) {
        "approved"
    } else if reviewers.is_empty() {
        ""
    } else {
        "required"
    }
    .into();
    pull.checks = checks(node["headPipeline"]["status"].as_str()).into();
    pull.review_requested_from_me = login.is_some_and(|l| reviewers.contains(&l));
    Some(pull)
}

pub fn checks(pipeline: Option<&str>) -> &'static str {
    match pipeline {
        Some("SUCCESS") => "passing",
        Some("FAILED" | "CANCELED") => "failing",
        Some("RUNNING" | "PENDING" | "CREATED" | "PREPARING" | "WAITING_FOR_RESOURCE" | "SCHEDULED" | "MANUAL") => "pending",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_merge_requests() {
        let hooks = GitRemote::new("gitlab.com", "acme/platform/hooks", Some(Forge::GitLab));
        let other = GitRemote::new("gitlab.com", "acme/other", Some(Forge::GitLab));
        let page = parse(include_str!("../fixtures/gitlab-merge-requests.json"), &[hooks.clone(), other.clone()]).unwrap();
        assert_eq!(page.login.as_deref(), Some("me"));
        assert_eq!(page.unreachable, [other].into());
        let mr = &page.pull_requests[0];
        assert_eq!(mr.number(), 41);
        assert!(mr.link.gitlab && mr.is_in(&hooks));
        assert_eq!((mr.checks.as_str(), mr.review.as_str()), ("pending", "required"));
        assert!(mr.review_requested_from_me && !mr.is_mine);
        assert!(query(&[hooks]).contains(r#"project(fullPath: "acme/platform/hooks")"#));
    }
}
