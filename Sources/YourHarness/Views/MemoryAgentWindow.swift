import HarnessCore
import SwiftUI

/// Which specialist, for which project's memory: what a memory agent window is opened with.
struct MemoryAgentKey: Codable, Hashable {
    var projectID: UUID
    var role: MemoryRole
}

/// The Importer or the Reconciler in a window of its own: set it going, talk it through,
/// and watch what it changes as it goes.
struct MemoryAgentWindow: View {
    @Environment(AppModel.self) private var app
    let key: MemoryAgentKey

    var body: some View {
        Group {
            if let project = app.project(id: key.projectID) {
                if let agent = app.existingMemoryAgent(key.role, projectID: key.projectID) {
                    MemoryAgentView(project: project, role: key.role, agent: agent)
                        .id(agent.id)
                } else {
                    ProgressView()
                        .onAppear { _ = app.memoryAgent(key.role, projectID: key.projectID) }
                }
            } else {
                ContentUnavailableView("Project Removed", systemImage: "folder.badge.minus")
            }
        }
        .frame(minWidth: 760, minHeight: 520)
        .canvasBackground()
    }
}

private struct MemoryAgentView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.openWindow) private var openWindow
    let project: Project
    let role: MemoryRole
    let agent: ThreadModel

    private var started: Bool { !agent.entries.isEmpty || agent.phase == .running }

    var body: some View {
        HStack(spacing: 0) {
            Group {
                if started {
                    MemoryAgentConversation(agent: agent)
                } else if role == .importer {
                    ImporterSetup(project: project, agent: agent)
                } else {
                    ReconcilerSetup(project: project)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            Divider()
            SessionChanges(project: project, role: role, agent: agent)
                .frame(width: 270)
        }
        .environment(agent)
        .navigationTitle(role.title)
        .navigationSubtitle(project.name)
        .toolbar {
            ToolbarItemGroup {
                Menu {
                    Picker("Model", selection: Binding(get: { agent.model ?? "" }, set: { agent.setModel($0.isEmpty ? nil : $0) })) {
                        Text("Default (\(app.modelDisplayName(for: app.defaults.model)))").tag("")
                        ForEach(app.claudeModels.filter { $0.value != "default" }, id: \.value) { model in
                            Text(model.displayName).tag(model.value)
                        }
                    }
                    .pickerStyle(.inline)
                } label: {
                    Label(app.modelDisplayName(for: agent.effectiveModel), systemImage: "cpu")
                        .labelStyle(.titleAndIcon)
                }
                .help("The model this specialist uses")
                Button("Open Memory", systemImage: "brain") {
                    app.showMemory(of: project)
                    openWindow(id: "main")
                }
                .help("Show \(project.name)'s memory in the main window")
                Button("New Session", systemImage: "square.and.pencil") {
                    app.closeMemoryAgent(role, projectID: project.id)
                    _ = app.memoryAgent(role, projectID: project.id)
                }
                .help("End this conversation and start over")
                .disabled(!started)
            }
        }
    }
}

// MARK: - Setting off

private struct ImporterSetup: View {
    @Environment(AppModel.self) private var app
    let project: Project
    let agent: ThreadModel

    struct Source: Identifiable {
        let path: String
        let title: String
        let detail: String
        var isOn: Bool
        var id: String { path }
    }

    @State private var sources: [Source] = []
    @State private var pasted = ""
    @State private var note = ""

    private var canStart: Bool { sources.contains(where: \.isOn) || !pasted.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                SetupHeading(symbol: "square.and.arrow.down", title: "Bring memory into \(project.name)",
                             detail: "The Importer reads what you give it (Claude Code's memory, notes, CLAUDE.md files, exports from ChatGPT or other assistants) and turns it into this workspace's memory: one fact a memory, sorted into kinds, without duplicates or secrets. It asks when something's unclear.")
                VStack(alignment: .leading, spacing: 8) {
                    SectionLabel("Sources")
                    VStack(spacing: 0) {
                        if sources.isEmpty {
                            Text("No sources yet. Add files or folders, or paste text below.")
                                .foregroundStyle(.tertiary)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(12)
                        }
                        ForEach($sources) { $source in
                            Toggle(isOn: $source.isOn) {
                                VStack(alignment: .leading, spacing: 1) {
                                    Text(source.title)
                                    Text(source.detail).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                                }
                            }
                            .toggleStyle(.checkbox)
                            .padding(.horizontal, 12)
                            .padding(.vertical, 8)
                            if source.id != sources.last?.id { Divider().padding(.leading, 12) }
                        }
                    }
                    .card()
                    Button("Add Files or Folders…", systemImage: "plus") { addSources() }
                        .buttonStyle(.glass)
                }
                VStack(alignment: .leading, spacing: 8) {
                    SectionLabel("Or paste text")
                    TextEditor(text: $pasted)
                        .font(.callout)
                        .scrollContentBackground(.hidden)
                        .frame(height: 110)
                        .padding(8)
                        .card()
                }
                VStack(alignment: .leading, spacing: 8) {
                    SectionLabel("Anything the Importer should know")
                    TextField("e.g. only what's about the iOS app; I'm the tech lead", text: $note, axis: .vertical)
                        .textFieldStyle(.plain)
                        .padding(10)
                        .card(radius: Theme.innerRadius)
                }
                HStack {
                    Spacer()
                    Button("Start Import", systemImage: "arrow.right") { start() }
                        .buttonStyle(.glassProminent)
                        .tint(.claude)
                        .controlSize(.large)
                        .keyboardShortcut(.defaultAction)
                        .disabled(!canStart)
                }
            }
            .frame(maxWidth: 620, alignment: .leading)
            .padding(28)
            .frame(maxWidth: .infinity)
        }
        .onAppear(perform: findSources)
    }

    private func findSources() {
        guard sources.isEmpty else { return }
        var found: [Source] = []
        let claude = app.claudeCodeMemoryFiles(for: project)
        if !claude.isEmpty {
            found.append(Source(path: app.claudeCodeMemoryFolder(for: project).path, title: "Claude Code's memory for this folder",
                                detail: "\(claude.count) memor\(claude.count == 1 ? "y" : "ies")", isOn: true))
        }
        for url in app.instructionFiles(in: project) {
            found.append(Source(path: url.path, title: url.lastPathComponent,
                                detail: "Agents already read this every session; import only what belongs in memory", isOn: false))
        }
        for path in agent.pendingImportSources where !found.contains(where: { $0.path == path }) {
            found.append(Source(path: path, title: (path as NSString).lastPathComponent, detail: path, isOn: true))
        }
        agent.pendingImportSources = []
        sources = found
    }

    private func addSources() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = true
        panel.prompt = "Add"
        panel.message = "Choose what to import: memory folders, notes, exports, instruction files."
        guard panel.runModal() == .OK else { return }
        for url in panel.urls where !sources.contains(where: { $0.path == url.path }) {
            sources.append(Source(path: url.path, title: url.lastPathComponent, detail: url.path, isOn: true))
        }
    }

    private func start() {
        var paths = sources.filter(\.isOn).map(\.path)
        let text = pasted.trimmingCharacters(in: .whitespacesAndNewlines)
        if !text.isEmpty, let saved = app.savePastedMemorySource(text) { paths.append(saved) }
        guard !paths.isEmpty else { return }
        app.startMemoryImport(projectID: project.id, sources: paths, note: note.trimmingCharacters(in: .whitespacesAndNewlines))
    }
}

private struct ReconcilerSetup: View {
    @Environment(AppModel.self) private var app
    let project: Project
    @State private var note = ""

    var body: some View {
        let stats = app.memoryLibrary(for: project).stats
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                SetupHeading(symbol: "wand.and.sparkles", title: "Tidy up \(project.name)'s memory",
                             detail: "The Reconciler goes through the memory with you: it merges duplicates, fixes or archives what's out of date (checking the code when a memory names it), tightens long or vague memories and links related ones. It shows you the plan before it changes anything, and every change keeps the old version.")
                if let stats {
                    VStack(alignment: .leading, spacing: 6) {
                        SectionLabel("What it starts from")
                        VStack(alignment: .leading, spacing: 6) {
                            Label("\(stats.active) memories in use, \(ByteCountFormatter.string(fromByteCount: Int64(stats.bytes), countStyle: .file))", systemImage: "brain")
                            if !stats.neverRecalled.isEmpty { Label("\(stats.neverRecalled.count) never read by an agent", systemImage: "moon.zzz") }
                            if !stats.likelyDuplicates.isEmpty { Label("\(stats.likelyDuplicates.count) groups that look like duplicates", systemImage: "square.on.square") }
                            if !stats.missedQueries.isEmpty { Label("\(stats.missedQueries.count) searches that found nothing", systemImage: "magnifyingglass") }
                            if stats.orphans > 0 { Label("\(stats.orphans) memories without links", systemImage: "link") }
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(12)
                        .card()
                    }
                }
                VStack(alignment: .leading, spacing: 8) {
                    SectionLabel("Anything to focus on")
                    TextField("e.g. merge the deploy notes; drop what's about the old API", text: $note, axis: .vertical)
                        .textFieldStyle(.plain)
                        .padding(10)
                        .card(radius: Theme.innerRadius)
                }
                HStack {
                    Spacer()
                    Button("Start Review", systemImage: "arrow.right") {
                        app.startMemoryReview(projectID: project.id, note: note.trimmingCharacters(in: .whitespacesAndNewlines))
                    }
                    .buttonStyle(.glassProminent)
                    .tint(.claude)
                    .controlSize(.large)
                    .keyboardShortcut(.defaultAction)
                }
            }
            .frame(maxWidth: 620, alignment: .leading)
            .padding(28)
            .frame(maxWidth: .infinity)
        }
        .task(id: app.memoryRevision(for: project.id)) {
            app.memoryLibrary(for: project).refresh(revision: app.memoryRevision(for: project.id))
        }
    }
}

private struct SetupHeading: View {
    let symbol: String
    let title: String
    let detail: String

    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            Image(systemName: symbol)
                .font(.system(size: 22, weight: .medium))
                .foregroundStyle(Color.claude)
                .frame(width: 44, height: 44)
                .card()
            VStack(alignment: .leading, spacing: 4) {
                Text(title).font(.title2.weight(.semibold))
                Text(detail).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

// MARK: - Conversation

private struct MemoryAgentConversation: View {
    let agent: ThreadModel

    private var showsTypingIndicator: Bool {
        agent.phase == .running && agent.pendingPermissions.isEmpty && !(agent.entries.last?.isStreaming ?? false)
    }

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 14) {
                    ForEach(agent.entries) { entry in
                        EntryView(entry: entry)
                    }
                    if showsTypingIndicator {
                        TypingIndicator(text: agent.activity ?? "Working…")
                    }
                }
                .frame(maxWidth: 760, alignment: .leading)
                .padding(20)
                .frame(maxWidth: .infinity)
            }
            .defaultScrollAnchor(.bottom)
            .defaultScrollAnchor(.bottom, for: .sizeChanges)

            VStack(spacing: 8) {
                ForEach(agent.pendingPermissions) { request in
                    RequestCard(request: request, thread: agent)
                }
                SideChatComposer(placeholder: "Reply, or tell it what to do next…", isRunning: agent.phase == .running, stop: agent.stop) { text in
                    agent.send(text)
                }
            }
            .frame(maxWidth: 760)
            .padding(14)
        }
        .animation(.snappy, value: agent.pendingPermissions)
    }
}

/// What this session has done to the memory so far, as it happens.
private struct SessionChanges: View {
    @Environment(AppModel.self) private var app
    @Environment(\.openWindow) private var openWindow
    let project: Project
    let role: MemoryRole
    let agent: ThreadModel

    var body: some View {
        let library = app.memoryLibrary(for: project)
        let events = library.events.filter {
            $0.threadID == agent.id.uuidString && ![.recalled, .searched, .injected].contains($0.kind)
        }
        let reads = library.events.count { $0.threadID == agent.id.uuidString && $0.kind == .recalled }
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Changes").font(.headline)
                Text(summary(events, reads: reads)).font(.caption).foregroundStyle(.secondary)
            }
            .padding(14)
            Divider()
            if events.isEmpty {
                Text(role == .importer ? "Memories it adds and updates show here as it goes." : "What it merges, fixes and archives shows here as it goes. Every change can be undone from the memory's history.")
                    .font(.callout)
                    .foregroundStyle(.tertiary)
                    .padding(14)
                Spacer()
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 2) {
                        ForEach(events) { event in
                            Button {
                                guard let id = event.memoryID else { return }
                                app.showMemory(of: project)
                                library.tab = .memories
                                library.showsArchived = library.byID[id]?.status != .active
                                library.selection = id
                                openWindow(id: "main")
                            } label: {
                                HStack(alignment: .top, spacing: 8) {
                                    Image(systemName: symbol(event.kind)).foregroundStyle(.secondary).frame(width: 16)
                                    VStack(alignment: .leading, spacing: 1) {
                                        Text(library.byID[event.memoryID ?? ""]?.title ?? event.title ?? "Memory").lineLimit(2)
                                        Text(event.kind.rawValue.capitalized).font(.caption).foregroundStyle(.secondary)
                                    }
                                    Spacer(minLength: 0)
                                }
                                .padding(.horizontal, 10)
                                .padding(.vertical, 6)
                                .contentShape(.rect)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .padding(6)
                }
            }
        }
        .task(id: app.memoryRevision(for: project.id)) {
            library.refresh(revision: app.memoryRevision(for: project.id))
        }
    }

    private func summary(_ events: [HarnessDatabase.MemoryEvent], reads: Int) -> String {
        let added = events.count { $0.kind == .created }
        let updated = events.count { $0.kind == .updated || $0.kind == .merged }
        let archived = events.count { $0.kind == .archived }
        var parts: [String] = []
        if added > 0 { parts.append("\(added) added") }
        if updated > 0 { parts.append("\(updated) updated") }
        if archived > 0 { parts.append("\(archived) archived") }
        if reads > 0 { parts.append("\(reads) read") }
        return parts.isEmpty ? "Nothing yet" : parts.joined(separator: " · ")
    }

    private func symbol(_ kind: HarnessDatabase.MemoryEvent.Kind) -> String {
        switch kind {
        case .created: "plus.circle"
        case .updated: "pencil.circle"
        case .merged: "arrow.triangle.merge"
        case .archived: "archivebox"
        case .linked, .unlinked: "link"
        case .restored: "arrow.uturn.backward.circle"
        case .pinned, .unpinned: "pin"
        default: "circle"
        }
    }
}
