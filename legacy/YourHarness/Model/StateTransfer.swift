import AppKit
import HarnessCore
import UniformTypeIdentifiers
import WebKit

/// Export State and Import State: everything the app keeps, as one `.yhstate` file
/// (`StateArchive`), for moving to another Mac or reproducing a bug with someone's exact
/// state. Only from the File menu and Settings: agents have no tool for it.
@MainActor @Observable
final class StateTransfer {
    /// A file read and checked, waiting for you to say it may replace everything.
    struct PendingImport: Identifiable {
        let id = UUID()
        let file: URL
        let opened: StateArchive.Opened
        /// What you have now: what you'd lose.
        let current: StateArchive.Contents
        let workingThreads: Int
    }

    /// What's under way ("Exporting…"), nil when nothing is.
    private(set) var busy: String?
    var pending: PendingImport?
    /// Why the import couldn't go ahead, shown in its sheet.
    private(set) var importProblem: String?

    static let fileType = UTType(filenameExtension: StateArchive.fileExtension) ?? .data

    // MARK: - Export

    func presentExport(app: AppModel) {
        guard busy == nil else { return }
        let panel = NSSavePanel()
        panel.title = "Export State"
        panel.message = "Everything in YourHarness, as one file: projects, threads, memories, settings and details of this Mac and app."
        panel.prompt = "Export"
        panel.nameFieldStringValue = "YourHarness State \(Self.stamp()).\(StateArchive.fileExtension)"
        panel.allowedContentTypes = [Self.fileType]
        let signIns = NSButton(checkboxWithTitle: "Include browser sign-ins (cookies)", target: nil, action: nil)
        signIns.state = .off
        let note = NSTextField(wrappingLabelWithString: "Anyone with the file could use them. Leave this off for a file you'll share.")
        note.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        note.textColor = .secondaryLabelColor
        let stack = NSStackView(views: [signIns, note])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 4
        stack.edgeInsets = NSEdgeInsets(top: 10, left: 20, bottom: 10, right: 20)
        note.preferredMaxLayoutWidth = 360
        panel.accessoryView = stack
        guard panel.runModal() == .OK, let file = panel.url else { return }
        let includingSignIns = signIns.state == .on
        Task {
            do {
                let manifest = try await export(app: app, to: file, includingSignIns: includingSignIns)
                Self.announceExport(manifest, file: file)
            } catch {
                Self.alert("Couldn't export the state", Self.describe(error))
            }
        }
    }

    /// Everything in memory saved, then written to `file`, off the main thread.
    func export(app: AppModel, to file: URL, includingSignIns: Bool, saying label: String = "Exporting…") async throws -> StateArchive.Manifest {
        busy = label
        defer { busy = nil }
        for thread in app.threads { thread.saveForExport() }
        app.saveNow()
        if app.cliStatuses.isEmpty { await app.refreshAgents() }
        let extras = StateArchive.Extras(defaults: Self.exportedDefaults(),
                                         cookies: includingSignIns ? await Self.browserCookies(app.projects) : nil)
        let manifest = StateArchive.Manifest(app: .current, system: .current, tools: Self.tools(app))
        let store = app.store
        await app.tokenUsage.flush()
        let usage = app.tokenUsage.store
        return try await Task.detached(priority: .userInitiated) {
            // What saveNow handed over, on disk before the snapshot.
            store.flush()
            return try StateArchive.export(store.database, usage: usage, dataFolder: store.root, extras: extras, manifest: manifest, to: file)
        }.value
    }

    private static func announceExport(_ manifest: StateArchive.Manifest, file: URL) {
        let alert = NSAlert()
        alert.messageText = "State exported"
        var detail = summary(manifest.contents) + "."
        if manifest.contents.browserCookies != nil {
            detail += "\n\nIt includes your browser sign-ins: anyone with the file could use them."
        }
        alert.informativeText = detail
        alert.addButton(withTitle: "OK")
        alert.addButton(withTitle: "Show in Finder")
        if alert.runModal() == .alertSecondButtonReturn { NSWorkspace.shared.activateFileViewerSelecting([file]) }
    }

    // MARK: - Import

    func presentImport(app: AppModel) {
        guard busy == nil, pending == nil else { return }
        let panel = NSOpenPanel()
        panel.title = "Import State"
        panel.message = "Pick a state exported from YourHarness. You'll see what's in it before anything changes."
        panel.prompt = "Open"
        panel.allowedContentTypes = [Self.fileType]
        panel.allowsMultipleSelection = false
        guard panel.runModal() == .OK, let file = panel.url else { return }
        Task { await read(file, app: app) }
    }

    /// Unpacks and checks the file, then asks: nothing changes until you say so.
    func read(_ file: URL, app: AppModel) async {
        busy = "Reading the file…"
        defer { busy = nil }
        let root = app.store.root
        do {
            let (opened, current) = try await Task.detached(priority: .userInitiated) {
                (try StateArchive.open(file), StateArchive.contents(ofDataFolder: root))
            }.value
            importProblem = nil
            pending = PendingImport(file: file, opened: opened, current: current,
                                    workingThreads: app.threads.filter(\.isWorking).count)
            // The question is asked in the main window, wherever you started.
            NSApp.windows.first { $0.frameAutosaveName == "main" }?.makeKeyAndOrderFront(nil)
        } catch {
            Self.alert("Couldn't import “\(file.lastPathComponent)”", Self.describe(error))
        }
    }

    func cancelImport() {
        pending?.opened.discard()
        pending = nil
        importProblem = nil
    }

    /// Yes: a copy of what you have now first (unless you said not to), then the file's state
    /// goes in at a restart, before anything is read.
    func confirmImport(app: AppModel, keepingCopy: Bool) async {
        guard let pending, busy == nil else { return }
        importProblem = nil
        if keepingCopy {
            do {
                let folder = Self.backupsFolder(app.store)
                try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
                let backup = folder.appending(path: "State before import \(Self.stamp()).\(StateArchive.fileExtension)")
                _ = try await export(app: app, to: backup, includingSignIns: true, saying: "Saving a copy of your state…")
            } catch {
                importProblem = "Your current state couldn't be saved, so nothing was replaced: \(Self.describe(error))"
                return
            }
        }
        busy = "Restarting…"
        let root = app.store.root
        do {
            try await Task.detached { try StateArchive.stage(pending.opened, in: root) }.value
        } catch {
            busy = nil
            importProblem = "The file couldn't be put in place, so nothing was replaced: \(Self.describe(error))"
            return
        }
        // A window with a sheet on it won't let the app quit.
        self.pending = nil
        try? await Task.sleep(for: .milliseconds(500))
        Self.relaunch()
    }

    /// Where Import State keeps what it replaced: in the data folder, but not one of the
    /// app's items, so an export doesn't take it and an import doesn't replace it.
    static func backupsFolder(_ store: Store) -> URL {
        store.root.appending(path: "backups", directoryHint: .isDirectory)
    }

    /// Opens the app again once this process is gone, started the way it was (a development
    /// run keeps its scratch state and comes back in the background), then quits.
    static func relaunch() {
        let bundle = Bundle.main.bundleURL
        // `swift run`: nothing to open; it's started again by hand.
        guard bundle.pathExtension == "app" else {
            NSApp.terminate(nil)
            return
        }
        // Not the state hooks: they'd import again, and again. `open` hands its own
        // environment on too, so they're taken out of that as well.
        let environment = ProcessInfo.processInfo.environment.filter { !$0.key.hasPrefix("YH_STATE_") }
        var open: [String] = []
        if environment["YH_DATA_DIR"] != nil { open.append("-g") }
        for (key, value) in environment.sorted(by: { $0.key < $1.key }) where key.hasPrefix("YH_") {
            open += ["--env", "\(key)=\(value)"]
        }
        let script = """
        pid=$1 app=$2; shift 2
        i=0
        while kill -0 "$pid" 2>/dev/null; do
          sleep 0.1; i=$((i+1)); [ $i -gt 600 ] && exit 1
        done
        open "$@" "$app"
        """
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = ["-c", script, "sh", String(getpid()), bundle.path] + open
        process.environment = environment
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        try? process.run()
        NSApp.terminate(nil)
    }

    // MARK: - At launch, after an import

    /// The import is in; what's left is the browser's sign-ins, when the file had them: each
    /// of its projects' browser gets exactly the file's. Without them, the browser keeps what
    /// it has.
    static func finish(_ applied: StateArchive.Applied, projects: [Project]) async {
        if let cookies = applied.extras.cookies {
            for project in projects {
                await BrowserProfiles.dataStore(for: project.id).removeData(ofTypes: WKWebsiteDataStore.allWebsiteDataTypes(), modifiedSince: .distantPast)
                _ = await CookieImport.store(cookies[project.id.uuidString] ?? [], in: project.id)
            }
        }
        applied.finish()
    }

    // MARK: - Defaults

    /// About this copy of the app rather than the state: which version last ran here.
    nonisolated private static let localDefaults = ["lastRunVersion"]

    /// What macOS keeps for the app (window sizes and places, the Settings tab), as a
    /// property list.
    nonisolated static func exportedDefaults() -> Data? {
        guard let id = Bundle.main.bundleIdentifier, var domain = UserDefaults.standard.persistentDomain(forName: id) else { return nil }
        for key in localDefaults { domain[key] = nil }
        return try? PropertyListSerialization.data(fromPropertyList: domain, format: .binary, options: 0)
    }

    /// At launch, before any window opens, so they open where the file says.
    nonisolated static func applyDefaults(_ data: Data) {
        guard let id = Bundle.main.bundleIdentifier,
              var domain = (try? PropertyListSerialization.propertyList(from: data, format: nil)) as? [String: Any]
        else { return }
        let current = UserDefaults.standard.persistentDomain(forName: id) ?? [:]
        for key in localDefaults { domain[key] = current[key] }
        UserDefaults.standard.setPersistentDomain(domain, forName: id)
    }

    // MARK: - Pieces

    /// Every project's browser cookies that has any, by project ID.
    private static func browserCookies(_ projects: [Project]) async -> [String: [ImportedCookie]] {
        var all: [String: [ImportedCookie]] = [:]
        // Not `WKWebsiteDataStore.allDataStoreIdentifiers`: it never answers.
        for id in projects.map(\.id) {
            let cookies = await BrowserProfiles.dataStore(for: id).httpCookieStore.allCookies()
            if !cookies.isEmpty { all[id.uuidString] = cookies.map(ImportedCookie.init) }
        }
        return all
    }

    private static func tools(_ app: AppModel) -> [StateArchive.Tool] {
        let clis = AgentCLI.all.map { ($0.name, app.cliStatuses[$0.id]) }
        let others = ["git", Forge.github.cliName, Forge.gitlab.cliName].map { ($0, app.toolStatuses[$0]) }
        return (clis + others).map { name, status in
            switch status?.state {
            case .ready(let version)?: StateArchive.Tool(name: name, path: status?.path?.path, version: version)
            case .broken(let reason)?: StateArchive.Tool(name: name, path: status?.path?.path, problem: reason)
            case .notFound?: StateArchive.Tool(name: name, problem: "Not found")
            case nil: StateArchive.Tool(name: name, problem: "Not looked for yet")
            }
        }
    }

    /// "3 projects, 44 threads, 12 memories, 2 images · 4.1 MB".
    nonisolated static func summary(_ contents: StateArchive.Contents) -> String {
        let counts = [
            count(contents.projects.count, "project"), count(contents.threads, "thread"),
            count(contents.memories, "memory", "memories"), count(contents.attachments, "image"),
        ]
        return counts.joined(separator: ", ") + " · " + ByteCountFormatter.string(fromByteCount: contents.bytes, countStyle: .file)
    }

    nonisolated static func count(_ count: Int, _ one: String, _ many: String? = nil) -> String {
        "\(count.formatted()) \(count == 1 ? one : many ?? one + "s")"
    }

    /// "2026-10-27 at 20.15", for file names.
    private static func stamp() -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd 'at' HH.mm.ss"
        return formatter.string(from: .now)
    }

    static func describe(_ error: Error) -> String {
        (error as? StateArchive.Problem)?.description ?? error.localizedDescription
    }

    private static func alert(_ title: String, _ detail: String) {
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = title
        alert.informativeText = detail
        alert.runModal()
    }

    // MARK: - Development

    /// Development: `YH_STATE_EXPORT=<file>` exports at launch (`YH_STATE_EXPORT_SIGNINS=1`
    /// with the browser's cookies) and prints the manifest to stderr. `YH_STATE_IMPORT_ASK=<file>`
    /// reads a file as Import State does and asks; `YH_STATE_IMPORT_CONFIRM=1` then says yes
    /// (keeping a copy) and restarts. `YH_STATE_IMPORT` (in `Store`) imports without asking.
    static func runIfRequested(app: AppModel) {
        let environment = ProcessInfo.processInfo.environment
        if let path = environment["YH_STATE_EXPORT"], !path.isEmpty {
            Task {
                try? await Task.sleep(for: .seconds(2))
                do {
                    let manifest = try await app.stateTransfer.export(app: app, to: URL(fileURLWithPath: path),
                                                                      includingSignIns: environment["YH_STATE_EXPORT_SIGNINS"] == "1")
                    let encoder = JSONEncoder()
                    encoder.dateEncodingStrategy = .iso8601
                    encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
                    log("== exported \(path)\n" + String(decoding: (try? encoder.encode(manifest)) ?? Data(), as: UTF8.self))
                } catch {
                    log("== export failed: \(describe(error))")
                }
            }
        }
        if let path = environment["YH_STATE_IMPORT_ASK"], !path.isEmpty {
            Task {
                try? await Task.sleep(for: .seconds(1))
                await app.stateTransfer.read(URL(fileURLWithPath: path), app: app)
                guard let pending = app.stateTransfer.pending else { return log("== import: nothing to ask") }
                log("== import asks: file \(summary(pending.opened.manifest.contents)); now \(summary(pending.current)); \(pending.workingThreads) working")
                guard environment["YH_STATE_IMPORT_CONFIRM"] == "1" else { return }
                try? await Task.sleep(for: .seconds(2))
                await app.stateTransfer.confirmImport(app: app, keepingCopy: true)
                if let problem = app.stateTransfer.importProblem { log("== import failed: \(problem)") }
            }
        }
    }

    private static func log(_ text: String) {
        FileHandle.standardError.write(Data((text + "\n").utf8))
    }
}

extension ImportedCookie {
    init(_ cookie: HTTPCookie) {
        self.init(domain: cookie.domain, name: cookie.name, value: cookie.value, path: cookie.path,
                  expires: cookie.expiresDate, isSecure: cookie.isSecure, isHTTPOnly: cookie.isHTTPOnly,
                  sameSite: cookie.sameSitePolicy.map { $0.rawValue.lowercased() })
    }
}
