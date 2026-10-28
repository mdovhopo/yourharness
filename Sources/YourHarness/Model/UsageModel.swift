import Foundation
import HarnessCore
import Observation
import Synchronization

/// The usage view: what Claude Code's session files and the app's own off-the-record
/// sessions spent, for a range of days. Read off the main thread; the index catches up with
/// new history each time the view opens, and while it's open.
@MainActor @Observable
final class UsageModel {
    enum Range: String, CaseIterable, Identifiable, Codable {
        case week, month, quarter, halfYear, year

        var id: Self { self }

        var days: Int {
            switch self {
            case .week: 7
            case .month: 30
            case .quarter: 90
            case .halfYear: 180
            case .year: 365
            }
        }

        /// In the picker.
        var label: String {
            switch self {
            case .week: "Week"
            case .month: "Month"
            case .quarter: "90 Days"
            case .halfYear: "180 Days"
            case .year: "Year"
            }
        }

        var title: String {
            switch self {
            case .week: "Past week"
            case .month: "Past month"
            case .quarter: "Past 90 days"
            case .halfYear: "Past 180 days"
            case .year: "Past year"
            }
        }
    }

    private(set) var isOpen = false
    var range: Range = .week {
        didSet {
            guard range != oldValue else { return }
            onRangeChange?()
            reload()
        }
    }
    private(set) var report: UsageReport = .empty
    /// Session files read so far and to read, while the index catches up.
    private(set) var progress: (read: Int, total: Int)?
    private(set) var isReading = false
    private(set) var updatedAt: Date?

    /// Set up once the data folder is known.
    @ObservationIgnored private(set) var store: UsageStore?
    @ObservationIgnored var onRangeChange: (() -> Void)?
    /// Where Claude Code keeps its history: known once the environment is loaded.
    @ObservationIgnored var configDirectory: () async -> URL = { ClaudeSessionArchive.configDirectory(environment: ProcessInfo.processInfo.environment) }
    @ObservationIgnored private var pending: [MessageUsage] = []
    @ObservationIgnored private var flushScheduled = false
    @ObservationIgnored private var reportTask: Task<Void, Never>?

    func attach(directory: URL) {
        store = UsageStore(directory: directory)
    }

    func open() {
        isOpen = true
        refresh()
    }

    func close() {
        isOpen = false
    }

    func toggle() {
        isOpen ? close() : open()
    }

    /// Reads what's new in the session files, then sums the range again.
    func refresh() {
        guard !isReading, let store else { return }
        isReading = true
        let days = range.days
        Task {
            let config = await configDirectory()
            let started = Date.now
            let result = await Task.detached(priority: .userInitiated) {
                // A first read of a long history takes a while: show what's in so far as it goes.
                let partial = Mutex(Date.now)
                return store.refresh(configDirectory: config) { read, total in
                    let due = partial.withLock { last -> Bool in
                        guard Date.now.timeIntervalSince(last) > 1.5 else { return false }
                        last = .now
                        return true
                    }
                    let report = due ? store.report(days: days) : nil
                    Task { @MainActor in
                        self.progress = total > 0 && read < total ? (read, total) : nil
                        if let report, self.range.days == days { self.report = report }
                    }
                }
            }.value
            // Development runs say how long reading took.
            if ProcessInfo.processInfo.environment["YH_DATA_DIR"] != nil {
                FileHandle.standardError.write(Data("Usage: read \(result.filesRead) of \(result.filesTotal) changed files, \(result.records) records, in \(String(format: "%.2f", Date.now.timeIntervalSince(started)))s\n".utf8))
            }
            isReading = false
            progress = nil
            updatedAt = .now
            reload()
        }
    }

    /// Sums the range again from the index, without reading files.
    func reload() {
        reportTask?.cancel()
        guard let store else { return }
        let days = range.days
        reportTask = Task {
            let started = Date.now
            let report = await Task.detached(priority: .userInitiated) { store.report(days: days) }.value
            if ProcessInfo.processInfo.environment["YH_DATA_DIR"] != nil {
                FileHandle.standardError.write(Data("Usage: \(days)-day report in \(String(format: "%.3f", Date.now.timeIntervalSince(started)))s\n".utf8))
            }
            guard !Task.isCancelled else { return }
            self.report = report
        }
    }

    /// A response from a session that leaves no file (a side chat, a memory agent). Saved a
    /// moment later, with the rest of its stream.
    func record(_ usage: MessageUsage) {
        pending.append(usage)
        guard !flushScheduled else { return }
        flushScheduled = true
        Task {
            try? await Task.sleep(for: .seconds(2))
            await flush()
            if isOpen { reload() }
        }
    }

    /// Writes what `record` is holding now: before Export State takes a copy.
    func flush() async {
        flushScheduled = false
        let batch = pending
        pending = []
        guard !batch.isEmpty, let store else { return }
        await Task.detached(priority: .utility) { store.record(batch) }.value
    }
}
