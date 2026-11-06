import Foundation

/// A repo inside a project folder: the project's own repo, a nested clone, or one the
/// workspace references (compose `build.context`) but hasn't cloned yet.
public struct WorkspaceRepo: Sendable, Hashable, Identifiable {
    /// Path relative to the project root; empty for the root repo itself.
    public var relativePath: String
    public var url: URL
    public var isCloned: Bool

    public var id: String { relativePath }
    public var isRoot: Bool { relativePath.isEmpty }
    public var name: String { isRoot ? url.lastPathComponent : relativePath }

    public init(relativePath: String, url: URL, isCloned: Bool) {
        self.relativePath = relativePath
        self.url = url
        self.isCloned = isCloned
    }
}

public struct WorkspaceLayout: Sendable, Equatable {
    public enum Kind: Sendable, Equatable {
        /// No git anywhere we looked.
        case plain
        /// One repo, nothing nested.
        case repo
        /// A plain folder holding repos.
        case folder
        /// A repo whose job is to hold other repos (gitignored inside it).
        case shell
    }

    public var root: URL
    public var kind: Kind
    /// Root repo first (if any), then nested repos by path, then uncloned ones.
    public var repos: [WorkspaceRepo]

    public var isWorkspace: Bool { kind == .folder || kind == .shell }
    public var clonedRepos: [WorkspaceRepo] { repos.filter(\.isCloned) }
}

public enum WorkspaceScanner {
    /// Folders that never contain repos worth showing, and can be enormous.
    static let skippedNames: Set<String> = [
        "node_modules", "vendor", "Pods", "Carthage", "DerivedData", "build", "dist", "out",
        "target", "venv", "__pycache__", "bower_components", "Library", "Applications",
    ]

    /// Walks `root` looking for nested repos. Bounded by depth and by the number of folders
    /// visited, so pointing it at a huge tree stays cheap.
    public static func scan(_ root: URL, maxDepth: Int = 4, maxDirectories: Int = 5_000) -> WorkspaceLayout {
        let root = root.standardizedFileURL
        let fileManager = FileManager.default
        let rootIsRepo = isRepo(root)

        var nested: [WorkspaceRepo] = []
        var queue: [(URL, Int)] = [(root, 0)]
        var visited = 0
        while !queue.isEmpty, visited < maxDirectories {
            let (directory, depth) = queue.removeFirst()
            visited += 1
            guard depth < maxDepth,
                  let children = try? fileManager.contentsOfDirectory(
                      at: directory,
                      includingPropertiesForKeys: [.isDirectoryKey, .isSymbolicLinkKey, .isPackageKey],
                      options: [.skipsHiddenFiles]
                  )
            else { continue }
            for child in children.sorted(by: { $0.lastPathComponent < $1.lastPathComponent }) {
                guard let values = try? child.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey, .isPackageKey]),
                      values.isDirectory == true, values.isSymbolicLink != true, values.isPackage != true,
                      !skippedNames.contains(child.lastPathComponent)
                else { continue }
                if isRepo(child) {
                    nested.append(WorkspaceRepo(relativePath: relativePath(of: child, in: root), url: child, isCloned: true))
                } else {
                    queue.append((child, depth + 1))
                }
            }
        }

        let clonedPaths = Set(nested.map(\.relativePath))
        let uncloned = composeBuildContexts(in: root)
            .filter { path in
                // Skip contexts that live inside a cloned repo, or are folders the shell owns.
                !clonedPaths.contains(where: { path == $0 || path.hasPrefix($0 + "/") }) && isEmptyOrMissing(root.appending(path: path))
            }
            .map { WorkspaceRepo(relativePath: $0, url: root.appending(path: $0, directoryHint: .isDirectory), isCloned: false) }

        var repos: [WorkspaceRepo] = []
        if rootIsRepo { repos.append(WorkspaceRepo(relativePath: "", url: root, isCloned: true)) }
        repos += nested.sorted { $0.relativePath < $1.relativePath }
        repos += uncloned.sorted { $0.relativePath < $1.relativePath }

        let kind: WorkspaceLayout.Kind = switch (rootIsRepo, nested.isEmpty) {
        case (true, true): .repo
        case (true, false): .shell
        case (false, false): .folder
        case (false, true): .plain
        }
        return WorkspaceLayout(root: root, kind: kind, repos: kind == .plain ? [] : repos)
    }

    /// `.git` is a directory in a normal clone and a file in a worktree or submodule.
    static func isRepo(_ directory: URL) -> Bool {
        FileManager.default.fileExists(atPath: directory.appending(path: ".git").path)
    }

    static func relativePath(of url: URL, in root: URL) -> String {
        let rootPath = root.standardizedFileURL.path
        let path = url.standardizedFileURL.path
        guard path.hasPrefix(rootPath) else { return url.lastPathComponent }
        return String(path.dropFirst(rootPath.count)).trimmingCharacters(in: CharacterSet(charactersIn: "/"))
    }

    static func isEmptyOrMissing(_ directory: URL) -> Bool {
        guard let contents = try? FileManager.default.contentsOfDirectory(atPath: directory.path) else { return true }
        return contents.allSatisfy { $0.hasPrefix(".") }
    }

    // MARK: - Compose

    static let composeFileNames = ["compose.yaml", "compose.yml", "docker-compose.yaml", "docker-compose.yml"]

    /// Local build contexts from compose files at the root. Line-based on purpose: a full
    /// YAML parser isn't worth a dependency, and a missed entry only hides an uncloned repo.
    static func composeBuildContexts(in root: URL) -> [String] {
        var contexts: Set<String> = []
        for name in composeFileNames {
            guard let text = try? String(contentsOf: root.appending(path: name), encoding: .utf8) else { continue }
            contexts.formUnion(parseBuildContexts(compose: text))
        }
        return contexts.sorted()
    }

    static func parseBuildContexts(compose text: String) -> [String] {
        var results: [String] = []
        for rawLine in text.split(separator: "\n") {
            let line = rawLine.trimmingCharacters(in: .whitespaces)
            guard !line.hasPrefix("#") else { continue }
            let value: Substring
            if line.hasPrefix("context:") {
                value = line.dropFirst("context:".count)
            } else if line.hasPrefix("build:") {
                value = line.dropFirst("build:".count)
            } else {
                continue
            }
            var path = value.split(separator: "#", maxSplits: 1).first.map(String.init) ?? ""
            path = path.trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "\"'"))
            guard let normalized = normalize(path) else { continue }
            results.append(normalized)
        }
        return results
    }

    /// Keeps relative paths that stay inside the root; drops URLs, variables and `.`.
    static func normalize(_ path: String) -> String? {
        guard !path.isEmpty, !path.hasPrefix("/"), !path.contains("://"), !path.contains("$"), !path.hasPrefix("{") else { return nil }
        var parts: [Substring] = []
        for part in path.split(separator: "/") {
            switch part {
            case ".": continue
            case "..": return nil
            default: parts.append(part)
            }
        }
        return parts.isEmpty ? nil : parts.joined(separator: "/")
    }
}
