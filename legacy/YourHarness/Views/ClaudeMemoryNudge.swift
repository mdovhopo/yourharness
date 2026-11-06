import HarnessCore
import SwiftUI

/// A nudge toward one memory: shown while Claude Code still keeps its own for the project's
/// folder, or has memories YourHarness doesn't. One button brings them in and turns it off.
struct ClaudeMemoryNudge: View {
    @Environment(AppModel.self) private var app
    @Environment(\.openWindow) private var openWindow
    let project: Project
    /// One line with a way to the Memory page, for the pull requests page.
    var compact = false
    @State private var status: AppModel.ClaudeMemoryStatus?

    var body: some View {
        // Something always there, so the status loads before there's anything to show.
        ZStack {
            Color.clear.frame(width: 0, height: 0)
            if let status, status.needsNudge, !app.claudeMemoryNudgeDismissed.contains(project.id) || !status.unimported.isEmpty {
                if compact { line(status) } else { card(status) }
            }
        }
        .task(id: app.memoryRevision(for: project.id)) { status = await app.claudeMemoryStatus(for: project) }
        .animation(.snappy, value: status)
    }

    private func message(_ status: AppModel.ClaudeMemoryStatus) -> String {
        let count = status.unimported.count
        let memories = count == 1 ? "1 memory" : "\(count) memories"
        switch (status.isOn, count > 0) {
        case (true, true):
            return "Claude Code also keeps its own memory for this folder: \(memories) there aren't in YourHarness, and `claude` sessions you start outside the app keep adding to it. Two memories drift apart; bring them in and keep one."
        case (true, false):
            return status.isHome
                ? "Claude Code still keeps its own memory when you run `claude` outside YourHarness, so two memories would drift apart."
                : "Claude Code still keeps its own memory when you run `claude` in this folder outside YourHarness, so two memories would drift apart."
        default:
            return "Claude Code has \(memories) for this folder that YourHarness doesn't."
        }
    }

    private func primaryTitle(_ status: AppModel.ClaudeMemoryStatus) -> String {
        switch (status.isOn, !status.unimported.isEmpty) {
        case (true, true): "Import & Use One Memory"
        case (true, false): status.isHome ? "Use One Memory Everywhere" : "Use One Memory"
        default: "Import"
        }
    }

    private var detail: String {
        URL(fileURLWithPath: project.path).standardizedFileURL.path == NSHomeDirectory()
            ? "Turns off Claude Code's memory in your Claude Code settings (autoMemoryEnabled: false), for every folder."
            : "Turns off Claude Code's memory for this folder only: autoMemoryEnabled: false in its .claude/settings.local.json, which stays out of git. Its memory files are left as they are."
    }

    private func card(_ status: AppModel.ClaudeMemoryStatus) -> some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: "square.on.square.dashed")
                .font(.title3)
                .foregroundStyle(.orange)
                .frame(width: 24)
            VStack(alignment: .leading, spacing: 8) {
                Text(status.isOn ? "Two memories" : "Not in YourHarness yet").font(.headline)
                Text(markdown(message(status))).fixedSize(horizontal: false, vertical: true)
                HStack(spacing: 8) {
                    Button(primaryTitle(status)) { app.useOneMemory(project, status: status) }
                        .buttonStyle(.glassProminent)
                        .tint(.claude)
                        .help(status.isOn ? detail : "Brings Claude Code's memory files for this folder in.")
                    if !status.unimported.isEmpty {
                        Button("Open in the Importer…") {
                            let agent = app.memoryAgent(.importer, projectID: project.id)
                            agent.pendingImportSources = [app.claudeCodeMemoryFolder(for: project).path]
                            openWindow(id: "memory-agent", value: MemoryAgentKey(projectID: project.id, role: .importer))
                        }
                        .buttonStyle(.glass)
                        .help("Let the Importer go through them: it merges what YourHarness already has, in other words")
                        Button("They're Already In") { app.markClaudeMemorySynced(project) }
                            .buttonStyle(.glass)
                    }
                    if status.isOn {
                        Button("Not Now") { app.dismissClaudeMemoryNudge(project) }
                            .buttonStyle(.borderless)
                    }
                }
                if status.isOn {
                    Text(detail).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(14)
        .card(tint: .orange)
    }

    private func line(_ status: AppModel.ClaudeMemoryStatus) -> some View {
        HStack(spacing: 10) {
            Image(systemName: "square.on.square.dashed").foregroundStyle(.orange)
            Text(status.isOn ? "Claude Code keeps a second memory for this folder." : "Claude Code has memories here that YourHarness doesn't.")
            Spacer()
            Button("Review in Memory") { app.showMemory(of: project) }
                .buttonStyle(.glass)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .card(tint: .orange)
    }

    private func markdown(_ text: String) -> AttributedString {
        (try? AttributedString(markdown: text)) ?? AttributedString(text)
    }
}

/// "One memory": Claude Code's own is off for the folder and nothing of it is left out.
struct OneMemoryChip: View {
    @Environment(AppModel.self) private var app
    let project: Project
    @State private var status: AppModel.ClaudeMemoryStatus?

    var body: some View {
        ZStack {
            Color.clear.frame(width: 0, height: 0)
            if let status, !status.needsNudge {
                Label("One memory", systemImage: "checkmark.seal")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .help(status.files > 0
                          ? "Claude Code's own memory is off for this folder, and its \(status.files) memory files are in YourHarness."
                          : "Claude Code's own memory is off for this folder: YourHarness keeps the only memory.")
            }
        }
        .task(id: app.memoryRevision(for: project.id)) { status = await app.claudeMemoryStatus(for: project) }
    }
}
