// What the phone knows about the Mac: projects, threads, and the open thread's rows. State
// lives here; views draw it.

import Foundation
import Observation

@Observable @MainActor
final class Store {
    var state: Connection.State = .connecting
    var serverName: String?
    var projects: [Project] = []
    private(set) var threads: [String: ThreadInfo] = [:]
    /// Thread IDs in the order the Mac lists them.
    private var order: [String] = []
    /// The navigation stack: thread IDs.
    var path: [String] = []
    var serverAddress: String

    @ObservationIgnored let connection: Connection
    /// The open thread's rows; not observed, the transcript applies its updates itself.
    @ObservationIgnored private(set) var transcript: TranscriptModel?
    /// `YH_SELECT`: a thread (title or id) opened once the list arrives; `active`, the first
    /// thread that's working, whenever that is.
    @ObservationIgnored private var selectOnLoad = ProcessInfo.processInfo.environment["YH_SELECT"]

    static let defaultAddress = "ws://127.0.0.1:7878/"

    init() {
        let env = ProcessInfo.processInfo.environment["YH_SERVER"]
        let address = env ?? UserDefaults.standard.string(forKey: "server") ?? Store.defaultAddress
        serverAddress = address
        connection = Connection(url: URL(string: address) ?? URL(string: Store.defaultAddress)!)
        connection.onState = { [weak self] in self?.state = $0 }
        connection.onMessages = { [weak self] in self?.apply($0) }
        connection.connect()
    }

    func connect(to address: String) {
        guard let url = URL(string: address), url.scheme == "ws" || url.scheme == "wss" else { return }
        serverAddress = address
        UserDefaults.standard.set(address, forKey: "server")
        connection.connect(to: url)
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
            guard !mine.isEmpty else { return nil }
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
        let model = TranscriptModel(
            threadID: threadID,
            askEarlier: { [weak self] before in self?.connection.send(.page(threadID, beforeSeq: before, limit: TranscriptModel.pageSize)) },
            resubscribe: { [weak self] in self?.connection.send(.subscribeThread(threadID, limit: TranscriptModel.pageSize)) })
        transcript = model
        connection.send(.subscribeThread(threadID, limit: TranscriptModel.pageSize))
        return model
    }

    func close(_ model: TranscriptModel) {
        guard transcript === model else { return }
        transcript = nil
        connection.send(.unsubscribeThread)
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
            case .hello(let server, _):
                serverName = server
                connection.send(.subscribeApp)
                if let t = transcript {
                    t.resubscribing()
                    connection.send(.subscribeThread(t.threadID, limit: TranscriptModel.pageSize))
                }
            case .threads(let projects, let list):
                self.projects = projects
                order = list.map(\.id)
                threads = Dictionary(list.map { ($0.id, $0) }, uniquingKeysWith: { $1 })
                selectIfAsked()
            case .threadUpserted(let thread):
                if threads[thread.id] == nil { order.append(thread.id) }
                threads[thread.id] = thread
                selectIfAsked()
            case .threadRemoved(let id):
                threads[id] = nil
                order.removeAll { $0 == id }
            case .snapshot(let thread, let firstSeq, let count, let rows):
                if let t = transcript, t.threadID == thread {
                    flush(&rowsByThread, into: t)
                    t.applySnapshot(firstSeq: firstSeq, count: count, rows: rows)
                }
            case .rowsChanged(let thread, let count, let rows):
                guard let t = transcript, t.threadID == thread else { continue }
                rowsByThread[thread, default: []] += rows
                t.count = max(t.count, count)
            case .pageRows(let thread, let rows):
                if let t = transcript, t.threadID == thread {
                    flush(&rowsByThread, into: t)
                    t.applyPage(rows)
                }
            case .error(let message):
                print("server: \(message)")
            case .unknown:
                break
            }
        }
        if let t = transcript { flush(&rowsByThread, into: t) }
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

    func applySnapshot(firstSeq: Int, count: Int, rows: [RowModel]) {
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
