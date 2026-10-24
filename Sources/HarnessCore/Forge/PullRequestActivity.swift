import Foundation

/// What a tool call did to pull requests, read from the call itself: opening, merging,
/// editing, reviewing or commenting on one, as opposed to looking at one (`gh pr view`)
/// or mentioning it.
public struct PullRequestActivity: Sendable, Equatable {
    public enum Target: Sendable, Equatable {
        /// A PR the call names by URL, or one its output links (`gh pr create` prints it).
        case link(PullRequestLink)
        /// `gh pr merge 57`, with `-R owner/repo` when given; otherwise the repo it ran in.
        case number(Int, repo: String?)
        /// No PR named: the one of the branch it ran on.
        case currentBranch
    }

    public var targets: [Target]

    /// Nil when the call doesn't change a PR.
    public static func of(toolName: String, inputJSON: String, result: String) -> PullRequestActivity? {
        let input = (try? JSONSerialization.jsonObject(with: Data(inputJSON.utf8))) as? [String: Any] ?? [:]
        if toolName == "Bash" {
            guard let command = input["command"] as? String, var targets = commandTargets(command) else { return nil }
            let linked = PullRequestLink.find(in: result).map(Target.link)
            // `gh stack` names its PRs in its output; the branch's own PR is only a fallback.
            if !linked.isEmpty, command.contains("gh stack") { targets.removeAll { $0 == .currentBranch } }
            targets += linked
            return PullRequestActivity(targets: unique(targets))
        }
        return toolTargets(toolName: toolName, input: input, inputJSON: inputJSON, result: result)
    }

    // MARK: - Commands

    static let ghWrites: Set<String> = ["create", "merge", "edit", "ready", "close", "reopen", "review", "comment", "update-branch", "lock", "unlock"]
    /// `gh stack` commands that open, update or merge a stack's PRs: which ones, their output says.
    static let stackWrites: Set<String> = ["submit", "merge", "sync", "push", "link", "rebase", "unstack"]
    static let glabWrites: Set<String> = ["create", "merge", "update", "close", "reopen", "approve", "unapprove", "note", "rebase", "revoke"]

    /// The PRs a shell command changes; nil when it changes none.
    static func commandTargets(_ command: String) -> [Target]? {
        var targets: [Target] = [], changes = false
        for segment in segments(command) {
            let words = shellWords(segment)
            for start in words.indices {
                let tool = (words[start] as NSString).lastPathComponent
                let rest = Array(words[(start + 1)...])
                if tool == "gh", rest.first == "pr", rest.count > 1, ghWrites.contains(rest[1]) {
                    changes = true
                    targets.append(contentsOf: named(in: Array(rest.dropFirst(2))))
                } else if tool == "gh", rest.first == "stack", rest.count > 1, stackWrites.contains(rest[1]) {
                    // The PRs are the ones its output links; failing that, the branch's own.
                    changes = true
                    targets.append(.currentBranch)
                } else if tool == "glab", rest.first == "mr", rest.count > 1, glabWrites.contains(rest[1]) {
                    changes = true
                    targets.append(contentsOf: named(in: Array(rest.dropFirst(2))))
                } else if tool == "gh", rest.first == "api", let api = apiTarget(Array(rest.dropFirst())) {
                    changes = true
                    targets.append(api)
                } else {
                    continue
                }
                break
            }
        }
        return changes ? targets : nil
    }

    /// What `gh pr <verb>` was pointed at: a URL, a number (with `-R`), or nothing.
    private static func named(in args: [String]) -> [Target] {
        var repo: String?
        var index = 0
        while index < args.count {
            let arg = args[index]
            if arg == "-R" || arg == "--repo", index + 1 < args.count { repo = args[index + 1] }
            if arg.hasPrefix("--repo=") { repo = String(arg.dropFirst("--repo=".count)) }
            index += 1
        }
        for arg in args where !arg.hasPrefix("-") {
            if let link = PullRequestLink.find(in: arg).first { return [.link(link)] }
            let digits = arg.hasPrefix("#") || arg.hasPrefix("!") ? String(arg.dropFirst()) : arg
            if let number = Int(digits), number > 0 { return [.number(number, repo: repo)] }
        }
        return [.currentBranch]
    }

    /// `gh api repos/o/r/pulls/57/merge -X PUT`, or with fields (a POST).
    private static func apiTarget(_ args: [String]) -> Target? {
        guard let path = args.first(where: { $0.contains("/pulls/") }),
              let match = path.firstMatch(of: /repos\/([^\/]+\/[^\/]+)\/pulls\/(\d+)/), let number = Int(match.output.2)
        else { return nil }
        var method = "GET"
        for (index, arg) in args.enumerated() {
            if arg == "-X" || arg == "--method", index + 1 < args.count { method = args[index + 1].uppercased() }
            if arg.hasPrefix("--method=") { method = String(arg.dropFirst("--method=".count)).uppercased() }
            if ["-f", "-F", "--field", "--raw-field", "--input"].contains(arg), method == "GET" { method = "POST" }
        }
        return method == "GET" ? nil : .number(number, repo: String(match.output.1))
    }

    private static func segments(_ command: String) -> [String] {
        command.components(separatedBy: CharacterSet(charactersIn: ";|&\n")).filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
    }

    /// Words of a command line, quotes taken off: enough to read `gh` arguments.
    static func shellWords(_ text: String) -> [String] {
        var words: [String] = [], current = "", quote: Character?, inWord = false
        for character in text {
            if let open = quote {
                if character == open { quote = nil } else { current.append(character) }
            } else if character == "\"" || character == "'" {
                quote = character
                inWord = true
            } else if character.isWhitespace {
                if inWord { words.append(current) }
                current = ""
                inWord = false
            } else {
                current.append(character)
                inWord = true
            }
        }
        if inWord { words.append(current) }
        return words
    }

    // MARK: - Forge tools

    /// A forge's MCP tool (`mcp__github__merge_pull_request`, …) that changes a PR.
    private static func toolTargets(toolName: String, input: [String: Any], inputJSON: String, result: String) -> PullRequestActivity? {
        let name = toolName.lowercased()
        guard name.hasPrefix("mcp__"), name.contains("pull_request") || name.contains("merge_request"),
              ["create", "merge", "update", "review", "comment", "close", "edit", "request"].contains(where: name.contains),
              !name.contains("get_"), !name.contains("list_"), !name.contains("search_")
        else { return nil }
        var targets = PullRequestLink.find(in: inputJSON + "\n" + result).map(Target.link)
        let number = ["pullNumber", "pull_number", "pr_number", "number", "merge_request_iid", "iid"].lazy
            .compactMap { input[$0] as? Int ?? (input[$0] as? String).flatMap(Int.init) }.first
        if let number {
            let repo = (input["owner"] as? String).flatMap { owner in (input["repo"] as? String).map { "\(owner)/\($0)" } }
                ?? input["project_id"] as? String
            targets.append(.number(number, repo: repo))
        }
        return PullRequestActivity(targets: unique(targets))
    }

    private static func unique(_ targets: [Target]) -> [Target] {
        var seen: [Target] = []
        for target in targets where !seen.contains(target) { seen.append(target) }
        return seen
    }
}
