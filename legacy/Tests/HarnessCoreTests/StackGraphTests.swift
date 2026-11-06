import Foundation
import Testing
@testable import HarnessCore

struct StackGraphTests {
    private static let remote = GitRemote(host: "github.com", path: "acme/api", forge: .github)

    /// PRs named by letter; each entry is (name, base), with "main" for the bottom.
    private static func stack(_ edges: [(String, String)]) -> PullRequestStack {
        let pulls = edges.enumerated().map { index, edge in
            PullRequest(remote: remote, number: index + 1, title: edge.0, url: URL(string: "https://github.com/acme/api/pull/\(index + 1)")!,
                        author: nil, isDraft: false, headBranch: edge.0, baseBranch: edge.1, updatedAt: nil,
                        review: .none, checks: .none, isMine: true, reviewRequestedFromMe: false)
        }
        return PullRequestStack.build(pulls)[0]
    }

    private static func picture(_ graph: StackGraph) -> [String] {
        graph.rows.map { "\($0.pullRequest.title)@\($0.lane)" }
    }

    @Test func aChainIsOneLane() {
        let graph = StackGraph.layout(Self.stack([("A", "main"), ("B", "A"), ("C", "B")]))
        #expect(Self.picture(graph) == ["A@0", "B@0", "C@0"])
        #expect(graph.laneCount == 1)
        #expect(graph.rows[0].segments == [.toBelow(0)])
        #expect(graph.rows[1].segments == [.fromAbove(0), .toBelow(0)])
        #expect(graph.rows[2].segments == [.fromAbove(0)])
        #expect(graph.rows.map(\.layer) == [1, 2, 3])
    }

    /// A-B-C-D-E-F with G-H also on C: the branch follows C, the main line runs past it.
    @Test func aForkOpensALaneAndTheMainLineRunsPast() {
        let graph = StackGraph.layout(Self.stack([("A", "main"), ("B", "A"), ("C", "B"), ("D", "C"), ("E", "D"), ("F", "E"), ("G", "C"), ("H", "G")]))
        #expect(Self.picture(graph) == ["A@0", "B@0", "C@0", "G@1", "H@1", "D@0", "E@0", "F@0"])
        #expect(graph.laneCount == 2)
        let g = graph.rows[3], h = graph.rows[4], d = graph.rows[5]
        #expect(g.segments == [.branch(from: 0, to: 1), .through(0), .toBelow(1)])
        #expect(h.segments == [.fromAbove(1), .through(0)])
        #expect(d.segments == [.fromAbove(0), .toBelow(0)])
        #expect(g.layer == 4 && d.layer == 4 && h.layer == 5)
    }

    /// Forks at several levels, and a fork inside a side branch: lanes are reused once free.
    /// Two lines of the same length: the older PR keeps the lane.
    @Test func olderLineKeepsTheLaneOnATie() {
        let graph = StackGraph.layout(Self.stack([("A", "main"), ("B", "A"), ("C", "A")]))
        #expect(Self.picture(graph) == ["A@0", "C@1", "B@0"])
    }

    @Test func nestedForksGetTheirOwnLanes() {
        let graph = StackGraph.layout(Self.stack([
            ("A", "main"), ("B", "A"), ("C", "B"), ("D", "C"), ("E", "D"), ("F", "E"), ("L", "F"),
            ("I", "A"),                 // a branch right at the bottom
            ("G", "C"), ("H", "G"), ("J", "G"), ("K", "J"),   // a branch with its own fork
        ]))
        #expect(Self.picture(graph) == ["A@0", "I@1", "B@0", "C@0", "G@1", "H@2", "J@1", "K@1", "D@0", "E@0", "F@0", "L@0"])
        #expect(graph.laneCount == 3)
        // The main line passes every side row in lane 0.
        for row in graph.rows where row.lane != 0 { #expect(row.segments.contains(.through(0))) }
        // H branches off G into lane 2 while G's own line runs on in lane 1.
        #expect(graph.rows[5].segments == [.branch(from: 1, to: 2), .through(0), .through(1)])
    }

    /// The repos view's staircase: each PR a step in from its base, the main line first,
    /// and the base's line running past the main line's rows down to a later sibling.
    @Test func depthLanesMakeAStaircase() {
        let graph = StackGraph.layout(Self.stack([("A", "main"), ("B", "A"), ("C", "B"), ("D", "C"), ("G", "B")]), lanes: .depth)
        #expect(Self.picture(graph) == ["A@0", "B@1", "C@2", "D@3", "G@2"])
        #expect(graph.rows[1].segments == [.branch(from: 0, to: 1), .toBelow(1)])
        // B's line passes C's subtree on its way to G.
        #expect(graph.rows[2].segments == [.branch(from: 1, to: 2), .toBelow(2), .through(1)])
        #expect(graph.rows[3].segments == [.branch(from: 2, to: 3), .through(1)])
        #expect(graph.rows[4].segments == [.branch(from: 1, to: 2)])
    }
}
