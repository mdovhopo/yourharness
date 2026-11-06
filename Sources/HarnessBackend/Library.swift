import Foundation
import HarnessCore

/// Projects, threads and settings, in the same `harness.sqlite` and the same JSON as the frozen
/// app (so the user's threads carry over, both ways). Records are kept as the JSON they were
/// read as and changed field by field: whatever the old app wrote, this keeps.
@MainActor
public final class Library {
    public let db: HarnessDatabase
    private var values: [String: Data]
    private(set) var projects: [[String: Any]]
    private(set) var threads: [[String: Any]]
    private var saveScheduled = false

    static let iso: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime]
        return f
    }()

    public init(directory: URL) {
        db = HarnessDatabase(directory: directory)
        let state = db.loadState()
        values = state.values
        projects = state.projects.compactMap { try? JSONSerialization.jsonObject(with: $0.json) as? [String: Any] }
        threads = state.threads.compactMap { try? JSONSerialization.jsonObject(with: $0.json) as? [String: Any] }
    }

    // MARK: Reading

    func project(_ id: String) -> [String: Any]? { projects.first { $0["id"] as? String == id } }
    func thread(_ id: String) -> [String: Any]? { threads.first { $0["id"] as? String == id } }

    func date(_ value: Any?) -> Date? { (value as? String).flatMap(Self.iso.date(from:)) }

    /// A setting as JSON (`defaults`, `claudeModels`, `homeProjectID`…).
    func setting(_ key: String) -> Any? {
        values[key].flatMap { try? JSONSerialization.jsonObject(with: $0, options: .fragmentsAllowed) }
    }

    func setSetting(_ key: String, _ value: Any) {
        values[key] = try? JSONSerialization.data(withJSONObject: value, options: [.fragmentsAllowed, .sortedKeys, .withoutEscapingSlashes])
        scheduleSave()
    }

    /// Where a project's threads run: its folder, or for one that's gone, a folder in the data dir.
    func folder(ofProject id: String, dataDirectory: URL) -> String {
        if let path = project(id)?["path"] as? String, FileManager.default.fileExists(atPath: path) { return path }
        let home = dataDirectory.appendingPathComponent("home", isDirectory: true)
        try? FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        return home.path
    }

    func projectWire(_ p: [String: Any]) -> Wire {
        ["id": p["id"] as? String ?? "", "name": p["name"] as? String ?? "Project", "path": p["path"] as? String ?? "",
         "mode": p["permissionMode"] as? String ?? "auto",
         "home": (setting("homeProjectID") as? String) == (p["id"] as? String)]
    }

    func threadWire(_ t: [String: Any], running: Bool, waiting: Bool) -> Wire {
        let updated = date(t["updatedAt"]) ?? .distantPast
        let reactivated = date(t["reactivatedAt"]) ?? .distantPast
        var o: Wire = ["id": t["id"] as? String ?? "", "projectId": t["projectID"] as? String ?? "",
                       "title": t["title"] as? String ?? "Thread",
                       "updatedAt": max(updated, reactivated).timeIntervalSince1970 * 1000,
                       "pinned": t["isPinned"] as? Bool ?? false,
                       "running": running, "waiting": waiting]
        if let settled = date(t["settledAt"]) { o["settledAt"] = settled.timeIntervalSince1970 * 1000 }
        if let model = t["model"] as? String { o["model"] = model }
        if let effort = t["effort"] as? String { o["effort"] = effort }
        if let draft = t["draft"] as? String, !draft.isEmpty { o["draft"] = draft }
        return o
    }

    // MARK: Changing

    func updateThread(_ id: String, _ change: (inout [String: Any]) -> Void) {
        guard let i = threads.firstIndex(where: { $0["id"] as? String == id }) else { return }
        change(&threads[i])
        scheduleSave()
    }

    func updateProject(_ id: String, _ change: (inout [String: Any]) -> Void) {
        guard let i = projects.firstIndex(where: { $0["id"] as? String == id }) else { return }
        change(&projects[i])
        scheduleSave()
    }

    func addThread(projectID: String) -> String {
        let id = UUID().uuidString
        let now = Self.iso.string(from: Date())
        threads.insert(["id": id, "projectID": projectID, "title": "New thread", "createdAt": now, "updatedAt": now], at: 0)
        scheduleSave()
        return id
    }

    /// A folder as a project; the one already there if it's known.
    @discardableResult
    public func addProject(path: String) -> String {
        if let known = projects.first(where: { $0["path"] as? String == path }), let id = known["id"] as? String { return id }
        let id = UUID().uuidString
        projects.append(["id": id, "name": (path as NSString).lastPathComponent, "path": path, "permissionMode": "auto"])
        scheduleSave()
        return id
    }

    func removeThread(_ id: String) {
        threads.removeAll { $0["id"] as? String == id }
        if let uuid = UUID(uuidString: id) { db.deleteTranscript(uuid) }
        scheduleSave()
    }

    func now() -> String { Self.iso.string(from: Date()) }

    // MARK: Saving

    /// Changes collapse into one save a moment later; the database writes off the main thread.
    func scheduleSave() {
        guard !saveScheduled else { return }
        saveScheduled = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in
            MainActor.assumeIsolated { self?.saveNow() }
        }
    }

    public func saveNow() {
        saveScheduled = false
        func data(_ object: [String: Any]) -> Data {
            (try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes])) ?? Data("{}".utf8)
        }
        let state = HarnessDatabase.StateRows(
            values: values,
            projects: projects.map { .init(id: $0["id"] as? String ?? "", json: data($0)) },
            threads: threads.map {
                .init(id: $0["id"] as? String ?? "", projectID: $0["projectID"] as? String ?? "",
                      updatedAt: date($0["updatedAt"]), sessionID: $0["sessionID"] as? String, json: data($0))
            })
        db.saveState(state)
    }
}
