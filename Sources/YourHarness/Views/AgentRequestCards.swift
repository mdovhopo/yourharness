import HarnessCore
import SwiftUI

/// The agent asks; you pick from its options or write your own answer.
struct QuestionCard: View {
    let questions: [AgentQuestion]
    let answer: ([String: [String]]) -> Void
    let skip: () -> Void

    @State private var selections: [String: Set<String>] = [:]
    @State private var others: [String: String] = [:]

    private func answers(for question: AgentQuestion) -> [String] {
        let chosen = question.options.map(\.label).filter { selections[question.id]?.contains($0) == true }
        let other = others[question.id, default: ""].trimmingCharacters(in: .whitespacesAndNewlines)
        return other.isEmpty ? chosen : chosen + [other]
    }

    private var isComplete: Bool {
        questions.allSatisfy { !answers(for: $0).isEmpty }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            CappedHeight(420) {
                VStack(alignment: .leading, spacing: 14) {
                    ForEach(questions) { question in
                        QuestionView(
                            question: question,
                            selection: Binding(get: { selections[question.id, default: []] }, set: { selections[question.id] = $0 }),
                            other: Binding(get: { others[question.id, default: ""] }, set: { others[question.id] = $0 })
                        )
                    }
                }
            }
            HStack {
                Spacer()
                Button("Skip") { skip() }
                    .keyboardShortcut(.delete, modifiers: .command)
                    .help("Don't answer; Claude continues on its own judgment (⌘⌫)")
                Button("Answer") {
                    answer(Dictionary(uniqueKeysWithValues: questions.map { ($0.question, answers(for: $0)) }))
                }
                .keyboardShortcut(.return, modifiers: .command)
                .buttonStyle(.glassProminent)
                .disabled(!isComplete)
                .help("Answer (⌘↩)")
            }
            .controlSize(.large)
        }
        .padding(14)
        .glassEffect(.regular.tint(.blue.opacity(0.1)), in: .rect(cornerRadius: Theme.floatingRadius))
    }
}

private struct QuestionView: View {
    let question: AgentQuestion
    @Binding var selection: Set<String>
    @Binding var other: String

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: "questionmark.bubble")
                    .foregroundStyle(.blue)
                if let header = question.header, !header.isEmpty {
                    Chip(header, tint: .blue)
                }
                Text(question.question)
                    .font(.headline)
            }
            VStack(alignment: .leading, spacing: 2) {
                ForEach(question.options) { option in
                    OptionRow(option: option, multiSelect: question.multiSelect, isSelected: selection.contains(option.label)) {
                        toggle(option.label)
                    }
                }
                HStack(spacing: 10) {
                    Image(systemName: question.multiSelect ? "square" : "circle")
                        .foregroundStyle(.tertiary)
                        .opacity(0)
                    TextField(question.options.isEmpty ? "Your answer" : "Something else…", text: $other)
                        .textFieldStyle(.roundedBorder)
                        .onChange(of: other) { _, text in
                            // Writing your own answer replaces a single choice.
                            if !question.multiSelect, !text.isEmpty { selection = [] }
                        }
                }
                .padding(.horizontal, 8)
                .padding(.top, 4)
            }
        }
    }

    private func toggle(_ label: String) {
        if question.multiSelect {
            if selection.contains(label) { selection.remove(label) } else { selection.insert(label) }
        } else {
            selection = [label]
            other = ""
        }
    }
}

private struct OptionRow: View {
    let option: AgentQuestion.Option
    let multiSelect: Bool
    let isSelected: Bool
    let toggle: () -> Void
    @State private var hovering = false

    private var symbol: String {
        switch (multiSelect, isSelected) {
        case (true, true): "checkmark.square.fill"
        case (true, false): "square"
        case (false, true): "largecircle.fill.circle"
        case (false, false): "circle"
        }
    }

    var body: some View {
        Button(action: toggle) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Image(systemName: symbol)
                    .foregroundStyle(isSelected ? AnyShapeStyle(.tint) : AnyShapeStyle(.secondary))
                VStack(alignment: .leading, spacing: 2) {
                    Text(option.label)
                        .fontWeight(isSelected ? .semibold : .regular)
                    if let description = option.description, !description.isEmpty {
                        Text(description)
                            .font(.callout)
                            .foregroundStyle(.secondary)
                    }
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .background(hovering || isSelected ? Color.primary.opacity(isSelected ? 0.07 : 0.04) : .clear,
                        in: .rect(cornerRadius: Theme.innerRadius))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}

/// The agent's plan, waiting for a go-ahead: approve into a permission mode, or keep
/// planning with feedback.
struct PlanCard: View {
    @Environment(AppModel.self) private var app
    let plan: String
    let thread: ThreadModel
    let decide: (PermissionMode?, String) -> Void
    @State private var feedback = ""

    /// Auto when the model supports it, else edits go through and commands still ask.
    private var recommended: PermissionMode {
        app.supportsAutoMode(thread.effectiveModel) ? .auto : .acceptEdits
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Label("Plan ready for review", systemImage: "list.bullet.clipboard")
                .font(.headline)
                .foregroundStyle(.primary)
            CappedHeight(320) {
                MarkdownView(text: plan)
                    .textSelection(.enabled)
                    .padding(.trailing, 8)
            }
            TextField("Feedback for Claude (optional)", text: $feedback, axis: .vertical)
                .textFieldStyle(.roundedBorder)
                .lineLimit(1...4)
            HStack {
                Spacer()
                Button("Keep Planning") { decide(nil, feedback) }
                    .keyboardShortcut(.delete, modifiers: .command)
                    .help("Send your feedback and keep planning (⌘⌫)")
                Menu {
                    ForEach([PermissionMode.auto, .acceptEdits, .ask], id: \.self) { mode in
                        Button("Approve, \(mode.label)") { decide(mode, feedback) }
                            .disabled(mode == .auto && !app.supportsAutoMode(thread.effectiveModel))
                    }
                } label: {
                    Text("Approve · \(recommended.shortLabel)")
                } primaryAction: {
                    decide(recommended, feedback)
                }
                .menuStyle(.button)
                .buttonStyle(.glassProminent)
                .fixedSize()
                .keyboardShortcut(.return, modifiers: .command)
                .help("Approve and continue in \(recommended.label) (⌘↩). The menu has other modes.")
            }
            .controlSize(.large)
        }
        .padding(14)
        .glassEffect(.regular.tint(.green.opacity(0.1)), in: .rect(cornerRadius: Theme.floatingRadius))
    }
}

/// Content at its natural height up to `limit`, scrolling beyond it. A card's height never
/// grows the window: wrapped text asks for its height at the narrowest width, so letting
/// it size the window once made the window taller than the screen.
private struct CappedHeight<Content: View>: View {
    let limit: CGFloat
    @ViewBuilder let content: Content

    init(_ limit: CGFloat, @ViewBuilder content: () -> Content) {
        self.limit = limit
        self.content = content()
    }

    /// Measured at the card's real width, so it's the height the content actually needs.
    @State private var height: CGFloat?

    var body: some View {
        ScrollView {
            content.onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height = $0 }
        }
        .scrollBounceBehavior(.basedOnSize)
        .frame(height: min(height ?? limit, limit))
    }
}
