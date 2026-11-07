//! PR stacks: a PR and the PRs stacked on it (`PullRequestStack`), and a stack drawn as a tree,
//! git-graph style (`StackGraph`). Pure: no CLI, nothing blocks.

use crate::forge::PullRequest;
use std::collections::{BTreeSet, HashMap, HashSet};

/// A PR plus the PRs stacked on top of it. GitHub's own stacks (`gh stack`) come as GitHub
/// orders them, merged layers included; other stacks are worked out from branches: a PR whose
/// base branch is another open PR's head branch sits on that PR (plain git, Graphite, spr).
#[derive(Clone, Debug, PartialEq)]
pub struct PullRequestStack {
    pub pull_request: PullRequest,
    pub children: Vec<PullRequestStack>,
}

impl PullRequestStack {
    pub fn id(&self) -> String {
        self.pull_request.id()
    }

    /// Bottom to top with depth, for drawing as a rail.
    pub fn flattened(&self) -> Vec<(&PullRequest, usize)> {
        let mut out = vec![(&self.pull_request, 0)];
        for child in &self.children {
            out.extend(child.flattened().into_iter().map(|(p, d)| (p, d + 1)));
        }
        out
    }

    pub fn all(&self) -> Vec<&PullRequest> {
        self.flattened().into_iter().map(|(p, _)| p).collect()
    }

    pub fn is_stacked(&self) -> bool {
        !self.children.is_empty()
    }

    /// One PR on another, no forks: the only shape a GitHub stack can take.
    pub fn is_line(&self) -> bool {
        self.flattened().iter().enumerate().all(|(i, (_, d))| i == *d)
    }

    /// GitHub's number for the stack, when it's one of GitHub's own.
    pub fn number(&self) -> Option<u64> {
        self.pull_request.stack.map(|s| s.number)
    }

    /// How many PRs the longest line up from here has.
    pub fn height(&self) -> usize {
        1 + self.children.iter().map(PullRequestStack::height).max().unwrap_or(0)
    }

    /// Groups PRs into stacks per repo. Roots keep the input order.
    pub fn build(pulls: &[PullRequest]) -> Vec<PullRequestStack> {
        let index_of = |id: &str| pulls.iter().position(|p| p.id() == id).unwrap_or(0);
        // GitHub's own stacks: a line, bottom first, in the order GitHub gives.
        let mut native: HashMap<String, Vec<&PullRequest>> = HashMap::new();
        let mut order: Vec<String> = vec![];
        for pr in pulls {
            let Some(place) = pr.stack else { continue };
            let key = format!("{}/{}#{}", pr.link.host, pr.link.path.to_lowercase(), place.number);
            if !native.contains_key(&key) {
                order.push(key.clone());
            }
            native.entry(key).or_default().push(pr);
        }
        let mut placed: Vec<(usize, PullRequestStack)> = vec![];
        for key in order {
            let mut line = native.remove(&key).unwrap_or_default();
            line.sort_by_key(|p| p.stack.map(|s| s.position).unwrap_or(0));
            let first = line.iter().map(|p| index_of(&p.id())).min().unwrap_or(0);
            let mut above: Option<PullRequestStack> = None;
            for pr in line.into_iter().rev() {
                above = Some(PullRequestStack { pull_request: pr.clone(), children: above.into_iter().collect() });
            }
            if let Some(stack) = above {
                placed.push((first, stack));
            }
        }
        let rest: Vec<PullRequest> = pulls.iter().filter(|p| p.stack.is_none()).cloned().collect();
        for stack in inferred(&rest) {
            placed.push((index_of(&stack.id()), stack));
        }
        placed.sort_by_key(|(i, _)| *i);
        placed.into_iter().map(|(_, s)| s).collect()
    }
}

/// Stacks from branches: each PR on the open PR whose head is its base.
fn inferred(pulls: &[PullRequest]) -> Vec<PullRequestStack> {
    let key = |pr: &PullRequest, branch: &str| format!("{}/{}@{}", pr.link.host, pr.link.path.to_lowercase(), branch);
    let mut by_head: HashMap<String, &PullRequest> = HashMap::new();
    for pr in pulls {
        by_head.insert(key(pr, &pr.head), pr);
    }
    let mut children: HashMap<String, Vec<&PullRequest>> = HashMap::new();
    let mut roots: Vec<&PullRequest> = vec![];
    for pr in pulls {
        match by_head.get(&key(pr, &pr.base)) {
            Some(parent) if parent.id() != pr.id() => children.entry(parent.id()).or_default().push(pr),
            _ => roots.push(pr),
        }
    }
    fn node(pr: &PullRequest, children: &HashMap<String, Vec<&PullRequest>>, visited: &mut HashSet<String>) -> PullRequestStack {
        visited.insert(pr.id());
        let mut kids = vec![];
        for child in children.get(&pr.id()).map(Vec::as_slice).unwrap_or(&[]) {
            if !visited.contains(&child.id()) {
                kids.push(node(child, children, visited));
            }
        }
        PullRequestStack { pull_request: pr.clone(), children: kids }
    }
    let mut visited = HashSet::new();
    let mut stacks: Vec<PullRequestStack> = roots.iter().map(|pr| node(pr, &children, &mut visited)).collect();
    // A cycle (A on B, B on A) has no root; show its members flat rather than drop them.
    for pr in pulls {
        if !visited.contains(&pr.id()) {
            stacks.push(node(pr, &children, &mut visited));
        }
    }
    stacks
}

/// A stack drawn as a tree, git-graph style: one row per PR, bottom of the stack first, each
/// in a lane. A line of PRs keeps its lane; a fork opens a lane to its right. Side branches
/// come right after the PR they branch off, and the main line (the longest line up, the older
/// PR on a tie) runs on past them in its lane, so every PR sits just below its base.
#[derive(Clone, Debug, PartialEq)]
pub struct StackGraph {
    pub rows: Vec<GraphRow>,
    pub lane_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Segment {
    /// A line passing through this row in a lane, top to bottom.
    Through(usize),
    /// From the top of the row down to this row's dot.
    FromAbove(usize),
    /// From this row's dot down to the bottom.
    ToBelow(usize),
    /// From a lane at the top of the row, curving over into this row's dot.
    Branch { from: usize, to: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub struct GraphRow {
    pub pull_request: PullRequest,
    /// 1 is the bottom of the stack.
    pub layer: usize,
    pub lane: usize,
    pub segments: BTreeSet<Segment>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lanes {
    /// A line of PRs shares a lane; only a fork opens one: narrow, for the sidebar.
    Compact,
    /// Every PR one step in from its base, a staircase, with side branches after the main
    /// line: for the wide repos view.
    Depth,
}

impl StackGraph {
    pub fn layout(stack: &PullRequestStack, lanes: Lanes) -> StackGraph {
        struct Placed<'a> {
            pull: &'a PullRequest,
            layer: usize,
            lane: usize,
            parent: Option<usize>,
        }
        fn visit<'a>(node: &'a PullRequestStack, lane: usize, layer: usize, parent: Option<usize>, lanes: Lanes, placed: &mut Vec<Placed<'a>>, busy: &mut HashSet<usize>) {
            let row = placed.len();
            placed.push(Placed { pull: &node.pull_request, layer, lane, parent });
            let rank = |c: &PullRequestStack| (c.height() as i64, -(c.pull_request.number() as i64));
            let mut children: Vec<&PullRequestStack> = node.children.iter().collect();
            if lanes == Lanes::Depth {
                // The longest line first; each child one step in.
                children.sort_by_key(|c| std::cmp::Reverse(rank(c)));
                for child in children {
                    visit(child, layer, layer + 1, Some(row), lanes, placed, busy);
                }
                return;
            }
            // Side branches first, shortest first; the longest line carries the lane on.
            children.sort_by_key(|c| rank(c));
            let last = children.len().saturating_sub(1);
            for (i, child) in children.into_iter().enumerate() {
                if i == last {
                    visit(child, lane, layer + 1, Some(row), lanes, placed, busy);
                } else {
                    let side = (lane + 1..).find(|l| !busy.contains(l)).unwrap();
                    busy.insert(side);
                    visit(child, side, layer + 1, Some(row), lanes, placed, busy);
                    busy.remove(&side);
                }
            }
        }
        let mut placed = vec![];
        let mut busy: HashSet<usize> = [0].into();
        visit(stack, 0, 1, None, lanes, &mut placed, &mut busy);

        let mut rows: Vec<GraphRow> = placed.iter().map(|p| GraphRow { pull_request: p.pull.clone(), layer: p.layer, lane: p.lane, segments: BTreeSet::new() }).collect();
        for (child, entry) in placed.iter().enumerate() {
            let Some(parent) = entry.parent else { continue };
            let from = placed[parent].lane;
            rows[parent].segments.insert(Segment::ToBelow(from));
            for row in rows.iter_mut().take(child).skip(parent + 1) {
                row.segments.insert(Segment::Through(from));
            }
            rows[child].segments.insert(if from == entry.lane { Segment::FromAbove(from) } else { Segment::Branch { from, to: entry.lane } });
        }
        let lane_count = placed.iter().map(|p| p.lane).max().unwrap_or(0) + 1;
        StackGraph { rows, lane_count }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::{Link, StackPlace};
    use crate::git::{Forge, GitRemote};

    fn pull(number: u64, head: &str, base: &str) -> PullRequest {
        let remote = GitRemote::new("github.com", "acme/api", Some(Forge::GitHub));
        let mut p = PullRequest::new(Link::to(&remote, number, None), head, base);
        p.title = head.into();
        p
    }

    /// PRs named by letter, each (name, base), "main" for the bottom.
    fn stack(edges: &[(&str, &str)]) -> PullRequestStack {
        let pulls: Vec<PullRequest> = edges.iter().enumerate().map(|(i, (h, b))| pull(i as u64 + 1, h, b)).collect();
        PullRequestStack::build(&pulls).remove(0)
    }

    fn picture(g: &StackGraph) -> Vec<String> {
        g.rows.iter().map(|r| format!("{}@{}", r.pull_request.title, r.lane)).collect()
    }

    fn segs(s: &[Segment]) -> BTreeSet<Segment> {
        s.iter().copied().collect()
    }

    use Segment::*;

    #[test]
    fn a_chain_is_one_lane() {
        let g = StackGraph::layout(&stack(&[("A", "main"), ("B", "A"), ("C", "B")]), Lanes::Compact);
        assert_eq!(picture(&g), ["A@0", "B@0", "C@0"]);
        assert_eq!(g.lane_count, 1);
        assert_eq!(g.rows[0].segments, segs(&[ToBelow(0)]));
        assert_eq!(g.rows[1].segments, segs(&[FromAbove(0), ToBelow(0)]));
        assert_eq!(g.rows[2].segments, segs(&[FromAbove(0)]));
        assert_eq!(g.rows.iter().map(|r| r.layer).collect::<Vec<_>>(), [1, 2, 3]);
    }

    #[test]
    fn a_fork_opens_a_lane_and_the_main_line_runs_past() {
        let g = StackGraph::layout(&stack(&[("A", "main"), ("B", "A"), ("C", "B"), ("D", "C"), ("E", "D"), ("F", "E"), ("G", "C"), ("H", "G")]), Lanes::Compact);
        assert_eq!(picture(&g), ["A@0", "B@0", "C@0", "G@1", "H@1", "D@0", "E@0", "F@0"]);
        assert_eq!(g.lane_count, 2);
        assert_eq!(g.rows[3].segments, segs(&[Branch { from: 0, to: 1 }, Through(0), ToBelow(1)]));
        assert_eq!(g.rows[4].segments, segs(&[FromAbove(1), Through(0)]));
        assert_eq!(g.rows[5].segments, segs(&[FromAbove(0), ToBelow(0)]));
        assert_eq!((g.rows[3].layer, g.rows[5].layer, g.rows[4].layer), (4, 4, 5));
    }

    #[test]
    fn the_older_line_keeps_the_lane_on_a_tie() {
        let g = StackGraph::layout(&stack(&[("A", "main"), ("B", "A"), ("C", "A")]), Lanes::Compact);
        assert_eq!(picture(&g), ["A@0", "C@1", "B@0"]);
    }

    #[test]
    fn nested_forks_get_their_own_lanes() {
        let g = StackGraph::layout(
            &stack(&[
                ("A", "main"), ("B", "A"), ("C", "B"), ("D", "C"), ("E", "D"), ("F", "E"), ("L", "F"),
                ("I", "A"),
                ("G", "C"), ("H", "G"), ("J", "G"), ("K", "J"),
            ]),
            Lanes::Compact,
        );
        assert_eq!(picture(&g), ["A@0", "I@1", "B@0", "C@0", "G@1", "H@2", "J@1", "K@1", "D@0", "E@0", "F@0", "L@0"]);
        assert_eq!(g.lane_count, 3);
        for row in g.rows.iter().filter(|r| r.lane != 0) {
            assert!(row.segments.contains(&Through(0)));
        }
        assert_eq!(g.rows[5].segments, segs(&[Branch { from: 1, to: 2 }, Through(0), Through(1)]));
    }

    #[test]
    fn depth_lanes_make_a_staircase() {
        let g = StackGraph::layout(&stack(&[("A", "main"), ("B", "A"), ("C", "B"), ("D", "C"), ("G", "B")]), Lanes::Depth);
        assert_eq!(picture(&g), ["A@0", "B@1", "C@2", "D@3", "G@2"]);
        assert_eq!(g.rows[1].segments, segs(&[Branch { from: 0, to: 1 }, ToBelow(1)]));
        assert_eq!(g.rows[2].segments, segs(&[Branch { from: 1, to: 2 }, ToBelow(2), Through(1)]));
        assert_eq!(g.rows[3].segments, segs(&[Branch { from: 2, to: 3 }, Through(1)]));
        assert_eq!(g.rows[4].segments, segs(&[Branch { from: 1, to: 2 }]));
    }

    #[test]
    fn stacks_from_base_branches() {
        let gone = GitRemote::new("github.com", "acme/gone", Some(Forge::GitHub));
        let api = GitRemote::new("github.com", "acme/api", Some(Forge::GitHub));
        let page = crate::github::parse(include_str!("../fixtures/github-pulls.json"), &[api, gone]).unwrap();
        let stacks = PullRequestStack::build(&page.pull_requests);
        assert_eq!(stacks.len(), 2);
        let s = stacks.iter().find(|s| s.pull_request.number() == 12).unwrap();
        assert!(s.is_stacked());
        assert_eq!(s.flattened().iter().map(|(p, d)| (p.number(), *d)).collect::<Vec<_>>(), [(12, 0), (13, 1)]);
    }

    #[test]
    fn same_branch_names_in_other_repos_dont_stack() {
        let pr = |repo: &str, n: u64, head: &str, base: &str| {
            let remote = GitRemote::new("github.com", &format!("acme/{repo}"), Some(Forge::GitHub));
            PullRequest::new(Link::to(&remote, n, None), head, base)
        };
        let stacks = PullRequestStack::build(&[pr("api", 1, "feature", "main"), pr("web", 2, "feature-ui", "feature"), pr("web", 3, "a", "b"), pr("web", 4, "b", "a")]);
        assert_eq!(stacks.iter().map(|s| s.all().len()).sum::<usize>(), 4);
        assert!(!stacks.iter().find(|s| s.pull_request.number() == 1).unwrap().is_stacked());
    }

    #[test]
    fn github_stacks_keep_githubs_order() {
        let with = |mut p: PullRequest, n, size, position, state: &str| {
            p.stack = Some(StackPlace { number: n, size, position });
            p.state = state.into();
            p
        };
        let pulls = [
            with(pull(3, "stack-c", "stack-b"), 4, 3, 3, "open"),
            pull(9, "solo", "main"),
            with(pull(2, "stack-b", "main"), 4, 3, 2, "open"),
            with(pull(1, "stack-a", "main"), 4, 3, 1, "merged"),
        ];
        let stacks = PullRequestStack::build(&pulls);
        assert_eq!(stacks.len(), 2);
        assert_eq!(stacks[0].number(), Some(4));
        assert_eq!(stacks[0].all().iter().map(|p| p.number()).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(stacks[1].all().iter().map(|p| p.number()).collect::<Vec<_>>(), [9]);
        assert_eq!(stacks[1].number(), None);
        let line = PullRequestStack::build(&[pull(1, "a", "main"), pull(2, "b", "a"), pull(3, "c", "b")]);
        assert!(line.len() == 1 && line[0].is_line());
        let fork = PullRequestStack::build(&[pull(1, "a", "main"), pull(2, "b", "a"), pull(3, "c", "a")]);
        assert!(fork.len() == 1 && !fork[0].is_line());
    }
}
