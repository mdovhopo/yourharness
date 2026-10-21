import Foundation

/// Where a thread has been working: the folders its tool calls touched, from which the
/// repos and worktrees it works in are worked out. Discovered, never declared: a thread
/// can work in any number of them.
public enum ThreadFootprint {
    /// Kept per thread, newest last.
    public static let limit = 300

    /// Absolute folders one tool call touched. Files count as their folder; shell commands
    /// give whatever path-like words they contain, resolved against `workingDirectory`.
    public static func folders(toolName: String, inputJSON: String, workingDirectory: String) -> [String] {
        guard let input = (try? JSONSerialization.jsonObject(with: Data(inputJSON.utf8))) as? [String: Any] else { return [] }
        var found: [String] = []
        for key in ["file_path", "notebook_path"] {
            if let path = input[key] as? String, let absolute = resolve(path, in: workingDirectory) {
                found.append((absolute as NSString).deletingLastPathComponent)
            }
        }
        if let path = input["path"] as? String, let absolute = resolve(path, in: workingDirectory) { found.append(absolute) }
        if toolName == "Bash", let command = input["command"] as? String {
            found += pathWords(in: command).filter { isPath($0, in: workingDirectory) }.compactMap { resolve($0, in: workingDirectory) }
        }
        return unique(found)
    }

    /// Absolute paths in a tool's result: where `EnterWorktree` put the session, say.
    public static func folders(inResult result: String) -> [String] {
        unique(pathWords(in: result).filter { $0.hasPrefix("/") }.compactMap { word in
            // The end of a sentence isn't part of the path.
            resolve(word.trimmingCharacters(in: CharacterSet(charactersIn: ".:")), in: "/")
        })
    }

    /// Adds `new` to `existing`, most recent last, without repeats, within the limit.
    public static func merge(_ new: [String], into existing: [String]) -> [String] {
        guard !new.isEmpty else { return existing }
        let fresh = Set(new)
        let kept = existing.filter { !fresh.contains($0) } + new
        return Array(kept.suffix(limit))
    }

    /// Whether any of `folders` is `root` or inside it.
    public static func touches(_ folders: [String], root: String) -> Bool {
        let prefix = root.hasSuffix("/") ? root : root + "/"
        return folders.contains { $0 == root || $0.hasPrefix(prefix) }
    }

    static func pathWords(in text: String) -> [String] {
        let separators = CharacterSet.whitespacesAndNewlines.union(CharacterSet(charactersIn: ";|&()<>\"'`,"))
        return text.components(separatedBy: separators).compactMap { word -> String? in
            var word = word
            // --prefix=/x, cwd=…
            if let equals = word.firstIndex(of: "="), word.hasPrefix("-") || word[..<equals].allSatisfy({ $0.isLetter || $0 == "_" }) {
                word = String(word[word.index(after: equals)...])
            }
            guard word.contains("/"), !word.hasPrefix("-"), !word.contains("://"), !word.contains("$"),
                  !word.contains("*"), word.count < 400 else { return nil }
            return word
        }
        .prefix(40).map { $0 }
    }

    /// In a command, `feature/login` is as likely a branch as a folder. A word counts when it
    /// is clearly a path (`/…`, `~/…`, `./…`, `../…`) or starts with something that's there.
    static func isPath(_ word: String, in workingDirectory: String) -> Bool {
        if ["/", "~", "./", "../"].contains(where: { word.hasPrefix($0) }) { return true }
        guard let first = word.split(separator: "/").first else { return false }
        return FileManager.default.fileExists(atPath: (workingDirectory as NSString).appendingPathComponent(String(first)))
    }

    /// Absolute and standardized, with `/tmp` and `/var` as their real `/private` paths.
    public static func resolve(_ path: String, in workingDirectory: String) -> String? {
        guard !path.isEmpty else { return nil }
        let expanded = (path as NSString).expandingTildeInPath
        let absolute = expanded.hasPrefix("/") ? expanded : (workingDirectory as NSString).appendingPathComponent(expanded)
        var standardized = (absolute as NSString).standardizingPath
        // /tmp and /var are links into /private; git reports the real paths.
        for link in ["/tmp", "/var"] where standardized == link || standardized.hasPrefix(link + "/") {
            standardized = "/private" + standardized
        }
        while standardized.count > 1, standardized.hasSuffix("/") { standardized.removeLast() }
        return standardized.count > 1 ? standardized : nil
    }

    private static func unique(_ paths: [String]) -> [String] {
        var seen = Set<String>()
        return paths.filter { seen.insert($0).inserted }
    }
}
