import Foundation

/// Open PRs for many repos through `gh api graphql`: one request per batch of repos instead
/// of one per repo. Uses whatever account `gh` is signed in with.
public struct GitHubClient: Sendable {
    public let executable: URL
    public let environment: [String: String]
    static let batchSize = 20
    static let pullsPerRepo = 20

    public init(executable: URL, environment: [String: String]) {
        self.executable = executable
        self.environment = environment
    }

    public func fetch(_ remotes: [GitRemote]) async -> ForgeSnapshot {
        await ForgeBatches.fetch(
            forge: .github, remotes: remotes, defaultHost: "github.com", batchSize: Self.batchSize,
            executable: executable, environment: environment, query: Self.query(for:), parse: Self.parse(_:batch:)
        )
    }

    static func query(for batch: [GitRemote]) -> String {
        let repos = batch.enumerated().map { index, remote in
            """
            r\(index): repository(owner: \(quoted(remote.owner)), name: \(quoted(remote.name))) {
              pullRequests(states: OPEN, first: \(pullsPerRepo), orderBy: {field: UPDATED_AT, direction: DESC}) {
                nodes {
                  number title url isDraft headRefName baseRefName updatedAt reviewDecision
                  author { login }
                  reviewRequests(first: 20) { nodes { requestedReviewer { ... on User { login } } } }
                  commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
                }
              }
            }
            """
        }
        return "query {\n  viewer { login }\n" + repos.joined(separator: "\n") + "\n}"
    }

    struct Page {
        var login: String?
        var pullRequests: [PullRequest] = []
        var unreachable: Set<GitRemote> = []
    }

    static func parse(_ json: String, batch: [GitRemote]) -> Page? {
        guard let object = try? JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any],
              let data = object["data"] as? [String: Any]
        else { return nil }
        var page = Page()
        page.login = (data["viewer"] as? [String: Any])?["login"] as? String
        for (index, remote) in batch.enumerated() {
            guard let repository = data["r\(index)"] as? [String: Any] else {
                page.unreachable.insert(remote)
                continue
            }
            let nodes = (repository["pullRequests"] as? [String: Any])?["nodes"] as? [[String: Any]] ?? []
            page.pullRequests += nodes.compactMap { pullRequest(from: $0, remote: remote, login: page.login) }
        }
        return page
    }

    static func pullRequest(from node: [String: Any], remote: GitRemote, login: String?) -> PullRequest? {
        guard let number = node["number"] as? Int,
              let urlString = node["url"] as? String, let url = URL(string: urlString),
              let head = node["headRefName"] as? String, let base = node["baseRefName"] as? String
        else { return nil }
        let author = (node["author"] as? [String: Any])?["login"] as? String
        let reviewers = ((node["reviewRequests"] as? [String: Any])?["nodes"] as? [[String: Any]] ?? [])
            .compactMap { ($0["requestedReviewer"] as? [String: Any])?["login"] as? String }
        let rollup = (((node["commits"] as? [String: Any])?["nodes"] as? [[String: Any]])?.first?["commit"] as? [String: Any])?["statusCheckRollup"] as? [String: Any]

        return PullRequest(
            remote: remote,
            number: number,
            title: node["title"] as? String ?? "",
            url: url,
            author: author,
            isDraft: node["isDraft"] as? Bool ?? false,
            headBranch: head,
            baseBranch: base,
            updatedAt: ForgeDates.parse(node["updatedAt"]),
            review: review(node["reviewDecision"] as? String),
            checks: checks(rollup?["state"] as? String),
            isMine: login != nil && author == login,
            reviewRequestedFromMe: login.map(reviewers.contains) ?? false
        )
    }

    static func review(_ decision: String?) -> PullRequest.Review {
        switch decision {
        case "APPROVED": .approved
        case "CHANGES_REQUESTED": .changesRequested
        case "REVIEW_REQUIRED": .required
        default: .none
        }
    }

    static func checks(_ state: String?) -> PullRequest.Checks {
        switch state {
        case "SUCCESS": .passing
        case "FAILURE", "ERROR": .failing
        case "PENDING", "EXPECTED": .pending
        default: .none
        }
    }

    static func describeFailure(_ output: CommandOutput, tool: String, service: String) -> String {
        if output.timedOut { return "\(service) didn't respond in time." }
        let message = output.stderr.trimmingCharacters(in: .whitespacesAndNewlines)
        if message.contains("auth login") || message.contains("not logged") { return "\(tool) isn't signed in. Run `\(tool) auth login`." }
        return message.split(separator: "\n").first.map(String.init) ?? "\(tool) exited with code \(output.status)."
    }

    static func quoted(_ value: String) -> String {
        let escaped = value.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"")
        return "\"\(escaped)\""
    }
}

/// Runs batched GraphQL queries through a forge CLI, a few at a time, and merges the pages.
enum ForgeBatches {
    static let concurrency = 3

    static func fetch(
        forge: Forge, remotes: [GitRemote], defaultHost: String, batchSize: Int,
        executable: URL, environment: [String: String],
        query: @escaping @Sendable ([GitRemote]) -> String,
        parse: @escaping @Sendable (String, [GitRemote]) -> GitHubClient.Page?
    ) async -> ForgeSnapshot {
        let byHost = Dictionary(grouping: Set(remotes).sorted { $0.path < $1.path }, by: \.host)
        let batches: [(host: String, remotes: [GitRemote])] = byHost.sorted { $0.key < $1.key }.flatMap { host, remotes in
            stride(from: 0, to: remotes.count, by: batchSize).map { start in
                (host, Array(remotes[start..<min(start + batchSize, remotes.count)]))
            }
        }

        let results = await withTaskGroup(of: (String, GitHubClient.Page?, CommandOutput).self) { group in
            var pending = batches.makeIterator()
            func start(_ batch: (host: String, remotes: [GitRemote])) {
                group.addTask {
                    var arguments = ["api", "graphql", "-f", "query=\(query(batch.remotes))"]
                    if batch.host != defaultHost { arguments += ["--hostname", batch.host] }
                    let output = await CommandRunner.run(
                        executable, arguments: arguments, environment: environment,
                        timeout: 30, outputLimit: 8 * 1024 * 1024
                    )
                    // The CLI exits non-zero when any repo fails to resolve, but still prints the rest.
                    return (batch.host, parse(output.stdout, batch.remotes), output)
                }
            }
            for _ in 0..<concurrency {
                guard let batch = pending.next() else { break }
                start(batch)
            }
            var results: [(String, GitHubClient.Page?, CommandOutput)] = []
            while let result = await group.next() {
                results.append(result)
                if let batch = pending.next() { start(batch) }
            }
            return results
        }

        var snapshot = ForgeSnapshot(forge: forge)
        for (host, page, output) in results {
            guard let page else {
                snapshot.error = GitHubClient.describeFailure(output, tool: forge.cliName, service: forge.displayName)
                continue
            }
            if let login = page.login { snapshot.accounts[host] = login }
            snapshot.pullRequests += page.pullRequests
            snapshot.unreachable.formUnion(page.unreachable)
        }
        return snapshot
    }
}
