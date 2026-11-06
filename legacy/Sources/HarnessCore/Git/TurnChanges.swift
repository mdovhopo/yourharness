import Foundation

/// A working folder as it stood at one moment, for telling which files changed by the next:
/// whoever changed them, Claude's edit tools, a shell command, a script it ran, a commit.
/// Read from git without touching the repo's index or files: tracked files go through a copy
/// of the index into a tree object, and untracked ones are compared by size and modification
/// time rather than added (a big untracked file would be copied into the repo's objects on
/// every turn).
public struct WorkingTreeSnapshot: Sendable, Equatable {
    /// The repo's top folder.
    public let top: URL
    /// Tracked files, as git would commit them.
    let tree: String
    /// Untracked files that aren't ignored, relative to `top`; nil when there were too many
    /// to keep.
    let untracked: [String: FileStamp]?

    struct FileStamp: Sendable, Equatable {
        var size: Int64
        var modified: Date
    }

    /// More untracked files than this (a build folder nobody ignored) and they're left out.
    static let untrackedLimit = 20_000
}

extension Git {
    /// Where `folder`'s repo stands now; nil outside a repo or when git fails.
    public func snapshot(of folder: URL) async -> WorkingTreeSnapshot? {
        let top = await run(["rev-parse", "--show-toplevel"], in: folder)
        guard top.succeeded else { return nil }
        let root = URL(fileURLWithPath: top.stdout.trimmingCharacters(in: .whitespacesAndNewlines), isDirectory: true)
        guard let tree = await trackedTree(in: root) else { return nil }
        return WorkingTreeSnapshot(top: root, tree: tree, untracked: await untrackedStamps(in: root))
    }

    /// Absolute paths of the files that changed (added, edited, removed) since `start`; nil
    /// when git fails.
    public func changes(since start: WorkingTreeSnapshot) async -> [String]? {
        guard let tree = await trackedTree(in: start.top) else { return nil }
        let diff = await run(["diff-tree", "-r", "--name-only", "-z", "--no-renames", start.tree, tree], in: start.top,
                             outputLimit: 4 * 1024 * 1024)
        guard diff.succeeded else { return nil }
        var changed = Set(diff.stdout.split(separator: "\0").map(String.init))
        if let before = start.untracked, let after = await untrackedStamps(in: start.top) {
            for (path, stamp) in after where before[path] != stamp { changed.insert(path) }
            for path in before.keys where after[path] == nil { changed.insert(path) }
        }
        return changed.sorted().map { start.top.appending(path: $0).path }
    }

    /// The tracked files as a tree object, through a copy of the index: `add -u` on it only
    /// reads files whose size or time changed.
    private func trackedTree(in root: URL) async -> String? {
        let indexPath = await run(["rev-parse", "--git-path", "index"], in: root)
        guard indexPath.succeeded else { return nil }
        let path = indexPath.stdout.trimmingCharacters(in: .whitespacesAndNewlines)
        let index = path.hasPrefix("/") ? URL(fileURLWithPath: path) : root.appending(path: path)
        let copy = FileManager.default.temporaryDirectory.appending(path: "yourharness-index-\(UUID().uuidString)")
        defer {
            try? FileManager.default.removeItem(at: copy)
            try? FileManager.default.removeItem(at: copy.appendingPathExtension("lock"))
        }
        // A repo with nothing added yet has no index: an empty one is right.
        try? FileManager.default.copyItem(at: index, to: copy)
        let environment = ["GIT_INDEX_FILE": copy.path]
        guard await run(["add", "-u"], in: root, adding: environment).succeeded else { return nil }
        let tree = await run(["write-tree"], in: root, adding: environment)
        guard tree.succeeded else { return nil }
        let hash = tree.stdout.trimmingCharacters(in: .whitespacesAndNewlines)
        return hash.isEmpty ? nil : hash
    }

    private func untrackedStamps(in root: URL) async -> [String: WorkingTreeSnapshot.FileStamp]? {
        let listed = await run(["ls-files", "-z", "--others", "--exclude-standard"], in: root, outputLimit: 8 * 1024 * 1024)
        guard listed.succeeded else { return nil }
        let paths = listed.stdout.split(separator: "\0")
        guard paths.count <= WorkingTreeSnapshot.untrackedLimit else { return nil }
        var stamps: [String: WorkingTreeSnapshot.FileStamp] = [:]
        for path in paths.map(String.init) {
            guard let attributes = try? FileManager.default.attributesOfItem(atPath: root.appending(path: path).path) else { continue }
            stamps[path] = .init(size: (attributes[.size] as? NSNumber)?.int64Value ?? -1,
                                 modified: attributes[.modificationDate] as? Date ?? .distantPast)
        }
        return stamps
    }
}
