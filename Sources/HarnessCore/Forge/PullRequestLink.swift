import Foundation

/// A link to a pull request or merge request, as it shows up in a thread: printed by
/// `gh pr create`, pasted by you, cited by Claude.
public struct PullRequestLink: Sendable, Hashable, Identifiable {
    public var remote: GitRemote
    public var number: Int
    public var url: URL

    public var id: String { "\(remote.host)/\(remote.path)#\(number)" }

    public init(remote: GitRemote, number: Int, url: URL) {
        self.remote = remote
        self.number = number
        self.url = url
    }

    /// GitHub's `…/owner/repo/pull/12` and GitLab's `…/group/repo/-/merge_requests/12`.
    private static var pattern: Regex<(Substring, Substring, Substring, Substring)> { /https?:\/\/([A-Za-z0-9.\-]+)\/((?:[A-Za-z0-9_.\-]+\/)+?[A-Za-z0-9_.\-]+)\/(?:pull|-\/merge_requests)\/(\d+)/ }

    public static func find(in text: String) -> [PullRequestLink] {
        guard text.contains("/pull/") || text.contains("/merge_requests/") else { return [] }
        var seen = Set<String>(), links: [PullRequestLink] = []
        for match in text.matches(of: pattern) {
            let host = String(match.output.1).lowercased(), path = String(match.output.2)
            guard let number = Int(match.output.3), let url = URL(string: String(match.output.0)) else { continue }
            let forge: Forge = String(match.output.0).contains("/merge_requests/") ? .gitlab : .github
            let link = PullRequestLink(remote: GitRemote(host: host, path: path, forge: forge), number: number, url: url)
            if seen.insert(link.id).inserted { links.append(link) }
        }
        return links
    }

    public func matches(_ pullRequest: PullRequest) -> Bool {
        pullRequest.number == number && pullRequest.remote.host.lowercased() == remote.host
            && pullRequest.remote.path.lowercased() == remote.path.lowercased()
    }
}
