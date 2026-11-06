import Foundation
import HarnessCore

/// Everything the app knows and does, without a UI. The page sends requests (intents) to
/// `handle`; what changed goes out through `sink` as patches (docs/protocol.md).
///
/// Topics: `library` (projects, thread summaries, settings) goes to the page always; a
/// thread's rows only while the page has it open (`subscribe`), each row patch numbered with
/// the thread's revision, so the page can line patches up with the snapshot it fetched.
@MainActor
public final class Backend {
    public let library: Library
    public let dataDirectory: URL
    let store = ThreadStore(resultLimit: WireLimits.liveResult, inputLimit: WireLimits.toolText)
    private var runtimes: [String: ThreadRuntime] = [:]
    private var subscribed = Set<String>()
    var subscribedThreads: Set<String> { subscribed }
    var fileIndexes: [String: (at: Date, index: FileIndex)] = [:]
    private var lastPlanCount: [String: Int] = [:]
    private var revisions: [String: Int] = [:]
    private var environment: [String: String]?
    private var executable: URL?
    /// Where patches go: the host batches them to the page once a frame.
    public var sink: (Wire) -> Void = { _ in }
    /// Notifications and the Dock badge, shown natively by the host.
    public var attention: (Attention) -> Void = { _ in }
    /// The latest usage limits Claude reported.
    var limits: Any?

    public init(dataDirectory: URL) {
        self.dataDirectory = dataDirectory
        library = Library(directory: dataDirectory)
        Task { await recoverCutOffTurns() }
    }

    /// Threads unread since their last turn ended, until the page says you've seen them.
    private var unread = Set<String>()

    /// Turns a quit or crash cut off: finished from the session files, in the background.
    private func recoverCutOffTurns() async {
        let cut = library.threads.filter { $0["turnInProgress"] as? Bool == true }
        guard !cut.isEmpty else { return }
        let environment = (try? await cli().0) ?? ProcessInfo.processInfo.environment
        for meta in cut {
            guard let id = meta["id"] as? String, let uuid = UUID(uuidString: id) else { continue }
            let plan = await Recovery.plan(db: library.db, thread: uuid, sessionID: meta["sessionID"] as? String, environment: environment)
            // Talked to meanwhile: what's there now counts.
            guard runtimes[id] == nil, store.base(id) == nil else { continue }
            library.db.saveTranscriptChanges(uuid, count: plan.count, changes: plan.changes)
            library.updateThread(id) { $0["turnInProgress"] = nil }
            if subscribed.contains(id) { sink(["t": "reload", "th": id]) }
        }
    }

    // MARK: Requests

    public func handle(_ method: String, _ p: [String: Any]) async throws -> Any? {
        let th = p["th"] as? String ?? ""
        switch method {
        case "library":
            return librarySnapshot()
        case "subscribe":
            subscribed.insert(th)
            return threadSnapshot(th, limit: p["limit"] as? Int ?? 160)
        case "unsubscribe":
            subscribed.remove(th)
            if runtimes[th]?.running != true, runtimes[th]?.hasSession != true {
                runtimes[th]?.saveNow()
                store.forget(th)
            }
            return true
        case "page":
            return page(th, before: p["before"] as? Int, limit: p["limit"] as? Int ?? 160)
        case "send":
            prepareRows(th)
            let text = p["text"] as? String ?? ""
            library.updateThread(th) {
                $0["draft"] = nil
                $0["settledAt"] = nil
                // Named after its first message until a title is generated or chosen.
                if $0["title"] as? String == "New thread", $0["titleChosen"] as? Bool != true {
                    $0["title"] = String(text.split(separator: "\n").first ?? "").trimmingCharacters(in: .whitespaces).prefix(60).description
                }
            }
            threadChanged(th)
            if library.thread(th)?["titleChosen"] as? Bool != true, store.base(th) == 0 || store.base(th) == nil,
               store.entries(th).isEmpty, !text.isEmpty {
                generateTitle(th, firstMessage: text)
            }
            let images = prepareImages(p["images"] as? [String] ?? [], thread: th)
            try await runtime(th).send(text, images: images, now: p["now"] as? Bool ?? false)
            return true
        case "files":
            return ["v": await mentionFiles(th, query: p["q"] as? String ?? "")]
        case "interrupt":
            await runtimes[th]?.interrupt()
            return true
        case "respond", "answer", "plan":
            guard let request = store.pendingPermission(th), request.requestID == p["id"] as? String else { return false }
            let decision: PermissionDecision
            switch method {
            case "answer":
                let answers = p["answers"] as? [String: [String]] ?? [:]
                decision = .allow(updatedInputJSON: AgentQuestion.answeredInput(inputJSON: request.inputJSON, answers: answers))
            case "plan":
                if let mode = p["approve"] as? String {
                    setProjectMode(ofThread: th, mode)
                    decision = .allow(mode: PermissionModes.control(mode))
                } else {
                    decision = .deny(message: p["feedback"] as? String ?? "Keep planning.")
                }
            default:
                decision = (p["allow"] as? Bool ?? false) ? .allow() : .deny(message: "The user denied this action.")
            }
            await runtime(th).respond(request, decision)
            return true
        case "setMode":
            let mode = p["mode"] as? String ?? "auto"
            setProjectMode(ofThread: th, mode)
            await runtimes[th]?.setPermissionMode(PermissionModes.control(mode))
            return true
        case "setModel":
            let model = (p["model"] as? String).flatMap { $0.isEmpty ? nil : $0 }
            library.updateThread(th) { $0["model"] = model }
            threadChanged(th)
            await runtimes[th]?.setModel(model)
            return true
        case "setEffort":
            let effort = (p["effort"] as? String).flatMap { $0.isEmpty ? nil : $0 }
            library.updateThread(th) { $0["effort"] = effort }
            threadChanged(th)
            return true
        case "newThread":
            let id = library.addThread(projectID: p["projectId"] as? String ?? "")
            threadChanged(id)
            return id
        case "rename":
            library.updateThread(th) { $0["title"] = p["title"] as? String ?? "Thread"; $0["titleChosen"] = true }
            threadChanged(th)
            return true
        case "pin":
            library.updateThread(th) { $0["isPinned"] = (p["on"] as? Bool ?? false) ? true : nil }
            threadChanged(th)
            return true
        case "settle":
            let on = p["on"] as? Bool ?? true
            if on { runtimes[th]?.end() }
            library.updateThread(th) {
                $0["settledAt"] = on ? self.library.now() : nil
                if !on { $0["reactivatedAt"] = self.library.now() }
            }
            threadChanged(th)
            return true
        case "delete":
            runtimes.removeValue(forKey: th)?.end()
            store.forget(th)
            subscribed.remove(th)
            library.removeThread(th)
            sink(["t": "threadGone", "th": th])
            return true
        case "addProject":
            let id = library.addProject(path: p["path"] as? String ?? "")
            if let project = library.project(id) { sink(["t": "project", "p": library.projectWire(project)]) }
            return id
        case "search":
            return ["v": search(p["q"] as? String ?? "")]
        case "settings":
            return settingsWire()
        case "setSetting":
            setSetting(p["key"] as? String ?? "", p["value"] ?? NSNull())
            return true
        case "read":
            if unread.remove(th) != nil { threadChanged(th) }
            return true
        case "draft":
            let text = p["text"] as? String ?? ""
            library.updateThread(th) { $0["draft"] = text.isEmpty ? nil : text }
            return true
        default:
            throw BackendError("unknown request \(method)")
        }
    }

    /// Saves everything now, ends every `claude`: the app is quitting.
    public func prepareForQuit() {
        for runtime in runtimes.values {
            runtime.saveNow()
            runtime.end()
        }
        library.saveNow()
        library.db.flush()
    }

    // MARK: Topics

    private func librarySnapshot() -> Wire {
        let defaults = library.setting("defaults") as? [String: Any] ?? [:]
        return ["projects": library.projects.map(library.projectWire),
                "threads": library.threads.map { threadWire($0) },
                "defaults": ["model": defaults["model"] ?? "", "effort": defaults["effort"] ?? ""],
                "models": library.setting("claudeModels") ?? [],
                "limits": limits ?? NSNull(),
                "settings": settingsWire()]
    }

    private func threadWire(_ t: [String: Any]) -> Wire {
        let id = t["id"] as? String ?? ""
        var wire = library.threadWire(t, running: runtimes[id]?.running ?? false, waiting: store.pendingPermission(id) != nil)
        if unread.contains(id) { wire["unread"] = true }
        return wire
    }

    /// Something about a thread's summary changed: the sidebar gets its new state.
    /// After tool results: a task tool may have moved the plan on.
    func planMayHaveChanged(_ th: String) {
        guard subscribed.contains(th) else { return }
        let items = plan(th)
        guard !items.isEmpty || lastPlanCount[th, default: 0] > 0 else { return }
        lastPlanCount[th] = items.count
        sink(["t": "plan", "th": th, "v": items])
    }

    /// A turn ended: the thread is unread until the page says it's been seen.
    func turnEnded(_ id: String) {
        unread.insert(id)
        threadChanged(id)
        noteAttention(id, waiting: false)
        if library.setting("notifications") as? Bool ?? true, let title = library.thread(id)?["title"] as? String {
            attention(.notify(thread: id, title: title, body: "Claude finished"))
        }
    }

    func threadChanged(_ id: String) {
        guard let t = library.thread(id) else { return }
        sink(["t": "thread", "th": threadWire(t)])
    }

    /// The rows before the store's first come from the database: tell the store where it starts.
    private func prepareRows(_ th: String) {
        guard store.base(th) == nil, let uuid = UUID(uuidString: th) else { return }
        store.setBase(library.db.transcriptCount(uuid), thread: th)
    }

    private func threadSnapshot(_ th: String, limit: Int) -> Wire {
        prepareRows(th)
        var snapshot = page(th, before: nil, limit: limit)
        snapshot["rv"] = revisions[th] ?? 0
        if let request = store.pendingPermission(th) { snapshot["perm"] = request.wire }
        snapshot["running"] = runtimes[th]?.running ?? false
        snapshot["commands"] = commands(ofThread: th)
        snapshot["plan"] = plan(th)
        return snapshot
    }

    /// Rows `before - limit ..< before` of the whole transcript: saved ones from the
    /// database, the ones held by the store from there.
    private func page(_ th: String, before: Int?, limit: Int) -> Wire {
        prepareRows(th)
        guard let uuid = UUID(uuidString: th) else { return ["total": 0, "from": 0, "entries": []] }
        let base = store.base(th) ?? library.db.transcriptCount(uuid)
        let total = store.total(th) ?? base
        let end = min(before ?? total, total), from = max(0, end - limit)
        var entries: [Wire] = []
        if from < base {
            entries += library.db.loadTranscriptPage(uuid, from: from, limit: min(end, base) - from).map(\.wire)
        }
        entries += store.rows(th, from: from, to: end).map(\.wire)
        return ["total": total, "from": from, "entries": entries]
    }

    /// The store's patches, as the page gets them: row patches only for open threads.
    func emit(_ patches: [ThreadStore.ThreadPatch], thread th: String) {
        for patch in patches {
            switch patch {
            case .running, .permission:
                // In the thread's summary, which threadChanged sends; and, open, its own topic.
                if case .permission(_, let request) = patch, subscribed.contains(th) {
                    sink(["t": "perm", "th": th, "rv": bump(th), "p": request?.wire ?? NSNull()])
                }
                if case .running(_, let value) = patch, subscribed.contains(th) {
                    sink(["t": "run", "th": th, "rv": bump(th), "v": value])
                }
            case .session(_, let sid):
                library.updateThread(th) { $0["sessionID"] = sid }
            case .add(_, let entry):
                guard subscribed.contains(th) else { bumpQuietly(th); continue }
                sink(["t": "add", "th": th, "rv": bump(th), "e": entry.wire])
            case .append(_, let id, let field, let text):
                guard subscribed.contains(th) else { bumpQuietly(th); continue }
                sink(["t": "app", "th": th, "rv": bump(th), "id": id, "f": field == .text ? "t" : "i", "s": text])
            case .update(_, let entry):
                guard subscribed.contains(th) else { bumpQuietly(th); continue }
                sink(["t": "upd", "th": th, "rv": bump(th), "e": entry.wire])
            }
        }
    }

    private func bump(_ th: String) -> Int {
        let next = (revisions[th] ?? 0) + 1
        revisions[th] = next
        return next
    }

    private func bumpQuietly(_ th: String) { _ = bump(th) }

    // MARK: Helpers

    private func runtime(_ th: String) -> ThreadRuntime {
        if let runtime = runtimes[th] { return runtime }
        let runtime = ThreadRuntime(id: th, backend: self)
        runtimes[th] = runtime
        return runtime
    }

    func model(of th: String) -> String? {
        (library.thread(th)?["model"] as? String) ?? ((library.setting("defaults") as? [String: Any])?["model"] as? String)
    }

    func effort(of th: String) -> String? {
        (library.thread(th)?["effort"] as? String) ?? ((library.setting("defaults") as? [String: Any])?["effort"] as? String)
    }

    private func setProjectMode(ofThread th: String, _ mode: String) {
        guard let projectID = library.thread(th)?["projectID"] as? String else { return }
        library.updateProject(projectID) { $0["permissionMode"] = mode }
        if let project = library.project(projectID) { sink(["t": "project", "p": library.projectWire(project)]) }
    }

    func noteCatalog(_ catalog: ClaudeCatalog) {
        guard let data = try? JSONEncoder().encode(catalog.models),
              let models = try? JSONSerialization.jsonObject(with: data) else { return }
        library.setSetting("claudeModels", models)
        sink(["t": "models", "v": models])
    }

    /// The user's environment (`YH_PATH` in development, else their login shell's) and `claude`.
    func cli() async throws -> ([String: String], URL) {
        if let environment, let executable { return (environment, executable) }
        let environment = await ShellEnvironment.load()
        guard let path = await CLIDiscovery.probe(.claude, environment: environment).path else {
            throw BackendError("claude isn't on your PATH")
        }
        self.environment = environment
        executable = path
        return (environment, path)
    }
}

public struct BackendError: Error, CustomStringConvertible {
    public let description: String
    public init(_ description: String) { self.description = description }
}
