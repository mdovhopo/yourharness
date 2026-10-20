import SwiftUI

/// How a tool call reads at a glance: an icon, a verb, and the one argument that matters.
struct ToolPresentation {
    let symbol: String
    let verb: String
    let detail: String
    let input: [String: Any]

    init(name: String, inputJSON: String) {
        // Partial JSON while streaming won't parse yet; the header just fills in once it does.
        let input = (try? JSONSerialization.jsonObject(with: Data(inputJSON.utf8))) as? [String: Any] ?? [:]
        self.input = input
        let string = { (key: String) in input[key] as? String ?? "" }

        switch name {
        case "Bash":
            (symbol, verb, detail) = ("terminal", "Run command", string("command"))
        case "Read":
            (symbol, verb, detail) = ("doc.text", "Read", Self.shortPath(string("file_path")))
        case "Write":
            (symbol, verb, detail) = ("doc.badge.plus", "Write", Self.shortPath(string("file_path")))
        case "Edit", "MultiEdit":
            (symbol, verb, detail) = ("pencil", "Edit", Self.shortPath(string("file_path")))
        case "NotebookEdit":
            (symbol, verb, detail) = ("pencil", "Edit notebook", Self.shortPath(string("notebook_path")))
        case "Grep":
            (symbol, verb, detail) = ("magnifyingglass", "Search", string("pattern"))
        case "Glob":
            (symbol, verb, detail) = ("folder.badge.questionmark", "Find files", string("pattern"))
        case "WebFetch":
            (symbol, verb, detail) = ("globe", "Fetch", string("url"))
        case "WebSearch":
            (symbol, verb, detail) = ("magnifyingglass.circle", "Search the web", string("query"))
        case "Task", "Agent":
            (symbol, verb, detail) = ("person.2", "Subagent", string("description"))
        case "TodoWrite":
            (symbol, verb, detail) = ("checklist", "Update plan", "")
        default:
            let first = input.values.compactMap { $0 as? String }.first ?? ""
            (symbol, verb, detail) = ("wrench.and.screwdriver", name, first)
        }
    }

    private static func shortPath(_ path: String) -> String {
        let components = path.split(separator: "/")
        return components.count > 3 ? "…/" + components.suffix(3).joined(separator: "/") : path
    }
}

struct ToolCallView: View {
    let entry: TranscriptEntry
    @State private var expanded = false

    var body: some View {
        let presentation = ToolPresentation(name: entry.toolName, inputJSON: entry.toolInput)
        VStack(alignment: .leading, spacing: 0) {
            Button {
                withAnimation(.snappy) { expanded.toggle() }
            } label: {
                header(presentation)
            }
            .buttonStyle(.plain)

            if expanded {
                Divider()
                details(presentation)
                    .padding(12)
            }
        }
        .card(border: entry.toolIsError ? AnyShapeStyle(.red.opacity(0.5)) : nil)
    }

    private func header(_ presentation: ToolPresentation) -> some View {
        HStack(spacing: 10) {
            Image(systemName: presentation.symbol)
                .foregroundStyle(.secondary)
                .frame(width: 18)
            Text(presentation.verb)
                .fontWeight(.medium)
            Text(presentation.detail)
                .font(.callout.monospaced())
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 8)
            if entry.isStreaming {
                ProgressView()
                    .controlSize(.small)
            } else if entry.toolIsError {
                Image(systemName: "xmark.circle.fill")
                    .foregroundStyle(.red)
            }
            Image(systemName: "chevron.right")
                .font(.caption.weight(.semibold))
                .foregroundStyle(.tertiary)
                .rotationEffect(.degrees(expanded ? 90 : 0))
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .contentShape(.rect)
    }

    @ViewBuilder
    private func details(_ presentation: ToolPresentation) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            if let old = presentation.input["old_string"] as? String,
               let new = presentation.input["new_string"] as? String {
                DiffView(old: old, new: new)
            } else if entry.toolName == "Bash", let command = presentation.input["command"] as? String {
                CodeBox(text: "$ " + command)
            } else if !entry.toolInput.isEmpty {
                CodeBox(text: Self.prettyJSON(entry.toolInput))
            }
            if let result = entry.toolResult, !result.isEmpty {
                CodeBox(text: Self.clipped(result), tint: entry.toolIsError ? .red : nil)
            }
        }
    }

    /// Huge outputs (full file reads, long logs) would stall layout; show the head.
    private static func clipped(_ text: String, limit: Int = 20_000) -> String {
        guard text.count > limit else { return text }
        return String(text.prefix(limit)) + "\n… \(text.count - limit) more characters"
    }

    private static func prettyJSON(_ json: String) -> String {
        guard let object = try? JSONSerialization.jsonObject(with: Data(json.utf8)),
              let data = try? JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes])
        else { return json }
        return String(decoding: data, as: UTF8.self)
    }
}

struct CodeBox: View {
    let text: String
    var tint: Color?

    var body: some View {
        ScrollView([.vertical, .horizontal]) {
            Text(text)
                .font(.system(.callout, design: .monospaced))
                .foregroundStyle(tint.map { AnyShapeStyle($0) } ?? AnyShapeStyle(.primary))
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(10)
        }
        .frame(maxHeight: 320)
        .fixedSize(horizontal: false, vertical: true)
        .background(.background.opacity(0.6), in: .rect(cornerRadius: Theme.innerRadius))
    }
}

/// Minimal line diff for Edit calls: removed lines, then added lines.
private struct DiffView: View {
    let old: String
    let new: String

    var body: some View {
        let removed = old.split(separator: "\n", omittingEmptySubsequences: false)
        let added = new.split(separator: "\n", omittingEmptySubsequences: false)
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(removed.enumerated()), id: \.offset) { _, line in
                DiffLine(marker: "−", text: line, color: .red)
            }
            ForEach(Array(added.enumerated()), id: \.offset) { _, line in
                DiffLine(marker: "+", text: line, color: .green)
            }
        }
        .font(.system(.callout, design: .monospaced))
        .textSelection(.enabled)
        .clipShape(.rect(cornerRadius: Theme.innerRadius))
    }
}

private struct DiffLine: View {
    let marker: String
    let text: Substring
    let color: Color

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Text(marker)
                .foregroundStyle(color)
                .frame(width: 12)
            Text(text.isEmpty ? " " : String(text))
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 1)
        .background(color.opacity(0.1))
    }
}
