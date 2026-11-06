import Foundation

/// Everything the app keeps, as one file to hand over: for reproducing a bug with someone's
/// exact state, or moving it to another Mac. A zip (`.yhstate`) laid out as
///
///     manifest.json          what made it, on what Mac, and what's inside (`Manifest`)
///     data/                  the app's items in its data folder (`items`): harness.sqlite
///                            and usage.sqlite (snapshots), attachments/, …
///     defaults.plist         what macOS keeps for the app: window sizes and places (optional)
///     browser-cookies.json   each project's browser sign-ins, by project ID (optional)
///
/// `formatVersion` goes up when the layout changes in a way an older app can't read, and an
/// older app refuses a newer file rather than half-import it.
///
/// Importing replaces the app's items in the data folder at the next launch, before the
/// database opens: `stage` unpacks the file into the folder's `pending-import`, and
/// `applyPending` swaps it in, in steps that pick up where they stopped after a crash. The
/// rest of the folder (git worktrees, a development run's project folders and logs) is left
/// alone.
public enum StateArchive {
    public static let format = "yourharness-state"
    public static let formatVersion = 1
    public static let fileExtension = "yhstate"

    /// What the app keeps in its data folder, and so what an export takes and an import
    /// replaces. Add here whatever it starts keeping there.
    /// `usage.sqlite` is the usage view's: mostly an index of Claude Code's session files, but
    /// side chats' and memory agents' usage is only there.
    public static let items = ["harness.sqlite", "usage.sqlite", "attachments", "memory-imports", "state.json", "threads"]
    /// The databases, snapshotted rather than copied.
    static let databases: Set<String> = ["harness.sqlite", "usage.sqlite"]
    /// Written with the databases, never copied: the snapshots have what they hold. An import
    /// moves them aside too, so an old WAL is never played over a new database.
    static let databaseCompanions = ["harness.sqlite-wal", "harness.sqlite-shm", "usage.sqlite-wal", "usage.sqlite-shm"]
    /// The database schema this version of the app writes: a file's higher one is from a
    /// newer app.
    public static var appSchemaVersion: Int { Schema.steps.count }
    /// In the data folder between the swap and the app applying the rest.
    public static let pendingFolderName = "pending-import"

    // MARK: - Manifest

    public struct Manifest: Codable, Sendable, Equatable {
        public var format = StateArchive.format
        public var formatVersion = StateArchive.formatVersion
        public var exportedAt: Date
        public var app: AppInfo
        public var system: SystemInfo
        /// The CLIs and tools the app found, as it found them.
        public var tools: [Tool]
        /// The database's `PRAGMA user_version`.
        public var schemaVersion: Int
        /// Where the data folder was.
        public var dataFolder: String
        public var contents: Contents

        public init(exportedAt: Date = .now, app: AppInfo, system: SystemInfo, tools: [Tool], schemaVersion: Int = 0,
                    dataFolder: String = "", contents: Contents = Contents()) {
            self.exportedAt = exportedAt
            self.app = app
            self.system = system
            self.tools = tools
            self.schemaVersion = schemaVersion
            self.dataFolder = dataFolder
            self.contents = contents
        }
    }

    public struct AppInfo: Codable, Sendable, Equatable {
        public var name: String
        public var bundleID: String?
        public var version: String?
        public var build: String?
        /// The commit it was built from, `-dirty` when there were changes on top.
        public var commit: String?

        public init(name: String, bundleID: String?, version: String?, build: String?, commit: String?) {
            self.name = name
            self.bundleID = bundleID
            self.version = version
            self.build = build
            self.commit = commit
        }

        /// The running app, from its Info.plist.
        public static var current: AppInfo {
            let info = Bundle.main.infoDictionary ?? [:]
            return AppInfo(name: info["CFBundleName"] as? String ?? ProcessInfo.processInfo.processName,
                           bundleID: Bundle.main.bundleIdentifier,
                           version: info["CFBundleShortVersionString"] as? String,
                           build: info["CFBundleVersion"] as? String,
                           commit: info["YHGitCommit"] as? String)
        }
    }

    public struct SystemInfo: Codable, Sendable, Equatable {
        /// "macOS 27.0 (Build 27A5321e)".
        public var os: String
        /// "Mac16,5".
        public var model: String?
        /// "Apple M4 Max".
        public var chip: String?
        public var memoryBytes: UInt64
        public var processors: Int
        public var locale: String
        public var timeZone: String

        public static var current: SystemInfo {
            let process = ProcessInfo.processInfo
            return SystemInfo(os: process.operatingSystemVersionString.hasPrefix("Version")
                                  ? "macOS " + process.operatingSystemVersionString.dropFirst("Version ".count)
                                  : process.operatingSystemVersionString,
                              model: sysctl("hw.model"), chip: sysctl("machdep.cpu.brand_string"),
                              memoryBytes: process.physicalMemory, processors: process.processorCount,
                              locale: Locale.current.identifier, timeZone: TimeZone.current.identifier)
        }

        private static func sysctl(_ name: String) -> String? {
            var size = 0
            guard sysctlbyname(name, nil, &size, nil, 0) == 0, size > 0 else { return nil }
            var bytes = [CChar](repeating: 0, count: size)
            guard sysctlbyname(name, &bytes, &size, nil, 0) == 0 else { return nil }
            return String(decoding: bytes.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }, as: UTF8.self)
        }
    }

    public struct Tool: Codable, Sendable, Equatable {
        public var name: String
        public var path: String?
        public var version: String?
        /// Why it didn't run, when it was found but didn't.
        public var problem: String?

        public init(name: String, path: String? = nil, version: String? = nil, problem: String? = nil) {
            self.name = name
            self.path = path
            self.version = version
            self.problem = problem
        }
    }

    /// What's in a state, to say before replacing one with another.
    public struct Contents: Codable, Sendable, Equatable {
        /// Names, in sidebar order.
        public var projects: [String] = []
        public var threads = 0
        /// Transcript rows.
        public var entries = 0
        /// Memories and tasks, archived ones too.
        public var memories = 0
        /// Images sent and screenshots taken.
        public var attachments = 0
        /// The data, unpacked.
        public var bytes: Int64 = 0
        /// Browser cookies, nil when they weren't exported.
        public var browserCookies: Int?
        public var hasDefaults = false

        public init() {}
    }

    /// What's kept outside the data folder.
    public struct Extras: Sendable, Equatable {
        /// The app's defaults domain, as a property list.
        public var defaults: Data?
        /// By project ID.
        public var cookies: [String: [ImportedCookie]]?

        public init(defaults: Data? = nil, cookies: [String: [ImportedCookie]]? = nil) {
            self.defaults = defaults
            self.cookies = cookies
        }
    }

    public struct Problem: Error, Equatable, CustomStringConvertible {
        public let description: String

        init(_ description: String) { self.description = description }
    }

    // MARK: - Export

    /// Writes everything in `dataFolder` the app keeps, with `database` and `usage` as of now,
    /// to `file`. `manifest` says what made it; what's inside is filled in. Blocks: off the
    /// main thread.
    @discardableResult
    public static func export(_ database: HarnessDatabase, usage: UsageStore? = nil, dataFolder: URL, extras: Extras,
                              manifest: Manifest, to file: URL) throws -> Manifest {
        let fileManager = FileManager.default
        let work = fileManager.temporaryDirectory.appending(path: "yourharness-export-\(UUID().uuidString)", directoryHint: .isDirectory)
        defer { try? fileManager.removeItem(at: work) }
        let data = work.appending(path: "data", directoryHint: .isDirectory)
        try fileManager.createDirectory(at: data, withIntermediateDirectories: true)

        try database.snapshot(to: data.appending(path: "harness.sqlite"))
        if let usage, FileManager.default.fileExists(atPath: usage.url.path) {
            try usage.snapshot(to: data.appending(path: "usage.sqlite"))
        }
        for name in items where !databases.contains(name) {
            let source = dataFolder.appending(path: name)
            guard fileManager.fileExists(atPath: source.path) else { continue }
            // A clone on APFS: instant, and no more space until one of them changes.
            try fileManager.copyItem(at: source, to: data.appending(path: name))
        }

        var manifest = manifest
        manifest.formatVersion = formatVersion
        manifest.format = format
        manifest.dataFolder = dataFolder.path
        manifest.schemaVersion = schemaVersion(of: data)
        manifest.contents = contents(ofDataFolder: data)
        if let defaults = extras.defaults {
            try defaults.write(to: work.appending(path: "defaults.plist"))
            manifest.contents.hasDefaults = true
        }
        if let cookies = extras.cookies {
            try encoder.encode(cookies).write(to: work.appending(path: "browser-cookies.json"))
            manifest.contents.browserCookies = cookies.values.reduce(0) { $0 + $1.count }
        }
        try encoder.encode(manifest).write(to: work.appending(path: "manifest.json"))

        let zip = fileManager.temporaryDirectory.appending(path: "yourharness-export-\(UUID().uuidString).zip")
        defer { try? fileManager.removeItem(at: zip) }
        try ditto(["-c", "-k", "--norsrc", "--noextattr", "--noqtn", work.path, zip.path], failure: "Couldn't write the file")
        if fileManager.fileExists(atPath: file.path) {
            _ = try fileManager.replaceItemAt(file, withItemAt: zip)
        } else {
            try fileManager.moveItem(at: zip, to: file)
        }
        return manifest
    }

    // MARK: - Import

    /// A state file unpacked and checked, waiting to be staged or thrown away.
    public struct Opened: Sendable {
        public let folder: URL
        public let manifest: Manifest

        /// What the file holds besides the data.
        public var extras: Extras { StateArchive.extras(in: folder) }

        /// Throws it away: the import was called off.
        public func discard() {
            try? FileManager.default.removeItem(at: folder)
        }
    }

    /// Unpacks `file` somewhere temporary and checks it's a state this app can import.
    /// Blocks: off the main thread.
    public static func open(_ file: URL) throws -> Opened {
        let fileManager = FileManager.default
        let folder = fileManager.temporaryDirectory.appending(path: "yourharness-import-\(UUID().uuidString)", directoryHint: .isDirectory)
        do {
            try ditto(["-x", "-k", file.path, folder.path], failure: "This isn't a YourHarness state file")
            let manifest = try readManifest(in: folder)
            let database = folder.appending(path: "data/harness.sqlite")
            guard fileManager.fileExists(atPath: database.path) else { throw Problem("The file has no database in it.") }
            let check = try? SQLiteConnection(url: database, readOnly: true).scalar("PRAGMA quick_check")
            guard check == .text("ok") else { throw Problem("The file is damaged: its database doesn't pass SQLite's check.") }
            return Opened(folder: folder, manifest: manifest)
        } catch {
            try? fileManager.removeItem(at: folder)
            throw error
        }
    }

    /// The manifest, refused when it isn't one, or is from a format this app doesn't know.
    static func readManifest(in folder: URL) throws -> Manifest {
        struct Header: Decodable {
            var format: String?
            var formatVersion: Int?
        }
        guard let data = try? Data(contentsOf: folder.appending(path: "manifest.json")),
              let header = try? decoder.decode(Header.self, from: data), header.format == format,
              let version = header.formatVersion
        else { throw Problem("This isn't a YourHarness state file.") }
        guard version <= formatVersion else {
            throw Problem("This file is from a newer YourHarness (state format \(version); this version reads up to \(formatVersion)). Update YourHarness, then import it.")
        }
        do {
            return try decoder.decode(Manifest.self, from: data)
        } catch {
            throw Problem("This state file's manifest doesn't read: \(error.localizedDescription)")
        }
    }

    /// Moves what `opened` holds into the data folder, to replace its state at the next
    /// launch (`applyPending`).
    public static func stage(_ opened: Opened, in dataFolder: URL) throws {
        let fileManager = FileManager.default
        let pending = dataFolder.appending(path: pendingFolderName, directoryHint: .isDirectory)
        try fileManager.createDirectory(at: dataFolder, withIntermediateDirectories: true)
        try? fileManager.removeItem(at: pending)
        try fileManager.moveItem(at: opened.folder, to: pending)
    }

    /// An import that's been swapped in, with what's left for the app to do: the defaults and
    /// the browser's cookies. `finish` once they're applied.
    public struct Applied: Sendable {
        public let manifest: Manifest
        public let extras: Extras
        public let dataFolder: URL

        public func finish() {
            try? FileManager.default.removeItem(at: dataFolder.appending(path: StateArchive.pendingFolderName))
        }
    }

    /// At launch, before the database opens: swaps in a staged import, if there is one, and
    /// returns what's left to apply (also when an earlier launch swapped it in but didn't
    /// get as far as applying the rest).
    ///
    /// First the app's items move aside, then a marker is written, then the imported ones move
    /// in: each step can run again after a crash without losing either state.
    public static func applyPending(in dataFolder: URL) -> Applied? {
        let fileManager = FileManager.default
        let pending = dataFolder.appending(path: pendingFolderName, directoryHint: .isDirectory)
        guard fileManager.fileExists(atPath: pending.path) else { return nil }
        let data = pending.appending(path: "data", directoryHint: .isDirectory)
        let replaced = pending.appending(path: "replaced", directoryHint: .isDirectory)
        let cleared = pending.appending(path: "cleared")
        do {
            if fileManager.fileExists(atPath: data.path) {
                if !fileManager.fileExists(atPath: cleared.path) {
                    try fileManager.createDirectory(at: replaced, withIntermediateDirectories: true)
                    do {
                        for name in items + databaseCompanions {
                            let item = dataFolder.appending(path: name)
                            guard fileManager.fileExists(atPath: item.path) else { continue }
                            let aside = replaced.appending(path: name)
                            try? fileManager.removeItem(at: aside)
                            try fileManager.moveItem(at: item, to: aside)
                        }
                        try Data().write(to: cleared)
                    } catch {
                        // Back as it was, so the app opens on the state it had.
                        for name in (try? fileManager.contentsOfDirectory(atPath: replaced.path)) ?? [] {
                            try? fileManager.moveItem(at: replaced.appending(path: name), to: dataFolder.appending(path: name))
                        }
                        throw error
                    }
                }
                for name in try fileManager.contentsOfDirectory(atPath: data.path) where items.contains(name) {
                    // Only there if a launch ran on the cleared folder after a failed try.
                    let destination = dataFolder.appending(path: name)
                    if fileManager.fileExists(atPath: destination.path) { try fileManager.removeItem(at: destination) }
                    try fileManager.moveItem(at: data.appending(path: name), to: destination)
                }
                try fileManager.removeItem(at: data)
            }
        } catch {
            FileHandle.standardError.write(Data("Importing the state didn't finish (\(error)); it's tried again at the next launch\n".utf8))
            return nil
        }
        try? fileManager.removeItem(at: cleared)
        if fileManager.fileExists(atPath: replaced.path) {
            DispatchQueue.global(qos: .utility).async { try? FileManager.default.removeItem(at: replaced) }
        }
        guard let manifest = try? readManifest(in: pending) else {
            try? fileManager.removeItem(at: pending)
            return nil
        }
        return Applied(manifest: manifest, extras: extras(in: pending), dataFolder: dataFolder)
    }

    // MARK: - Reading

    /// What a data folder holds: the live one, or an unpacked file's.
    public static func contents(ofDataFolder folder: URL) -> Contents {
        var contents = Contents()
        if let connection = try? SQLiteConnection(url: folder.appending(path: "harness.sqlite"), readOnly: true) {
            try? connection.query("SELECT json FROM projects ORDER BY position") { row in
                let project = row.data(0).flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
                contents.projects.append(project?["name"] as? String ?? "Unnamed")
            }
            contents.threads = (try? connection.scalar("SELECT count(*) FROM threads")?.int) ?? 0
            contents.entries = (try? connection.scalar("SELECT count(*) FROM entries")?.int) ?? 0
            contents.memories = (try? connection.scalar("SELECT count(*) FROM memories")?.int) ?? 0
        }
        let attachments = folder.appending(path: "attachments")
        contents.attachments = ((try? FileManager.default.contentsOfDirectory(atPath: attachments.path)) ?? [])
            .filter { !$0.hasPrefix(".") }.count
        for name in items + databaseCompanions {
            contents.bytes += size(of: folder.appending(path: name))
        }
        return contents
    }

    static func schemaVersion(of folder: URL) -> Int {
        (try? SQLiteConnection(url: folder.appending(path: "harness.sqlite"), readOnly: true).scalar("PRAGMA user_version")?.int) ?? 0
    }

    static func extras(in folder: URL) -> Extras {
        var extras = Extras()
        extras.defaults = try? Data(contentsOf: folder.appending(path: "defaults.plist"))
        if let data = try? Data(contentsOf: folder.appending(path: "browser-cookies.json")) {
            extras.cookies = try? decoder.decode([String: [ImportedCookie]].self, from: data)
        }
        return extras
    }

    private static func size(of url: URL) -> Int64 {
        let fileManager = FileManager.default
        var isDirectory: ObjCBool = false
        guard fileManager.fileExists(atPath: url.path, isDirectory: &isDirectory) else { return 0 }
        guard isDirectory.boolValue else {
            return (try? fileManager.attributesOfItem(atPath: url.path)[.size] as? Int64) ?? 0
        }
        var total: Int64 = 0
        for case let file as URL in fileManager.enumerator(at: url, includingPropertiesForKeys: [.fileSizeKey]) ?? FileManager.DirectoryEnumerator() {
            total += Int64((try? file.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0)
        }
        return total
    }

    /// `ditto` makes and opens zips as Finder does, and checks what it unpacks.
    private static func ditto(_ arguments: [String], failure: String) throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/ditto")
        process.arguments = arguments
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = FileHandle.nullDevice
        let errors = Pipe()
        process.standardError = errors
        try process.run()
        let message = String(decoding: errors.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            let detail = message.trimmingCharacters(in: .whitespacesAndNewlines)
            throw Problem(detail.isEmpty ? "\(failure)." : "\(failure): \(detail)")
        }
    }

    private static let encoder: JSONEncoder = {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        return encoder
    }()

    private static let decoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return decoder
    }()
}
