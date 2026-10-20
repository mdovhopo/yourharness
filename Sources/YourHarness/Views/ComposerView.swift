import HarnessCore
import SwiftUI

struct ComposerView: View {
    @Bindable var thread: ThreadModel
    @FocusState private var focused: Bool
    @State private var pasteMonitor = ImagePasteMonitor()

    private var canSend: Bool {
        !thread.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !thread.draftImages.isEmpty
    }

    /// Claude is working: ↩ steers (after its current step), ⌘↩ interrupts and sends.
    private var isSteering: Bool {
        thread.phase == .running && thread.hasLiveSession
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if !thread.queuedMessages.isEmpty {
                QueuedMessages(thread: thread)
            }
            if !thread.draftImages.isEmpty {
                DraftAttachments(thread: thread)
            }
            TextField(isSteering ? "Steer Claude… ↩ after its current step, ⌘↩ right now" : "Ask Claude…", text: $thread.draft, axis: .vertical)
                .textFieldStyle(.plain)
                .font(.body)
                .lineLimit(1...12)
                .focused($focused)
                .onSubmit { send() }
                .onKeyPress(.return, phases: .down) { press in
                    guard press.modifiers.contains(.command) else { return .ignored }
                    send(interrupting: true)
                    return .handled
                }
                .slashCommandMenu(text: $thread.draft, projectID: thread.projectID) { send() }
                .padding(.horizontal, 4)

            HStack(spacing: 4) {
                Button("Attach Images", systemImage: "paperclip") {
                    thread.attach(ImageIntake.choose())
                }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .foregroundStyle(.secondary)
                .padding(.trailing, 2)
                .help("Attach images (or paste and drop them)")
                ModelChip(thread: thread)
                EffortChip(thread: thread)
                PermissionChip(thread: thread)
                Spacer(minLength: 8)
                if let tokens = thread.contextTokens {
                    ContextIndicator(tokens: tokens, window: thread.effectiveContextWindow)
                        .padding(.trailing, 6)
                }
                if thread.phase == .running {
                    Button("Stop", systemImage: "stop.fill") { thread.stop() }
                        .labelStyle(.iconOnly)
                        .buttonStyle(.glass)
                        .buttonBorderShape(.circle)
                        .help("Stop (⌘.)")
                }
                if isSteering, canSend {
                    Button("Interrupt and Send", systemImage: "bolt.fill") { send(interrupting: true) }
                        .labelStyle(.iconOnly)
                        .buttonStyle(.glass)
                        .buttonBorderShape(.circle)
                        .help("Interrupt Claude and send this now (⌘↩)")
                        .transition(.scale.combined(with: .opacity))
                }
                Button(isSteering ? "Steer" : "Send", systemImage: isSteering ? "arrow.turn.down.right" : "arrow.up") { send() }
                    .labelStyle(.iconOnly)
                    .fontWeight(.semibold)
                    .buttonStyle(.glassProminent)
                    .buttonBorderShape(.circle)
                    .tint(.claude)
                    .disabled(!canSend)
                    .contentTransition(.symbolEffect(.replace))
                    .help(isSteering ? "Steer: Claude reads this after its current step (↩) · New line (⌥↩)" : "Send (↩) · New line (⌥↩)")
            }
            .controlSize(.regular)
        }
        .padding(.horizontal, 12)
        .padding(.top, 14)
        .padding(.bottom, 10)
        .glassEffect(.regular, in: .rect(cornerRadius: 22))
        .animation(.snappy, value: thread.draftImages.isEmpty)
        .animation(.snappy, value: thread.queuedMessages.map(\.id))
        .animation(.snappy, value: isSteering && canSend)
        .onAppear {
            focused = true
            pasteMonitor.start { [thread] sources in thread.attach(sources) }
        }
        .onDisappear { pasteMonitor.stop() }
        .onChange(of: focused, initial: true) { _, isFocused in pasteMonitor.isActive = isFocused }
    }

    private func send(interrupting: Bool = false) {
        guard canSend else { return }
        thread.send(thread.draft, images: thread.draftImages, interrupting: interrupting)
        thread.draft = ""
        thread.draftImages = []
    }
}

/// Messages sent while Claude works, waiting for its current step. Each can still go now
/// (Claude is interrupted), come back to the composer, or be dropped.
private struct QueuedMessages: View {
    let thread: ThreadModel

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(thread.queuedMessages) { message in
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Image(systemName: message.pending == .sendingNow ? "bolt.fill" : "clock")
                        .foregroundStyle(.secondary)
                        .frame(width: 16)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(message.text.isEmpty ? "Image" : message.text)
                            .lineLimit(2)
                        Text(status(message))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 8)
                    if message.pending == nil {
                        HStack(spacing: 2) {
                            Button("Send Now", systemImage: "bolt") { thread.sendNow(message) }
                                .help("Interrupt Claude and send this now")
                            Button("Edit", systemImage: "pencil") { thread.withdraw(message, toComposer: true) }
                                .help("Take it back into the composer")
                            Button("Remove", systemImage: "xmark") { thread.withdraw(message, toComposer: false) }
                                .help("Don't send it")
                        }
                        .labelStyle(.iconOnly)
                        .buttonStyle(.borderless)
                        .foregroundStyle(.secondary)
                    } else {
                        ProgressView().controlSize(.small)
                    }
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
                .card(radius: Theme.innerRadius + 4)
                .transition(.move(edge: .bottom).combined(with: .opacity))
            }
        }
    }

    private func status(_ message: ThreadModel.QueuedMessage) -> String {
        switch message.pending {
        case .sendingNow: "Interrupting Claude to send this…"
        case .withdrawing: "Taking it back…"
        case nil: "Queued · Claude reads it after its current step"
        }
    }
}

// MARK: - Chips

private struct ModelChip: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    @State private var open = false

    var body: some View {
        PickerChip(isOpen: $open, help: "Model") {
            ClaudeMark(size: 13)
            Text(app.modelDisplayName(for: thread.effectiveModel))
        } content: {
            ModelPicker(thread: thread) { open = false }
        }
    }
}

private struct EffortChip: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    @State private var open = false

    private var level: EffortLevel? { thread.effectiveEffort.flatMap(EffortLevel.init(rawValue:)) }

    var body: some View {
        PickerChip(isOpen: $open, help: "Reasoning effort") {
            Image(systemName: level?.symbol ?? "gauge.with.dots.needle.bottom.50percent")
                .foregroundStyle(.secondary)
            Text(level?.label ?? "Auto")
        } content: {
            PickerPanel {
                PickerSectionHeader("Reasoning Effort")
                PickerRow(symbol: "sparkles", title: "Auto", subtitle: "Let Claude Code decide",
                          isSelected: thread.effectiveEffort == nil) {
                    thread.setEffort(nil)
                    open = false
                }
                ForEach(app.effortLevels(for: thread.effectiveModel)) { level in
                    PickerRow(symbol: level.symbol, title: level.label, subtitle: level.detail,
                              isSelected: thread.effectiveEffort == level.rawValue) {
                        thread.setEffort(level.rawValue)
                        open = false
                    }
                }
                PickerFootnote("Applies from your next message.")
            }
        }
    }
}

private struct PermissionChip: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    @State private var open = false

    var body: some View {
        if let project = app.project(id: thread.projectID) {
            let autoUnavailable = !app.supportsAutoMode(thread.effectiveModel)
            let modelName = app.modelDisplayName(for: thread.effectiveModel)
            PickerChip(
                isOpen: $open,
                help: project.permissionMode == .auto && autoUnavailable
                    ? "\(modelName) doesn't support Auto, so Claude asks before changes"
                    : "Permissions for \(project.name)"
            ) {
                Image(systemName: project.permissionMode.symbol)
                    .foregroundStyle(project.permissionMode == .bypass ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary))
                Text(project.permissionMode.shortLabel)
            } content: {
                PickerPanel {
                    PickerSectionHeader("Permissions · \(project.name)")
                    ForEach(PermissionMode.allCases) { mode in
                        PickerRow(symbol: mode.symbol, title: mode.label,
                                  subtitle: mode == .auto && autoUnavailable
                                      ? "Not available with \(modelName). Claude asks instead."
                                      : mode.detail,
                                  isSelected: project.permissionMode == mode,
                                  tint: mode == .bypass ? .red : nil) {
                            app.setPermissionMode(mode, for: project.id)
                            open = false
                        }
                    }
                    PickerFootnote("Applies to every thread in this project, right away.")
                }
            }
        }
    }
}

private struct ModelPicker: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    let dismiss: () -> Void
    @State private var showVersions = false

    private static let fallbackAliases = ["default", "opus", "fable", "sonnet", "haiku"]

    private var aliases: [ClaudeModel] { app.claudeModels.filter(\.isAlias) }
    private var versions: [ClaudeModel] { app.claudeModels.filter { !$0.isAlias } }
    private var selected: String { thread.effectiveModel ?? "default" }

    var body: some View {
        PickerPanel {
            HStack(spacing: 8) {
                ClaudeMark(size: 18)
                Text("Claude Code")
                    .font(.headline)
                Spacer()
                if let plan = app.claudeAccount?.planName {
                    Chip(plan, tint: .claude)
                }
            }
            .padding(.horizontal, 8)
            .padding(.top, 4)
            .padding(.bottom, 6)

            if aliases.isEmpty {
                ForEach(Self.fallbackAliases, id: \.self) { alias in
                    row(value: alias, title: alias == "default" ? "Default" : alias.capitalized, subtitle: nil)
                }
            } else {
                ForEach(aliases) { model in
                    row(value: model.value,
                        title: model.value == "default" ? "Default · \(app.modelDisplayName(for: "default"))" : model.displayName,
                        subtitle: model.description)
                }
            }

            if !versions.isEmpty {
                Divider().padding(.vertical, 4)
                Button {
                    withAnimation(.snappy) { showVersions.toggle() }
                } label: {
                    HStack {
                        Text("Pinned Versions")
                        Spacer()
                        Image(systemName: "chevron.right")
                            .rotationEffect(.degrees(showVersions ? 90 : 0))
                    }
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .contentShape(.rect)
                }
                .buttonStyle(.plain)

                if showVersions {
                    ForEach(versions) { model in
                        row(value: model.value, title: model.displayName, subtitle: model.description)
                    }
                }
            }

            PickerFootnote("Switches instantly. Set the default in Settings.")
        }
    }

    private func row(value: String, title: String, subtitle: String?) -> some View {
        PickerRow(title: title, subtitle: subtitle, isSelected: selected == value) {
            thread.setModel(value)
            dismiss()
        }
    }
}

// MARK: - Context

/// A ring showing how full the context window is, like a battery for the conversation.
struct ContextIndicator: View {
    let tokens: Int
    let window: Int?

    private var fraction: Double {
        guard let window, window > 0 else { return 0 }
        return min(Double(tokens) / Double(window), 1)
    }

    private var tint: Color {
        switch fraction {
        case ..<0.6: .secondary
        case ..<0.85: .orange
        default: .red
        }
    }

    var body: some View {
        HStack(spacing: 6) {
            if window != nil {
                ZStack {
                    Circle().stroke(.quaternary, lineWidth: 2.5)
                    Circle()
                        .trim(from: 0, to: fraction)
                        .stroke(tint, style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
                        .rotationEffect(.degrees(-90))
                }
                .frame(width: 14, height: 14)
            }
            Text(window.map { "\(Self.compact(tokens)) / \(Self.compact($0))" } ?? Self.compact(tokens))
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
        }
        .help(helpText)
        .animation(.snappy, value: fraction)
    }

    private var helpText: String {
        let used = tokens.formatted()
        guard let window else { return "\(used) tokens in context" }
        return "\(used) of \(window.formatted()) tokens in context (\(fraction.formatted(.percent.precision(.fractionLength(0)))))"
    }

    static func compact(_ value: Int) -> String {
        switch value {
        case 1_000_000...:
            let millions = Double(value) / 1_000_000
            return millions.rounded() == millions ? "\(Int(millions))M" : String(format: "%.1fM", millions)
        case 1_000...:
            return "\(Int((Double(value) / 1_000).rounded()))k"
        default:
            return "\(value)"
        }
    }
}

// MARK: - Picker building blocks

/// A compact capsule that opens a popover, used for every composer setting.
private struct PickerChip<Label: View, Content: View>: View {
    @Binding var isOpen: Bool
    let help: String
    @ViewBuilder let label: Label
    @ViewBuilder let content: Content
    @State private var hovering = false

    var body: some View {
        Button {
            isOpen.toggle()
        } label: {
            HStack(spacing: 5) {
                label
                Image(systemName: "chevron.up.chevron.down")
                    .font(.system(size: 8, weight: .semibold))
                    .foregroundStyle(.tertiary)
            }
            .font(.callout.weight(.medium))
            .padding(.horizontal, 9)
            .padding(.vertical, 5)
            .background(hovering || isOpen ? AnyShapeStyle(.quaternary) : AnyShapeStyle(.clear), in: .capsule)
            .contentShape(.capsule)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help(help)
        .popover(isPresented: $isOpen, arrowEdge: .top) {
            content
        }
    }
}

struct PickerPanel<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            content
        }
        .padding(8)
        .frame(width: 320)
    }
}

struct PickerSectionHeader: View {
    let title: String

    init(_ title: String) {
        self.title = title
    }

    var body: some View {
        Text(title)
            .font(.caption.weight(.medium))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 8)
            .padding(.top, 4)
            .padding(.bottom, 4)
    }
}

struct PickerFootnote: View {
    let text: String

    init(_ text: String) {
        self.text = text
    }

    var body: some View {
        Text(text)
            .font(.caption)
            .foregroundStyle(.tertiary)
            .padding(.horizontal, 8)
            .padding(.top, 6)
    }
}

struct PickerRow: View {
    var symbol: String?
    let title: String
    var subtitle: String?
    let isSelected: Bool
    var tint: Color?
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) {
                if let symbol {
                    Image(systemName: symbol)
                        .foregroundStyle(tint.map { AnyShapeStyle($0) } ?? AnyShapeStyle(.secondary))
                        .frame(width: 18)
                }
                VStack(alignment: .leading, spacing: 1) {
                    Text(title)
                        .fontWeight(isSelected ? .semibold : .regular)
                    if let subtitle {
                        Text(subtitle)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(2)
                    }
                }
                Spacer(minLength: 8)
                if isSelected {
                    Image(systemName: "checkmark")
                        .font(.callout.weight(.semibold))
                        .foregroundStyle(.tint)
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .background(hovering ? AnyShapeStyle(.quaternary) : AnyShapeStyle(.clear), in: .rect(cornerRadius: 8))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}
