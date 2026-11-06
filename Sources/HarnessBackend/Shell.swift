import Foundation
import HarnessCore

/// The app around the threads: generated titles, usage limits, search, settings, and what
/// should reach you as a notification.
extension Backend {
    // MARK: Titles

    /// After the first message: a title written by a one-shot request with no tools, on the
    /// model chosen in Settings (Haiku by default), as the frozen app does.
    func generateTitle(_ th: String, firstMessage text: String) {
        let settings = library.setting("titleGeneration") as? [String: Any] ?? ["provider": "claude", "model": "haiku"]
        guard let providerName = settings["provider"] as? String, let provider = TitleGenerationSettings.Provider(rawValue: providerName) else { return }
        let model = settings["model"] as? String ?? provider.defaultModel
        Task {
            guard let (environment, executable) = try? await cli() else { return }
            let prompt = ThreadTitlePrompt.initial(message: text)
            var result = await TitleGenerator.generate(prompt: prompt, provider: provider, model: model, executable: executable, environment: environment)
            if case .failure = result {
                try? await Task.sleep(for: .seconds(2))
                result = await TitleGenerator.generate(prompt: prompt, provider: provider, model: model, executable: executable, environment: environment)
            }
            guard case .success(let generated) = result, !generated.title.isEmpty else { return }
            // A title you chose meanwhile stays.
            guard library.thread(th)?["titleChosen"] as? Bool != true else { return }
            library.updateThread(th) { $0["title"] = generated.title }
            threadChanged(th)
        }
    }

    // MARK: Limits

    func noteRateLimit(_ info: RateLimitInfo) {
        guard let data = try? JSONEncoder().encode(info), let object = try? JSONSerialization.jsonObject(with: data) else { return }
        limits = object
        sink(["t": "limits", "v": object])
    }

    // MARK: Search

    /// Threads whose title matches, then what was said in them (the full-text index).
    func search(_ query: String) -> [Wire] {
        let q = query.trimmingCharacters(in: .whitespaces)
        guard q.count >= 2 else { return [] }
        let lower = q.lowercased()
        var results: [Wire] = library.threads.compactMap { t in
            guard let title = t["title"] as? String, title.lowercased().contains(lower) else { return nil }
            return ["th": t["id"] as? String ?? "", "title": title, "kind": "title"]
        }.prefix(8).map { $0 }
        var seen = Set(results.compactMap { $0["th"] as? String })
        for hit in library.db.search(q, limit: 40) where !seen.contains(hit.threadID) {
            guard let t = library.thread(hit.threadID) else { continue }
            seen.insert(hit.threadID)
            results.append(["th": hit.threadID, "title": t["title"] as? String ?? "Thread", "kind": "text", "snippet": hit.snippet, "seq": hit.seq])
            if results.count >= 30 { break }
        }
        return results
    }

    // MARK: Settings

    func settingsWire() -> Wire {
        let defaults = library.setting("defaults") as? [String: Any] ?? [:]
        let titles = library.setting("titleGeneration") as? [String: Any]
        var o: Wire = [:]
        o["model"] = defaults["model"] as? String ?? ""
        o["effort"] = defaults["effort"] as? String ?? ""
        o["mode"] = defaults["permissionMode"] as? String ?? "auto"
        o["titles"] = titles?["provider"] != nil
        o["titleModel"] = titles?["model"] as? String ?? "haiku"
        o["notifications"] = library.setting("notifications") as? Bool ?? true
        o["appearance"] = library.setting("appearance") as? String ?? "system"
        return o
    }

    func setSetting(_ key: String, _ value: Any) {
        switch key {
        case "model", "effort", "mode":
            var defaults = library.setting("defaults") as? [String: Any] ?? [:]
            let name = key == "mode" ? "permissionMode" : key
            if let text = value as? String, !text.isEmpty { defaults[name] = text } else { defaults[name] = nil }
            library.setSetting("defaults", defaults)
        case "titles":
            let on = value as? Bool ?? true
            let titles: [String: Any] = on ? ["provider": "claude", "model": "haiku"] : [:]
            library.setSetting("titleGeneration", titles)
        case "notifications":
            library.setSetting("notifications", value as? Bool ?? true)
        case "appearance":
            library.setSetting("appearance", value as? String ?? "system")
        default:
            return
        }
        sink(["t": "settings", "v": settingsWire()])
    }

    // MARK: Notifications

    /// A turn ended or Claude is waiting on you: the host decides whether it's worth a
    /// notification (not when you're looking) and keeps the Dock badge.
    func noteAttention(_ th: String, waiting: Bool) {
        let count = library.threads.filter { t in (t["id"] as? String).map { store.pendingPermission($0) != nil } ?? false }.count
        attention(.badge(count))
        guard library.setting("notifications") as? Bool ?? true, let t = library.thread(th) else { return }
        let title = t["title"] as? String ?? "Thread"
        if waiting { attention(.notify(thread: th, title: title, body: "Claude is waiting for you")) }
    }
}

/// What the backend wants you to notice; the host shows it natively.
public enum Attention: Sendable {
    case badge(Int)
    case notify(thread: String, title: String, body: String)
}

extension Backend {
    // MARK: Resuming a Claude Code session

    /// A Claude Code session started elsewhere, as a thread that continues it (`--resume`) in
    /// its own folder, with its history. Read from `~/.claude/projects`, never written.
    func resumeSession(_ text: String) async throws -> String {
        guard let sessionID = ClaudeSessionArchive.sessionID(in: text) else { throw BackendError("That isn't a session ID.") }
        if let known = thread(forSession: sessionID), let id = known["id"] as? String { return id }
        let archive = try await loadClaudeSession(sessionID)
        let folder = archive.cwd.flatMap { FileManager.default.fileExists(atPath: $0) ? $0 : nil }
        let projectID = folder.map { library.addProject(path: $0) }
            ?? (library.setting("homeProjectID") as? String) ?? library.projects.first?["id"] as? String ?? ""
        if let project = library.project(projectID) { sink(["t": "project", "p": library.projectWire(project)]) }
        return importSession(archive, into: projectID)
    }

    /// The thread that already continues this Claude Code session, if any.
    func thread(forSession sessionID: String) -> [String: Any]? {
        let wanted = sessionID.lowercased()
        return library.threads.first { ($0["sessionID"] as? String)?.lowercased() == wanted }
    }

    /// Where Claude Code keeps its sessions for this user.
    func claudeConfigDirectory() async -> URL {
        let environment = (try? await cli().0) ?? ProcessInfo.processInfo.environment
        return ClaudeSessionArchive.configDirectory(environment: environment)
    }

    /// A session's history from Claude Code's files, read off the main thread.
    func loadClaudeSession(_ sessionID: String) async throws -> ClaudeSessionArchive {
        let config = await claudeConfigDirectory()
        return try await Task.detached(priority: .userInitiated) {
            guard let url = ClaudeSessionArchive.locate(sessionID: sessionID, configDirectory: config) else {
                throw BackendError("No Claude Code session with this ID.")
            }
            do { return try ClaudeSessionArchive.load(from: url, sessionID: sessionID) } catch { throw BackendError("Couldn't read this session's history.") }
        }.value
    }

    /// A loaded session as a new thread in `projectID`, with its history. When the session ran
    /// somewhere other than the project's folder (a folder inside it, or Home), the thread
    /// keeps running there.
    func importSession(_ archive: ClaudeSessionArchive, into projectID: String) -> String {
        let id = library.addThread(projectID: projectID)
        let title = archive.title ?? archive.firstMessage.map { String($0.split(separator: "\n").first ?? "").prefix(60).description } ?? "Resumed session"
        let projectPath = library.project(projectID)?["path"] as? String
        let folder = archive.cwd.flatMap { FileManager.default.fileExists(atPath: $0) ? $0 : nil }
        library.updateThread(id) {
            $0["sessionID"] = archive.sessionID
            $0["title"] = title
            if archive.hasCustomTitle { $0["titleChosen"] = true }
            if let date = archive.updatedAt { $0["updatedAt"] = Library.iso.string(from: date) }
            if let folder, folder != projectPath { $0["workingDirectory"] = folder }
        }
        if let uuid = UUID(uuidString: id) {
            let rows = archive.items.enumerated().map { Recovery.snapshot($1, id: "h\($0)") }
            library.db.saveTranscript(uuid, entries: rows)
        }
        threadChanged(id)
        return id
    }
}
