// What the phone knows about the Mac: projects, threads, and the open thread's rows. State
// lives here; views draw it.

import Foundation
import Observation

@Observable @MainActor
final class Store: Identifiable {
    /// One Mac: the phone holds a Store for each it knows (`Fleet`), all connected; only the one
    /// shown watches a thread.
    nonisolated let id = UUID()
    var state: Connection.State = .connecting
    var serverName: String?
    var projects: [Project] = []
    private(set) var threads: [String: ThreadInfo] = [:]
    /// Thread IDs in the order the Mac lists them.
    private var order: [String] = []
    /// The navigation stack: thread IDs.
    var path: [String] = []
    /// A thread opened from anywhere: over the thread list (the fleet turns to Threads).
    func show(_ thread: String) {
        onShow?()
        path = [thread]
    }
    @ObservationIgnored var onShow: (() -> Void)?
    /// The Mac the phone shows (the fleet says).
    @ObservationIgnored var shown = false
    /// The Mac said who it is (its name is kept with its address).
    @ObservationIgnored var onHello: (() -> Void)?
    /// The Mac's lasting ID (from its pairing link, then each `hello`): how the phone knows a Mac
    /// it paired with before, at whatever address.
    var macID: String?
    /// The Mac paired this phone: its token, kept in place of the pairing code.
    @ObservationIgnored var onPaired: ((String) -> Void)?
    /// Connecting with a pairing code (it works once): a refusal means it's used or out of date.
    var pairing = false
    /// Why pairing didn't work, shown on its row.
    var pairingFailed: String?

    /// Where it connects, for showing: host and port, never its token or pairing code; the
    /// address it reached the Mac at once connected.
    var shownAddress: String {
        guard let url = URL(string: serverAddress), let host = route ?? url.host() else { return serverAddress }
        return url.port.map { "\(host):\($0)" } ?? host
    }

    /// Every address the Mac may be reached at (its pairing link, then each `hello`), tried at
    /// once on every connect.
    var hosts: [String] = [] {
        didSet { connection.hosts = hosts }
    }
    /// The address the open connection reached the Mac at.
    var route: String?
    /// Reached over Tailscale (its MagicDNS name, or 100.64.0.0/10) rather than the local network.
    var viaTailscale: Bool {
        if route?.lowercased().hasSuffix(".ts.net") == true { return true }
        guard let parts = route?.split(separator: ".").compactMap({ Int($0) }), parts.count == 4 else { return route?.lowercased().hasPrefix("fd7a:115c:a1e0") ?? false }
        return parts[0] == 100 && (64..<128).contains(parts[1])
    }

    /// What the Mac is called here: its name once heard, else its address's host.
    var displayName: String {
        serverName ?? URL(string: serverAddress)?.host() ?? serverAddress
    }

    /// Threads on this Mac waiting on you.
    var waitingCount: Int { threads.values.filter(\.isWaiting).count }
    var runningCount: Int { threads.values.filter(\.isRunning).count }
    var serverAddress: String
    /// 1: the dev server, read-only; 2: the Mac app's server, which takes actions.
    var serverProtocol = 0
    /// The open thread's live state (protocol 2).
    var live: Live?
    /// Why the last action didn't happen, shown until dismissed.
    var actionError: String?
    /// Actions sent and not yet answered, by id: what to put back if one fails.
    private var pending: [String: FromApp] = [:]
    var canAct: Bool { serverProtocol >= 2 && state == .connected }
    /// Protocol 3: answering questions and plans, new threads, model, effort and mode, thread
    /// actions, pictures, whole tool output.
    var canControl: Bool { serverProtocol >= 3 && state == .connected }
    /// The models and permission modes the Mac offers (protocol 3).
    var models: [ModelOption] = []
    var modes: [String] = []
    /// Rows' whole tool input and result, asked for from a row's sheet, by "thread#seq".
    var whole: [String: WholeOutput] = [:]
    /// Sub-agents' whole runs, asked for from their sheet, by "thread#seq".
    var subagentRuns: [String: SubagentRun] = [:]
    /// Protocol 5: each project's slash commands, the usage limits, the last search's hits, and
    /// the open composer's `@` matches.
    var commands: [String: [SlashCommand]] = [:]
    var limits: [LimitWindow] = []
    var searchQuery = ""
    var searchHits: [SearchHit] = []
    var fileMatches: (thread: String, query: String, files: [String])?
    var canSearch: Bool { serverProtocol >= 5 && state == .connected }
    /// Ever connected since launch: tells "can't reach the Mac" apart from "connecting".
    var everConnected = false

    struct WholeOutput: Equatable {
        let input: String?
        let result: String?
    }

    @ObservationIgnored let connection: Connection
    /// The open thread's rows; not observed, the transcript applies its updates itself.
    @ObservationIgnored private(set) var transcript: TranscriptModel?
    /// `YH_SELECT`: a thread (title or id) opened once the list arrives; `active`, the first
    /// thread that's working, whenever that is.
    @ObservationIgnored private var selectOnLoad: String?
    /// A thread made from here, opened once the list has it.
    @ObservationIgnored private var openWhenListed: String?

    static let defaultAddress = "ws://127.0.0.1:7878/"

    /// `selects`: `YH_SELECT` is for this one (the Mac shown at launch).
    init(address: String, name: String? = nil, macID: String? = nil, hosts: [String] = [], selects: Bool = false) {
        serverAddress = address
        self.hosts = hosts
        serverName = name
        self.macID = macID
        if selects { selectOnLoad = ProcessInfo.processInfo.environment["YH_SELECT"] }
        connection = Connection(url: URL(string: address) ?? URL(string: Store.defaultAddress)!)
        pairing = address.contains("pair=")
        connection.hosts = hosts
        connection.onState = { [weak self] in
            guard let self else { return }
            state = $0
            if $0 == .connected {
                everConnected = true
                route = connection.host
                Perf.shared.log("ROUTE \(displayName) via \(route ?? "?")\(viaTailscale ? " (Tailscale)" : "")")
            } else {
                route = nil
            }
            // A pairing code the Mac refused won't work on a retry: stop, and say what to do.
            if pairing, case .waiting(let reason, _) = $0, reason.contains("refused") {
                pairingFailed = "That code is used or out of date. Show a new one on the Mac (Settings → Phone)."
                disconnect()
            }
        }
        connection.onMessages = { [weak self] in self?.apply($0) }
        connection.connect()
    }

    /// Shown now: pictures and diagrams in rows are asked of this Mac as they're shown.
    func becomeCurrent() {
        MediaCache.shared.ask = { [weak self] thread, seq, index in self?.connection.send(.media(thread: thread, seq: seq, index: index)) }
        DiagramCache.shared.ask = { [weak self] key, source in self?.connection.send(.diagram(key: key, source: source)) }
    }

    /// Paired: from now on it connects with its token (the connection open goes on as it is).
    func paired(address: String) {
        serverAddress = address
        pairing = false
        pairingFailed = nil
        connection.retarget(URL(string: address)!)
    }

    /// Removed from the list: no more connecting.
    func disconnect() {
        connection.close()
        state = .waiting(reason: "Removed", retryIn: 0)
    }

    func reconnect() {
        connection.connect()
    }

    // MARK: Threads

    struct Section: Identifiable {
        let project: Project
        let live: [ThreadInfo]
        let settled: [ThreadInfo]
        var id: String { project.id }
    }

    /// As the Mac's sidebar has them: per project, pinned first, then newest made first; settled
    /// ones on a shelf of their own.
    var sections: [Section] {
        let all = order.compactMap { threads[$0] }
        return projects.compactMap { project in
            let mine = all.filter { $0.projectId == project.id }
            // A project with no threads yet still shows, as the Mac's sidebar has it: its header
            // starts one, and its PRs are there.
            let live = mine.filter { !$0.settled }.sorted { ($0.pinned ? 1 : 0, $0.createdAt) > ($1.pinned ? 1 : 0, $1.createdAt) }
            let settled = mine.filter(\.settled).sorted { $0.updatedAt > $1.updatedAt }
            return Section(project: project, live: live, settled: settled)
        }
    }

    func projectName(of thread: ThreadInfo) -> String? {
        projects.first { $0.id == thread.projectId }?.name
    }

    // MARK: The open thread

    func open(_ threadID: String) -> TranscriptModel {
        if let t = transcript, t.threadID == threadID { return t }
        // A Mac not shown watches nothing: its screens can show again for a moment as a switch
        // takes them down, and mustn't pull the thread's rows.
        guard shown else { return TranscriptModel(threadID: threadID, askEarlier: { _ in }, resubscribe: {}) }
        let model = TranscriptModel(
            threadID: threadID,
            askEarlier: { [weak self] before in self?.connection.send(.page(threadID, beforeSeq: before, limit: TranscriptModel.pageSize)) },
            resubscribe: { [weak self] in self?.connection.send(.subscribeThread(threadID, limit: TranscriptModel.pageSize)) })
        transcript = model
        live = nil
        Perf.shared.log("WATCH \(displayName) \(threadID.prefix(8))")
        connection.send(.subscribeThread(threadID, limit: TranscriptModel.pageSize))
        return model
    }

    /// Off screen (the app in the background, the phone locked): the open thread and its side
    /// chat stop streaming, and pick up what changed, from the rows held, once back.
    @ObservationIgnored private var away = false

    func setAway(_ away: Bool) {
        guard self.away != away else { return }
        self.away = away
        guard state == .connected else { return }
        Perf.shared.log("\(away ? "AWAY" : "BACK") \(displayName)")
        if away {
            if let t = transcript { connection.send(.unsubscribe(t.threadID)) }
            if let a = aside { connection.send(.unsubscribe(a.threadID)) }
        } else {
            resubscribe()
        }
    }

    /// The threads on screen watched again (a reconnect, back from the background): protocol 5
    /// sends only what changed from the rows held.
    private func resubscribe() {
        for model in [transcript, aside].compactMap({ $0 }) {
            model.resubscribing()
            connection.send(.subscribeThread(model.threadID, limit: TranscriptModel.pageSize, known: serverProtocol >= 5 ? model.known : nil))
        }
    }

    func close(_ model: TranscriptModel) {
        guard transcript === model else { return }
        transcript = nil
        live = nil
        Perf.shared.log("UNWATCH \(displayName) \(model.threadID.prefix(8))")
        connection.send(.unsubscribeThread)
    }

    // MARK: Actions (protocol 2), as the Mac's own buttons take them

    /// Your message: starts a turn, or waits behind the one under way (`now`: stops it for this).
    func send(_ text: String, to thread: String, now: Bool = false, images: [OutgoingImage] = []) {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty || !images.isEmpty else { return }
        act(.send(id: Self.newID(), thread: thread, text: text, now: now, images: images))
    }

    // MARK: Protocol 3

    /// Claude's question answered: each question's text, and the options chosen (joined) or
    /// what was written.
    func answer(_ request: Request, answers: [String: String], in thread: String) {
        act(.answer(id: Self.newID(), thread: thread, requestID: request.id, answers: answers))
    }

    func skip(_ request: Request, in thread: String) {
        act(.skipQuestion(id: Self.newID(), thread: thread, requestID: request.id))
    }

    func approvePlan(_ request: Request, mode: String, feedback: String, in thread: String) {
        act(.approvePlan(id: Self.newID(), thread: thread, requestID: request.id, mode: mode, feedback: feedback.isEmpty ? nil : feedback))
    }

    func keepPlanning(_ request: Request, feedback: String, in thread: String) {
        act(.keepPlanning(id: Self.newID(), thread: thread, requestID: request.id, feedback: feedback.isEmpty ? nil : feedback))
    }

    func answerQuestion(_ question: MCPQuestion, accept: Bool, in thread: String) {
        act(.answerQuestion(id: Self.newID(), thread: thread, requestID: question.id, accept: accept))
    }

    /// A new thread in `project`; opened once the Mac has made it.
    func newThread(in project: String, text: String? = nil) {
        act(.newThread(id: Self.newID(), project: project, text: text))
    }

    func setModel(_ model: String?, of thread: String) { act(.setModel(id: Self.newID(), thread: thread, model: model)) }
    func setEffort(_ effort: String?, of thread: String) { act(.setEffort(id: Self.newID(), thread: thread, effort: effort)) }
    func setMode(_ mode: String, of thread: String) { act(.setMode(id: Self.newID(), thread: thread, mode: mode)) }
    func rename(_ thread: String, to title: String) { act(.rename(id: Self.newID(), thread: thread, title: title)) }
    func pin(_ thread: String, _ on: Bool) { act(.pin(id: Self.newID(), thread: thread, on: on)) }
    func settle(_ thread: String, _ on: Bool) { act(.settle(id: Self.newID(), thread: thread, on: on)) }

    func delete(_ thread: String) {
        if path.last == thread { path.removeLast() }
        act(.delete(id: Self.newID(), thread: thread))
    }

    /// A row's whole tool input and result, into `whole`.
    func askWhole(thread: String, seq: Int) {
        guard canControl, whole["\(thread)#\(seq)"] == nil else { return }
        connection.send(.toolResult(thread: thread, seq: seq))
    }

    /// A sub-agent's whole run, into `subagentRuns`.
    func askSubagent(thread: String, seq: Int) {
        guard serverProtocol >= 4, state == .connected else { return }
        connection.send(.subagent(thread: thread, seq: seq))
    }

    /// The project a thread is in, and its permission mode.
    func project(of thread: String) -> Project? {
        guard let t = threads[thread] else { return nil }
        return projects.first { $0.id == t.projectId }
    }

    func stop(_ thread: String) {
        act(.stop(id: Self.newID(), thread: thread))
    }

    func respond(_ request: Request, allow: Bool, in thread: String) {
        act(.respond(id: Self.newID(), thread: thread, requestID: request.id, allow: allow))
    }

    func withdraw(_ queued: Queued, in thread: String) {
        act(.withdraw(id: Self.newID(), thread: thread, message: queued.id))
    }

    func sendNow(_ queued: Queued, in thread: String) {
        act(.sendNow(id: Self.newID(), thread: thread, message: queued.id))
    }

    /// A message that failed to send, given back to the composer that sent it.
    var returnedDraft: (thread: String, text: String)?

    private func act(_ action: FromApp) {
        guard canAct, let id = action.actionID else {
            actionError = "The Mac isn't taking actions over this connection."
            return
        }
        pending[id] = action
        connection.send(action)
    }

    private static func newID() -> String { UUID().uuidString.lowercased() }

    // MARK: Protocol 5

    /// Each project's open PRs as last answered, and the projects being asked about.
    var pulls: [String: PullsAnswer] = [:]
    var askingPulls: Set<String> = []

    /// A project's PRs from the Mac (`fresh`: asked of GitHub again, not the last minute's).
    func askPulls(_ project: String, fresh: Bool = false) {
        guard canSearch else { return }
        askingPulls.insert(project)
        connection.send(.pulls(project: project, fresh: fresh))
    }

    /// Searches every thread's words; hits arrive in `searchHits`. Titles are matched here.
    func search(_ query: String) {
        let query = query.trimmingCharacters(in: .whitespaces)
        searchQuery = query
        searchHits = []
        guard canSearch, query.count >= 2 else { return }
        connection.send(.search(query: query))
    }

    /// Files in a thread's folder matching what follows `@`; into `fileMatches`.
    func findFiles(_ query: String, in thread: String) {
        guard canSearch else { return }
        connection.send(.files(thread: thread, query: query))
    }

    /// The slash commands a thread's composer offers.
    func commands(for thread: String) -> [SlashCommand] {
        // `/mcp` opens a sheet on the Mac; `/btw` always works, even before Claude listed the rest.
        let listed = threads[thread].flatMap { commands[$0.projectId] } ?? []
        let mine = listed.filter { $0.name != "mcp" }
        return mine.contains { $0.name == "btw" } ? mine : [SlashCommand(name: "btw", hint: "<question>", description: "Ask a side question. Claude sees this thread; nothing is added to it.")] + mine
    }

    /// The side chat beside a thread (`/btw`): its rows, its live state.
    @ObservationIgnored private(set) var aside: TranscriptModel?
    var asideLive: Live?

    static func asideID(of thread: String) -> String { "side:\(thread)" }

    func openAside(of thread: String) -> TranscriptModel {
        let id = Self.asideID(of: thread)
        if let a = aside, a.threadID == id { return a }
        if let a = aside { close(aside: a) }
        let model = TranscriptModel(
            threadID: id,
            askEarlier: { [weak self] before in self?.connection.send(.page(id, beforeSeq: before, limit: TranscriptModel.pageSize)) },
            resubscribe: { [weak self] in self?.connection.send(.subscribeThread(id, limit: TranscriptModel.pageSize)) })
        aside = model
        asideLive = nil
        connection.send(.subscribeThread(id, limit: TranscriptModel.pageSize))
        return model
    }

    func close(aside model: TranscriptModel) {
        guard aside === model else { return }
        aside = nil
        asideLive = nil
        connection.send(.unsubscribe(model.threadID))
    }

    /// A question asked beside the thread, without disturbing its turn.
    func askAside(_ text: String, in thread: String) {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        act(.askAside(id: Self.newID(), thread: thread, text: text))
    }

    // MARK: Messages

    private func apply(_ batch: [ToApp]) {
        let rows = batch.reduce(0) { n, m in
            switch m {
            case .snapshot(_, _, _, let r), .rowsChanged(_, _, let r), .pageRows(_, let r): n + r.count
            default: n
            }
        }
        Perf.shared.measure(rows: rows) { applyNow(batch) }
    }

    private func applyNow(_ batch: [ToApp]) {
        var rowsByThread: [String: [RowModel]] = [:]
        for message in batch {
            switch message {
            case .hello(let version, let server, _, let mac, let hosts):
                serverName = server
                if let mac { macID = mac }
                if !hosts.isEmpty, hosts != self.hosts { self.hosts = hosts }
                serverProtocol = version
                onHello?()
                connection.send(.subscribeApp)
                if !away { resubscribe() }
            case .threads(let projects, let list):
                self.projects = projects
                order = list.map(\.id)
                threads = Dictionary(list.map { ($0.id, $0) }, uniquingKeysWith: { $1 })
                selectIfAsked()
                if let wanted = openWhenListed, threads[wanted] != nil { path = [wanted]; openWhenListed = nil }
            case .threadUpserted(let thread):
                if threads[thread.id] == nil { order.append(thread.id) }
                threads[thread.id] = thread
                selectIfAsked()
                if openWhenListed == thread.id { path = [thread.id]; openWhenListed = nil }
            case .threadRemoved(let id):
                threads[id] = nil
                order.removeAll { $0 == id }
            case .snapshot(let thread, let firstSeq, let count, let rows):
                if let t = model(of: thread) {
                    flush(&rowsByThread, into: t)
                    t.applySnapshot(firstSeq: firstSeq, count: count, rows: rows)
                }
            case .rowsChanged(let thread, let count, let rows):
                guard let t = model(of: thread) else { continue }
                rowsByThread[thread, default: []] += rows
                t.count = max(t.count, count)
            case .pageRows(let thread, let rows):
                if let t = model(of: thread) {
                    flush(&rowsByThread, into: t)
                    t.applyPage(rows)
                }
            case .live(let thread, let state):
                if transcript?.threadID == thread, live != state { live = state }
                if aside?.threadID == thread, asideLive != state { asideLive = state }
            case .options(let models, let modes, let commands):
                self.models = models
                self.modes = modes
                self.commands = commands
            case .limits(let windows):
                if limits != windows { limits = windows }
            case .searchResults(let query, let hits):
                if query == searchQuery { searchHits = hits }
            case .files(let thread, let query, let files):
                fileMatches = (thread, query, files)
            case .toolResult(let thread, let seq, let input, let result):
                whole["\(thread)#\(seq)"] = WholeOutput(input: input, result: result)
            case .subagent(let thread, let seq, let run):
                subagentRuns["\(thread)#\(seq)"] = run
            case .media(let thread, let seq, let index, let image, let error):
                MediaCache.shared.deliver(thread: thread, seq: seq, index: index, image: image, error: error)
            case .diagram(let key, let drawing, let error):
                DiagramCache.shared.deliver(key: key, drawing: drawing, error: error)
            case .done(let id, let error, let made):
                let action = pending.removeValue(forKey: id)
                // A thread made from here: opened as soon as the list has it.
                if case .newThread? = action, let made, error == nil {
                    openWhenListed = made
                    if threads[made] != nil { path = [made]; openWhenListed = nil }
                }
                if let error {
                    actionError = error
                    if case .send(_, let thread, let text, _, _)? = action { returnedDraft = (thread, text) }
                }
                Perf.shared.log("DONE \(action?.name ?? id)\(error.map { ": " + $0 } ?? "")")
            case .pulls(let project, let answer):
                pulls[project] = answer
                askingPulls.remove(project)
                Perf.shared.log("PULLS \(answer.pulls.count) in \(projects.first { $0.id == project }?.name ?? project)")
            case .paired(_, let token):
                onPaired?(token)
                Perf.shared.log("PAIRED \(displayName)")
            case .error(let message):
                print("server: \(message)")
            case .unknown:
                break
            }
        }
        if let t = transcript { flush(&rowsByThread, into: t) }
        if let a = aside { flush(&rowsByThread, into: a) }
    }

    private func model(of thread: String) -> TranscriptModel? {
        if let t = transcript, t.threadID == thread { return t }
        if let a = aside, a.threadID == thread { return a }
        return nil
    }

    /// Row changes gathered from a batch, applied to the transcript as one update.
    private func flush(_ rows: inout [String: [RowModel]], into t: TranscriptModel) {
        if let changed = rows.removeValue(forKey: t.threadID), !changed.isEmpty {
            t.applyChanges(changed)
        }
    }

    private func selectIfAsked() {
        guard let wanted = selectOnLoad else { return }
        if wanted == "active" {
            guard let t = order.lazy.compactMap({ self.threads[$0] }).first(where: \.active) else { return }
            selectOnLoad = nil
            path = [t.id]
            return
        }
        selectOnLoad = nil
        if let t = threads[wanted] ?? threads.values.first(where: { $0.title == wanted }) {
            // `YH_SELECT_DELAY=<s>`: opened that much later, so its timings stand apart from launch's.
            let delay = ProcessInfo.processInfo.environment["YH_SELECT_DELAY"].flatMap(Double.init) ?? 0
            Task { @MainActor in
                try? await Task.sleep(for: .seconds(delay))
                Perf.shared.log("OPEN \(t.title)")
                path = [t.id]
            }
        }
    }
}

/// The open thread's rows: a run of consecutive `seq`s, the newest ones, growing upward as
/// earlier pages are read. Tells its view exactly which rows changed.
@MainActor
final class TranscriptModel {
    enum Update {
        case reload
        case changed(appended: [Int], changed: [Int])
        case prepended(Int)
        /// Rows to measure again with nothing new in them (a diagram in them was drawn).
        case remeasure([Int])
    }

    static let pageSize = 160

    let threadID: String
    private(set) var rows: [RowModel] = []
    private(set) var firstSeq = 0
    var count = 0
    private(set) var loaded = false
    private var asking = false
    private let askEarlier: (Int) -> Void
    private let resubscribe: () -> Void
    var onUpdate: ((Update) -> Void)?

    init(threadID: String, askEarlier: @escaping (Int) -> Void, resubscribe: @escaping () -> Void) {
        self.threadID = threadID
        self.askEarlier = askEarlier
        self.resubscribe = resubscribe
    }

    var hasEarlier: Bool { loaded && firstSeq > 0 }

    func row(_ seq: Int) -> RowModel? {
        let i = seq - firstSeq
        return rows.indices.contains(i) ? rows[i] : nil
    }

    func resubscribing() {
        asking = false
    }

    /// The rows held up to, for resuming after a reconnect (protocol 5).
    var known: Int? { loaded ? firstSeq + rows.count : nil }

    func applySnapshot(firstSeq: Int, count: Int, rows incoming: [RowModel]) {
        // Resuming: the snapshot starts inside what's held, so only what differs changes.
        let end = self.firstSeq + rows.count
        if loaded, firstSeq >= self.firstSeq, firstSeq <= end, count >= end {
            var appended: [Int] = []
            var changed: [Int] = []
            for row in incoming.sorted(by: { $0.seq < $1.seq }) {
                let i = row.seq - self.firstSeq
                if rows.indices.contains(i) {
                    if !row.row.sameContent(as: rows[i].row) {
                        rows[i] = row
                        changed.append(row.seq)
                    } else if row.rev > rows[i].rev {
                        // Same content, the server's newer number: kept so later changes compare right.
                        rows[i] = row
                    }
                } else if i == rows.count {
                    rows.append(row)
                    appended.append(row.seq)
                }
            }
            self.count = count
            asking = false
            Perf.shared.log("RESUME from \(firstSeq): \(changed.count) changed, \(appended.count) new")
            if !appended.isEmpty || !changed.isEmpty { onUpdate?(.changed(appended: appended, changed: changed)) }
            return
        }
        let rows = incoming
        self.firstSeq = firstSeq
        self.count = count
        self.rows = rows
        loaded = true
        asking = false
        onUpdate?(.reload)
    }

    func applyChanges(_ incoming: [RowModel]) {
        guard loaded else { return }
        var appended: [Int] = []
        var changed: Set<Int> = []
        for row in incoming.sorted(by: { ($0.seq, $0.rev) < ($1.seq, $1.rev) }) {
            let i = row.seq - firstSeq
            if rows.indices.contains(i) {
                if row.rev > rows[i].rev {
                    rows[i] = row
                    if !appended.contains(row.seq) { changed.insert(row.seq) }
                }
            } else if i == rows.count {
                rows.append(row)
                appended.append(row.seq)
            } else if i > rows.count {
                // A gap: rows we never got. Start over from a fresh snapshot.
                loaded = false
                resubscribe()
                return
            }
        }
        if !appended.isEmpty || !changed.isEmpty {
            onUpdate?(.changed(appended: appended, changed: changed.sorted()))
        }
    }

    func loadEarlier() {
        guard hasEarlier, !asking else { return }
        asking = true
        askEarlier(firstSeq)
    }

    func applyPage(_ page: [RowModel]) {
        asking = false
        let earlier = page.filter { $0.seq < firstSeq }.sorted { $0.seq < $1.seq }
        guard let last = earlier.last, last.seq == firstSeq - 1 else { return }
        rows.insert(contentsOf: earlier, at: 0)
        firstSeq = earlier[0].seq
        onUpdate?(.prepended(earlier.count))
    }
}
