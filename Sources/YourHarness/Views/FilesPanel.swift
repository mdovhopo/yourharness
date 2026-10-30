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
    @State private var query = ""
    @AppStorage("filesTreeHeight") private var treeHeight = 220.0

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
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
            .overlay {
                if !query.isEmpty {
                    GoToFileList(files: files, thread: thread, query: $query)
                        .background(.background)
                }
            }
        }
    }

    private var header: some View {
        HStack(spacing: 6) {
            Button {
                files.showsTree.toggle()
            } label: {
                Image(systemName: "list.bullet.indent")
            }
            .buttonStyle(.borderless)
            .foregroundStyle(files.showsTree ? AnyShapeStyle(.tint) : AnyShapeStyle(.secondary))
            .disabled(files.tabs.isEmpty)
            .help(files.showsTree ? "Hide the folder" : "Show the folder")
            GoToFileField(query: $query, files: files, thread: thread)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
    }

    private var tree: some View {
        FileTreeView(files: files, linker: app.fileLinker(for: thread), selectedPath: files.selectedPath, reveal: files.treeReveal)
    }

    private var document: some View {
        VStack(spacing: 0) {
            if !files.tabs.isEmpty {
                FileTabsBar(files: files)
                Divider()
            }
            if let path = files.selectedPath {
                FilePathBar(files: files, path: path)
                Divider()
                FileDocumentView(files: files, path: path)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
    }
}

/// The line between the folder and the file, dragged to give one more room.
private struct ResizeHandle: View {
    @Binding var height: Double
    let limit: Double
    @State private var start: Double?

    var body: some View {
        Divider()
            .overlay {
                Color.clear
                    .frame(height: 7)
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

private struct FileTabsBar: View {
    let files: FilesModel

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView(.horizontal) {
                HStack(spacing: 2) {
                    ForEach(files.tabs) { tab in
                        FileTabButton(files: files, tab: tab, isSelected: tab.path == files.selectedPath)
                            .id(tab.path)
                    }
                }
                .padding(.horizontal, 6)
                .padding(.vertical, 4)
            }
            .scrollIndicators(.never)
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
                .foregroundStyle(isSelected ? .primary : .secondary)
            Button {
                files.close(tab.path)
            } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 8, weight: .bold))
                    .frame(width: 14, height: 14)
                    .contentShape(.rect)
            }
            .buttonStyle(.borderless)
            .opacity(hovering || isSelected ? 1 : 0)
            .help("Close")
        }
        .font(.callout)
        .padding(.leading, 9)
        .padding(.trailing, 4)
        .padding(.vertical, 3)
        .background(isSelected ? AnyShapeStyle(.fill.secondary) : AnyShapeStyle(.clear), in: .rect(cornerRadius: 6))
        .contentShape(.rect)
        .onHover { hovering = $0 }
        .onTapGesture(count: 2) { files.keep(tab.path) }
        .onTapGesture { files.select(tab.path) }
        .help(tab.path)
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

/// Where the file is, and what you can do with it.
private struct FilePathBar: View {
    let files: FilesModel
    let path: String
    @AppStorage("filesWrapLines") private var wraps = false

    var body: some View {
        HStack(spacing: 6) {
            Text(path)
                .font(.caption.monospaced())
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.head)
                .textSelection(.enabled)
            Spacer(minLength: 4)
            Toggle(isOn: $wraps) {
                Image(systemName: "text.word.spacing")
            }
            .toggleStyle(.button)
            .buttonStyle(.borderless)
            .help(wraps ? "Don't wrap lines" : "Wrap lines")
            Menu {
                FileMenuItems(files: files, path: path)
            } label: {
                Image(systemName: "ellipsis")
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
            .help("More")
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 4)
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

private struct GoToFileField: View {
    @Environment(AppModel.self) private var app
    @Binding var query: String
    let files: FilesModel
    let thread: ThreadModel

    var body: some View {
        TextField("Go to File", text: $query)
            .textFieldStyle(.roundedBorder)
            .controlSize(.small)
            .onExitCommand { query = "" }
            // Return opens the best match.
            .onSubmit {
                let typed = query
                Task {
                    guard let index = await app.fileIndex(root: files.root), let first = index.matches(typed, limit: 1).first else { return }
                    query = ""
                    if first.hasSuffix("/") { files.open(first) } else { files.open(first, keep: true) }
                }
            }
    }
}

/// Files whose names match what's typed, best first, from the index: nothing is read.
private struct GoToFileList: View {
    @Environment(AppModel.self) private var app
    let files: FilesModel
    let thread: ThreadModel
    @Binding var query: String
    @State private var matches: [String] = []

    var body: some View {
        List(matches, id: \.self) { path in
            Button {
                open(path)
            } label: {
                VStack(alignment: .leading, spacing: 1) {
                    Text((path.hasSuffix("/") ? String(path.dropLast()) : path).split(separator: "/").last.map(String.init) ?? path)
                    Text(path)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.head)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
        }
        .listStyle(.sidebar)
        .scrollContentBackground(.hidden)
        .overlay {
            if matches.isEmpty {
                Text("No Matches")
                    .foregroundStyle(.secondary)
            }
        }
        .task(id: query) {
            guard let index = await app.fileIndex(root: files.root), !Task.isCancelled else { return }
            matches = index.matches(query, limit: 40)
        }
    }

    private func open(_ path: String) {
        query = ""
        if path.hasSuffix("/") { files.open(path) } else { files.open(path, keep: true) }
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
