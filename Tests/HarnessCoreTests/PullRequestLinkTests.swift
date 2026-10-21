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

    @Test func parsesLinkedPullRequestsWithState() {
        let link = PullRequestLink(remote: GitRemote(host: "github.com", path: "acme/api", forge: .github), number: 42,
                                   url: URL(string: "https://github.com/acme/api/pull/42")!)
        let json = #"{"data":{"viewer":{"login":"me"},"p0":{"pullRequest":{"number":42,"title":"Return 429","url":"https://github.com/acme/api/pull/42","isDraft":false,"headRefName":"fix/rate-limit","baseRefName":"main","state":"MERGED","reviewDecision":"APPROVED","author":{"login":"me"},"commits":{"nodes":[]}}}}}"#
        let pulls = GitHubClient.parse(links: json, [link])
        #expect(pulls.count == 1)
        #expect(pulls[0].state == .merged && pulls[0].isMine && pulls[0].review == .approved)
        #expect(link.matches(pulls[0]))
        #expect(GitHubClient.query(for: [link]).contains("pullRequest(number: 42)"))
    }
}
