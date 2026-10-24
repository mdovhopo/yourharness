import Foundation

/// A stack `gh stack` keeps in a repo: its branches bottom to top and, once submitted,
/// each one's PR and the stack's number on GitHub. Read from gh-stack's own file in the
/// repo's git folder (`gh-stack`), leniently: it's the extension's format, not ours.
public struct LocalStack: Sendable, Hashable, Identifiable {
    public struct Layer: Sendable, Hashable {
        public var branch: String
        /// Set once `gh stack submit` opened its PR.
        public var pullRequestNumber: Int?

        public init(branch: String, pullRequestNumber: Int? = nil) {
            self.branch = branch
            self.pullRequestNumber = pullRequestNumber
        }
    }

    /// GitHub's number for it; nil until it's submitted.
    public var number: Int?
    /// The branch the bottom layer goes into.
    public var trunk: String
    public var layers: [Layer]

    public var id: String { number.map { "#\($0)" } ?? trunk + ":" + layers.map(\.branch).joined(separator: ",") }

    public init(number: Int?, trunk: String, layers: [Layer]) {
        self.number = number
        self.trunk = trunk
        self.layers = layers
    }

    /// The stacks in a repo's checkout (a linked worktree reads its main repo's).
    public static func read(in repo: URL) -> [LocalStack] {
        guard let folder = gitFolder(of: repo), let data = try? Data(contentsOf: folder.appending(path: "gh-stack")) else { return [] }
        return parse(data)
    }

    public static func parse(_ data: Data) -> [LocalStack] {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let stacks = object["stacks"] as? [[String: Any]] else { return [] }
        return stacks.compactMap { stack in
            let trunk = (stack["trunk"] as? [String: Any])?["branch"] as? String ?? "main"
            let layers = (stack["branches"] as? [[String: Any]] ?? []).compactMap { branch -> Layer? in
                guard let name = branch["branch"] as? String else { return nil }
                return Layer(branch: name, pullRequestNumber: (branch["pullRequest"] as? [String: Any])?["number"] as? Int)
            }
            guard !layers.isEmpty else { return nil }
            return LocalStack(number: stack["number"] as? Int, trunk: trunk, layers: layers)
        }
    }

    /// The repo's shared git folder: `.git`, or for a linked worktree the main repo's.
    static func gitFolder(of repo: URL) -> URL? {
        let dotGit = repo.appending(path: ".git")
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: dotGit.path, isDirectory: &isDirectory) else { return nil }
        if isDirectory.boolValue { return dotGit }
        // A worktree's `.git` file points at its own folder, which says where the shared one is.
        guard let pointer = try? String(contentsOf: dotGit, encoding: .utf8),
              let line = pointer.split(whereSeparator: \.isNewline).first(where: { $0.hasPrefix("gitdir:") }) else { return nil }
        let own = URL(fileURLWithPath: line.dropFirst("gitdir:".count).trimmingCharacters(in: .whitespaces), relativeTo: repo).standardizedFileURL
        guard let common = try? String(contentsOf: own.appending(path: "commondir"), encoding: .utf8) else { return own }
        return URL(fileURLWithPath: common.trimmingCharacters(in: .whitespacesAndNewlines), relativeTo: own.appending(path: "/")).standardizedFileURL
    }
}

/// GitHub's stacked pull requests, through the official `gh stack` extension. Each command
/// runs in a checkout of the repo, never waits on a prompt, and returns nil when it worked,
/// else what went wrong in words.
public struct GitHubStacks: Sendable {
    public let executable: URL
    public let environment: [String: String]

    public init(executable: URL, environment: [String: String]) {
        self.executable = executable
        self.environment = environment
    }

    public static let installCommand = "gh extension install github/gh-stack"

    /// Whether `gh stack` is there to run.
    public func isInstalled() async -> Bool {
        let output = await CommandRunner.run(executable, arguments: ["extension", "list"], environment: environment, timeout: 15)
        return output.stdout.contains("gh-stack")
    }

    /// Merges a stack, or the layers up to and including `pullRequest`, squashed. All or
    /// nothing; GitHub rebases and retargets what stays open.
    public func merge(stack: Int?, upTo pullRequest: Int?, in repo: URL) async -> String? {
        let target = pullRequest ?? stack
        return await run(["stack", "merge"] + (target.map { ["\($0)"] } ?? []) + ["--squash", "--yes"], in: repo, timeout: 300)
    }

    /// Pushes the stack's branches and opens or updates their PRs, ready for review.
    public func submit(in checkout: URL) async -> String? {
        await run(["stack", "submit", "--auto", "--open"], in: checkout, timeout: 300)
    }

    /// Makes existing branches a stack, bottom first, onto `base` (the repo's default
    /// branch when nil). Nothing goes to GitHub until it's submitted.
    public func create(branches: [String], base: String?, in repo: URL) async -> String? {
        await run(["stack", "init"] + (base.map { ["--base", $0] } ?? []) + branches, in: repo, timeout: 60)
    }

    /// A new branch on top of the stack, at HEAD of `checkout` (on the stack's top branch).
    public func add(branch: String, in checkout: URL) async -> String? {
        await run(["stack", "add", branch], in: checkout, timeout: 60)
    }

    /// Makes PRs that already exist one GitHub stack, bottom first. Branches and PRs stay as
    /// they are; GitHub only learns they're a stack.
    public func link(_ pullRequests: [URL], in repo: URL) async -> String? {
        await run(["stack", "link"] + pullRequests.map(\.absoluteString), in: repo, timeout: 120)
    }

    /// Fetches, rebases what needs it, pushes and brings PR state up to date.
    public func sync(in checkout: URL) async -> String? {
        await run(["stack", "sync"], in: checkout, timeout: 300)
    }

    private func run(_ arguments: [String], in folder: URL, timeout: TimeInterval) async -> String? {
        let output = await CommandRunner.run(executable, arguments: arguments,
                                             environment: environment.merging(["GH_PROMPT_DISABLED": "1"]) { $1 },
                                             timeout: timeout, currentDirectory: folder)
        return output.succeeded ? nil : Self.describe(output)
    }

    /// What a failed `gh stack` command means, by its documented exit codes.
    static func describe(_ output: CommandOutput) -> String {
        let said = output.stderr.trimmingCharacters(in: .whitespacesAndNewlines)
        let detail = said.split(whereSeparator: \.isNewline).last.map(String.init) ?? ""
        if output.timedOut { return "gh stack didn't finish in time." }
        if output.launchFailed { return "gh couldn't be started." }
        if said.contains("official extension") || said.contains("unknown command \"stack\"") {
            return "gh stack isn't installed. Run `\(installCommand)`."
        }
        switch output.status {
        case 2: return "That isn't a stack, or the stack wasn't found."
        case 3: return "A rebase hit conflicts. Resolve them in the repo (or ask Claude), then run `gh stack rebase --continue`."
        case 4: return "GitHub didn't accept it: \(detail)"
        case 6: return "A branch is in more than one stack."
        case 7: return "A rebase is already in progress in this repo."
        case 8: return "Another gh stack command is running in this repo."
        case 9: return "Stacked pull requests aren't turned on for this repository."
        default: return detail.isEmpty ? "gh stack exited with code \(output.status)." : detail
        }
    }
}
