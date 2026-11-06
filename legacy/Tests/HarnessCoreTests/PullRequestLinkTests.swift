import Foundation
import Testing
@testable import HarnessCore

struct PullRequestLinkTests {
    @Test func findsGitHubAndGitLabLinks() {
        let text = """
        Created https://github.com/acme/api/pull/42 and see
        https://gitlab.example.com/group/sub/web/-/merge_requests/7.
        Again: https://github.com/acme/api/pull/42#discussion_r1, not https://github.com/acme/api/issues/3
        """
        let links = PullRequestLink.find(in: text)
        #expect(links.count == 2)
        #expect(links[0].number == 42 && links[0].remote.path == "acme/api" && links[0].remote.forge == .github)
        #expect(links[1].number == 7 && links[1].remote.path == "group/sub/web" && links[1].remote.host == "gitlab.example.com")
        #expect(links[1].remote.forge == .gitlab)
        #expect(PullRequestLink.find(in: "no links here").isEmpty)
    }

    /// What `gh` prints, exiting 1, when one of the links doesn't resolve: the others are still there.
    @Test func parsesLinkedPullRequestsAroundOnesThatDontResolve() {
        let remote = GitRemote(host: "github.com", path: "acme/api", forge: .github)
        let links = [41, 42, 43].map { PullRequestLink(remote: remote, number: $0, url: URL(string: "https://github.com/acme/api/pull/\($0)")!) }
        let json = #"{"data":{"viewer":{"login":"me"},"p0":{"pullRequest":null},"p1":{"pullRequest":{"number":42,"title":"Return 429","url":"https://github.com/acme/api/pull/42","isDraft":false,"headRefName":"fix/rate-limit","baseRefName":"main","state":"MERGED","mergedAt":"2026-10-21T10:00:00Z","reviewDecision":"APPROVED","author":{"login":"you"},"commits":{"nodes":[]}}},"p2":null},"errors":[{"type":"NOT_FOUND","path":["p0","pullRequest"],"message":"Could not resolve to a PullRequest with the number of 41."}]}"#
        let pulls = GitHubClient.parse(links: json, links)
        #expect(pulls.count == 1)
        #expect(pulls.first?.number == 42 && pulls.first?.state == .merged)
    }

    @Test func parsesLinkedPullRequestsWithState() {
        let link = PullRequestLink(remote: GitRemote(host: "github.com", path: "acme/api", forge: .github), number: 42,
                                   url: URL(string: "https://github.com/acme/api/pull/42")!)
        let json = #"{"data":{"viewer":{"login":"me"},"p0":{"pullRequest":{"number":42,"title":"Return 429","url":"https://github.com/acme/api/pull/42","isDraft":false,"headRefName":"fix/rate-limit","baseRefName":"main","state":"MERGED","mergedAt":"2026-10-21T10:00:00Z","mergeStateStatus":"UNKNOWN","isInMergeQueue":false,"autoMergeRequest":{"enabledAt":"2026-10-21T09:00:00Z"},"reviewDecision":"APPROVED","author":{"login":"me"},"commits":{"nodes":[]}}}}}"#
        let pulls = GitHubClient.parse(links: json, [link])
        #expect(pulls.count == 1)
        #expect(pulls[0].state == .merged && pulls[0].isMine && pulls[0].review == .approved)
        #expect(pulls[0].autoMerge && pulls[0].mergedAt != nil && !pulls[0].inMergeQueue)
        #expect(link.matches(pulls[0]))
        #expect(GitHubClient.query(for: [link]).contains("pullRequest(number: 42)"))
    }
}
