import Foundation

/// Open merge requests through `glab api graphql`, batched like `GitHubClient`.
public struct GitLabClient: Sendable {
    public let executable: URL
    public let environment: [String: String]
    static let batchSize = 20
    static let requestsPerProject = 20

    public init(executable: URL, environment: [String: String]) {
        self.executable = executable
        self.environment = environment
    }

    public func fetch(_ remotes: [GitRemote]) async -> ForgeSnapshot {
        await ForgeBatches.fetch(
            forge: .gitlab, remotes: remotes, defaultHost: "gitlab.com", batchSize: Self.batchSize,
            executable: executable, environment: environment, query: Self.query(for:), parse: Self.parse(_:batch:)
        )
    }

    static func query(for batch: [GitRemote]) -> String {
        let projects = batch.enumerated().map { index, remote in
            """
            p\(index): project(fullPath: \(GitHubClient.quoted(remote.path))) {
              mergeRequests(state: opened, first: \(requestsPerProject), sort: UPDATED_DESC) {
                nodes {
                  iid title webUrl draft sourceBranch targetBranch updatedAt approved
                  author { username }
                  reviewers { nodes { username } }
                  headPipeline { status }
                }
              }
            }
            """
        }
        return "query {\n  currentUser { username }\n" + projects.joined(separator: "\n") + "\n}"
    }

    static func parse(_ json: String, batch: [GitRemote]) -> GitHubClient.Page? {
        guard let object = try? JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any],
              let data = object["data"] as? [String: Any]
        else { return nil }
        var page = GitHubClient.Page()
        page.login = (data["currentUser"] as? [String: Any])?["username"] as? String
        for (index, remote) in batch.enumerated() {
            guard let project = data["p\(index)"] as? [String: Any] else {
                page.unreachable.insert(remote)
                continue
            }
            let nodes = (project["mergeRequests"] as? [String: Any])?["nodes"] as? [[String: Any]] ?? []
            page.pullRequests += nodes.compactMap { mergeRequest(from: $0, remote: remote, login: page.login) }
        }
        return page
    }

    static func mergeRequest(from node: [String: Any], remote: GitRemote, login: String?) -> PullRequest? {
        // `iid` is a String in GitLab's schema.
        guard let number = (node["iid"] as? String).flatMap(Int.init) ?? node["iid"] as? Int,
              let urlString = node["webUrl"] as? String, let url = URL(string: urlString),
              let head = node["sourceBranch"] as? String, let base = node["targetBranch"] as? String
        else { return nil }
        let author = (node["author"] as? [String: Any])?["username"] as? String
        let reviewers = ((node["reviewers"] as? [String: Any])?["nodes"] as? [[String: Any]] ?? [])
            .compactMap { $0["username"] as? String }
        let pipeline = (node["headPipeline"] as? [String: Any])?["status"] as? String

        return PullRequest(
            remote: remote,
            number: number,
            title: node["title"] as? String ?? "",
            url: url,
            author: author,
            isDraft: node["draft"] as? Bool ?? false,
            headBranch: head,
            baseBranch: base,
            updatedAt: ForgeDates.parse(node["updatedAt"]),
            review: (node["approved"] as? Bool) == true ? .approved : reviewers.isEmpty ? .none : .required,
            checks: checks(pipeline),
            isMine: login != nil && author == login,
            reviewRequestedFromMe: login.map(reviewers.contains) ?? false
        )
    }

    static func checks(_ pipeline: String?) -> PullRequest.Checks {
        switch pipeline {
        case "SUCCESS": .passing
        case "FAILED", "CANCELED": .failing
        case "RUNNING", "PENDING", "CREATED", "PREPARING", "WAITING_FOR_RESOURCE", "SCHEDULED", "MANUAL": .pending
        default: .none
        }
    }
}
