// The bottom of a thread: what waits on you (a tool to allow, a plan, Claude's question, an MCP
// server's question), your queued messages, what the run is doing, the thread's model, effort
// and mode, and the field you type in (with pictures). Actions go to the Mac as its own buttons
// would take them; the rows they cause arrive like any others.

import PhotosUI
import SwiftUI

struct ComposerBar: View {
    let threadID: String
    @Environment(Store.self) private var store
    @Binding var draft: String
    @Binding var images: [OutgoingImage]
    @FocusState private var focused: Bool
    @State private var picked: [PhotosPickerItem] = []

    private var live: Live? { store.live }
    private var running: Bool { live?.running ?? store.threads[threadID]?.isRunning ?? false }
    private var empty: Bool { draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && images.isEmpty }

    var body: some View {
        VStack(spacing: 8) {
            if let request = live?.requests.first {
                RequestCard(request: request, count: live?.requests.count ?? 1, threadID: threadID)
                    .id(request.id)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
            }
            if let question = live?.questions.first {
                MCPQuestionCard(question: question, threadID: threadID)
                    .id(question.id)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
            }
            ForEach(live?.queue ?? []) { queued in
                QueuedRow(queued: queued, threadID: threadID)
            }
            if running {
                StatusLine(activity: live?.activity, tokens: live?.contextTokens, window: live?.contextWindow)
            }
            if store.canAct {
                if store.canControl { SettingsRow(threadID: threadID) }
                if !images.isEmpty { attachments }
                field
            } else {
                Text(store.serverProtocol == 1 ? "Read-only: this server doesn't take actions." : "Connecting to your Mac…")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .padding(.vertical, 6)
            }
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
        .padding(.bottom, 6)
        .animation(.snappy(duration: 0.25), value: live?.requests.first?.id)
        .animation(.snappy(duration: 0.25), value: live?.questions.first?.id)
        .onChange(of: store.returnedDraft?.text) { _, text in
            if let returned = store.returnedDraft, returned.thread == threadID, let text, draft.isEmpty {
                draft = text
                store.returnedDraft = nil
            }
        }
        .onChange(of: picked) { _, items in
            guard !items.isEmpty else { return }
            picked = []
            Task {
                for item in items {
                    if let data = try? await item.loadTransferable(type: Data.self), let image = OutgoingImage.prepared(data) {
                        images.append(image)
                    }
                }
            }
        }
    }

    /// Pictures waiting to go with the message.
    private var attachments: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(Array(images.enumerated()), id: \.offset) { i, image in
                    ZStack(alignment: .topTrailing) {
                        if let ui = UIImage(data: image.data) {
                            Image(uiImage: ui).resizable().scaledToFill().frame(width: 56, height: 56).clipShape(.rect(cornerRadius: 10))
                        }
                        Button {
                            images.remove(at: i)
                        } label: {
                            Image(systemName: "xmark.circle.fill").symbolRenderingMode(.palette).foregroundStyle(.white, .black.opacity(0.6))
                        }
                        .offset(x: 6, y: -6)
                        .accessibilityLabel("Remove picture")
                    }
                }
            }
            .padding(.top, 6)
            .padding(.horizontal, 4)
        }
    }

    private var field: some View {
        HStack(alignment: .bottom, spacing: 8) {
            if store.canControl {
                PhotosPicker(selection: $picked, maxSelectionCount: 4, matching: .images) {
                    Image(systemName: "photo.on.rectangle")
                        .font(.system(size: 16, weight: .medium))
                        .frame(width: 38, height: 38)
                }
                .buttonStyle(.glass)
                .buttonBorderShape(.circle)
                .accessibilityLabel("Attach pictures")
            }
            TextField(running ? "Message (waits for Claude's step)" : "Message", text: $draft, axis: .vertical)
                .lineLimit(1...6)
                .focused($focused)
                .padding(.horizontal, 14)
                .padding(.vertical, 9)
                .glassEffect(.regular.interactive(), in: .rect(cornerRadius: 20))
                .submitLabel(.send)
            if empty, running {
                Button {
                    store.stop(threadID)
                } label: {
                    Image(systemName: "stop.fill")
                        .font(.system(size: 15, weight: .semibold))
                        .frame(width: 38, height: 38)
                }
                .buttonStyle(.glass)
                .buttonBorderShape(.circle)
                .accessibilityLabel("Stop")
            } else {
                Button {
                    store.send(draft, to: threadID, images: images)
                    draft = ""
                    images = []
                } label: {
                    Image(systemName: "arrow.up")
                        .font(.system(size: 16, weight: .bold))
                        .frame(width: 38, height: 38)
                }
                .buttonStyle(.glassProminent)
                .buttonBorderShape(.circle)
                .disabled(empty)
                .accessibilityLabel("Send")
            }
        }
        .sensoryFeedback(.impact(weight: .light), trigger: running)
    }
}

extension OutgoingImage {
    /// A picture as it goes to the Mac: at most 1600 points on its longer side, JPEG.
    static func prepared(_ data: Data) -> OutgoingImage? {
        guard let image = UIImage(data: data) else { return nil }
        let longest = max(image.size.width, image.size.height)
        let scale = min(1, 1600 / max(longest, 1))
        let size = CGSize(width: (image.size.width * scale).rounded(), height: (image.size.height * scale).rounded())
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let resized = UIGraphicsImageRenderer(size: size, format: format).image { _ in image.draw(in: CGRect(origin: .zero, size: size)) }
        guard let jpeg = resized.jpegData(compressionQuality: 0.8) else { return nil }
        return OutgoingImage(mediaType: "image/jpeg", data: jpeg)
    }
}

/// The thread's model and effort, and its project's permission mode, as the Mac's pickers set
/// them: one quiet line ("Haiku 5.5 · Medium · Accept Edits"), each part a menu.
private struct SettingsRow: View {
    let threadID: String
    @Environment(Store.self) private var store

    private var thread: ThreadInfo? { store.threads[threadID] }
    private var model: ModelOption? { store.models.first { $0.id == thread?.model } }

    var body: some View {
        HStack(spacing: 4) {
            Menu {
                Picker("Model", selection: Binding(get: { thread?.model ?? "" }, set: { store.setModel($0.isEmpty ? nil : $0, of: threadID) })) {
                    Text("Default").tag("")
                    ForEach(store.models.filter { !$0.id.hasPrefix("claude-") }) { m in Text(m.name).tag(m.id) }
                    Section("Versions") {
                        ForEach(store.models.filter { $0.id.hasPrefix("claude-") }) { m in Text(m.name).tag(m.id) }
                    }
                }
            } label: {
                part(model?.name ?? thread?.model.map(Self.short) ?? "Default model")
            }
            .accessibilityLabel("Model")
            dot
            let efforts = model?.efforts ?? ["low", "medium", "high", "xhigh", "max"]
            Menu {
                Picker("Effort", selection: Binding(get: { thread?.effort ?? "" }, set: { store.setEffort($0.isEmpty ? nil : $0, of: threadID) })) {
                    Text("Default").tag("")
                    ForEach(efforts, id: \.self) { e in Text(Self.effortName(e)).tag(e) }
                }
            } label: {
                part(thread?.effort.map(Self.effortName) ?? "Default effort")
            }
            .accessibilityLabel("Effort")
            dot
            let mode = store.project(of: threadID)?.mode ?? "auto"
            Menu {
                Picker("Permissions", selection: Binding(get: { mode }, set: { store.setMode($0, of: threadID) })) {
                    ForEach(store.modes.isEmpty ? ["auto", "ask", "acceptEdits", "plan", "bypass"] : store.modes, id: \.self) { m in
                        Text(Self.modeName(m)).tag(m)
                    }
                }
            } label: {
                part(Self.modeName(mode))
            }
            .accessibilityLabel("Permissions")
            Spacer(minLength: 0)
        }
        .font(.footnote)
        .foregroundStyle(.secondary)
        .padding(.horizontal, 14)
    }

    private var dot: some View {
        Text("·").foregroundStyle(.tertiary).accessibilityHidden(true)
    }

    /// A setting's name, a small chevron saying it opens a menu; no background.
    private func part(_ text: String) -> some View {
        HStack(spacing: 2) {
            Text(text).lineLimit(1)
            Image(systemName: "chevron.up.chevron.down").font(.system(size: 8, weight: .semibold)).foregroundStyle(.tertiary)
        }
        .padding(.vertical, 4)
        .contentShape(.rect)
    }

    static func short(_ model: String) -> String { model.replacingOccurrences(of: "claude-", with: "").capitalized }

    static func effortName(_ e: String) -> String {
        switch e {
        case "low": "Low"
        case "medium": "Medium"
        case "high": "High"
        case "xhigh": "Extra High"
        case "max": "Max"
        default: e.capitalized
        }
    }

    static func modeName(_ m: String) -> String {
        switch m {
        case "auto": "Auto"
        case "ask", "default": "Ask"
        case "acceptEdits": "Accept Edits"
        case "plan": "Plan"
        case "bypass", "bypassPermissions": "Bypass"
        default: m
        }
    }
}

/// What the run is doing, and how full the context is.
private struct StatusLine: View {
    let activity: String?
    let tokens: Int?
    let window: Int?

    var body: some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text(activity ?? "Working…")
                .font(.footnote)
                .foregroundStyle(.secondary)
                .lineLimit(1)
            Spacer(minLength: 0)
            if let tokens, let window, window > 0 {
                Text("\(min(100, tokens * 100 / window))% context")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.tertiary)
            }
        }
        .padding(.horizontal, 6)
        .accessibilityElement(children: .combine)
    }
}

/// Your message waiting for Claude's current step.
private struct QueuedRow: View {
    let queued: Queued
    let threadID: String
    @Environment(Store.self) private var store

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "clock").foregroundStyle(.secondary)
            Text(queued.text).font(.subheadline).lineLimit(2)
            Spacer(minLength: 0)
            Button("Send Now", systemImage: "bolt.fill") { store.sendNow(queued, in: threadID) }
                .labelStyle(.iconOnly)
                .accessibilityLabel("Send now")
            Button("Remove", systemImage: "xmark") { store.withdraw(queued, in: threadID) }
                .labelStyle(.iconOnly)
                .foregroundStyle(.secondary)
        }
        .buttonStyle(.borderless)
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .glassEffect(in: .rect(cornerRadius: 16))
    }
}

/// A card's frame: thick enough to read over the transcript scrolling beneath it.
private struct CardFrame<Content: View>: View {
    let title: String
    let symbol: String
    var trailing: String?
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline) {
                Label(title, systemImage: symbol).font(.headline)
                Spacer()
                if let trailing { Text(trailing).font(.caption).foregroundStyle(.secondary) }
            }
            content
        }
        .padding(16)
        .background(.thickMaterial, in: .rect(cornerRadius: 22))
        .shadow(color: .black.opacity(0.12), radius: 14, y: 4)
    }
}

/// Buttons sharing a card's width equally.
@ViewBuilder private func choice(_ title: String, prominent: Bool, disabled: Bool = false, action: @escaping () -> Void) -> some View {
    if prominent {
        Button(action: action) { Text(title).fontWeight(.semibold).frame(maxWidth: .infinity) }
            .buttonStyle(.glassProminent)
            .disabled(disabled)
    } else {
        Button(action: action) { Text(title).frame(maxWidth: .infinity) }
            .buttonStyle(.glass)
            .disabled(disabled)
    }
}

/// Markdown inline (bold, code, links), headings as bold lines: enough for a plan in a card.
private func inlineMarkdown(_ s: String) -> AttributedString {
    let lines = s.split(separator: "\n", omittingEmptySubsequences: false).map { line -> String in
        let trimmed = line.drop { $0 == "#" }
        guard trimmed.count < line.count, trimmed.first == " " else { return String(line) }
        return "**\(trimmed.trimmingCharacters(in: .whitespaces))**"
    }
    let text = lines.joined(separator: "\n")
    return (try? AttributedString(markdown: text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(text)
}

/// A request waiting on you, as the Mac's card shows it: a tool to allow, Claude's question, a plan.
struct RequestCard: View {
    let request: Request
    let count: Int
    let threadID: String

    var body: some View {
        Group {
            switch request.tool {
            case "AskUserQuestion": QuestionCard(request: request, count: count, threadID: threadID)
            case "ExitPlanMode": PlanCard(request: request, count: count, threadID: threadID)
            default: PermissionCard(request: request, count: count, threadID: threadID)
            }
        }
        .sensoryFeedback(.warning, trigger: request.id)
    }
}

/// A tool to allow or deny.
private struct PermissionCard: View {
    let request: Request
    let count: Int
    let threadID: String
    @Environment(Store.self) private var store

    var body: some View {
        CardFrame(title: "Allow \(ToolSummary.displayName(request.tool))?", symbol: "hand.raised", trailing: count > 1 ? "1 of \(count)" : nil) {
            VStack(alignment: .leading, spacing: 6) {
                let target = ToolSummary.target(tool: request.tool, input: request.input)
                if !request.description.isEmpty, !(target?.hasSuffix(request.description) ?? false) {
                    Text(request.description).font(.subheadline).lineLimit(3)
                }
                if let target {
                    Text(target)
                        .font(.footnote.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(4)
                        .padding(8)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(.quaternary.opacity(0.5), in: .rect(cornerRadius: 8))
                }
            }
            HStack(spacing: 10) {
                choice("Deny", prominent: false) { store.respond(request, allow: false, in: threadID) }
                choice("Allow", prominent: true) { store.respond(request, allow: true, in: threadID) }
            }
            .controlSize(.large)
        }
    }
}

/// One of Claude's questions, as `AskUserQuestion` asks it.
struct AskedQuestion: Identifiable {
    let id: Int
    let question: String
    let header: String?
    let multi: Bool
    let options: [(label: String, description: String?)]

    static func parse(_ input: String) -> [AskedQuestion] {
        guard let object = (try? JSONSerialization.jsonObject(with: Data(input.utf8))) as? [String: Any],
              let list = object["questions"] as? [[String: Any]] else { return [] }
        return list.enumerated().compactMap { i, q in
            guard let question = q["question"] as? String else { return nil }
            let options: [(label: String, description: String?)] = (q["options"] as? [[String: Any]] ?? []).compactMap { o in
                guard let label = o["label"] as? String else { return nil }
                let description = (o["description"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
                return (label, description.flatMap { $0.isEmpty || $0.caseInsensitiveCompare(label) == .orderedSame ? nil : $0 })
            }
            return AskedQuestion(id: i, question: question, header: q["header"] as? String, multi: q["multiSelect"] as? Bool ?? false, options: options)
        }
    }

    /// Each question's text, and its options chosen (in their order) and what was written, joined.
    static func answers(_ questions: [AskedQuestion], chosen: [Int: Set<Int>], written: [Int: String]) -> [String: String] {
        var out: [String: String] = [:]
        for q in questions {
            var parts = chosen[q.id, default: []].sorted().compactMap { q.options.indices.contains($0) ? q.options[$0].label : nil }
            let other = (written[q.id] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            if !other.isEmpty { parts.append(other) }
            out[q.question] = parts.joined(separator: ", ")
        }
        return out
    }
}

/// Claude asks: pick options (one, or several where it allows) or write your own answer;
/// Answer once every question has one, or Skip.
private struct QuestionCard: View {
    let request: Request
    let count: Int
    let threadID: String
    @Environment(Store.self) private var store
    @State private var chosen: [Int: Set<Int>] = [:]
    @State private var written: [Int: String] = [:]

    var body: some View {
        let questions = AskedQuestion.parse(request.input)
        CardFrame(title: questions.count > 1 ? "Claude asks \(questions.count) questions" : "Claude asks", symbol: "questionmark.bubble", trailing: count > 1 ? "1 of \(count)" : nil) {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    ForEach(questions) { q in question(q) }
                }
            }
            .frame(maxHeight: 300)
            .scrollBounceBehavior(.basedOnSize)
            HStack(spacing: 10) {
                choice("Skip", prominent: false) { store.skip(request, in: threadID) }
                choice("Answer", prominent: true, disabled: !questions.allSatisfy(answered)) {
                    store.answer(request, answers: AskedQuestion.answers(questions, chosen: chosen, written: written), in: threadID)
                }
            }
            .controlSize(.large)
        }
    }

    @ViewBuilder private func question(_ q: AskedQuestion) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            if let header = q.header {
                Text(header.uppercased()).font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
            }
            Text(q.question).font(.subheadline.weight(.semibold))
            ForEach(Array(q.options.enumerated()), id: \.offset) { oi, option in
                let on = chosen[q.id, default: []].contains(oi)
                Button {
                    toggle(q, oi)
                } label: {
                    HStack(alignment: .top, spacing: 10) {
                        Marker(on: on, multi: q.multi).padding(.top, 1)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(option.label).font(.subheadline)
                            if let d = option.description { Text(d).font(.caption).foregroundStyle(.secondary) }
                        }
                        Spacer(minLength: 0)
                    }
                    .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(on ? .isSelected : [])
            }
            TextField(q.options.isEmpty ? "Your answer" : "Or write your own", text: Binding(
                get: { written[q.id] ?? "" },
                set: { text in
                    written[q.id] = text
                    // A written answer replaces a single choice.
                    if !q.multi, !text.trimmingCharacters(in: .whitespaces).isEmpty { chosen[q.id] = [] }
                }), axis: .vertical)
                .font(.subheadline)
                .lineLimit(1...3)
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
                .background(.quaternary.opacity(0.5), in: .rect(cornerRadius: 10))
        }
    }

    private func toggle(_ q: AskedQuestion, _ option: Int) {
        var set = chosen[q.id, default: []]
        if q.multi {
            if set.contains(option) { set.remove(option) } else { set.insert(option) }
        } else {
            set = set.contains(option) ? [] : [option]
            written[q.id] = ""
        }
        chosen[q.id] = set
    }

    private func answered(_ q: AskedQuestion) -> Bool {
        !chosen[q.id, default: []].isEmpty || !(written[q.id] ?? "").trimmingCharacters(in: .whitespaces).isEmpty
    }
}

/// An option's mark: a circle for one choice, a box for several; filled when chosen.
private struct Marker: View {
    let on: Bool
    let multi: Bool

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: multi ? 5 : 10, style: .continuous)
        ZStack {
            shape.fill(on ? Color.accentColor : Color.clear)
            shape.strokeBorder(on ? Color.accentColor : Color(uiColor: .tertiaryLabel), lineWidth: 1.5)
            if on {
                Image(systemName: "checkmark").font(.system(size: 11, weight: .bold)).foregroundStyle(.white)
            }
        }
        .frame(width: 20, height: 20)
    }
}

/// Claude's plan: approve it into a permission mode (which becomes the project's), or keep
/// planning with what to change.
private struct PlanCard: View {
    let request: Request
    let count: Int
    let threadID: String
    @Environment(Store.self) private var store
    @State private var feedback = ""

    private var plan: String {
        let object = (try? JSONSerialization.jsonObject(with: Data(request.input.utf8))) as? [String: Any]
        return object?["plan"] as? String ?? request.description
    }

    /// Auto when the thread's model can use it, else Accept Edits.
    private var recommended: String {
        let model = store.models.first { $0.id == store.threads[threadID]?.model }
        return (model?.auto ?? true) ? "auto" : "acceptEdits"
    }

    var body: some View {
        CardFrame(title: "Plan ready", symbol: "list.bullet.clipboard", trailing: count > 1 ? "1 of \(count)" : nil) {
            ScrollView {
                Text(inlineMarkdown(plan))
                    .font(.subheadline)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled)
            }
            .frame(maxHeight: 220)
            .scrollBounceBehavior(.basedOnSize)
            TextField("Feedback for Claude (optional)", text: $feedback, axis: .vertical)
                .font(.subheadline)
                .lineLimit(1...3)
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
                .background(.quaternary.opacity(0.5), in: .rect(cornerRadius: 10))
            HStack(spacing: 10) {
                choice("Keep Planning", prominent: false) { store.keepPlanning(request, feedback: feedback, in: threadID) }
                Menu {
                    Button("Approve, Auto") { store.approvePlan(request, mode: "auto", feedback: feedback, in: threadID) }
                    Button("Approve, Accept Edits") { store.approvePlan(request, mode: "acceptEdits", feedback: feedback, in: threadID) }
                    Button("Approve, Ask") { store.approvePlan(request, mode: "ask", feedback: feedback, in: threadID) }
                } label: {
                    Text("Approve").fontWeight(.semibold).frame(maxWidth: .infinity)
                } primaryAction: {
                    store.approvePlan(request, mode: recommended, feedback: feedback, in: threadID)
                }
                .buttonStyle(.glassProminent)
            }
            .controlSize(.large)
        }
    }
}

/// An MCP server's question mid-call: a form accepted with its defaults, or a page to visit.
private struct MCPQuestionCard: View {
    let question: MCPQuestion
    let threadID: String
    @Environment(Store.self) private var store

    var body: some View {
        CardFrame(title: "\(question.server) asks", symbol: "questionmark.bubble") {
            Text(question.message).font(.subheadline)
            if question.mode == "url", let url = question.url.flatMap(URL.init(string:)) {
                Link(destination: url) {
                    Label(url.host() ?? "Open the page", systemImage: "safari")
                        .font(.footnote)
                }
            }
            HStack(spacing: 10) {
                choice("Decline", prominent: false) { store.answerQuestion(question, accept: false, in: threadID) }
                choice(question.mode == "url" ? "Done" : "Accept", prominent: true) { store.answerQuestion(question, accept: true, in: threadID) }
            }
            .controlSize(.large)
        }
    }
}
