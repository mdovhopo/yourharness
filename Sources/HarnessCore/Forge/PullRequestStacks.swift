import Foundation

/// A PR plus the PRs stacked on top of it. GitHub's own stacks (`gh stack`) come as GitHub
/// orders them, merged layers included; other stacks are worked out from branches: a PR
/// whose base branch is another open PR's head branch sits on that PR (plain git,
/// Graphite, spr).
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

    /// GitHub's number for the stack, when it's one of GitHub's own.
    public var number: Int? { pullRequest.stack?.number }

    /// Groups PRs into stacks per repo. Roots keep the input order.
    public static func build(_ pullRequests: [PullRequest]) -> [PullRequestStack] {
        // GitHub's own stacks: a line, bottom first, in the order GitHub gives.
        var native: [String: [PullRequest]] = [:]
        var order: [String] = []
        for pr in pullRequests {
            guard let place = pr.stack else { continue }
            let key = "\(pr.remote.host)/\(pr.remote.path)#\(place.number)"
            if native[key] == nil { order.append(key) }
            native[key, default: []].append(pr)
        }
        let stacked = order.compactMap { key -> (first: Int, stack: PullRequestStack)? in
            let line = (native[key] ?? []).sorted { $0.stack!.position < $1.stack!.position }
            guard let bottom = line.first else { return nil }
            let stack = line.dropFirst().reversed().reduce(nil as PullRequestStack?) { above, pr in
                PullRequestStack(pullRequest: pr, children: above.map { [$0] } ?? [])
            }
            let first = line.compactMap { pr in pullRequests.firstIndex { $0.id == pr.id } }.min() ?? 0
            return (first, PullRequestStack(pullRequest: bottom, children: stack.map { [$0] } ?? []))
        }
        let inferred = inferred(pullRequests.filter { $0.stack == nil })
            .map { stack in (pullRequests.firstIndex { $0.id == stack.id } ?? 0, stack) }
        return (stacked + inferred).sorted { $0.0 < $1.0 }.map(\.1)
    }

    /// Stacks from branches: each PR on the open PR whose head is its base.
    private static func inferred(_ pullRequests: [PullRequest]) -> [PullRequestStack] {
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
