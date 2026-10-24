import Foundation
import HarnessCore

/// Development: `YH_SEARCH_PROBE=<query>` moves every old transcript into the database and
/// brings the full-text index up to date at launch, then searches every thread for `query`
/// and prints to stderr how long each step took and what it found. For checking the move
/// and the index on real-sized data. Off unless set.
@MainActor
enum SearchProbe {
    static func runIfRequested(app: AppModel) {
        guard let query = ProcessInfo.processInfo.environment["YH_SEARCH_PROBE"], !query.isEmpty else { return }
        let database = app.store.database
        let titles = Dictionary(app.threads.map { ($0.id.uuidString, $0.title) }) { first, _ in first }
        Task.detached(priority: .utility) {
            let clock = ContinuousClock()
            var start = clock.now
            database.importAllLegacyTranscripts()
            let imported = clock.now - start
            start = clock.now
            database.catchUpIndex()
            let indexed = clock.now - start
            start = clock.now
            let hits = database.search(query, limit: 20)
            let searched = clock.now - start
            var lines = ["SEARCH moved in \(imported), indexed through rev \(database.bookmark("fts")) in \(indexed), searched in \(searched): \(hits.count) hits"]
            for hit in hits {
                lines.append("SEARCH \(titles[hit.threadID] ?? hit.threadID) #\(hit.seq) \(hit.kind.rawValue): \(hit.snippet.replacingOccurrences(of: "\n", with: " "))")
            }
            FileHandle.standardError.write(Data((lines.joined(separator: "\n") + "\n").utf8))
        }
    }
}
