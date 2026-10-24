import HarnessCore
import SwiftUI

/// Paste a Claude Code session ID; see what it is; open it as a thread that continues it.
struct ResumeSessionSheet: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    @State private var input = ""
    @State private var lookup = Lookup.idle
    /// Which project the thread goes in.
    @State private var destination = Destination.newProject
    @FocusState private var focused: Bool

    private enum Destination: Hashable {
        case project(UUID)
        /// A new project for the session's folder.
        case newProject
    }

    private enum Lookup: Equatable {
        case idle, loading
        case found(ClaudeSessionArchive)
        case failed(String)
    }

    private var sessionID: String? { ClaudeSessionArchive.sessionID(in: input) }

    private var archive: ClaudeSessionArchive? {
        if case .found(let archive) = lookup { archive } else { nil }
    }

    private var existing: ThreadModel? { sessionID.flatMap(app.thread(forSession:)) }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Resume a Claude Code Session")
                    .font(.headline)
                Text("Paste a session ID or a `claude --resume` command. The thread shows the session's history and continues where it left off.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            TextField("Session ID", text: $input, prompt: Text("e.g. 3f2b9c1e-…"))
                .font(.body.monospaced())
                .textFieldStyle(.roundedBorder)
                .focused($focused)
                .onSubmit(resume)

            preview
                .frame(maxWidth: .infinity, minHeight: 64, alignment: .topLeading)

            if let archive, existing == nil {
                destinationPicker(for: archive)
            }

            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button(existing == nil ? "Resume" : "Show Thread", action: resume)
                    .keyboardShortcut(.defaultAction)
                    .disabled(archive == nil && existing == nil)
            }
        }
        .padding(20)
        .frame(width: 480)
        .onAppear {
            focused = true
            if let preset = ProcessInfo.processInfo.environment["YH_RESUME_SESSION"] {
                input = preset
                return
            }
            // A copied ID is the usual starting point.
            if let pasted = NSPasteboard.general.string(forType: .string), ClaudeSessionArchive.sessionID(in: pasted) != nil,
               pasted.count < 300 {
                input = pasted.trimmingCharacters(in: .whitespacesAndNewlines)
            }
        }
        .task(id: sessionID) { await look(up: sessionID) }
    }

    @ViewBuilder
    private var preview: some View {
        switch lookup {
        case .idle:
            if !input.isEmpty, sessionID == nil {
                Label("That doesn't look like a session ID.", systemImage: "questionmark.circle")
                    .foregroundStyle(.secondary)
            }
        case .loading:
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Looking in Claude Code's history…").foregroundStyle(.secondary)
            }
        case .failed(let message):
            Label(message, systemImage: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
        case .found(let archive):
            SessionPreview(archive: archive, existing: existing)
        }
    }

    private func look(up sessionID: String?) async {
        guard let sessionID else {
            lookup = .idle
            return
        }
        lookup = .loading
        let result = await app.loadClaudeSession(sessionID)
        guard !Task.isCancelled else { return }
        switch result {
        case .success(let archive):
            destination = app.suggestedProject(forSessionFolder: archive.cwd).map { .project($0.id) } ?? .newProject
            lookup = .found(archive)
            // Development: `YH_RESUME_CONFIRM=1` resumes into the suggested project, as clicking Resume does.
            if ProcessInfo.processInfo.environment["YH_RESUME_CONFIRM"] == "1" { resume() }
        case .failure(let error): lookup = .failed(error.localizedDescription)
        }
    }

    private func resume() {
        if let existing {
            app.selectedThreadID = existing.id
            dismiss()
            return
        }
        guard let archive else { return }
        let project: Project? = if case .project(let id) = destination { app.project(id: id) } else { nil }
        do {
            try app.resumeClaudeSession(archive, into: project)
            dismiss()
        } catch {
            lookup = .failed(error.localizedDescription)
        }
    }
}

extension ResumeSessionSheet {
    /// Your projects, the one the session belongs in picked to start with. A project
    /// whose folder isn't the session's still runs the thread in the session's folder.
    private func destinationPicker(for archive: ClaudeSessionArchive) -> some View {
        let folderName = archive.cwd.map { URL(fileURLWithPath: $0).lastPathComponent }
        let hasOwnProject = archive.cwd == nil || app.projects.contains {
            URL(fileURLWithPath: $0.path).standardizedFileURL.path == URL(fileURLWithPath: archive.cwd!).standardizedFileURL.path
        }
        return Picker("Add to", selection: $destination) {
            if !hasOwnProject, let folderName {
                Label("New Project “\(folderName)”", systemImage: "folder.badge.plus")
                    .tag(Destination.newProject)
                Divider()
            }
            ForEach(app.projects) { project in
                Label(project.name, systemImage: app.isHome(project) ? "house" : "folder")
                    .tag(Destination.project(project.id))
            }
        }
        .pickerStyle(.menu)
    }
}

private struct SessionPreview: View {
    @Environment(AppModel.self) private var app
    let archive: ClaudeSessionArchive
    let existing: ThreadModel?

    private var folder: String {
        guard let cwd = archive.cwd else { return app.homeProject.name }
        return cwd.replacingOccurrences(of: NSHomeDirectory(), with: "~", options: .anchored)
    }

    private var details: String {
        var parts = [archive.messageCount == 1 ? "1 message" : "\(archive.messageCount) messages"]
        if let updated = archive.updatedAt {
            parts.append("active \(updated.formatted(.relative(presentation: .named)))")
        }
        if let model = archive.model { parts.append(model) }
        return parts.joined(separator: " · ")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(archive.title ?? archive.firstMessage.map(ThreadTitlePrompt.seed) ?? "Untitled session")
                .font(.body.weight(.semibold))
                .lineLimit(2)
            Label(folder, systemImage: "folder")
                .font(.callout.monospaced())
                .lineLimit(1)
                .truncationMode(.middle)
            Text(details)
                .font(.callout)
                .foregroundStyle(.secondary)
            if let existing {
                Label("Already open as “\(existing.title)”.", systemImage: "checkmark.circle")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .card()
    }
}
