import HarnessCore
import SwiftUI

extension View {
    /// The `@` menu for a composer's text field: while you type `@` and part of a path at
    /// the end of the text, it lists matching files and folders in the project above the
    /// field. ↑↓ pick, Tab or Return insert it, Esc closes. Claude Code reads the
    /// mention and attaches the file.
    func fileMentionMenu(text: Binding<String>, projectID: UUID) -> some View {
        modifier(FileMentionMenu(text: text, projectID: projectID))
    }
}

private struct FileMentionMenu: ViewModifier {
    @Environment(AppModel.self) private var app
    @Binding var text: String
    let projectID: UUID

    @State private var index: FileIndex?
    @State private var matches: [String] = []
    @State private var selection = 0
    /// The text when you pressed Esc; the menu stays closed until it changes.
    @State private var dismissedAt: String?
    @State private var listHeight: CGFloat = 0

    private var query: String? {
        guard dismissedAt != text else { return nil }
        return FileIndex.trailingMention(in: text)?.query
    }

    func body(content: Content) -> some View {
        let isOpen = query != nil && !matches.isEmpty
        content
            .onKeyPress(.downArrow) {
                guard isOpen else { return .ignored }
                selection = min(selection + 1, matches.count - 1)
                return .handled
            }
            .onKeyPress(.upArrow) {
                guard isOpen else { return .ignored }
                selection = max(selection - 1, 0)
                return .handled
            }
            .onKeyPress(.tab) {
                guard isOpen else { return .ignored }
                insert(matches[clamped])
                return .handled
            }
            .onKeyPress(.return) {
                guard isOpen, NSEvent.modifierFlags.isDisjoint(with: [.option, .shift]) else { return .ignored }
                insert(matches[clamped])
                return .handled
            }
            .onKeyPress(.escape) {
                guard isOpen else { return .ignored }
                dismissedAt = text
                return .handled
            }
            // Listed once you type `@`, then ranked as you go.
            .task(id: query) {
                selection = 0
                guard let query else {
                    matches = []
                    return
                }
                // Cached for a little while, so new files show up without waiting on every key.
                index = await app.fileIndex(for: projectID) ?? index
                guard let index, !Task.isCancelled else { return }
                matches = index.matches(query, limit: 40)
            }
            .overlay(alignment: .top) {
                if isOpen {
                    FileList(paths: matches, selection: clamped) { insert($0) }
                        .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { listHeight = $0 }
                        .offset(y: -listHeight - 24)
                        .transition(.opacity)
                }
            }
            .animation(.snappy(duration: 0.18), value: isOpen)
    }

    private var clamped: Int { min(max(selection, 0), matches.count - 1) }

    /// Replaces the `@…` being typed with the mention, and a space to carry on.
    private func insert(_ path: String) {
        guard let mention = FileIndex.trailingMention(in: text) else { return }
        text = String(text[..<mention.start]) + FileIndex.mention(path) + " "
        // A folder may be where you're headed, not where you're stopping.
        if path.hasSuffix("/") { text.removeLast() }
        matches = []
    }
}

private struct FileList: View {
    let paths: [String]
    let selection: Int
    let pick: (String) -> Void

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 1) {
                    ForEach(Array(paths.enumerated()), id: \.element) { index, path in
                        FileRow(path: path, isSelected: index == selection)
                            .id(path)
                            .onTapGesture { pick(path) }
                    }
                }
                .padding(6)
            }
            .scrollBounceBehavior(.basedOnSize)
            .frame(maxHeight: 300)
            .fixedSize(horizontal: false, vertical: true)
            .onChange(of: selection) { _, index in
                guard paths.indices.contains(index) else { return }
                proxy.scrollTo(paths[index])
            }
        }
        .glassEffect(.regular, in: .rect(cornerRadius: Theme.floatingRadius))
    }
}

private struct FileRow: View {
    let path: String
    let isSelected: Bool
    @State private var hovering = false

    private var isFolder: Bool { path.hasSuffix("/") }

    /// "Sources/App/Main.swift" → ("Main.swift", "Sources/App").
    private var parts: (name: String, folder: String) {
        let trimmed = isFolder ? String(path.dropLast()) : path
        guard let slash = trimmed.lastIndex(of: "/") else { return (trimmed, "") }
        return (String(trimmed[trimmed.index(after: slash)...]), String(trimmed[..<slash]))
    }

    var body: some View {
        let parts = parts
        HStack(spacing: 8) {
            Image(systemName: isFolder ? "folder" : "doc.text")
                .foregroundStyle(.secondary)
                .frame(width: 16)
            Text(parts.name + (isFolder ? "/" : ""))
                .font(.body.weight(.medium))
                .lineLimit(1)
            Text(parts.folder)
                .font(.callout)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.head)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 5)
        .background(isSelected ? AnyShapeStyle(.tint.opacity(0.18)) : hovering ? AnyShapeStyle(.fill.tertiary) : AnyShapeStyle(.clear),
                    in: .rect(cornerRadius: Theme.innerRadius))
        .contentShape(.rect)
        .onHover { hovering = $0 }
    }
}
