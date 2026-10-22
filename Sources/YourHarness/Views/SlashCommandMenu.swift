import HarnessCore
import SwiftUI

extension View {
    /// The "/" menu for a composer's text field: while a command name is being typed, at
    /// the start or as a word later on (skills to chain), it lists matches above the field. ↑↓ pick, Tab
    /// completes, Return runs one that takes no arguments (or completes one that does),
    /// Esc closes.
    func slashCommandMenu(text: Binding<String>, projectID: UUID, run: @escaping () -> Void) -> some View {
        modifier(SlashCommandMenu(text: text, projectID: projectID, run: run))
    }
}

private struct SlashCommandMenu: ViewModifier {
    @Environment(AppModel.self) private var app
    @Binding var text: String
    let projectID: UUID
    let run: () -> Void

    @State private var selection = 0
    /// The text when you pressed Esc; the menu stays closed until it changes.
    @State private var dismissedAt: String?
    @State private var listHeight: CGFloat = 0

    /// The command name being typed, without the slash: at the start, or as a word later on;
    /// nil once there's a space after it.
    private var query: String? {
        SlashCommand.trailingCommand(in: text)?.query
    }

    /// Typed after the start of the message: a skill to chain, not a command to run now.
    private var isChained: Bool {
        SlashCommand.trailingCommand(in: text).map { $0.start > text.startIndex } ?? false
    }

    private var matches: [SlashCommand] {
        guard let query, dismissedAt != text else { return [] }
        // Partway through a message, only what the agent can run itself.
        let commands = isChained ? app.chainableCommands(for: projectID) : app.slashCommands(for: projectID)
        guard !query.isEmpty else { return commands }
        let q = query.lowercased()
        func rank(_ command: SlashCommand) -> Int? {
            let name = command.name.lowercased()
            if name == q { return 0 }
            if name.hasPrefix(q) { return 1 }
            if command.aliases.contains(where: { $0.lowercased().hasPrefix(q) }) { return 2 }
            if name.contains(q) { return 3 }
            return nil
        }
        return commands.compactMap { command in rank(command).map { (command, $0) } }
            .sorted { $0.1 < $1.1 }
            .map(\.0)
    }

    func body(content: Content) -> some View {
        let matches = matches
        let isOpen = !matches.isEmpty
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
                complete(matches[clamped(matches)])
                return .handled
            }
            .onKeyPress(.return) {
                // ⌥↩ and ⇧↩ are a new line, never a pick.
                guard isOpen, NSEvent.modifierFlags.isDisjoint(with: [.option, .shift]) else { return .ignored }
                accept(matches[clamped(matches)])
                return .handled
            }
            .onKeyPress(.escape) {
                guard isOpen else { return .ignored }
                dismissedAt = text
                return .handled
            }
            .onChange(of: query) { _, query in
                selection = 0
                if query != nil { app.loadSlashCommands(for: projectID) }
            }
            .overlay(alignment: .top) {
                if isOpen {
                    // Above the composer: lifted by its own height, plus the composer's top edge.
                    CommandList(commands: matches, selection: clamped(matches)) { accept($0) }
                        .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { listHeight = $0 }
                        .offset(y: -listHeight - 24)
                        .transition(.opacity)
                }
            }
            .animation(.snappy(duration: 0.18), value: isOpen)
    }

    private func clamped(_ matches: [SlashCommand]) -> Int {
        min(max(selection, 0), matches.count - 1)
    }

    private func complete(_ command: SlashCommand) {
        let start = SlashCommand.trailingCommand(in: text)?.start ?? text.startIndex
        text = String(text[..<start]) + "/\(command.name) "
    }

    /// Runs it right away unless it needs something more: `/compact` (optional
    /// instructions) runs, `/hello <name>` waits for the name. Later in a message, it's
    /// only put in.
    private func accept(_ command: SlashCommand) {
        guard !isChained else { return complete(command) }
        let hint = command.argumentHint
        if hint.isEmpty || hint.hasPrefix("[") || hint.localizedCaseInsensitiveContains("optional") {
            text = "/\(command.name)"
            run()
        } else {
            complete(command)
        }
    }
}

private struct CommandList: View {
    let commands: [SlashCommand]
    let selection: Int
    let pick: (SlashCommand) -> Void

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 1) {
                    ForEach(Array(commands.enumerated()), id: \.element.id) { index, command in
                        CommandRow(command: command, isSelected: index == selection)
                            .id(command.id)
                            .onTapGesture { pick(command) }
                    }
                }
                .padding(6)
            }
            .scrollBounceBehavior(.basedOnSize)
            .frame(maxHeight: 300)
            .fixedSize(horizontal: false, vertical: true)
            .onChange(of: selection) { _, index in
                guard commands.indices.contains(index) else { return }
                proxy.scrollTo(commands[index].id)
            }
        }
        .glassEffect(.regular, in: .rect(cornerRadius: Theme.floatingRadius))
    }
}

private struct CommandRow: View {
    let command: SlashCommand
    let isSelected: Bool
    @State private var hovering = false

    /// "Say hello (project)" → ("Say hello", "project"). The app's own say so too.
    private var parts: (description: String, source: String?) {
        if AppModel.appCommands.contains(where: { $0.name == command.name }) { return (command.description, "YourHarness") }
        let description = command.description
        guard description.hasSuffix(")"), let open = description.lastIndex(of: "(") else { return (description, nil) }
        let source = description[description.index(after: open)..<description.index(before: description.endIndex)]
        guard !source.isEmpty, source.count <= 40, !source.contains(" ") || source.hasPrefix("plugin") else { return (description, nil) }
        return (description[..<open].trimmingCharacters(in: .whitespaces), String(source))
    }

    var body: some View {
        let parts = parts
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text("/\(command.name)")
                        .font(.body.monospaced().weight(.medium))
                    if !command.argumentHint.isEmpty {
                        Text(command.argumentHint)
                            .font(.callout.monospaced())
                            .foregroundStyle(.tertiary)
                            .lineLimit(1)
                    }
                }
                if !parts.description.isEmpty {
                    Text(parts.description)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 8)
            if let source = parts.source {
                Chip(source, tint: source == "YourHarness" ? .accentColor : nil)
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(isSelected ? AnyShapeStyle(.tint.opacity(0.18)) : hovering ? AnyShapeStyle(.fill.tertiary) : AnyShapeStyle(.clear),
                    in: .rect(cornerRadius: Theme.innerRadius))
        .contentShape(.rect)
        .onHover { hovering = $0 }
    }
}
