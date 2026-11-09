// A row opened from the transcript: a tool call's input and result (an edit as a diff), or
// thinking in full. Drawn from what the row carries (input up to 4000 characters, the result's
// first 2000); the rest stays on the Mac.

import SwiftUI

struct RowDetail: View {
    let row: Row
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    if row.kind == "thinking" {
                        Text(row.text).font(.body).textSelection(.enabled)
                    } else {
                        ToolDetail(row: row)
                    }
                }
                .padding()
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .navigationTitle(row.kind == "thinking" ? "Thinking" : ToolSummary.displayName(row.toolName ?? "Tool"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }
}

private struct ToolDetail: View {
    let row: Row

    private var tool: String { row.toolName ?? "" }
    private var input: String { row.toolInput ?? "" }
    private var json: [String: Any] {
        (try? JSONSerialization.jsonObject(with: Data(input.utf8))) as? [String: Any] ?? [:]
    }
    /// A field from the input, by pattern when the input was cut short and isn't JSON any more.
    private func field(_ key: String) -> String? {
        json[key] as? String ?? ToolSummary.field(key, in: input)
    }

    var body: some View {
        if let target = ToolSummary.target(tool: tool, input: input) {
            Text(target).font(.subheadline.monospaced()).foregroundStyle(.secondary).textSelection(.enabled)
        }
        switch tool {
        case "Edit":
            DiffView(old: field("old_string") ?? "", new: field("new_string") ?? "")
        case "MultiEdit":
            let edits = json["edits"] as? [[String: Any]] ?? []
            ForEach(Array(edits.enumerated()), id: \.offset) { _, e in
                DiffView(old: e["old_string"] as? String ?? "", new: e["new_string"] as? String ?? "")
            }
        case "Write":
            Box(title: "Content", text: field("content") ?? "", tint: .green)
        case "Bash":
            Box(title: "Command", text: field("command") ?? "")
        default:
            Box(title: "Input", text: pretty(input))
        }
        if let result = row.toolResult, !result.isEmpty {
            Box(title: row.toolIsError == true ? "Failed" : "Result", text: result, tint: row.toolIsError == true ? .red : nil)
            if let length = row.toolResultLength, length > result.count {
                Text("The first \(result.count.formatted()) of \(length.formatted()) characters. The rest is on your Mac.")
                    .font(.footnote).foregroundStyle(.secondary)
            }
        } else if row.toolIsError == nil {
            Label("No result yet", systemImage: "hourglass").font(.footnote).foregroundStyle(.secondary)
        }
        if input.count >= 4000 {
            Text("The input is cut short here; the whole of it is on your Mac.").font(.footnote).foregroundStyle(.secondary)
        }
    }

    private func pretty(_ s: String) -> String {
        guard let object = try? JSONSerialization.jsonObject(with: Data(s.utf8)),
              let data = try? JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .withoutEscapingSlashes, .sortedKeys])
        else { return s }
        return String(decoding: data, as: UTF8.self)
    }
}

/// Monospaced text in a box that scrolls sideways.
private struct Box: View {
    let title: String
    let text: String
    var tint: Color?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.caption).foregroundStyle(tint ?? .secondary)
            ScrollView(.horizontal, showsIndicators: false) {
                Text(text.isEmpty ? " " : text)
                    .font(.footnote.monospaced())
                    .textSelection(.enabled)
                    .padding(12)
            }
            .background(.quaternary.opacity(0.5), in: .rect(cornerRadius: 10))
        }
    }
}

/// An edit as lines removed and added, the unchanged ones between them dimmed.
struct DiffView: View {
    let old: String
    let new: String
    @State private var width: CGFloat = 0

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(Diff.lines(old: old, new: new).enumerated()), id: \.offset) { _, line in
                    HStack(spacing: 8) {
                        Text(line.mark).foregroundStyle(line.color).frame(width: 10)
                        Text(line.text.isEmpty ? " " : line.text).foregroundStyle(line.mark == " " ? .secondary : .primary)
                    }
                    .font(.footnote.monospaced())
                    .padding(.horizontal, 10)
                    .padding(.vertical, 1)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(line.color.opacity(line.mark == " " ? 0 : 0.14))
                }
            }
            .padding(.vertical, 8)
            // At least the box's width, so a line's color runs edge to edge.
            .frame(minWidth: width, alignment: .leading)
        }
        .onGeometryChange(for: CGFloat.self, of: { $0.size.width }) { width = $0 }
        .background(.quaternary.opacity(0.35), in: .rect(cornerRadius: 10))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Edit: \(old.split(separator: "\n").count) lines replaced by \(new.split(separator: "\n").count)")
    }
}

enum Diff {
    struct Line {
        let mark: String
        let text: String
        var color: Color { mark == "-" ? .red : mark == "+" ? .green : .clear }
    }

    /// A line diff by longest common subsequence; past a few hundred lines, all removed then all
    /// added, which is what an edit that size reads as anyway.
    static func lines(old: String, new: String) -> [Line] {
        let a = old.components(separatedBy: "\n"), b = new.components(separatedBy: "\n")
        guard a.count * b.count <= 160_000 else {
            return a.map { Line(mark: "-", text: $0) } + b.map { Line(mark: "+", text: $0) }
        }
        var lcs = Array(repeating: Array(repeating: 0, count: b.count + 1), count: a.count + 1)
        for i in stride(from: a.count - 1, through: 0, by: -1) {
            for j in stride(from: b.count - 1, through: 0, by: -1) {
                lcs[i][j] = a[i] == b[j] ? lcs[i + 1][j + 1] + 1 : max(lcs[i + 1][j], lcs[i][j + 1])
            }
        }
        var out: [Line] = []
        var i = 0, j = 0
        while i < a.count || j < b.count {
            if i < a.count, j < b.count, a[i] == b[j] {
                out.append(Line(mark: " ", text: a[i])); i += 1; j += 1
            } else if j < b.count, i == a.count || lcs[i][j + 1] >= lcs[i + 1][j] {
                out.append(Line(mark: "+", text: b[j])); j += 1
            } else {
                out.append(Line(mark: "-", text: a[i])); i += 1
            }
        }
        return out
    }
}
