import HarnessCore
import SwiftUI

/// Import State's question: what's in the file, what you'd lose, and that there's no undo.
struct StateImportSheet: View {
    @Environment(AppModel.self) private var app
    let pending: StateTransfer.PendingImport
    @State private var keepingCopy = true

    private var transfer: StateTransfer { app.stateTransfer }
    private var manifest: StateArchive.Manifest { pending.opened.manifest }
    private var file: StateArchive.Contents { manifest.contents }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(alignment: .top, spacing: 14) {
                Image(systemName: "exclamationmark.triangle.fill")
                    .font(.system(size: 30))
                    .foregroundStyle(.orange)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 6) {
                    Text("Replace everything with this state?")
                        .font(.headline)
                    Text("Every project, thread, memory and setting you have now is replaced by the ones in “\(pending.file.lastPathComponent)”. This is destructive and can't be undone.")
                        .font(.callout)
                        .fixedSize(horizontal: false, vertical: true)
                    Text(origin)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            comparison

            VStack(alignment: .leading, spacing: 6) {
                if !file.projects.isEmpty {
                    note("folder", "Projects in the file: \(file.projects.joined(separator: ", ")).")
                }
                if pending.workingThreads > 0 {
                    note("stop.circle", "\(StateTransfer.count(pending.workingThreads, "thread is", "threads are")) working now. They're stopped.")
                }
                if manifest.schemaVersion > StateArchive.appSchemaVersion {
                    note("arrow.up.circle", "It's from a newer version of YourHarness: what that version added may not show here.")
                }
                note("app.badge", "YourHarness restarts to bring it in. Your git worktrees and project folders aren't touched.")
            }

            VStack(alignment: .leading, spacing: 2) {
                Toggle("Save a copy of my current state first", isOn: $keepingCopy)
                Text("Into “backups” in YourHarness's data folder, with your browser sign-ins, so Import State can bring it back.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.leading, 20)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .disabled(transfer.busy != nil)

            if let problem = transfer.importProblem {
                Label(problem, systemImage: "exclamationmark.triangle.fill")
                    .font(.callout)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }

            HStack {
                if let busy = transfer.busy {
                    ProgressView().controlSize(.small)
                    Text(busy).font(.callout).foregroundStyle(.secondary)
                }
                Spacer()
                Button("Cancel", role: .cancel) { transfer.cancelImport() }
                    .keyboardShortcut(.cancelAction)
                // Not the default button: Return mustn't replace everything.
                Button("Replace Everything and Restart", role: .destructive) {
                    Task { await transfer.confirmImport(app: app, keepingCopy: keepingCopy) }
                }
                .buttonStyle(.borderedProminent)
                .tint(.red)
            }
            .disabled(transfer.busy != nil)
        }
        .padding(20)
        .frame(width: 520)
        .interactiveDismissDisabled()
    }

    /// "Exported from YourHarness 0.0.9 (125) on 27 Oct 2026 at 20:15, on a Mac16,5 with macOS 27.0."
    private var origin: String {
        let app = manifest.app
        var text = "Exported from \(app.name) \(app.version ?? "of an unknown version")"
        if let build = app.build { text += " (\(build))" }
        text += " on \(manifest.exportedAt.formatted(date: .abbreviated, time: .shortened))"
        if let model = manifest.system.model { text += ", on a \(model)" }
        return text + " with \(manifest.system.os)."
    }

    private var comparison: some View {
        Grid(alignment: .leading, horizontalSpacing: 18, verticalSpacing: 6) {
            GridRow {
                Text("")
                Text("You have now").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                Text("In the file").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
            }
            Divider().gridCellUnsizedAxes(.horizontal)
            row("Projects", pending.current.projects.count, file.projects.count)
            row("Threads", pending.current.threads, file.threads)
            row("Memories and tasks", pending.current.memories, file.memories)
            row("Images", pending.current.attachments, file.attachments)
            GridRow {
                Text("Size")
                Text(ByteCountFormatter.string(fromByteCount: pending.current.bytes, countStyle: .file)).monospacedDigit()
                Text(ByteCountFormatter.string(fromByteCount: file.bytes, countStyle: .file)).monospacedDigit()
            }
            GridRow {
                Text("Browser sign-ins")
                Text(file.browserCookies == nil ? "Kept" : "Replaced")
                Text(file.browserCookies.map { StateTransfer.count($0, "cookie") } ?? "Not in the file")
            }
            .foregroundStyle(file.browserCookies == nil ? .secondary : .primary)
        }
        .font(.callout)
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .card(radius: Theme.innerRadius)
    }

    private func row(_ label: String, _ now: Int, _ incoming: Int) -> some View {
        GridRow {
            Text(label)
            Text(now.formatted()).monospacedDigit()
            Text(incoming.formatted()).monospacedDigit()
        }
    }

    private func note(_ symbol: String, _ text: String) -> some View {
        Label(text, systemImage: symbol)
            .font(.callout)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
    }
}
