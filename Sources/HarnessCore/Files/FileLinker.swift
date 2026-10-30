import Foundation

/// A file or folder a message mentions, and where in it: `Sources/App.swift:12`,
/// `App.swift:12-20`, `App.swift#L12`, `/abs/path/App.swift`, `@docs/notes.md`.
public struct FileReference: Sendable, Hashable {
    /// Character offsets into the searched text.
    public var range: Range<Int>
    /// Relative to the linker's root. Folders end in "/".
    public var path: String
    public var line: Int?
    public var endLine: Int?

    public var isFolder: Bool { path.hasSuffix("/") }
}

/// Finds the paths in agent text that are files of the thread's folder, from its file index
/// alone: nothing is read from disk, so it can run on every paragraph rendered. A name that
/// could be several files isn't linked rather than linked to the wrong one.
public struct FileLinker: Sendable, Equatable {
    /// The folder paths are relative to, standardized, without a trailing "/".
    public let root: String
    /// Changes only when the files do, so text linked against it is built again only then.
    public let revision: Int
    /// Every file is in it: a big monorepo's index stops at `FileIndex.limit`.
    public let isComplete: Bool
    /// Held by reference: the linker goes through SwiftUI's environment, which compares
    /// values to see what changed, and comparing tens of thousands of paths each time made
    /// switching threads slower.
    private let tables: Tables

    private final class Tables: Sendable {
        let paths: Set<String>
        /// Last path component (folders without their "/") to every path with it.
        let byName: [String: [String]]

        init(paths: Set<String>, byName: [String: [String]]) {
            self.paths = paths
            self.byName = byName
        }
    }

    private var paths: Set<String> { tables.paths }
    private var byName: [String: [String]] { tables.byName }

    /// The same files in the same folder: what the revision says, without comparing them.
    public static func == (lhs: FileLinker, rhs: FileLinker) -> Bool {
        lhs.revision == rhs.revision && lhs.root == rhs.root && lhs.isComplete == rhs.isComplete
    }

    public static let empty = FileLinker(root: "", files: [])

    public init(root: String, index: FileIndex) {
        self.init(root: root, files: index.paths, isComplete: index.paths.count < FileIndex.limit)
    }

    public init(root: String, files: [String], isComplete: Bool = true) {
        self.isComplete = isComplete
        let root = root.hasSuffix("/") && root.count > 1 ? String(root.dropLast()) : root
        self.root = root
        var byName: [String: [String]] = [:]
        var hasher = Hasher()
        hasher.combine(root)
        for path in files {
            hasher.combine(path)
            let trimmed = path.hasSuffix("/") ? path.dropLast() : path[...]
            let name = trimmed.lastIndex(of: "/").map { String(trimmed[trimmed.index(after: $0)...]) } ?? String(trimmed)
            byName[name, default: []].append(path)
        }
        tables = Tables(paths: Set(files), byName: byName)
        revision = files.isEmpty ? 0 : hasher.finalize()
    }

    public var isEmpty: Bool { paths.isEmpty }

    /// Whether the index has this path (folders end in "/"): what git doesn't ignore.
    public func contains(_ path: String) -> Bool { paths.contains(path) }

    /// The absolute path of a path relative to the root.
    public func absolute(_ path: String) -> String {
        root + "/" + (path.hasSuffix("/") ? String(path.dropLast()) : path)
    }

    /// Made once: it runs on every paragraph rendered. A run of path characters, then a line
    /// (`:12`, `:12-20`, `:12:5`, `#L12`, `#L12-L20`).
    private static let pattern = try! NSRegularExpression(pattern: #"(?<![\w.~/@+\-])([\w~@+\-]*[./][\w.~/@+\-]*)(?::(\d{1,7})(?:[-–](\d{1,7})|:\d{1,5})?|#L(\d{1,7})(?:-L?(\d{1,7}))?)?"#)

    /// Whether `text` could hold a path at all: most sentences can be skipped outright.
    public static func mightMention(_ text: String) -> Bool {
        text.utf8.contains { $0 == UInt8(ascii: "/") || $0 == UInt8(ascii: ".") }
    }

    public func references(in text: String) -> [FileReference] {
        guard !paths.isEmpty, Self.mightMention(text) else { return [] }
        let ns = text as NSString
        var found: [FileReference] = []
        // Character offsets, worked out walking forward once.
        var index = text.startIndex
        var offset = 0
        func characterOffset(_ utf16: Int) -> Int? {
            guard let target = String.Index(utf16Offset: utf16, in: text).samePosition(in: text) else { return nil }
            guard target >= index else { return nil }
            offset += text.distance(from: index, to: target)
            index = target
            return offset
        }
        for match in Self.pattern.matches(in: text, range: NSRange(location: 0, length: ns.length)) {
            var token = match.range(at: 1)
            // Trim what ends a sentence or opens a mention: "see App.swift.", "@App.swift".
            while token.length > 0, ".,;:!?'\"".utf16.contains(ns.character(at: token.location + token.length - 1)) {
                token.length -= 1
            }
            while token.length > 0, ns.character(at: token.location) == UInt16(UInt8(ascii: "@")) {
                token.location += 1
                token.length -= 1
            }
            guard token.length > 1 else { continue }
            let candidate = ns.substring(with: token)
            guard candidate.contains("/") || candidate.contains("."), !candidate.hasPrefix("//"),
                  let path = resolve(candidate) else { continue }
            func number(_ group: Int) -> Int? {
                let range = match.range(at: group)
                return range.location == NSNotFound ? nil : Int(ns.substring(with: range))
            }
            // The line goes with the path only when nothing was trimmed between them.
            let trimmedEnd = token.location + token.length == match.range(at: 1).location + match.range(at: 1).length
            let line = trimmedEnd ? (number(2) ?? number(4)) : nil
            let endLine = line.flatMap { line in (number(3) ?? number(5)).flatMap { $0 >= line ? $0 : nil } }
            let end = line == nil ? token.location + token.length : match.range.location + match.range.length
            guard let lower = characterOffset(token.location), let upper = characterOffset(end) else { continue }
            found.append(FileReference(range: lower..<upper, path: path, line: line, endLine: endLine))
        }
        return found
    }

    /// The path, relative to the root, that `candidate` names, if it's one of the root's.
    public func resolve(_ candidate: String) -> String? {
        var relative = candidate
        if relative.hasPrefix("~/") { relative = NSHomeDirectory() + relative.dropFirst() }
        if relative.hasPrefix("/") {
            // Outside the folder: never linked, so nothing outside it is ever opened.
            guard relative.hasPrefix(root + "/") else { return nil }
            relative = String(relative.dropFirst(root.count + 1))
        }
        while relative.hasPrefix("./") { relative = String(relative.dropFirst(2)) }
        guard !relative.isEmpty, !relative.hasPrefix("../") else { return nil }
        let bare = relative.hasSuffix("/") ? String(relative.dropLast()) : relative
        if !relative.hasSuffix("/"), paths.contains(bare) { return bare }
        if paths.contains(bare + "/") { return bare + "/" }
        // A name or the end of a path: linked when only one file ends that way.
        let name = bare.split(separator: "/").last.map(String.init) ?? bare
        guard let named = byName[name] else { return nil }
        let matching = named.filter { path in
            let trimmed = path.hasSuffix("/") ? String(path.dropLast()) : path
            return trimmed == bare || trimmed.hasSuffix("/" + bare)
        }
        // A folder named like the text matches too; a file wins over it.
        let files = matching.filter { !$0.hasSuffix("/") }
        if files.count == 1 { return files[0] }
        return files.isEmpty && matching.count == 1 ? matching[0] : nil
    }
}

/// The links file references become: `yh-file:///abs/path?line=12&end=20`. The thread
/// whose text it's in decides where it opens.
public enum FileLink {
    public static let scheme = "yh-file"

    public static func url(path: String, line: Int? = nil, endLine: Int? = nil) -> URL? {
        var components = URLComponents()
        components.scheme = scheme
        components.host = ""
        components.path = path
        var query: [URLQueryItem] = []
        if let line { query.append(URLQueryItem(name: "line", value: String(line))) }
        if let endLine { query.append(URLQueryItem(name: "end", value: String(endLine))) }
        components.queryItems = query.isEmpty ? nil : query
        return components.url
    }

    public static func parse(_ url: URL) -> (path: String, line: Int?, endLine: Int?)? {
        guard url.scheme == scheme, let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
              !components.path.isEmpty else { return nil }
        let items = components.queryItems ?? []
        let value = { (name: String) in items.first { $0.name == name }?.value.flatMap { Int($0) } }
        return (components.path, value("line"), value("end"))
    }
}
