import SwiftUI

/// ⌘F in a thread: what you're looking for, and which messages have it. Steps go from
/// message to message, starting at the newest, where you are.
@MainActor @Observable
final class ThreadFind {
    var isOpen = false
    var query = ""
    private(set) var matches: [String] = []
    private(set) var index = 0
    /// Bumped to put the cursor in the field, also when it's already open.
    private(set) var focusRequest = 0
    @ObservationIgnored private var matchSet: Set<String> = []

    var current: String? { matches.indices.contains(index) ? matches[index] : nil }
    var isSearching: Bool { isOpen && !query.trimmingCharacters(in: .whitespaces).isEmpty }

    func open() {
        isOpen = true
        focusRequest += 1
    }

    func close() {
        isOpen = false
        query = ""
        matches = []
        matchSet = []
    }

    func step(_ delta: Int) {
        guard !matches.isEmpty else { return }
        index = (index + delta + matches.count) % matches.count
    }

    /// New results: stay on the message you were on if it still matches, else the newest.
    func update(_ found: [String]) {
        let current = current
        matches = found
        matchSet = Set(found)
        index = current.flatMap { found.firstIndex(of: $0) } ?? max(found.count - 1, 0)
    }

    /// How one row should mark what you're looking for; nil when it has none of it.
    func highlight(for id: String) -> FindHighlight? {
        guard isSearching, matchSet.contains(id) else { return nil }
        return FindHighlight(query: query.trimmingCharacters(in: .whitespaces), isCurrent: id == current)
    }

    /// The messages containing `query`, oldest first. Off the main thread: a long thread
    /// is a lot of text to go through on every keystroke.
    nonisolated static func search(_ query: String, in texts: [(id: String, text: String)]) async -> [String] {
        await Task.detached(priority: .userInitiated) {
            texts.compactMap { item in
                item.text.range(of: query, options: [.caseInsensitive, .diacriticInsensitive]) == nil ? nil : item.id
            }
        }.value
    }
}

struct FindHighlight: Equatable {
    var query: String
    /// The message the find bar is on: marked more strongly than the rest.
    var isCurrent: Bool
}

extension EnvironmentValues {
    @Entry var findHighlight: FindHighlight?
}

extension AttributedString {
    /// Marks every occurrence of what you're finding.
    func highlighted(_ highlight: FindHighlight?) -> AttributedString {
        guard let highlight, !highlight.query.isEmpty else { return self }
        var text = self
        let plain = String(text.characters)
        var searchRange = plain.startIndex..<plain.endIndex
        let color = highlight.isCurrent ? Color.orange.opacity(0.6) : Color.yellow.opacity(0.35)
        while let found = plain.range(of: highlight.query, options: [.caseInsensitive, .diacriticInsensitive], range: searchRange) {
            let lower = plain.distance(from: plain.startIndex, to: found.lowerBound)
            let length = plain.distance(from: found.lowerBound, to: found.upperBound)
            let start = text.characters.index(text.characters.startIndex, offsetBy: lower)
            let end = text.characters.index(start, offsetBy: length)
            text[start..<end].backgroundColor = color
            searchRange = found.upperBound..<plain.endIndex
        }
        return text
    }
}

/// The find bar: floats over the top of the thread.
struct FindBar: View {
    @Bindable var find: ThreadFind
    @FocusState private var focused: Bool

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .foregroundStyle(.secondary)
            TextField("Find in thread", text: $find.query)
                .textFieldStyle(.plain)
                .focused($focused)
                .frame(minWidth: 160)
                .onSubmit { find.step(1) }
                .onKeyPress(.return, phases: .down) { press in
                    guard press.modifiers.contains(.shift) else { return .ignored }
                    find.step(-1)
                    return .handled
                }
                .onKeyPress(.escape) {
                    find.close()
                    return .handled
                }
            if find.isSearching {
                Text(find.matches.isEmpty ? "No matches" : "\(find.index + 1) of \(find.matches.count)")
                    .font(.callout.monospacedDigit())
                    .foregroundStyle(.secondary)
                    .fixedSize()
            }
            HStack(spacing: 2) {
                Button("Previous", systemImage: "chevron.up") { find.step(-1) }
                    .help("Previous match (⇧⌘G)")
                Button("Next", systemImage: "chevron.down") { find.step(1) }
                    .help("Next match (⌘G)")
            }
            .labelStyle(.iconOnly)
            .buttonStyle(.borderless)
            .disabled(find.matches.isEmpty)
            Button("Done") { find.close() }
                .buttonStyle(.borderless)
                .help("Close (Esc)")
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .glassEffect(.regular, in: .capsule)
        .frame(maxWidth: 460)
        .onAppear { focused = true }
        .onChange(of: find.focusRequest) { focused = true }
    }
}
