import Foundation
import HarnessCore

/// What the composer needs from the backend: slash commands, files to mention, images to send,
/// and the plan Claude is working through.
extension Backend {
    // MARK: Slash commands

    /// A project's commands and skills, as the CLI last listed them (cached per project, as the
    /// frozen app did, so the menu works before the first message).
    func commands(ofThread th: String) -> [Wire] {
        guard let projectID = library.thread(th)?["projectID"] as? String,
              let all = library.setting("slashCommands") as? [String: Any],
              let list = all[projectID] as? [[String: Any]] else { return [] }
        return list.map { ["name": $0["name"] ?? "", "d": $0["description"] ?? "", "hint": $0["argumentHint"] ?? ""] }
    }

    func noteCommands(_ commands: [SlashCommand], thread th: String) {
        guard let projectID = library.thread(th)?["projectID"] as? String,
              let data = try? JSONEncoder().encode(commands), let list = try? JSONSerialization.jsonObject(with: data) else { return }
        var all = library.setting("slashCommands") as? [String: Any] ?? [:]
        all[projectID] = list
        library.setSetting("slashCommands", all)
        for open in subscribedThreads where library.thread(open)?["projectID"] as? String == projectID {
            sink(["t": "commands", "th": open, "v": self.commands(ofThread: open)])
        }
    }

    // MARK: Mentions

    /// Files in the thread's folder matching `query`, ranked; the listing is kept 30 seconds.
    func mentionFiles(_ th: String, query: String) async -> [String] {
        let projectID = library.thread(th)?["projectID"] as? String ?? ""
        let folder = library.folder(ofProject: projectID, dataDirectory: dataDirectory)
        if let cached = fileIndexes[folder], Date().timeIntervalSince(cached.at) < 30 { return cached.index.matches(query) }
        // Git lists what it sees in each repo; outside one, the index walks the folder.
        let environment = (try? await cli().0) ?? ProcessInfo.processInfo.environment
        let git = Git(executable: URL(fileURLWithPath: "/usr/bin/git"), environment: environment)
        let index = await FileIndex.list(URL(fileURLWithPath: folder, isDirectory: true), git: git)
        fileIndexes[folder] = (Date(), index)
        return index.matches(query)
    }

    // MARK: Images

    /// Pasted or dropped images (base64), scaled to what the API takes and saved next to the
    /// database, as the frozen app does; returns them and their saved names.
    func prepareImages(_ list: [String], thread th: String) -> [(ImageAttachment, String)] {
        let directory = dataDirectory.appendingPathComponent("attachments", isDirectory: true)
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return list.compactMap { base64 in
            guard let data = Data(base64Encoded: base64), let image = ImageAttachment.prepare(data) else { return nil }
            let name = "\(th)-\(image.id.uuidString).\(image.fileExtension)"
            guard (try? image.data.write(to: directory.appendingPathComponent(name), options: .atomic)) != nil else { return nil }
            return (image, name)
        }
    }

    // MARK: Plan

    /// The checklist Claude keeps with its task tools, from the rows held.
    func plan(_ th: String) -> [Wire] {
        var plan = TaskPlan()
        for entry in store.entries(th) where entry.kind == .tool {
            if let name = entry.toolName, TaskPlan.toolNames.contains(name) {
                plan.apply(toolName: name, inputJSON: entry.toolInput ?? "", result: entry.toolResult, toolUseID: entry.id)
            }
        }
        return plan.items.map { ["s": $0.subject, "a": $0.activeForm ?? "", "st": $0.status.rawValue] }
    }
}
