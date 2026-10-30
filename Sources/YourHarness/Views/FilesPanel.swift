import AppKit
import HarnessCore
import SwiftUI

/// The right sidebar's Files tab: the thread's folder as a tree, and the files opened from it
/// or from links in the chat, in tabs. Read-only.
struct FilesPanel: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel

    var body: some View {
        Group {
            if let files = thread.files, files.root == thread.fileRoot?.standardizedFileURL {
                FilesContent(files: files, thread: thread)
            } else if thread.fileRoot == nil {
                ContentUnavailableView("No Project Folder", systemImage: "folder",
                                       description: Text("Files show for threads in a project folder."))
            } else {
                Color.clear
            }
        }
        // Made here rather than while drawing: making it changes the thread.
        .task(id: thread.fileRoot?.path) {
            _ = thread.ensureFiles()
            await app.loadFileLinker(for: thread)
        }
    }
}

private struct FilesContent: View {
    @Environment(AppModel.self) private var app
    let files: FilesModel
    let thread: ThreadModel
    @AppStorage("filesWrapLines") private var wraps = false
    // `YH_FILES_QUERY=<words>`: typed at launch, for looking at the results.
    @State private var query = ProcessInfo.processInfo.environment["YH_FILES_QUERY"] ?? ""
    @State private var matches: [String] = []
    @State private var highlighted = 0

    var body: some View {
        VStack(spacing: 0) {
            // As the browser's address bar: everything that acts on the tab in one capsule.
            topBar
                .padding(.horizontal, 10)
                .padding(.top, 8)
                .padding(.bottom, 6)
            // A view of its own, so typing in Go to File doesn't update the tree and the file.
            FilesPanes(files: files, thread: thread)
                .overlay(alignment: .top) {
                    if !query.isEmpty {
                        GoToFileResults(matches: matches, highlighted: highlighted, open: open)
                            .padding(.horizontal, 10)
                    }
                }
        }
        .task(id: query) { await search() }
    }

    private var topBar: some View {
        HStack(spacing: 4) {
            Button(files.showsTree ? "Hide Folder" : "Show Folder", systemImage: "list.bullet.indent") { files.showsTree.toggle() }
                .foregroundStyle(files.showsTree ? AnyShapeStyle(.primary) : AnyShapeStyle(.tint))
                .disabled(files.tabs.isEmpty)
                .help(files.showsTree ? "Hide the folder" : "Show the folder")
            HStack(spacing: 4) {
                Image(systemName: "magnifyingglass")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                TextField("Go to File", text: $query)
                    .textFieldStyle(.plain)
                    .onSubmit { openHighlighted() }
                    .onKeyPress(.downArrow) { move(1) }
                    .onKeyPress(.upArrow) { move(-1) }
                    .onExitCommand { query = "" }
            }
            .padding(.horizontal, 4)
            Button(wraps ? "Don't Wrap Lines" : "Wrap Lines", systemImage: "text.word.spacing") { wraps.toggle() }
                .foregroundStyle(wraps ? AnyShapeStyle(.tint) : AnyShapeStyle(.primary))
                .disabled(files.selectedPath == nil)
                .help(wraps ? "Don't wrap lines" : "Wrap lines")
            Menu {
                if let path = files.selectedPath {
                    FileMenuItems(files: files, path: path)
                }
            } label: {
                Label("More", systemImage: "ellipsis")
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
            .disabled(files.selectedPath == nil)
            .help("More")
        }
        .labelStyle(.iconOnly)
        .buttonStyle(.borderless)
        .padding(.horizontal, 6)
        .padding(.vertical, 4)
        .glassEffect(.regular, in: .capsule)
    }

    /// Best first, from the index: nothing is read. Off the main thread: a big project has
    /// tens of thousands of paths to rank.
    private func search() async {
        highlighted = 0
        guard !query.isEmpty else {
            matches = []
            return
        }
        guard let index = await app.fileIndex(root: files.root), !Task.isCancelled else { return }
        let typed = query
        let found = await Task.detached(priority: .userInitiated) { index.matches(typed, limit: 40) }.value
        if !Task.isCancelled { matches = found }
    }

    private func move(_ step: Int) -> KeyPress.Result {
        guard !matches.isEmpty else { return .ignored }
        highlighted = min(max(highlighted + step, 0), matches.count - 1)
        return .handled
    }

    private func openHighlighted() {
        if matches.indices.contains(highlighted) { open(matches[highlighted]) }
    }

    private func open(_ path: String) {
        query = ""
        if path.hasSuffix("/") { files.open(path) } else { files.open(path, keep: true) }
    }
}

private struct FilesPanes: View {
    @Environment(AppModel.self) private var app
    let files: FilesModel
    let thread: ThreadModel
    @AppStorage("filesTreeHeight") private var treeHeight = 220.0

    var body: some View {
        // The tree keeps its place whatever else shows, so it's never made again (read
        // again, its folders reopened) because a file opened or you searched. A plain
        // stack and a handle rather than a split view: that one hosts each pane on its own,
        // and measuring them made opening the tab twice as slow.
        GeometryReader { proxy in
            VStack(spacing: 0) {
                let showsTree = files.showsTree || files.tabs.isEmpty
                if showsTree {
                    tree
                        .frame(height: files.tabs.isEmpty ? proxy.size.height : min(max(treeHeight, 80), max(proxy.size.height - 160, 80)))
                }
                if !files.tabs.isEmpty {
                    if showsTree {
                        ResizeHandle(height: $treeHeight, limit: max(proxy.size.height - 160, 80))
                    }
                    document
                        .frame(maxHeight: .infinity)
                }
            }
        }
    }

    private var tree: some View {
        FileTreeView(files: files, linker: app.fileLinker(for: thread), selectedPath: files.selectedPath, reveal: files.treeReveal)
    }

    private var document: some View {
        VStack(spacing: 6) {
            FileTabStrip(files: files)
            if let path = files.selectedPath {
                VStack(spacing: 0) {
                    FilePath(path: path)
                    Divider()
                    FileDocumentView(files: files, path: path)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
                .clipShape(.rect(cornerRadius: Theme.cardRadius, style: .continuous))
                .card()
                .padding([.horizontal, .bottom], 10)
            }
        }
        .padding(.top, 6)
    }
}

/// The line between the folder and the file, dragged to give one more room.
private struct ResizeHandle: View {
    @Binding var height: Double
    let limit: Double
    @State private var start: Double?

    var body: some View {
        Divider()
            .padding(.horizontal, 10)
            .overlay {
                Color.clear
                    .frame(height: 9)
                    .contentShape(.rect)
                    .pointerStyle(.rowResize)
                    .gesture(DragGesture(minimumDistance: 1, coordinateSpace: .global)
                        .onChanged { drag in
                            let from = start ?? height
                            start = from
                            height = min(max(from + drag.translation.height, 80), limit)
                        }
                        .onEnded { _ in start = nil })
            }
            .accessibilityHidden(true)
    }
}

// MARK: - Tabs

/// As the browser's tabs: capsules, the one you're on filled. In italics until you keep it.
private struct FileTabStrip: View {
    let files: FilesModel

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView(.horizontal) {
                HStack(spacing: 4) {
                    ForEach(files.tabs) { tab in
                        FileTabButton(files: files, tab: tab, isSelected: tab.path == files.selectedPath)
                            .id(tab.path)
                    }
                }
                .padding(.horizontal, 10)
            }
            .scrollIndicators(.hidden)
            .onChange(of: files.selectedPath) { _, path in
                guard let path else { return }
                withAnimation(.snappy) { proxy.scrollTo(path) }
            }
        }
    }
}

private struct FileTabButton: View {
    let files: FilesModel
    let tab: FilesModel.Tab
    let isSelected: Bool
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 4) {
            Text(tab.name)
                .italic(tab.isPreview)
                .lineLimit(1)
                .frame(maxWidth: 140)
                .foregroundStyle(isSelected ? .primary : .secondary)
            Button("Close", systemImage: "xmark") { files.close(tab.path) }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .font(.caption2.weight(.semibold))
                .foregroundStyle(.secondary)
                .opacity(hovering || isSelected ? 1 : 0)
        }
        .font(.caption)
        .padding(.leading, 8)
        .padding(.trailing, 4)
        .padding(.vertical, 3)
        .background(isSelected ? AnyShapeStyle(.quaternary) : AnyShapeStyle(.clear), in: .capsule)
        .contentShape(.capsule)
        .onHover { hovering = $0 }
        .onTapGesture(count: 2) { files.keep(tab.path) }
        .onTapGesture { files.select(tab.path) }
        .help(tab.isPreview ? "\(tab.path)\nDouble-click to keep it open" : tab.path)
        .contextMenu {
            Button("Keep Open") { files.keep(tab.path) }
                .disabled(!tab.isPreview)
            Button("Close") { files.close(tab.path) }
            Button("Close Others") { files.closeOthers(tab.path) }
                .disabled(files.tabs.count < 2)
            Divider()
            FileMenuItems(files: files, path: tab.path)
        }
    }
}

/// Where the file is, at the top of its card: the name stands out, and stays when it's cut.
private struct FilePath: View {
    let path: String

    var body: some View {
        let folder = (path as NSString).deletingLastPathComponent
        let name = Text((path as NSString).lastPathComponent).foregroundStyle(.primary)
        Group {
            if folder.isEmpty {
                name
            } else {
                Text("\(Text(folder + "/").foregroundStyle(.secondary))\(name)")
            }
        }
        .font(.caption)
        .lineLimit(1)
        .truncationMode(.head)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .help(path)
    }
}

private struct FileMenuItems: View {
    let files: FilesModel
    let path: String

    var body: some View {
        let url = files.url(for: path)
        Button("Open in Default App") { NSWorkspace.shared.open(url) }
        Button("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting([url]) }
        Divider()
        Button("Copy Path") { FileActions.copy(url.path) }
        Button("Copy Relative Path") { FileActions.copy(path) }
    }
}

// MARK: - Go to file

/// What's typed in Go to File finds, floating over the tree and the file.
private struct GoToFileResults: View {
    let matches: [String]
    let highlighted: Int
    let open: (String) -> Void

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(matches.enumerated()), id: \.element) { index, path in
                        Button {
                            open(path)
                        } label: {
                            row(path, isHighlighted: index == highlighted)
                        }
                        .buttonStyle(.plain)
                        .id(path)
                    }
                }
                .padding(6)
                .overlay {
                    if matches.isEmpty {
                        Text("No Matches")
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .padding(10)
                    }
                }
            }
            .onChange(of: highlighted) { _, index in
                if matches.indices.contains(index) { proxy.scrollTo(matches[index]) }
            }
        }
        .frame(height: min(CGFloat(max(matches.count, 1)) * 40 + 12, 340))
        .glassEffect(.regular, in: .rect(cornerRadius: Theme.floatingRadius))
    }

    private func row(_ path: String, isHighlighted: Bool) -> some View {
        let isFolder = path.hasSuffix("/")
        let trimmed = isFolder ? String(path.dropLast()) : path
        return HStack(spacing: 8) {
            Image(systemName: FileSymbol.name(for: trimmed, isFolder: isFolder))
                .font(.callout)
                .foregroundStyle(.secondary)
                .frame(width: 16)
            VStack(alignment: .leading, spacing: 1) {
                Text((trimmed as NSString).lastPathComponent)
                    .font(.callout)
                    .lineLimit(1)
                Text((trimmed as NSString).deletingLastPathComponent)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.head)
            }
        }
        .padding(.horizontal, 8)
        .frame(maxWidth: .infinity, minHeight: 40, alignment: .leading)
        .background(isHighlighted ? AnyShapeStyle(.quaternary) : AnyShapeStyle(.clear), in: .rect(cornerRadius: Theme.innerRadius))
        .contentShape(.rect)
    }
}

/// File links clicked in a thread or its side chat.
@MainActor
enum FileLinkAction {
    /// Opens a file link in the thread's Files tab, or with ⌘ in the file's own app. False
    /// for links that aren't to files.
    static func open(_ url: URL, in thread: ThreadModel) -> Bool {
        guard let link = FileLink.parse(url) else { return false }
        if NSEvent.modifierFlags.contains(.command) {
            NSWorkspace.shared.open(URL(fileURLWithPath: link.path))
        } else {
            thread.openFile(atPath: link.path, line: link.line, endLine: link.endLine)
        }
        return true
    }
}
