import Foundation

/// The JSON files the app kept before the database: `state.json` and one
/// `threads/<id>.json` per transcript. Read once to move them into the database, never
/// written, and left in place so an older version of the app still finds its data.
struct LegacyStore: Sendable {
    let root: URL

    var stateURL: URL { root.appending(path: "state.json") }
    var threadsDirectory: URL { root.appending(path: "threads", directoryHint: .isDirectory) }

    func transcriptURL(_ id: String) -> URL { threadsDirectory.appending(path: "\(id).json") }

    /// The state split the way the database keeps it: each top-level setting on its own,
    /// projects and threads one row each. Nil when there's no state file, or it isn't JSON.
    func state() throws -> HarnessDatabase.StateRows? {
        guard let data = try? Data(contentsOf: stateURL) else { return nil }
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw CocoaError(.coderReadCorrupt)
        }
        var rows = HarnessDatabase.StateRows()
        for (key, value) in object where key != "projects" && key != "threads" {
            rows.values[key] = try Self.json(value)
        }
        for project in object["projects"] as? [[String: Any]] ?? [] {
            guard let id = project["id"] as? String else { continue }
            rows.projects.append(.init(id: id, json: try Self.json(project)))
        }
        for thread in object["threads"] as? [[String: Any]] ?? [] {
            guard let id = thread["id"] as? String, let projectID = thread["projectID"] as? String else { continue }
            rows.threads.append(.init(
                id: id, projectID: projectID,
                updatedAt: (thread["updatedAt"] as? String).flatMap { try? Date.ISO8601FormatStyle().parse($0) },
                sessionID: thread["sessionID"] as? String,
                json: try Self.json(thread)))
        }
        return rows
    }

    /// IDs of the transcripts on disk.
    func transcriptIDs() -> [String] {
        let names = (try? FileManager.default.contentsOfDirectory(atPath: threadsDirectory.path)) ?? []
        return names.compactMap { name in
            guard name.hasSuffix(".json") else { return nil }
            let id = String(name.dropLast(".json".count))
            return UUID(uuidString: id) == nil ? nil : id
        }
    }

    /// One entry it can't read (a newer app's, a damaged one) costs that entry, not the thread.
    func transcript(_ id: String) -> [EntrySnapshot]? {
        guard let data = try? Data(contentsOf: transcriptURL(id)) else { return nil }
        if let entries = try? Self.decoder.decode([EntrySnapshot].self, from: data) { return entries }
        return (try? Self.decoder.decode([Lossy<EntrySnapshot>].self, from: data))?.compactMap(\.value) ?? []
    }

    func transcriptModified(_ id: String) -> Date? {
        try? FileManager.default.attributesOfItem(atPath: transcriptURL(id).path)[.modificationDate] as? Date
    }

    func deleteTranscript(_ id: String) {
        try? FileManager.default.removeItem(at: transcriptURL(id))
    }

    private static func json(_ value: Any) throws -> Data {
        try JSONSerialization.data(withJSONObject: value, options: [.fragmentsAllowed, .sortedKeys, .withoutEscapingSlashes])
    }

    private static let decoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return decoder
    }()
}
