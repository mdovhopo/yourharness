import Foundation

/// A workspace's memory as files: JSON that brings back everything (for backups and moving
/// between Macs), or a Markdown folder in Claude Code's own memory format (a file a memory
/// with frontmatter, and a `MEMORY.md` index) that people and other tools can read. Both come
/// back in without an agent; anything else goes through the importer.
public enum MemoryArchive {
    public static let format = "yourharness-memory"
    public static let version = 1

    // MARK: - JSON

    struct Document: Codable {
        struct Workspace: Codable {
            var name: String
            var path: String?
        }

        struct Item: Codable {
            var id: String
            var kind: String
            var title: String
            var description: String?
            var body: String
            var status: String?
            var pinned: Bool?
            var origin: String?
            var createdAt: Date?
            var updatedAt: Date?
            var recallCount: Int?
            var lastRecalledAt: Date?
            var links: [HarnessDatabase.MemoryLink]?
            var sources: [HarnessDatabase.MemorySource]?
            var taskState: String?
            var steps: [HarnessDatabase.TaskStep]?
        }

        var format: String
        var version: Int
        var exportedAt: Date
        var workspace: Workspace?
        var memories: [Item]
    }

    public static func json(_ memories: [HarnessDatabase.Memory], workspace: String, path: String?, now: Date = .now) throws -> Data {
        let document = Document(format: format, version: version, exportedAt: now, workspace: .init(name: workspace, path: path),
                                memories: memories.map { memory in
            Document.Item(id: memory.id, kind: memory.kind, title: memory.title, description: memory.description, body: memory.body,
                          status: memory.status.rawValue, pinned: memory.pinned ? true : nil, origin: memory.origin,
                          createdAt: memory.createdAt, updatedAt: memory.updatedAt, recallCount: memory.recallCount,
                          lastRecalledAt: memory.lastRecalledAt, links: memory.links.isEmpty ? nil : memory.links,
                          sources: memory.sources.isEmpty ? nil : memory.sources, taskState: memory.taskState?.rawValue,
                          steps: memory.steps.isEmpty ? nil : memory.steps)
        })
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        return try encoder.encode(document)
    }

    public enum ReadError: Error, Equatable, CustomStringConvertible {
        case notAnExport
        case newerVersion(Int)

        public var description: String {
            switch self {
            case .notAnExport: "This isn't a YourHarness memory export."
            case .newerVersion(let version): "This export is from a newer YourHarness (format \(version))."
            }
        }
    }

    /// The memories in an export, for `scope`.
    public static func memories(fromJSON data: Data, scope: String) throws -> [HarnessDatabase.Memory] {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        guard let document = try? decoder.decode(Document.self, from: data), document.format == format else { throw ReadError.notAnExport }
        guard document.version <= version else { throw ReadError.newerVersion(document.version) }
        return document.memories.map { item in
            HarnessDatabase.Memory(id: item.id, scope: scope, kind: item.kind, title: item.title, description: item.description ?? "",
                                   body: item.body, status: item.status.flatMap(HarnessDatabase.Memory.Status.init) ?? .active,
                                   pinned: item.pinned ?? false, origin: item.origin, createdAt: item.createdAt ?? .now,
                                   updatedAt: item.updatedAt ?? .now, recallCount: item.recallCount ?? 0,
                                   lastRecalledAt: item.lastRecalledAt, sources: item.sources ?? [], links: item.links ?? [],
                                   taskState: item.taskState.flatMap(HarnessDatabase.TaskState.init), steps: item.steps ?? [])
        }
    }

    // MARK: - Markdown

    /// File name for a memory: its title in kebab case, made unique.
    static func slugs(_ memories: [HarnessDatabase.Memory]) -> [String: String] {
        var used = Set<String>(), slugs: [String: String] = [:]
        for memory in memories {
            var base = memory.title.lowercased().map { $0.isLetter || $0.isNumber ? String($0) : "-" }.joined()
                .split(separator: "-").joined(separator: "-")
            if base.isEmpty { base = "memory" }
            base = String(base.prefix(60))
            var slug = base, n = 2
            while !used.insert(slug).inserted { slug = "\(base)-\(n)"; n += 1 }
            slugs[memory.id] = slug
        }
        return slugs
    }

    /// The files of a Markdown export, by name: one a memory and `MEMORY.md`.
    public static func markdownFiles(_ memories: [HarnessDatabase.Memory], workspace: String) -> [String: String] {
        let slugs = slugs(memories)
        var files: [String: String] = [:]
        var index = "# \(workspace) memory\n\nExported from YourHarness. One file a memory; links are [[file-name]].\n\n"
        for kind in MemoryKind.allCases {
            let group = memories.filter { MemoryKind.of($0.kind) == kind && $0.status == .active }
            guard !group.isEmpty else { continue }
            index += "## \(kind.label)\n\n" + group.map { "- [\($0.title)](\(slugs[$0.id]!).md) — \($0.description)" }.joined(separator: "\n") + "\n\n"
        }
        let archived = memories.filter { $0.status != .active }
        if !archived.isEmpty {
            index += "## Archived\n\n" + archived.map { "- [\($0.title)](\(slugs[$0.id]!).md) — \($0.description)" }.joined(separator: "\n") + "\n"
        }
        files["MEMORY.md"] = index
        for memory in memories {
            let slug = slugs[memory.id]!
            var text = "---\nname: \(slug)\ndescription: \(yaml(memory.description))\nmetadata:\n  type: \(memory.kind)\n"
            text += "  title: \(yaml(memory.title))\n  id: \(memory.id)\n"
            text += "  created: \(memory.createdAt.formatted(.iso8601))\n  updated: \(memory.updatedAt.formatted(.iso8601))\n"
            if memory.status != .active { text += "  status: \(memory.status.rawValue)\n" }
            if memory.pinned { text += "  pinned: true\n" }
            if let state = memory.taskState { text += "  state: \(state.rawValue)\n" }
            text += "---\n\n\(memory.body.trimmingCharacters(in: .whitespacesAndNewlines))\n"
            if !memory.steps.isEmpty {
                text += "\n## Steps\n\n" + memory.steps.map { "- [\($0.done ? "x" : " ")] \($0.text)" }.joined(separator: "\n") + "\n"
            }
            let links = memory.links.compactMap { slugs[$0.to] }
            if !links.isEmpty { text += "\nRelated: " + links.map { "[[\($0)]]" }.joined(separator: " ") + "\n" }
            files[slug + ".md"] = text
        }
        return files
    }

    private static func yaml(_ value: String) -> String {
        let flat = value.replacingOccurrences(of: "\n", with: " ")
        let needsQuotes = flat.isEmpty || flat.contains(": ") || flat.contains("#") || "\"'[]{}>|*&!%@`-".contains(flat.first!)
        return needsQuotes ? "\"" + flat.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"") + "\"" : flat
    }

    /// Memories from Markdown files with memory frontmatter (`name`, `description`, a type),
    /// as Claude Code and a Markdown export write them. Files without it (a CLAUDE.md, notes)
    /// are left out: those need the importer. Titles come from `MEMORY.md` when it names them.
    public static func memories(fromMarkdown files: [String: String], scope: String, now: Date = .now) -> [HarnessDatabase.Memory] {
        let titles = indexTitles(files["MEMORY.md"] ?? "")
        var parsed: [(name: String, memory: HarnessDatabase.Memory, linkNames: [String])] = []
        for (file, text) in files.sorted(by: { $0.key < $1.key }) where file != "MEMORY.md" && file.hasSuffix(".md") {
            guard let (front, body) = frontmatter(text) else { continue }
            let name = front["name"] ?? String(file.dropLast(3))
            guard let type = front["metadata.type"] ?? front["type"], front["description"] != nil || front["metadata.type"] != nil else { continue }
            var cleanBody = body
            var linkNames = wikiLinks(body)
            // Our own export's footer comes back as links, not text.
            if let range = cleanBody.range(of: "\nRelated: [[", options: .backwards) {
                cleanBody = String(cleanBody[..<range.lowerBound])
            }
            linkNames = Array(NSOrderedSet(array: linkNames)) as? [String] ?? linkNames
            var steps: [HarnessDatabase.TaskStep] = []
            if let range = cleanBody.range(of: "\n## Steps\n") {
                for line in cleanBody[range.upperBound...].split(separator: "\n") {
                    let trimmed = line.trimmingCharacters(in: .whitespaces)
                    if trimmed.hasPrefix("- [x] ") || trimmed.hasPrefix("- [X] ") { steps.append(.init(String(trimmed.dropFirst(6)), done: true)) }
                    else if trimmed.hasPrefix("- [ ] ") { steps.append(.init(String(trimmed.dropFirst(6)))) }
                }
                cleanBody = String(cleanBody[..<range.lowerBound])
            }
            let fileName = String(file.split(separator: "/").last ?? Substring(file))
            let title = front["metadata.title"] ?? titles[fileName] ?? humanize(name)
            let memory = HarnessDatabase.Memory(
                id: front["metadata.id"] ?? UUID().uuidString, scope: scope, kind: MemoryKind.of(type).rawValue, title: title,
                description: front["description"] ?? "", body: cleanBody.trimmingCharacters(in: .whitespacesAndNewlines),
                status: front["metadata.status"].flatMap(HarnessDatabase.Memory.Status.init) ?? .active,
                pinned: front["metadata.pinned"] == "true", origin: "import",
                createdAt: front["metadata.created"].flatMap(date) ?? now, updatedAt: front["metadata.updated"].flatMap(date) ?? front["metadata.modified"].flatMap(date) ?? now,
                taskState: front["metadata.state"].flatMap(HarnessDatabase.TaskState.init), steps: steps)
            parsed.append((name, memory, linkNames))
        }
        let ids = Dictionary(parsed.map { ($0.name, $0.memory.id) }) { first, _ in first }
        return parsed.map { item in
            var memory = item.memory
            memory.links = item.linkNames.compactMap { ids[$0] }.filter { $0 != memory.id }.map { .init(to: $0) }
            return memory
        }
    }

    /// Reads every `.md` file in a folder (not below it), by name.
    public static func readMarkdownFolder(_ folder: URL) -> [String: String] {
        let names = (try? FileManager.default.contentsOfDirectory(atPath: folder.path)) ?? []
        var files: [String: String] = [:]
        for name in names where name.hasSuffix(".md") {
            if let text = try? String(contentsOf: folder.appending(path: name), encoding: .utf8) { files[name] = text }
        }
        return files
    }

    static func frontmatter(_ text: String) -> ([String: String], String)? {
        let lines = text.components(separatedBy: "\n")
        guard lines.first?.trimmingCharacters(in: .whitespaces) == "---",
              let end = lines.dropFirst().firstIndex(where: { $0.trimmingCharacters(in: .whitespaces) == "---" }) else { return nil }
        var values: [String: String] = [:], parent: String?
        for line in lines[1..<end] where !line.trimmingCharacters(in: .whitespaces).isEmpty {
            let indented = line.hasPrefix(" ") || line.hasPrefix("\t")
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            guard let colon = trimmed.firstIndex(of: ":") else { continue }
            let key = String(trimmed[..<colon]).trimmingCharacters(in: .whitespaces)
            let value = unquote(String(trimmed[trimmed.index(after: colon)...]).trimmingCharacters(in: .whitespaces))
            if indented, let parent {
                values["\(parent).\(key)"] = value
            } else if value.isEmpty {
                parent = key
            } else {
                parent = nil
                values[key] = value
            }
        }
        return (values, lines[(end + 1)...].joined(separator: "\n"))
    }

    private static func unquote(_ value: String) -> String {
        guard value.count >= 2, let first = value.first, first == value.last, first == "\"" || first == "'" else { return value }
        return String(value.dropFirst().dropLast()).replacingOccurrences(of: "\\\"", with: "\"").replacingOccurrences(of: "\\\\", with: "\\")
    }

    private static func wikiLinks(_ text: String) -> [String] {
        guard let regex = try? NSRegularExpression(pattern: #"\[\[([^\]\|]+)(?:\|[^\]]*)?\]\]"#) else { return [] }
        return regex.matches(in: text, range: NSRange(text.startIndex..., in: text)).compactMap {
            Range($0.range(at: 1), in: text).map { String(text[$0]).trimmingCharacters(in: .whitespaces) }
        }
    }

    /// `[Title](file.md)` lines in an index, title by file name.
    private static func indexTitles(_ index: String) -> [String: String] {
        guard let regex = try? NSRegularExpression(pattern: #"\[([^\]]+)\]\(([^)\s]+\.md)\)"#) else { return [:] }
        var titles: [String: String] = [:]
        for match in regex.matches(in: index, range: NSRange(index.startIndex..., in: index)) {
            guard let title = Range(match.range(at: 1), in: index), let file = Range(match.range(at: 2), in: index) else { continue }
            titles[String(index[file]).split(separator: "/").last.map(String.init) ?? ""] = String(index[title])
        }
        return titles
    }

    private static func humanize(_ name: String) -> String {
        let words = name.split { $0 == "-" || $0 == "_" }.map(String.init)
        guard let first = words.first else { return name }
        return ([first.prefix(1).uppercased() + first.dropFirst()] + words.dropFirst()).joined(separator: " ")
    }

    private static func date(_ string: String) -> Date? {
        if let date = try? Date(string, strategy: .iso8601) { return date }
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter.date(from: string)
    }

    // MARK: - Bringing memories in

    public struct ImportReport: Sendable, Equatable {
        public var added = 0
        public var updated = 0
        /// Already there, the same.
        public var unchanged = 0
        public var total: Int { added + updated + unchanged }
    }

    /// Adds `incoming` to `scope`: a memory it already has (same ID, or same title) is updated
    /// when the incoming one is newer; one whose ID another workspace uses gets a new ID.
    @discardableResult
    public static func merge(_ incoming: [HarnessDatabase.Memory], into scope: String, database: HarnessDatabase,
                             actor: HarnessDatabase.MemoryActor) -> ImportReport {
        let existing = database.memories(scope: scope)
        let byID = Dictionary(existing.map { ($0.id, $0) }) { first, _ in first }
        // In use first: a title that's also on an archived memory matches the one in use.
        let byTitle = Dictionary(existing.sorted { ($0.status == .active ? 0 : 1) < ($1.status == .active ? 0 : 1) }
            .map { ($0.title.lowercased(), $0) }) { first, _ in first }
        var report = ImportReport(), remap: [String: String] = [:], toSave: [HarnessDatabase.Memory] = []
        for var memory in incoming {
            memory.scope = scope
            if let current = byID[memory.id] ?? byTitle[memory.title.lowercased()] {
                remap[memory.id] = current.id
                // One you archived stays archived: an import doesn't bring it back.
                if current.status == .archived && current.id != memory.id {
                    report.unchanged += 1
                    continue
                }
                let same = current.title == memory.title && current.description == memory.description && current.body == memory.body
                    && current.kind == memory.kind && current.status == memory.status && current.taskState == memory.taskState
                    && current.steps == memory.steps
                if same || memory.updatedAt <= current.updatedAt {
                    report.unchanged += 1
                    continue
                }
                var updated = current
                updated.title = memory.title
                updated.description = memory.description
                updated.body = memory.body
                updated.kind = memory.kind
                updated.status = memory.status
                updated.taskState = memory.taskState
                updated.steps = memory.steps
                updated.pinned = memory.pinned || current.pinned
                updated.updatedAt = max(memory.updatedAt, current.updatedAt)
                updated.links = Array(Set(current.links + memory.links))
                toSave.append(updated)
                report.updated += 1
            } else {
                if database.memory(memory.id) != nil {
                    let fresh = UUID().uuidString
                    remap[memory.id] = fresh
                    memory.id = fresh
                }
                toSave.append(memory)
                report.added += 1
            }
        }
        for index in toSave.indices {
            toSave[index].links = toSave[index].links.map { .init(to: remap[$0.to] ?? $0.to, relation: $0.relation) }
                .filter { $0.to != toSave[index].id }
        }
        database.saveMemories(toSave, actor: actor)
        if report.total > 0 {
            database.recordMemoryEvent(scope: scope, kind: .imported, actor: actor, count: report.added + report.updated,
                                       detail: "\(report.added) added, \(report.updated) updated, \(report.unchanged) unchanged")
        }
        return report
    }
}

/// Claude Code's own memory for a folder: `~/.claude/projects/<folder as a name>/memory`.
public enum ClaudeCodeMemory {
    /// Claude Code names a project's folder after its path, every character that isn't a
    /// letter or digit a dash.
    public static func directory(for folder: String, configDirectory: URL) -> URL {
        // The real path, as Claude Code sees its working directory: `resolvingSymlinksInPath`
        // would drop the `/private` of `/private/tmp`, and name another folder.
        let path = realpath(folder, nil).map { pointer in defer { free(pointer) }; return String(cString: pointer) } ?? folder
        let name = String(path.map { $0.isASCII && ($0.isLetter || $0.isNumber) ? $0 : "-" })
        return configDirectory.appending(path: "projects").appending(path: name).appending(path: "memory")
    }

    /// The memory files there, not counting the index.
    public static func files(for folder: String, configDirectory: URL) -> [URL] {
        let directory = directory(for: folder, configDirectory: configDirectory)
        let names = (try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? []
        return names.filter { $0.hasSuffix(".md") && $0 != "MEMORY.md" }.sorted().map { directory.appending(path: $0) }
    }

    // MARK: - On or off

    /// Whether Claude Code keeps its own memory for a folder, as its settings say
    /// (`autoMemoryEnabled`): the folder's `.claude/settings.local.json` and
    /// `.claude/settings.json`, then your own `settings.json`.
    public struct Setting: Sendable, Equatable {
        /// Off in the folder's own settings.
        public var offHere: Bool
        /// Off in your Claude Code settings, for every folder.
        public var offEverywhere: Bool
        /// The folder's settings turn it back on, whatever yours say.
        public var onHere: Bool

        public var isOff: Bool { offHere || offEverywhere && !onHere }
    }

    public static let settingKey = "autoMemoryEnabled"

    /// Where a folder's own setting goes: the local file, which isn't shared with the repo.
    public static func localSettings(for folder: String) -> URL {
        URL(fileURLWithPath: folder).appending(path: ".claude").appending(path: "settings.local.json")
    }

    public static func setting(for folder: String, configDirectory: URL) -> Setting {
        let claude = URL(fileURLWithPath: folder).appending(path: ".claude")
        // A folder that is your config's own parent (your home folder) has no settings of its own.
        let own = claude.standardizedFileURL.path == configDirectory.standardizedFileURL.path
            ? [] : [claude.appending(path: "settings.local.json"), claude.appending(path: "settings.json")]
        let here = own.lazy.compactMap(value).first
        let everywhere = value(configDirectory.appending(path: "settings.json"))
        return Setting(offHere: here == false, offEverywhere: everywhere == false, onHere: here == true)
    }

    private static func value(_ url: URL) -> Bool? {
        guard let data = try? Data(contentsOf: url),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
        return object[settingKey] as? Bool
    }

    public enum WriteError: Error, CustomStringConvertible {
        case unreadable(URL)

        public var description: String {
            switch self {
            case .unreadable(let url): "\(url.path) isn't valid JSON, so it was left alone."
            }
        }
    }

    /// Turns Claude Code's own memory off in `url` (a settings file), keeping everything else
    /// in it. A file that isn't JSON is left alone.
    public static func turnOff(in url: URL) throws {
        var object: [String: Any] = [:]
        if let data = try? Data(contentsOf: url), !data.isEmpty {
            guard let existing = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw WriteError.unreadable(url) }
            object = existing
        }
        object[settingKey] = false
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        let data = try JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes])
        try (data + Data("\n".utf8)).write(to: url, options: .atomic)
    }

    /// Keeps a folder's local settings out of git, in the repo's own `.git/info/exclude` (never
    /// a file that's committed), when the folder is a repo's root.
    public static func excludeFromGit(_ folder: String) {
        let info = URL(fileURLWithPath: folder).appending(path: ".git").appending(path: "info")
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: URL(fileURLWithPath: folder).appending(path: ".git").path, isDirectory: &isDirectory),
              isDirectory.boolValue else { return }
        let exclude = info.appending(path: "exclude"), line = ".claude/settings.local.json"
        let current = (try? String(contentsOf: exclude, encoding: .utf8)) ?? ""
        guard !current.split(separator: "\n").contains(where: { $0.trimmingCharacters(in: .whitespaces) == line }) else { return }
        try? FileManager.default.createDirectory(at: info, withIntermediateDirectories: true)
        let text = current.isEmpty || current.hasSuffix("\n") ? current + line + "\n" : current + "\n" + line + "\n"
        try? text.write(to: exclude, atomically: true, encoding: .utf8)
    }

    /// Memory files there that `memories` doesn't have (by title or what they say), changed
    /// after `since`.
    public static func unimported(for folder: String, configDirectory: URL, memories: [HarnessDatabase.Memory], since: Date?) -> [URL] {
        let files = files(for: folder, configDirectory: configDirectory)
        guard !files.isEmpty else { return [] }
        let titles = Set(memories.map { $0.title.lowercased() })
        let bodies = Set(memories.map { $0.body.trimmingCharacters(in: .whitespacesAndNewlines) })
        let index = (try? String(contentsOf: directory(for: folder, configDirectory: configDirectory).appending(path: "MEMORY.md"), encoding: .utf8)) ?? ""
        return files.filter { url in
            if let since, let modified = (try? FileManager.default.attributesOfItem(atPath: url.path)[.modificationDate]) as? Date,
               modified <= since { return false }
            guard let text = try? String(contentsOf: url, encoding: .utf8),
                  let memory = MemoryArchive.memories(fromMarkdown: [url.lastPathComponent: text, "MEMORY.md": index], scope: "").first
            else { return false }
            return !titles.contains(memory.title.lowercased()) && !bodies.contains(memory.body)
        }
    }
}
