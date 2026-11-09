// The bottom of a thread: what waits on you (a tool to allow, a plan, Claude's question), your
// queued messages, what the run is doing, and the field you type in. Actions go to the Mac as
// its own buttons would take them; the rows they cause arrive like any others.

import SwiftUI

struct ComposerBar: View {
    let threadID: String
    @Environment(Store.self) private var store
    @Binding var draft: String
    @FocusState private var focused: Bool

    private var live: Live? { store.live }
    private var running: Bool { live?.running ?? store.threads[threadID]?.isRunning ?? false }

    var body: some View {
        VStack(spacing: 8) {
            if let request = live?.requests.first {
                RequestCard(request: request, count: live?.requests.count ?? 1, threadID: threadID)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
            }
            if let question = live?.questions.first {
                Label("\(question.server) asks: \(question.message) Answer it on your Mac.", systemImage: "questionmark.bubble")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 6)
            }
            ForEach(live?.queue ?? []) { queued in
                QueuedRow(queued: queued, threadID: threadID)
            }
            if running {
                StatusLine(activity: live?.activity, tokens: live?.contextTokens, window: live?.contextWindow)
            }
            if store.canAct {
                field
            } else {
                Text(store.serverProtocol == 1 ? "Read-only: this is the development server." : "Connecting to your Mac…")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .padding(.vertical, 6)
            }
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
        .padding(.bottom, 6)
        .animation(.snappy(duration: 0.25), value: live?.requests.first?.id)
        .onChange(of: store.returnedDraft?.text) { _, text in
            if let returned = store.returnedDraft, returned.thread == threadID, let text, draft.isEmpty {
                draft = text
                store.returnedDraft = nil
            }
        }
    }

    private var field: some View {
        HStack(alignment: .bottom, spacing: 8) {
            TextField(running ? "Message (waits for Claude's step)" : "Message", text: $draft, axis: .vertical)
                .lineLimit(1...6)
                .focused($focused)
                .padding(.horizontal, 14)
                .padding(.vertical, 9)
                .glassEffect(.regular.interactive(), in: .rect(cornerRadius: 20))
                .submitLabel(.send)
            if draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, running {
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
                    store.send(draft, to: threadID)
                    draft = ""
                } label: {
                    Image(systemName: "arrow.up")
                        .font(.system(size: 16, weight: .bold))
                        .frame(width: 38, height: 38)
                }
                .buttonStyle(.glassProminent)
                .buttonBorderShape(.circle)
                .disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                .accessibilityLabel("Send")
            }
        }
        .sensoryFeedback(.impact(weight: .light), trigger: running)
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

/// A request waiting on you, as the Mac's card shows it.
struct RequestCard: View {
    let request: Request
    let count: Int
    let threadID: String
    @Environment(Store.self) private var store

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline) {
                Label(title, systemImage: symbol)
                    .font(.headline)
                Spacer()
                if count > 1 {
                    Text("1 of \(count)").font(.caption).foregroundStyle(.secondary)
                }
            }
            content
            buttons
        }
        .padding(16)
        // Thick enough to read over the transcript scrolling beneath it.
        .background(.thickMaterial, in: .rect(cornerRadius: 22))
        .shadow(color: .black.opacity(0.12), radius: 14, y: 4)
        .sensoryFeedback(.warning, trigger: request.id)
    }

    private var input: [String: Any] {
        (try? JSONSerialization.jsonObject(with: Data(request.input.utf8))) as? [String: Any] ?? [:]
    }

    private var title: String {
        switch request.tool {
        case "ExitPlanMode": "Plan ready"
        case "AskUserQuestion": "Claude asks"
        default: "Allow \(ToolSummary.displayName(request.tool))?"
        }
    }

    private var symbol: String {
        switch request.tool {
        case "ExitPlanMode": "list.bullet.clipboard"
        case "AskUserQuestion": "questionmark.bubble"
        default: "hand.raised"
        }
    }

    @ViewBuilder private var content: some View {
        switch request.tool {
        case "ExitPlanMode":
            let plan = input["plan"] as? String ?? request.description
            ScrollView {
                Text(Self.markdown(plan))
                    .font(.subheadline)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(maxHeight: 240)
            .scrollBounceBehavior(.basedOnSize)
        case "AskUserQuestion":
            let questions = input["questions"] as? [[String: Any]] ?? []
            VStack(alignment: .leading, spacing: 6) {
                ForEach(Array(questions.prefix(3).enumerated()), id: \.offset) { _, q in
                    Text(q["question"] as? String ?? "").font(.subheadline)
                    let options = (q["options"] as? [[String: Any]] ?? []).compactMap { $0["label"] as? String }
                    if !options.isEmpty {
                        Text(options.map { "• \($0)" }.joined(separator: "\n"))
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    }
                }
                Text("Choose an answer on your Mac.")
                    .font(.footnote)
                    .foregroundStyle(.tertiary)
            }
        default:
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
        }
    }

    @ViewBuilder private var buttons: some View {
        HStack(spacing: 10) {
            switch request.tool {
            case "AskUserQuestion":
                choice("Decline", prominent: false) { store.respond(request, allow: false, in: threadID) }
            case "ExitPlanMode":
                choice("Keep Planning", prominent: false) { store.respond(request, allow: false, in: threadID) }
                choice("Approve", prominent: true) { store.respond(request, allow: true, in: threadID) }
            default:
                choice("Deny", prominent: false) { store.respond(request, allow: false, in: threadID) }
                choice("Allow", prominent: true) { store.respond(request, allow: true, in: threadID) }
            }
        }
        .controlSize(.large)
    }

    /// Buttons sharing the card's width equally.
    @ViewBuilder private func choice(_ title: String, prominent: Bool, action: @escaping () -> Void) -> some View {
        if prominent {
            Button(action: action) { Text(title).fontWeight(.semibold).frame(maxWidth: .infinity) }
                .buttonStyle(.glassProminent)
        } else {
            Button(action: action) { Text(title).frame(maxWidth: .infinity) }
                .buttonStyle(.glass)
        }
    }

    private static func markdown(_ s: String) -> AttributedString {
        (try? AttributedString(markdown: s, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(s)
    }
}
