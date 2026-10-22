import HarnessCore
import SwiftUI

/// The top of the limits box while there's something to do about an update: a new version
/// to get, its download, then Restart. Hidden the rest of the time.
struct UpdateBanner: View {
    @Environment(AppModel.self) private var app
    @State private var showsNotes = false

    private var updater: Updater { app.updater }

    var body: some View {
        Group {
            switch updater.phase {
            case .available(let release):
                HStack(spacing: 8) {
                    icon("arrow.down.circle.fill", .accentColor)
                    VStack(alignment: .leading, spacing: 1) {
                        Text("Version \(release.version.description)")
                            .font(.callout.weight(.semibold))
                        Button("What's New") { showsNotes = true }
                            .buttonStyle(.link)
                            .font(.caption)
                            .popover(isPresented: $showsNotes, arrowEdge: .trailing) {
                                ReleaseNotesView(after: updater.current, through: release.version, known: release.releaseNotes)
                            }
                    }
                    Spacer(minLength: 4)
                    Button("Update") { updater.downloadUpdate() }
                        .buttonStyle(.glassProminent)
                        .controlSize(.small)
                        .help("Download \(release.version.description). Nothing changes until you restart.")
                }
            case .downloading(let release, let progress):
                VStack(alignment: .leading, spacing: 6) {
                    HStack(spacing: 6) {
                        Text("Downloading \(release.version.description)")
                            .font(.callout.weight(.medium))
                        Spacer(minLength: 4)
                        Text(progress, format: .percent.precision(.fractionLength(0)))
                            .font(.caption.monospacedDigit())
                            .foregroundStyle(.secondary)
                            .contentTransition(.numericText())
                        Button("Cancel", systemImage: "xmark.circle.fill") { updater.cancelDownload() }
                            .labelStyle(.iconOnly)
                            .buttonStyle(.borderless)
                            .foregroundStyle(.secondary)
                            .help("Stop downloading")
                    }
                    ProgressView(value: progress)
                        .progressViewStyle(.linear)
                        .controlSize(.small)
                        .animation(.linear(duration: 0.1), value: progress)
                }
            case .preparing(let release):
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Preparing \(release.version.description)…")
                        .font(.callout.weight(.medium))
                    Spacer(minLength: 0)
                }
            case .ready(let version):
                HStack(spacing: 8) {
                    icon("checkmark.circle.fill", .green)
                    VStack(alignment: .leading, spacing: 1) {
                        Text("\(version.description) is ready")
                            .font(.callout.weight(.semibold))
                        Text("Restart to use it")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 4)
                    RestartToUpdateButton(version: version)
                        .buttonStyle(.glassProminent)
                        .controlSize(.small)
                }
            case .failed(let message, let release):
                HStack(alignment: .top, spacing: 8) {
                    icon("exclamationmark.triangle.fill", .orange)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(release == nil ? "Couldn't check for updates" : "Couldn't update")
                            .font(.callout.weight(.semibold))
                        Text(message)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(3)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Spacer(minLength: 4)
                    if release != nil {
                        Button("Retry") { updater.downloadUpdate() }
                            .controlSize(.small)
                    } else {
                        Button("Dismiss", systemImage: "xmark.circle.fill") { updater.dismissFailure() }
                            .labelStyle(.iconOnly)
                            .buttonStyle(.borderless)
                            .foregroundStyle(.secondary)
                    }
                }
            case .upToDate where updater.announcesUpToDate:
                HStack(spacing: 8) {
                    icon("checkmark.seal.fill", .green)
                    Text("\(updater.versionLabel) is the latest")
                        .font(.callout.weight(.medium))
                    Spacer(minLength: 0)
                }
            case .checking where updater.announcesUpToDate:
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Checking for updates…")
                        .font(.callout.weight(.medium))
                    Spacer(minLength: 0)
                }
            default:
                if let from = updater.updatedFrom, let current = updater.current {
                    HStack(spacing: 8) {
                        icon("sparkles", .accentColor)
                        VStack(alignment: .leading, spacing: 1) {
                            Text("Updated to \(current.description)")
                                .font(.callout.weight(.semibold))
                            Button("What's New") { showsNotes = true }
                                .buttonStyle(.link)
                                .font(.caption)
                                .popover(isPresented: $showsNotes, arrowEdge: .trailing) {
                                    ReleaseNotesView(after: from, through: current)
                                }
                        }
                        Spacer(minLength: 4)
                        Button("Dismiss", systemImage: "xmark.circle.fill") { updater.dismissUpdated() }
                            .labelStyle(.iconOnly)
                            .buttonStyle(.borderless)
                            .foregroundStyle(.secondary)
                            .help("Hide this")
                    }
                }
            }
        }
        .transition(.asymmetric(insertion: .push(from: .bottom), removal: .opacity).combined(with: .opacity))
        .onChange(of: showsNotes) { _, open in
            if !open { updater.dismissUpdated() }
        }
    }

    private func icon(_ name: String, _ color: Color) -> some View {
        Image(systemName: name)
            .font(.title3)
            .foregroundStyle(color)
            .symbolRenderingMode(.hierarchical)
    }
}

extension Updater.Phase {
    /// Which banner shows, for animating between them (not on every progress tick).
    var bannerKind: Int {
        switch self {
        case .idle: 0
        case .checking: 1
        case .upToDate: 2
        case .available: 3
        case .downloading: 4
        case .preparing: 5
        case .ready: 6
        case .failed: 7
        }
    }
}

/// The version, small, in the limits box's bottom row; click for where it stands.
struct VersionButton: View {
    @Environment(AppModel.self) private var app
    @State private var showing = false

    var body: some View {
        Button {
            showing.toggle()
        } label: {
            Text(app.updater.versionLabel)
                .font(.caption2.monospacedDigit())
                .foregroundStyle(.tertiary)
                .lineLimit(1)
        }
        .buttonStyle(.borderless)
        .help("YourHarness \(app.updater.versionLabel)")
        .popover(isPresented: $showing, arrowEdge: .top) {
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 10) {
                    Image(nsImage: NSApp.applicationIconImage)
                        .resizable()
                        .frame(width: 40, height: 40)
                    VStack(alignment: .leading, spacing: 2) {
                        Text("YourHarness \(app.updater.versionLabel)")
                            .font(.headline)
                        if let build = app.updater.build {
                            Text("Build \(build)")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                    Spacer(minLength: 0)
                    CurrentReleaseNotesButton()
                }
                UpdateStatus()
                    .font(.callout)
            }
            .padding(14)
            .frame(width: 300, alignment: .leading)
        }
    }
}

/// The notes of the version you're running.
struct CurrentReleaseNotesButton: View {
    @Environment(AppModel.self) private var app
    @State private var showing = false

    var body: some View {
        if let current = app.updater.current {
            Button("Release Notes") { showing = true }
                .help("What changed in \(current.description)")
                .popover(isPresented: $showing, arrowEdge: .bottom) {
                    ReleaseNotesView(after: nil, through: current)
                }
        }
    }
}

/// Where updates stand and what you can do next, in a line: for Settings and the version popover.
struct UpdateStatus: View {
    @Environment(AppModel.self) private var app
    @State private var showsNotes = false

    private var updater: Updater { app.updater }

    var body: some View {
        HStack(spacing: 8) {
            if !updater.isEnabled {
                Text(updater.whyDisabled)
                    .foregroundStyle(.secondary)
            } else {
                switch updater.phase {
                case .idle, .upToDate:
                    VStack(alignment: .leading, spacing: 1) {
                        Text(updater.phase == .upToDate ? "Up to date" : "Not checked yet")
                        if let checked = updater.lastChecked {
                            TimelineView(.everyMinute) { _ in
                                Text("Checked \(checked.formatted(.relative(presentation: .named)))")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }
                    Spacer(minLength: 8)
                    Button("Check Now") { updater.checkNow() }
                case .checking:
                    ProgressView().controlSize(.small)
                    Text("Checking…")
                    Spacer(minLength: 0)
                case .available(let release):
                    Text("\(release.version.description) is available")
                    Spacer(minLength: 8)
                    Button("What's New") { showsNotes = true }
                        .popover(isPresented: $showsNotes, arrowEdge: .bottom) {
                            ReleaseNotesView(after: updater.current, through: release.version, known: release.releaseNotes)
                        }
                    Button("Update") { updater.downloadUpdate() }
                        .buttonStyle(.glassProminent)
                case .downloading(let release, let progress):
                    Text("Downloading \(release.version.description)")
                    ProgressView(value: progress)
                        .frame(maxWidth: 140)
                    Button("Cancel") { updater.cancelDownload() }
                case .preparing(let release):
                    ProgressView().controlSize(.small)
                    Text("Preparing \(release.version.description)…")
                    Spacer(minLength: 0)
                case .ready(let version):
                    Text("\(version.description) is ready")
                    Spacer(minLength: 8)
                    RestartToUpdateButton(version: version)
                        .buttonStyle(.glassProminent)
                case .failed(let message, let release):
                    Text(message)
                        .foregroundStyle(.orange)
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: 8)
                    if release != nil {
                        Button("Retry") { updater.downloadUpdate() }
                    } else {
                        Button("Check Now") { updater.checkNow() }
                    }
                }
            }
        }
    }
}

/// Restart, after saying what it does to threads that are working.
struct RestartToUpdateButton: View {
    @Environment(AppModel.self) private var app
    let version: SemanticVersion
    @State private var confirming = false

    private var working: Int { app.threads.count { $0.phase == .running } }

    var body: some View {
        Button("Restart") { confirming = true }
            .help("Quit and open \(version.description)")
            .alert("Restart to update to \(version.description)?", isPresented: $confirming) {
                Button("Restart") { app.updater.restart() }
                    .keyboardShortcut(.defaultAction)
                Button("Later", role: .cancel) {}
            } message: {
                Text(message)
            }
    }

    private var message: String {
        let reopen = "YourHarness quits and opens again on \(version.description); your threads and drafts carry over."
        switch working {
        case 0: return reopen
        case 1: return "One thread is working: restarting stops it, and you can send again after. " + reopen
        default: return "\(working) threads are working: restarting stops them, and you can send again after. " + reopen
        }
    }
}

/// Release notes from GitHub, rendered here: every release after `after` up to `through`, newest
/// first, each with a link to its page. Without `after`, just `through`.
struct ReleaseNotesView: View {
    @Environment(AppModel.self) private var app
    let after: SemanticVersion?
    let through: SemanticVersion
    /// Shown at once while the rest load (the release an update offers is already known).
    var known: ReleaseNotes?
    @State private var notes: [ReleaseNotes] = []
    @State private var loading = true
    @State private var error: String?

    private var title: String {
        if let after, after < through, notes.count > 1 { return "What's New Since \(after.description)" }
        return "What's New in \(through.description)"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline) {
                Text(title)
                    .font(.title3.weight(.semibold))
                Spacer()
                if loading, !notes.isEmpty {
                    ProgressView().controlSize(.mini)
                }
            }
            .padding([.horizontal, .top], 16)
            .padding(.bottom, 10)
            Divider()
            ScrollView {
                VStack(alignment: .leading, spacing: 22) {
                    if notes.isEmpty {
                        if loading {
                            ProgressView("Loading the notes from GitHub…")
                                .frame(maxWidth: .infinity, minHeight: 120)
                        } else {
                            VStack(alignment: .leading, spacing: 6) {
                                Text(error.map { "Couldn't load the notes: \($0)" } ?? "There are no notes for \(through.description).")
                                    .foregroundStyle(.secondary)
                                Link("Read them on GitHub", destination: Updater.releasesPage)
                            }
                        }
                    }
                    ForEach(notes) { release in
                        ReleaseNotesSection(notes: release, showsVersion: notes.count > 1)
                    }
                }
                .padding(16)
            }
            // Sized up front: a popover doesn't grow when the notes arrive after it opens.
            .frame(minHeight: 380, maxHeight: 460)
            .animation(.smooth(duration: 0.3), value: notes.map(\.id))
            Divider()
            HStack {
                Link("All Releases on GitHub", destination: Updater.releasesPage)
                Spacer()
                if case .available(let release) = app.updater.phase, release.version == through {
                    Button("Update") { app.updater.downloadUpdate() }
                        .buttonStyle(.glassProminent)
                }
            }
            .padding(12)
        }
        .frame(width: 460)
        .task {
            if let known, notes.isEmpty { notes = [known] }
            do {
                let loaded = try await app.updater.releaseNotes(after: after, through: through)
                if !loaded.isEmpty { notes = loaded }
            } catch {
                self.error = Updater.describe(error)
            }
            loading = false
        }
    }
}

private struct ReleaseNotesSection: View {
    let notes: ReleaseNotes
    let showsVersion: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                if showsVersion {
                    Text(notes.version.description)
                        .font(.headline)
                }
                if let date = notes.publishedAt {
                    Text(date.formatted(date: .abbreviated, time: .omitted))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                if let page = notes.page {
                    Link(destination: page) {
                        Label("View on GitHub", systemImage: "arrow.up.forward.square")
                    }
                    .font(.caption)
                    .help(page.absoluteString)
                }
            }
            MarkdownView(text: notes.text.isEmpty ? "No notes for this release." : notes.text)
                .textSelection(.enabled)
            if showsVersion { Divider().padding(.top, 4) }
        }
    }
}
