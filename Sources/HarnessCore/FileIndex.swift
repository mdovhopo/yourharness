import Foundation

/// A project's files and folders, for `@` mentions in the composer: paths relative to the
/// project, folders ending in "/". Ranked the way you'd expect when typing part of a name.
public struct FileIndex: Sendable {
    public let paths: [String]
    private let lowered: [String]
    private let names: [String]

    /// Big monorepos have hundreds of thousands of files; past this, nobody's scrolling.
    public static let limit = 50_000

    public init(files: some Sequence<String>) {
        var seen = Set<String>()
        var paths: [String] = []
        for file in files where !file.isEmpty {
            // Each folder on the way, so a whole directory can be mentioned too.
            var folder = ""
            for component in file.split(separator: "/").dropLast() {
                folder += component + "/"
                if seen.insert(folder).inserted { paths.append(folder) }
            }
            if seen.insert(file).inserted { paths.append(file) }
            if paths.count >= Self.limit { break }
        }
        self.paths = paths
        lowered = paths.map { $0.lowercased() }
        names = lowered.map { path in
            let trimmed = path.hasSuffix("/") ? String(path.dropLast()) : path
            return trimmed.split(separator: "/").last.map(String.init) ?? trimmed
        }
    }

    /// Everything in a folder, read off the main thread: what git sees (nothing ignored) in
    /// the folder and in each repo cloned inside it, however a workspace is laid out. Finds
    /// the repos itself, so the first listing doesn't depend on a scan having finished.
    public static func list(_ root: URL, git: Git?) async -> FileIndex {
        guard let git else { return FileIndex(files: walk(root)) }
        // A worktree inside the project is a copy of a repo: its files would make every name ambiguous.
        let nested = WorkspaceScanner.scan(root).clonedRepos.filter { !$0.isRoot && !isLinkedWorktree($0.url) }
        var files = await git.files(in: root) ?? walk(root, skipping: Set(nested.map(\.relativePath)))
        for repo in nested {
            files += (await git.files(in: repo.url) ?? []).map { repo.relativePath + "/" + $0 }
        }
        return FileIndex(files: files)
    }

    /// `.git` is a file in a worktree and in a submodule; a worktree's points into `worktrees/`.
    private static func isLinkedWorktree(_ repo: URL) -> Bool {
        guard let pointer = try? String(contentsOf: repo.appending(path: ".git"), encoding: .utf8) else { return false }
        return pointer.contains("/worktrees/")
    }

    /// Files under `root` when it isn't a git repo: hidden files and the usual build and
    /// dependency folders are skipped, and `skipping`, paths relative to it.
    public static func walk(_ root: URL, skipping: Set<String> = []) -> [String] {
        let skipped: Set<String> = ["node_modules", "build", "dist", "DerivedData", "Pods", "target", "vendor", "__pycache__"]
        guard let enumerator = FileManager.default.enumerator(
            at: root, includingPropertiesForKeys: [.isDirectoryKey], options: [.skipsHiddenFiles, .skipsPackageDescendants]) else { return [] }
        let base = root.standardizedFileURL.path.count + 1
        var files: [String] = []
        for case let url as URL in enumerator {
            if (try? url.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory == true {
                if skipped.contains(url.lastPathComponent) || skipping.contains(String(url.standardizedFileURL.path.dropFirst(base))) {
                    enumerator.skipDescendants()
                }
                continue
            }
            files.append(String(url.standardizedFileURL.path.dropFirst(base)))
            if files.count >= limit { break }
        }
        return files
    }

    /// Best first: the name itself, then names starting with it, containing it, paths
    /// containing it, and paths with its letters in order. Shorter paths win ties.
    public func matches(_ query: String, limit: Int = 40) -> [String] {
        let query = query.lowercased()
        guard !query.isEmpty else {
            // Nothing typed yet: what's at the top of the project.
            return paths.indices.sorted { (depth(paths[$0]), paths[$0]) < (depth(paths[$1]), paths[$1]) }
                .prefix(limit).map { paths[$0] }
        }
        var scored: [(score: Int, index: Int)] = []
        for index in paths.indices {
            let name = names[index], path = lowered[index]
            let score: Int
            if name == query { score = 0 }
            else if name.hasPrefix(query) { score = 1 }
            else if name.contains(query) { score = 2 }
            else if path.contains(query) { score = 3 }
            else if Self.isSubsequence(query, of: path) { score = 4 }
            else { continue }
            scored.append((score, index))
        }
        return scored
            .sorted { ($0.score, paths[$0.index].count, paths[$0.index]) < ($1.score, paths[$1.index].count, paths[$1.index]) }
            .prefix(limit).map { paths[$0.index] }
    }

    private func depth(_ path: String) -> Int {
        path.dropLast(path.hasSuffix("/") ? 1 : 0).count { $0 == "/" }
    }

    private static func isSubsequence(_ needle: String, of haystack: String) -> Bool {
        var remaining = needle[...]
        for character in haystack where character == remaining.first {
            remaining = remaining.dropFirst()
            if remaining.isEmpty { return true }
        }
        return remaining.isEmpty
    }

    /// How a path goes into a message: `@path`, quoted when it has spaces, as Claude Code reads it.
    public static func mention(_ path: String) -> String {
        path.contains(" ") ? "@\"\(path)\"" : "@\(path)"
    }

    /// The `@…` being typed at the end of `text`: where it starts, and what follows the `@`.
    public static func trailingMention(in text: String) -> (start: String.Index, query: String)? {
        guard let at = text.lastIndex(of: "@") else { return nil }
        // At the start, or after a space: `name@example.com` isn't a mention.
        if at > text.startIndex, !text[text.index(before: at)].isWhitespace { return nil }
        let query = text[text.index(after: at)...]
        guard !query.contains(where: \.isWhitespace), !query.contains("\"") else { return nil }
        return (at, String(query))
    }
}
