import HarnessCore
import SwiftUI

/// A project's memory: everything its agents remember, what they do with it, and the tools
/// to tend it (import, export, the reconciler).
struct MemoryView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.openWindow) private var openWindow
    let project: Project
    @State private var library: MemoryLibrary

    init(project: Project, library: MemoryLibrary) {
        self.project = project
        _library = State(initialValue: library)
    }

    var body: some View {
        VStack(spacing: 0) {
            MemoryHeader(project: project, library: library)
                .padding(.horizontal, 28)
                .padding(.top, 20)
                .padding(.bottom, 14)
            ClaudeMemoryNudge(project: project)
                .padding(.horizontal, 28)
                .padding(.bottom, 14)
            Divider()
            Group {
                if !library.isLoaded {
                    ProgressView()
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else if library.memories.isEmpty {
                    MemoryEmptyState(project: project, library: library)
                } else {
                    switch library.tab {
                    case .memories: MemoriesTab(project: project, library: library)
                    case .tasks: MemoryTasksView(library: library)
                    case .insights: MemoryInsightsView(project: project, library: library)
                    case .activity: MemoryActivityView(library: library)
                    case .graph: MemoryGraphView(library: library)
                    }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            // A tab change shows the other tab at once.
            .transaction(value: library.tab) { $0.animation = nil }
        }
        .task(id: app.memoryRevision(for: project.id)) {
            library.refresh(revision: app.memoryRevision(for: project.id))
        }
        .toolbar {
            ToolbarItemGroup {
                MemoryImportMenu(project: project)
                Menu("Export", systemImage: "square.and.arrow.up") {
                    Button("Export as JSON…") { app.exportMemory(of: project, as: .json) }
                    Button("Export as Markdown Folder…") { app.exportMemory(of: project, as: .markdown) }
                }
                .help("Export this workspace's memory")
                .disabled(library.memories.isEmpty)
                Button("Reconcile", systemImage: "wand.and.sparkles") {
                    openWindow(id: "memory-agent", value: MemoryAgentKey(projectID: project.id, role: .reconciler))
                }
                .help("Open the Memory Reconciler: an agent that finds duplicates, stale and vague memories, and fixes them with you")
                .disabled(library.memories.isEmpty)
                Button("New Memory", systemImage: "plus") {
                    library.tab = .memories
                    library.newMemory()
                }
                .help("Write a memory yourself")
            }
        }
        .sheet(item: Bindable(app).importReport) { report in
            MemoryImportReportSheet(report: report, project: project)
        }
    }
}

/// The Import toolbar menu: the importer, a file, or Claude Code's own memory.
struct MemoryImportMenu: View {
    @Environment(AppModel.self) private var app
    @Environment(\.openWindow) private var openWindow
    let project: Project

    var body: some View {
        Menu("Import", systemImage: "square.and.arrow.down") {
            Button("Import with the Importer…") { openImporter() }
            Button("Import File…") {
                app.presentImportMemoryFiles(into: project) { paths in openImporter(paths) }
            }
            let claudeFiles = app.claudeCodeMemoryFiles(for: project)
            if !claudeFiles.isEmpty {
                Divider()
                Button("Import Claude Code's Memory (\(claudeFiles.count))") {
                    app.importMemoryFiles([app.claudeCodeMemoryFolder(for: project)], into: project) { paths in openImporter(paths) }
                }
            }
        }
        .help("Bring memory in: from Claude Code, an export, or anything else with the Importer")
    }

    private func openImporter(_ paths: [String] = []) {
        if !paths.isEmpty { app.memoryAgent(.importer, projectID: project.id).pendingImportSources = paths }
        openWindow(id: "memory-agent", value: MemoryAgentKey(projectID: project.id, role: .importer))
    }
}

// MARK: - Header

private struct MemoryHeader: View {
    @Environment(AppModel.self) private var app
    let project: Project
    let library: MemoryLibrary

    private var summary: String {
        guard let stats = library.stats else { return "Loading…" }
        guard stats.active + stats.archived > 0 else { return "Nothing remembered yet" }
        var parts = [stats.active == 1 ? "1 memory" : "\(stats.active) memories",
                     ByteCountFormatter.string(fromByteCount: Int64(stats.bytes), countStyle: .file)]
        if stats.sessions > 0 {
            parts.append("~\(MemoryStats.tokens(stats.averageInjectedBytes).formatted()) tokens a session")
        }
        if stats.recallsInWindow > 0 { parts.append("\(stats.recallsInWindow) reads this month") }
        if stats.archived > 0 { parts.append("\(stats.archived) archived") }
        return parts.joined(separator: " · ")
    }

    var body: some View {
        HStack(spacing: 14) {
            Image(systemName: "brain")
                .font(.system(size: 22, weight: .medium))
                .foregroundStyle(Color.claude)
                .frame(width: 44, height: 44)
                .card()
            VStack(alignment: .leading, spacing: 3) {
                Text("\(project.name) Memory")
                    .font(.title2.weight(.semibold))
                Text(summary)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .contentTransition(.opacity)
            }
            Spacer()
            OneMemoryChip(project: project)
            if !library.memories.isEmpty {
                Picker("View", selection: Bindable(library).tab) {
                    ForEach(MemoryLibrary.Tab.allCases) { tab in
                        Text(tab.label).tag(tab)
                    }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
            }
            if !app.memoryEnabled {
                Label("Off for agents", systemImage: "pause.circle")
                    .font(.callout)
                    .foregroundStyle(.orange)
                    .help("Agents don't get memory. Turn it on in Settings → General → Memory.")
            }
        }
    }
}

// MARK: - Empty

private struct MemoryEmptyState: View {
    @Environment(AppModel.self) private var app
    @Environment(\.openWindow) private var openWindow
    let project: Project
    let library: MemoryLibrary

    var body: some View {
        let claudeFiles = app.claudeCodeMemoryFiles(for: project)
        ContentUnavailableView {
            Label("Nothing Remembered Yet", systemImage: "brain")
        } description: {
            Text("Agents working in \(project.name) save what's worth knowing next time: who you are, how you like to work, decisions and where things live. Every thread here starts with it. You can also bring memory in, or write it yourself.")
        } actions: {
            VStack(spacing: 10) {
                if !claudeFiles.isEmpty {
                    Button("Import Claude Code's Memory (\(claudeFiles.count))") {
                        app.importMemoryFiles([app.claudeCodeMemoryFolder(for: project)], into: project)
                    }
                    .buttonStyle(.glassProminent)
                    .controlSize(.large)
                }
                HStack {
                    Button("Open the Importer…") {
                        openWindow(id: "memory-agent", value: MemoryAgentKey(projectID: project.id, role: .importer))
                    }
                    Button("New Memory") { library.newMemory() }
                }
                .buttonStyle(.glass)
            }
        }
    }
}

// MARK: - Memories

private struct MemoriesTab: View {
    let project: Project
    @Bindable var library: MemoryLibrary

    var body: some View {
        HStack(spacing: 0) {
            MemoryList(library: library)
                .frame(width: 330)
            Divider()
            Group {
                if let id = library.selection, let memory = library.byID[id] {
                    MemoryDetail(memory: memory, library: library)
                        .id(id)
                } else {
                    ContentUnavailableView("No Memory Selected", systemImage: "brain",
                                           description: Text("Choose a memory to see what it says, how agents use it and how it changed."))
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

private struct MemoryList: View {
    @Bindable var library: MemoryLibrary

    var body: some View {
        let visible = library.visibleMemories
        VStack(spacing: 0) {
            VStack(spacing: 8) {
                HStack(spacing: 6) {
                    Image(systemName: "magnifyingglass").foregroundStyle(.tertiary)
                    TextField("Search memories", text: $library.searchText)
                        .textFieldStyle(.plain)
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 6)
                .card(radius: Theme.innerRadius)
                ScrollView(.horizontal) {
                    HStack(spacing: 4) {
                        FilterChip(title: "All", isOn: library.kindFilter == nil) { library.kindFilter = nil }
                        ForEach(MemoryKind.allCases, id: \.self) { kind in
                            FilterChip(title: kind.label, tint: kind.color, isOn: library.kindFilter == kind) {
                                library.kindFilter = library.kindFilter == kind ? nil : kind
                            }
                        }
                    }
                }
                .scrollIndicators(.never)
                Picker("Status", selection: $library.showsArchived) {
                    Text("In Use").tag(false)
                    Text("Archived").tag(true)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
            }
            .padding(12)
            Divider()
            if visible.isEmpty {
                ContentUnavailableView.search(text: library.searchText)
                    .frame(maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(spacing: 2) {
                        ForEach(visible) { memory in
                            MemoryRow(memory: memory, isSelected: library.selection == memory.id)
                                .onTapGesture { library.selection = memory.id }
                                .contextMenu { MemoryActions(memory: memory, library: library) }
                        }
                    }
                    .padding(8)
                }
                Text(visible.count == 1 ? "1 memory" : "\(visible.count) memories")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .padding(.vertical, 6)
            }
        }
        .onKeyPress(.upArrow) { step(-1, in: visible) }
        .onKeyPress(.downArrow) { step(1, in: visible) }
        .onAppear {
            if library.selection == nil { library.selection = visible.first?.id }
        }
    }

    private func step(_ offset: Int, in visible: [MemoryLibrary.Memory]) -> KeyPress.Result {
        guard !visible.isEmpty else { return .ignored }
        let index = visible.firstIndex { $0.id == library.selection } ?? -offset
        library.selection = visible[min(max(index + offset, 0), visible.count - 1)].id
        return .handled
    }
}

private struct FilterChip: View {
    let title: String
    var tint: Color = .accentColor
    let isOn: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(.caption.weight(.medium))
                .padding(.horizontal, 8)
                .padding(.vertical, 3)
                .foregroundStyle(isOn ? AnyShapeStyle(tint) : AnyShapeStyle(.secondary))
                .background(isOn ? AnyShapeStyle(tint.opacity(0.16)) : AnyShapeStyle(.fill.quaternary), in: .capsule)
        }
        .buttonStyle(.plain)
    }
}

private struct MemoryRow: View {
    let memory: MemoryLibrary.Memory
    let isSelected: Bool

    var body: some View {
        HStack(alignment: .top, spacing: 9) {
            Image(systemName: MemoryKind.of(memory.kind).symbol)
                .font(.callout)
                .foregroundStyle(MemoryKind.of(memory.kind).color)
                .frame(width: 18)
                .padding(.top, 1)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 4) {
                    Text(memory.title.isEmpty ? "Untitled" : memory.title)
                        .fontWeight(.medium)
                        .lineLimit(1)
                    if memory.pinned {
                        Image(systemName: "pin.fill")
                            .font(.system(size: 9))
                            .foregroundStyle(.secondary)
                    }
                }
                Text(memory.description.isEmpty ? memory.body : memory.description)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                if memory.isTask {
                    HStack(spacing: 6) {
                        Chip(memory.taskState?.label ?? "In Progress", tint: (memory.taskState ?? .active).color)
                        if !memory.steps.isEmpty {
                            Text("\(memory.stepsDone) of \(memory.steps.count) steps").font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
                HStack(spacing: 8) {
                    Label("\(memory.recallCount)", systemImage: "eye")
                        .help(memory.recallCount == 1 ? "Read by an agent once" : "Read by agents \(memory.recallCount) times")
                    if !memory.links.isEmpty {
                        Label("\(memory.links.count)", systemImage: "link")
                    }
                    Spacer(minLength: 0)
                    Text(memory.updatedAt, format: .relative(presentation: .named))
                }
                .labelStyle(.titleAndIcon)
                .font(.caption.monospacedDigit())
                .foregroundStyle(.tertiary)
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background {
            RoundedRectangle(cornerRadius: Theme.innerRadius, style: .continuous)
                .fill(isSelected ? Color.accentColor.opacity(0.16) : .clear)
        }
        .contentShape(.rect)
    }
}

/// What you can do to a memory, in its context menu and its detail's menu.
private struct MemoryActions: View {
    let memory: MemoryLibrary.Memory
    let library: MemoryLibrary

    var body: some View {
        Button(memory.pinned ? "Unpin" : "Pin (Always Load in Full)", systemImage: memory.pinned ? "pin.slash" : "pin") {
            library.setPinned(!memory.pinned, memory.id)
        }
        if memory.status == .active {
            Button("Archive", systemImage: "archivebox") { library.setStatus(.archived, memory.id) }
        } else {
            Button("Restore", systemImage: "arrow.uturn.backward") { library.setStatus(.active, memory.id) }
        }
        Button("Copy ID", systemImage: "number") {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(memory.shortID, forType: .string)
        }
        Divider()
        Button("Delete", systemImage: "trash", role: .destructive) { library.delete(memory.id) }
    }
}

// MARK: - Detail

private struct MemoryDetail: View {
    @Environment(AppModel.self) private var app
    let memory: MemoryLibrary.Memory
    let library: MemoryLibrary
    @State private var draft: MemoryLibrary.Memory
    @State private var editingBody: Bool
    @State private var versions: [HarnessDatabase.MemoryVersion] = []
    @State private var shownVersion: HarnessDatabase.MemoryVersion?

    init(memory: MemoryLibrary.Memory, library: MemoryLibrary) {
        self.memory = memory
        self.library = library
        _draft = State(initialValue: memory)
        _editingBody = State(initialValue: memory.body.isEmpty)
    }

    private var isDirty: Bool {
        draft.title != memory.title || draft.description != memory.description || draft.body != memory.body || draft.kind != memory.kind
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                titleBlock
                if memory.isTask { TaskSection(task: memory, library: library) }
                bodyBlock
                MemoryUsageCard(memory: memory)
                links
                sources
                history
            }
            .frame(maxWidth: 760, alignment: .leading)
            .padding(24)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .safeAreaInset(edge: .bottom) {
            if isDirty {
                HStack {
                    Text("Edited").foregroundStyle(.secondary)
                    Spacer()
                    Button("Revert") { draft = memory; editingBody = false }
                        .buttonStyle(.glass)
                    Button("Save") { save() }
                        .buttonStyle(.glassProminent)
                        .keyboardShortcut("s")
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .glassEffect(.regular, in: .rect(cornerRadius: Theme.floatingRadius))
                .padding(14)
                .transition(.move(edge: .bottom).combined(with: .opacity))
            }
        }
        .animation(.snappy, value: isDirty)
        .onChange(of: memory) { old, new in
            // Changed elsewhere (an agent, a restore) while you weren't editing: show it.
            if draft.title == old.title && draft.description == old.description && draft.body == old.body && draft.kind == old.kind {
                draft = new
            }
            versions = library.versions(of: memory.id)
        }
        .task(id: memory.id) { versions = library.versions(of: memory.id) }
    }

    private func save() {
        var saved = memory
        saved.title = draft.title.trimmingCharacters(in: .whitespacesAndNewlines)
        saved.description = draft.description.trimmingCharacters(in: .whitespacesAndNewlines)
        saved.body = draft.body.trimmingCharacters(in: .whitespacesAndNewlines)
        saved.kind = draft.kind
        library.save(saved)
        draft = saved
        editingBody = false
    }

    private var titleBlock: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Menu {
                    Picker("Kind", selection: $draft.kind) {
                        ForEach(MemoryKind.allCases, id: \.self) { kind in
                            Label(kind.label, systemImage: kind.symbol).tag(kind.rawValue)
                        }
                    }
                    .pickerStyle(.inline)
                } label: {
                    Chip(MemoryKind.of(draft.kind).label, tint: MemoryKind.of(draft.kind).color)
                }
                .menuStyle(.button)
                .buttonStyle(.plain)
                .fixedSize()
                .help(MemoryKind.of(draft.kind).detail)
                if memory.pinned { Chip("Pinned") }
                if memory.status != .active { Chip(memory.status == .archived ? "Archived" : "Proposed", tint: .orange) }
                Text(memory.shortID)
                    .font(.caption.monospaced())
                    .foregroundStyle(.tertiary)
                    .textSelection(.enabled)
                    .help("The ID agents use")
                Spacer()
                Button(memory.pinned ? "Unpin" : "Pin", systemImage: memory.pinned ? "pin.slash" : "pin") {
                    library.setPinned(!memory.pinned, memory.id)
                }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .help(memory.pinned ? "Unpin: agents see its index line only" : "Pin: every session gets it in full, not only its index line")
                Menu("More", systemImage: "ellipsis.circle") {
                    MemoryActions(memory: memory, library: library)
                }
                .labelStyle(.iconOnly)
                .menuStyle(.button)
                .buttonStyle(.borderless)
                .fixedSize()
            }
            TextField("Title", text: $draft.title, axis: .vertical)
                .textFieldStyle(.plain)
                .font(.title2.weight(.semibold))
            TextField("One line on when this matters (what agents see in the index)", text: $draft.description, axis: .vertical)
                .textFieldStyle(.plain)
                .font(.body)
                .foregroundStyle(.secondary)
            if let merged = memory.mergedInto, let target = library.byID[merged] {
                Button("Merged into “\(target.title)”", systemImage: "arrow.triangle.merge") { library.selection = merged }
                    .buttonStyle(.link)
                    .font(.callout)
            }
        }
    }

    private var bodyBlock: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                SectionLabel("Memory")
                Spacer()
                Button(editingBody ? "Preview" : "Edit") { editingBody.toggle() }
                    .buttonStyle(.borderless)
                    .font(.callout)
            }
            Group {
                if editingBody {
                    TextEditor(text: $draft.body)
                        .font(.system(size: MarkdownMetrics.body))
                        .scrollContentBackground(.hidden)
                        .frame(minHeight: 140)
                } else if draft.body.isEmpty {
                    Text("Empty. Click Edit to write it.")
                        .foregroundStyle(.tertiary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                } else {
                    MarkdownView(text: draft.body)
                        .textSelection(.enabled)
                        .onTapGesture(count: 2) { editingBody = true }
                }
            }
            .padding(12)
            .card()
        }
    }

    @ViewBuilder
    private var links: some View {
        let outgoing = memory.links.compactMap { link in library.byID[link.to].map { ($0, link.relation) } }
        let incoming = library.backlinks(to: memory.id).filter { back in !memory.links.contains { $0.to == back.id } }
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                SectionLabel("Linked")
                Spacer()
                Menu("Link To", systemImage: "plus") {
                    ForEach(library.memories.filter { $0.status == .active && $0.id != memory.id && !memory.links.map(\.to).contains($0.id) }
                        .sorted { $0.title < $1.title }) { other in
                        Button(other.title) { library.link(memory.id, to: other.id) }
                    }
                }
                .menuStyle(.button)
                .buttonStyle(.borderless)
                .labelStyle(.titleAndIcon)
                .font(.callout)
                .fixedSize()
            }
            if outgoing.isEmpty && incoming.isEmpty {
                Text("Not linked to other memories. Links let an agent that reads one find the rest.")
                    .font(.callout)
                    .foregroundStyle(.tertiary)
            } else {
                VStack(spacing: 0) {
                    ForEach(outgoing, id: \.0.id) { other, relation in
                        LinkRow(memory: other, relation: relation, direction: "arrow.right") { library.selection = other.id } unlink: {
                            library.unlink(memory.id, other.id)
                        }
                    }
                    ForEach(incoming) { other in
                        LinkRow(memory: other, relation: nil, direction: "arrow.left") { library.selection = other.id } unlink: {
                            library.unlink(other.id, memory.id)
                        }
                    }
                }
                .card()
            }
        }
    }

    @ViewBuilder
    private var sources: some View {
        let threads = memory.sources.compactMap { source in app.threads.first { $0.id.uuidString == source.threadID }.map { ($0, source) } }
        if !threads.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                SectionLabel("Learned In")
                VStack(spacing: 0) {
                    ForEach(threads, id: \.1.entryID) { thread, source in
                        Button {
                            app.selectedThreadID = thread.id
                            thread.reveal(source.entryID)
                        } label: {
                            HStack {
                                Image(systemName: "bubble.left.and.text.bubble.right").foregroundStyle(.secondary)
                                Text(thread.title).lineLimit(1)
                                Spacer()
                                Image(systemName: "arrow.up.forward").foregroundStyle(.tertiary)
                            }
                            .padding(.horizontal, 12)
                            .padding(.vertical, 8)
                            .contentShape(.rect)
                        }
                        .buttonStyle(.plain)
                    }
                }
                .card()
            }
        }
    }

    @ViewBuilder
    private var history: some View {
        if versions.count > 1 {
            VStack(alignment: .leading, spacing: 6) {
                SectionLabel("History")
                VStack(spacing: 0) {
                    ForEach(versions.reversed()) { version in
                        VersionRow(version: version, isCurrent: version.version == versions.last?.version,
                                   isShown: shownVersion?.version == version.version) {
                            shownVersion = shownVersion?.version == version.version ? nil : version
                        } restore: {
                            library.restore(memory.id, version: version.version)
                            shownVersion = nil
                        }
                        if version.version != versions.first?.version { Divider().padding(.leading, 12) }
                    }
                }
                .card()
            }
        }
    }
}

struct SectionLabel: View {
    let text: String

    init(_ text: String) {
        self.text = text
    }

    var body: some View {
        Text(text)
            .font(.subheadline.weight(.semibold))
            .foregroundStyle(.secondary)
    }
}

private struct LinkRow: View {
    let memory: MemoryLibrary.Memory
    let relation: String?
    let direction: String
    let open: () -> Void
    let unlink: () -> Void
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: direction)
                .font(.caption)
                .foregroundStyle(.tertiary)
            Image(systemName: MemoryKind.of(memory.kind).symbol)
                .foregroundStyle(MemoryKind.of(memory.kind).color)
            Text(memory.title).lineLimit(1)
            if let relation { Chip(relation) }
            Spacer()
            if hovering {
                Button("Unlink", systemImage: "link.badge.minus", action: unlink)
                    .labelStyle(.iconOnly)
                    .buttonStyle(.borderless)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .contentShape(.rect)
        .onTapGesture(perform: open)
        .onHover { hovering = $0 }
    }
}

private struct VersionRow: View {
    let version: HarnessDatabase.MemoryVersion
    let isCurrent: Bool
    let isShown: Bool
    let toggle: () -> Void
    let restore: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Version \(version.version)").fontWeight(.medium)
                Text("by \(MemoryActorName.of(version.actor))").foregroundStyle(.secondary)
                if version.status != .active { Chip(version.status.rawValue.capitalized) }
                Spacer()
                Text(version.savedAt, format: .dateTime.month().day().hour().minute())
                    .font(.callout.monospacedDigit())
                    .foregroundStyle(.secondary)
                if isCurrent {
                    Chip("Current")
                } else {
                    Image(systemName: "chevron.right")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.tertiary)
                        .rotationEffect(.degrees(isShown ? 90 : 0))
                }
            }
            .contentShape(.rect)
            .onTapGesture { if !isCurrent { withAnimation(.snappy) { toggle() } } }
            if isShown {
                VStack(alignment: .leading, spacing: 4) {
                    Text(version.title).fontWeight(.semibold)
                    if !version.description.isEmpty { Text(version.description).foregroundStyle(.secondary) }
                    Text(version.body).font(.callout).textSelection(.enabled)
                }
                Button("Restore This Version", systemImage: "arrow.uturn.backward", action: restore)
                    .buttonStyle(.glass)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
    }
}

/// How agents have used a memory, in a row of small figures.
private struct MemoryUsageCard: View {
    let memory: MemoryLibrary.Memory

    var body: some View {
        HStack(spacing: 0) {
            figure("Read", memory.recallCount == 0 ? "Never" : "\(memory.recallCount)×",
                   memory.lastRecalledAt.map { "last \($0.formatted(.relative(presentation: .named)))" } ?? "by an agent")
            Divider().frame(height: 34)
            figure("In searches", "\(memory.surfacedCount)×", "came up")
            Divider().frame(height: 34)
            figure("Size", "\(memory.size.formatted()) B", "~\(MemoryStats.tokens(memory.size)) tokens")
            Divider().frame(height: 34)
            figure("Saved by", MemoryActorName.origin(memory.origin), memory.createdAt.formatted(.dateTime.month().day().year()))
        }
        .padding(.vertical, 10)
        .card()
    }

    private func figure(_ label: String, _ value: String, _ detail: String) -> some View {
        VStack(spacing: 2) {
            Text(label).font(.caption).foregroundStyle(.secondary)
            Text(value).font(.title3.weight(.semibold).monospacedDigit())
            Text(detail).font(.caption2).foregroundStyle(.tertiary).lineLimit(1)
        }
        .frame(maxWidth: .infinity)
    }
}

// MARK: - Import report

private struct MemoryImportReportSheet: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openWindow) private var openWindow
    let report: MemoryImportReport
    let project: Project

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Label("Imported into \(report.projectName)", systemImage: "square.and.arrow.down")
                .font(.title3.weight(.semibold))
            Text(summary)
            if !report.unstructured.isEmpty {
                Text("These aren't memory files, so they weren't read: \(report.unstructured.map { ($0 as NSString).lastPathComponent }.joined(separator: ", ")). The Importer can turn them into memories.")
                    .foregroundStyle(.secondary)
            }
            if !report.problems.isEmpty {
                Text(report.problems.joined(separator: "\n")).foregroundStyle(.red)
            }
            HStack {
                Spacer()
                if !report.unstructured.isEmpty {
                    Button("Open the Importer") {
                        app.memoryAgent(.importer, projectID: project.id).pendingImportSources = report.unstructured
                        openWindow(id: "memory-agent", value: MemoryAgentKey(projectID: project.id, role: .importer))
                        dismiss()
                    }
                }
                Button("Done") { dismiss() }
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(22)
        .frame(width: 440)
    }

    private var summary: String {
        let r = report.report
        var parts: [String] = []
        if r.added > 0 { parts.append(r.added == 1 ? "1 memory added" : "\(r.added) memories added") }
        if r.updated > 0 { parts.append("\(r.updated) updated") }
        if r.unchanged > 0 { parts.append("\(r.unchanged) already here") }
        return parts.isEmpty ? "Nothing to import." : parts.joined(separator: ", ") + "."
    }
}

// MARK: - Names and colors

extension MemoryKind {
    var color: Color {
        switch self {
        case .user: .blue
        case .feedback: .orange
        case .project: .purple
        case .reference: .teal
        case .task: .pink
        }
    }

    var symbol: String {
        switch self {
        case .user: "person.crop.circle"
        case .feedback: "bubble.left.and.exclamationmark.bubble.right"
        case .project: "hammer"
        case .reference: "link"
        case .task: "checklist"
        }
    }
}

/// Who did something, as the activity log and history say it.
enum MemoryActorName {
    static func of(_ actor: String?) -> String {
        switch actor {
        case "agent": "Claude"
        case "user": "You"
        case "importer": "the Importer"
        case "reconciler": "the Reconciler"
        default: "YourHarness"
        }
    }

    static func origin(_ origin: String?) -> String {
        switch origin {
        case "user": "You"
        case "import": "Import"
        case "reconciler": "Reconciler"
        default: "Claude"
        }
    }
}
