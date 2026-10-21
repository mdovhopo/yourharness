import Foundation

/// A stack drawn as a tree, git-graph style: one row per PR, bottom of the stack first,
/// each in a lane. A line of PRs keeps its lane; a fork opens a lane to its right. Side
/// branches come right after the PR they branch off, and the main line (the longest line
/// up, the older PR on a tie) runs on past them in its lane, so every PR sits just below
/// its base.
public struct StackGraph: Sendable, Equatable {
    public enum Segment: Hashable, Sendable {
        /// A line passing through this row in a lane, top to bottom.
        case through(Int)
        /// From the top of the row down to this row's dot.
        case fromAbove(Int)
        /// From this row's dot down to the bottom.
        case toBelow(Int)
        /// From a lane at the top of the row, curving over into this row's dot.
        case branch(from: Int, to: Int)
    }

    public struct Row: Sendable, Equatable {
        public let pullRequest: PullRequest
        /// 1 is the bottom of the stack.
        public let layer: Int
        public let lane: Int
        public internal(set) var segments: Set<Segment>
    }

    public let rows: [Row]
    public let laneCount: Int

    /// How many PRs the longest line up from here has.
    static func height(_ node: PullRequestStack) -> Int {
        1 + (node.children.map(height).max() ?? 0)
    }

    public static func layout(_ stack: PullRequestStack) -> StackGraph {
        var placed: [(pull: PullRequest, layer: Int, lane: Int, parent: Int?)] = []
        var busy: Set<Int> = [0]

        func visit(_ node: PullRequestStack, lane: Int, layer: Int, parent: Int?) {
            let row = placed.count
            placed.append((node.pullRequest, layer, lane, parent))
            // Side branches first, shortest first; the longest line carries the lane on.
            let children = node.children.sorted { (height($0), -$0.pullRequest.number) < (height($1), -$1.pullRequest.number) }
            for (index, child) in children.enumerated() {
                if index == children.count - 1 {
                    visit(child, lane: lane, layer: layer + 1, parent: row)
                } else {
                    let side = ((lane + 1)...).first { !busy.contains($0) }!
                    busy.insert(side)
                    visit(child, lane: side, layer: layer + 1, parent: row)
                    busy.remove(side)
                }
            }
        }
        visit(stack, lane: 0, layer: 1, parent: nil)

        var rows = placed.map { Row(pullRequest: $0.pull, layer: $0.layer, lane: $0.lane, segments: []) }
        for (child, entry) in placed.enumerated() {
            guard let parent = entry.parent else { continue }
            let from = placed[parent].lane
            rows[parent].segments.insert(.toBelow(from))
            for between in (parent + 1)..<child { rows[between].segments.insert(.through(from)) }
            rows[child].segments.insert(from == entry.lane ? .fromAbove(entry.lane) : .branch(from: from, to: entry.lane))
        }
        return StackGraph(rows: rows, laneCount: (placed.map(\.lane).max() ?? 0) + 1)
    }
}
