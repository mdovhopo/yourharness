import Foundation

/// A PR plus the PRs stacked on top of it. Stacks are discovered from forge data alone:
/// a PR whose base branch is another open PR's head branch sits on that PR. Works however
/// the stack was made (plain git, Graphite, spr, forge-native).
public struct PullRequestStack: Sendable, Hashable, Identifiable {
    public var pullRequest: PullRequest
    public var children: [PullRequestStack]

    public var id: String { pullRequest.id }

    /// Bottom-to-top with depth, for rendering as a rail.
    public var flattened: [(pullRequest: PullRequest, depth: Int)] {
        [(pullRequest, 0)] + children.flatMap { $0.flattened.map { ($0.pullRequest, $0.depth + 1) } }
    }

    public var all: [PullRequest] { flattened.map(\.pullRequest) }
    public var isStacked: Bool { !children.isEmpty }

    /// Groups PRs into stacks per repo. Roots keep the input order.
    public static func build(_ pullRequests: [PullRequest]) -> [PullRequestStack] {
        var byHead: [String: PullRequest] = [:]
        for pr in pullRequests {
            byHead[key(pr.remote, pr.headBranch)] = pr
        }
        var children: [String: [PullRequest]] = [:]
        var roots: [PullRequest] = []
        for pr in pullRequests {
            if let parent = byHead[key(pr.remote, pr.baseBranch)], parent.id != pr.id {
                children[parent.id, default: []].append(pr)
            } else {
                roots.append(pr)
            }
        }

        var visited: Set<String> = []
        func node(_ pr: PullRequest) -> PullRequestStack {
            visited.insert(pr.id)
            let kids = (children[pr.id] ?? []).filter { !visited.contains($0.id) }.map(node)
            return PullRequestStack(pullRequest: pr, children: kids)
        }
        var stacks = roots.map(node)
        // A cycle (A on B, B on A) has no root; show its members flat rather than drop them.
        for pr in pullRequests where !visited.contains(pr.id) {
            stacks.append(node(pr))
        }
        return stacks
    }

    private static func key(_ remote: GitRemote, _ branch: String) -> String {
        "\(remote.host)/\(remote.path)@\(branch)"
    }
}
