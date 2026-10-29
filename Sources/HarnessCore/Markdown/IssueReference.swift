import Foundation

/// `#12`, `!12` (GitLab merge requests), `api#12` or `acme/api#12` in agent text. A
/// prefix like `Done!1` is found too; the linker drops it unless it names a known repo.
public struct IssueReference: Equatable, Sendable {
    /// Character offsets into the searched text.
    public var range: Range<Int>
    /// `owner/name` when the text spelled it out.
    public var path: String?
    /// A bare repo name, like `api` in `api#12`.
    public var repoName: String?
    public var number: Int
    public var isMergeRequest: Bool

    /// Made once: it runs on every paragraph rendered.
    private static let pattern = try! NSRegularExpression(pattern: #"(?<![\w/#&.-])(?:([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)|([A-Za-z0-9_.-]+))?([#!])(\d{1,7})\b"#)

    public static func find(in text: String) -> [IssueReference] {
        let regex = pattern
        let ns = text as NSString
        return regex.matches(in: text, range: NSRange(location: 0, length: ns.length)).compactMap { match in
            func group(_ index: Int) -> String? {
                let range = match.range(at: index)
                return range.location == NSNotFound ? nil : ns.substring(with: range)
            }
            guard let sigil = group(3), let digits = group(4), let number = Int(digits) else { return nil }
            guard let range = Range(match.range, in: text) else { return nil }
            let lower = text.distance(from: text.startIndex, to: range.lowerBound)
            let upper = text.distance(from: text.startIndex, to: range.upperBound)
            return IssueReference(
                range: lower..<upper, path: group(1), repoName: group(2),
                number: number, isMergeRequest: sigil == "!"
            )
        }
    }
}

/// Turns references into URLs using what the harness knows about the project: its repos'
/// remotes and their open PRs. When a bare `#12` is ambiguous across repos, a repo named
/// in the same paragraph wins; if nothing settles it, there's no link rather than a wrong one.
public struct IssueLinker: Sendable, Equatable {
    public struct Repo: Sendable, Equatable {
        public var name: String
        public var remote: GitRemote

        public init(name: String, remote: GitRemote) {
            self.name = name
            self.remote = remote
        }
    }

    public var repos: [Repo]
    public var openPullRequests: [PullRequest]

    public init(repos: [Repo], openPullRequests: [PullRequest]) {
        self.repos = repos.filter { $0.remote.forge != nil }
        self.openPullRequests = openPullRequests
    }

    public var isEmpty: Bool { repos.isEmpty }

    public func url(for reference: IssueReference, context: String) -> URL? {
        if let path = reference.path {
            let remote = repos.first { $0.remote.path.lowercased() == path.lowercased() }?.remote
                ?? GitRemote(host: "github.com", path: path, forge: .github)
            return url(reference, in: remote)
        }
        if let name = reference.repoName {
            guard let repo = repos.first(where: { matches($0, name) }) else { return nil }
            return url(reference, in: repo.remote)
        }

        let words = Set(context.lowercased().split { !$0.isLetter && !$0.isNumber && $0 != "-" && $0 != "_" && $0 != "." }.map(String.init))
        let mentioned = repos.filter { repo in words.contains(repo.name.lowercased()) || words.contains(repo.remote.name.lowercased()) }

        // `#N` is a GitHub PR or issue; on GitLab, merge requests are `!N`.
        let forge: Forge = reference.isMergeRequest ? .gitlab : .github
        let open = openPullRequests.filter { $0.number == reference.number && $0.remote.forge == forge }
        if open.count == 1 { return open[0].url }
        if open.count > 1 {
            let preferred = open.filter { pr in mentioned.contains { $0.remote == pr.remote } }
            if preferred.count == 1 { return preferred[0].url }
        }
        if mentioned.count == 1 { return url(reference, in: mentioned[0].remote) }
        if repos.count == 1 { return url(reference, in: repos[0].remote) }
        return nil
    }

    private func matches(_ repo: Repo, _ name: String) -> Bool {
        let name = name.lowercased()
        return repo.name.lowercased() == name || repo.remote.name.lowercased() == name
    }

    /// GitHub redirects `/issues/N` to `/pull/N` when N is a PR, so one URL serves both.
    private func url(_ reference: IssueReference, in remote: GitRemote) -> URL? {
        let base = "https://\(remote.host)/\(remote.path)"
        switch remote.forge {
        case .gitlab:
            return URL(string: base + (reference.isMergeRequest ? "/-/merge_requests/" : "/-/issues/") + "\(reference.number)")
        case .github, nil:
            return reference.isMergeRequest ? nil : URL(string: base + "/issues/\(reference.number)")
        }
    }
}
