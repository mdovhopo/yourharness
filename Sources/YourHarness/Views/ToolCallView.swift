import SwiftUI

/// How a tool call reads at a glance: an icon, a verb, and the one argument that matters.
struct ToolPresentation {
    let symbol: String
    let verb: String
    let detail: String
    let input: [String: Any]

    init(name: String, inputJSON: String, result: String? = nil) {
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
        case "ToolSearch" where string("query").hasPrefix("select:") && string("query").contains(BrowserCall.prefix):
            // Claude Code loads MCP tools when they're first needed.
            (symbol, verb, detail) = ("safari", "Load browser tools", "")
        case let name where name.hasPrefix(BrowserCall.prefix):
            (symbol, verb, detail) = BrowserCall.presentation(String(name.dropFirst(BrowserCall.prefix.count)), input: input, result: result)
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
    @Environment(\.findHighlight) private var findHighlight

    var body: some View {
        let presentation = ToolPresentation(name: entry.toolName, inputJSON: entry.toolInput, result: entry.isStreaming ? nil : entry.toolResult)
        VStack(alignment: .leading, spacing: 0) {
            Button {
                withAnimation(.snappy) { expanded.toggle() }
            } label: {
                header(presentation)
            }
            .buttonStyle(.plain)

            if entry.toolName == BrowserCall.prefix + "screenshot", !entry.toolIsError, !expanded {
                BrowserShotThumbnail(toolUseID: entry.id)
                    .padding([.horizontal, .bottom], 12)
            }
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
            Text(AttributedString(presentation.detail).highlighted(findHighlight))
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
            if entry.toolName == BrowserCall.prefix + "screenshot", !entry.toolIsError {
                BrowserShotThumbnail(toolUseID: entry.id, large: true)
            }
            if entry.toolName.hasPrefix(BrowserCall.prefix) {
                ShowBrowserButton()
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

/// Browser tool calls (`mcp__browser__*`), read as steps: "Click button "Sign in"".
enum BrowserCall {
    static let prefix = "mcp__browser__"

    static func presentation(_ tool: String, input: [String: Any], result: String?) -> (String, String, String) {
        let string = { (key: String) in input[key] as? String ?? "" }
        // The result's first line says what was hit: `Clicked button "Sign in".`
        let done = result.flatMap { $0.split(separator: "\n").first.map(String.init) }
        let target = { (verb: String) -> String in
            if let done, let range = done.range(of: verb + " ") {
                return String(done[range.upperBound...]).trimmingCharacters(in: CharacterSet(charactersIn: "."))
            }
            return [string("label"), string("selector"), string("ref")].first { !$0.isEmpty } ?? ""
        }
        switch tool {
        case "open": return ("safari", "Open", string("url"))
        case "navigate": return ("arrow.right.circle", string("action").isEmpty ? "Go to" : string("action").capitalized, string("url"))
        case "snapshot": return ("list.bullet.indent", "Read the page", string("selector"))
        case "screenshot": return ("camera.viewfinder", "Screenshot", input["full_page"] as? Bool == true ? "full page" : target("of"))
        case "click": return ("cursorarrow.click", "Click", target("Clicked"))
        case "hover": return ("cursorarrow", "Hover", target("over"))
        // A password field says so in the result, which then gives only the length.
        case "type": return ("keyboard", "Type", done?.contains(" characters into ") == true ? "••••••" : string("text"))
        case "press": return ("keyboard", "Press", string("key"))
        case "select": return ("filemenu.and.selection", "Select", (input["values"] as? [String] ?? []).joined(separator: ", "))
        case "scroll": return ("scroll", "Scroll", string("to").isEmpty ? string("direction") : string("to"))
        case "drag": return ("hand.draw", "Drag", "")
        case "wait_for": return ("hourglass", "Wait for", [string("text"), string("selector"), string("url"), string("gone")].first { !$0.isEmpty } ?? "")
        case "console": return ("terminal", "Read the console", string("level"))
        case "network": return ("network", "Read network requests", input["failed_only"] as? Bool == true ? "failed" : "")
        case "evaluate": return ("curlybraces", "Run JavaScript", string("expression"))
        case "resize": return ("rectangle.expand.diagonal", "Resize", string("preset").isEmpty ? [input["width"], input["height"]].compactMap { ($0 as? NSNumber)?.stringValue }.joined(separator: "×") : string("preset"))
        case "dialog": return ("questionmark.bubble", input["accept"] as? Bool == false ? "Dismiss dialog" : "Accept dialog", "")
        case "upload": return ("square.and.arrow.up", "Upload", (input["paths"] as? [String] ?? []).map { ($0 as NSString).lastPathComponent }.joined(separator: ", "))
        case "close": return ("xmark.square", "Close tab", "")
        case "status": return ("safari", "Check the browser", "")
        default: return ("safari", tool, "")
        }
    }
}

/// A screenshot the agent took, from where the app kept it.
private struct BrowserShotThumbnail: View {
    @Environment(AppModel.self) private var app
    @Environment(ThreadModel.self) private var thread: ThreadModel?
    let toolUseID: String
    var large = false
    @State private var image: NSImage?

    var body: some View {
        // Not an empty Group: with nothing in it, the task below would never start.
        ZStack {
            if let image {
                Image(nsImage: image)
                    .resizable()
                    .aspectRatio(contentMode: .fit)
                    .frame(maxWidth: large ? .infinity : 260, maxHeight: large ? 480 : 160, alignment: .leading)
                    .clipShape(.rect(cornerRadius: Theme.innerRadius))
                    .overlay(RoundedRectangle(cornerRadius: Theme.innerRadius).stroke(.separator))
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .task(id: toolUseID) {
            guard let thread else { return }
            let url = app.store.browserScreenshotURL(thread: thread.id, toolUseID: toolUseID)
            // It's written a moment after the call returns.
            for _ in 0..<10 {
                if let loaded = await Task.detached(operation: { NSImage(contentsOf: url) }).value {
                    image = loaded
                    return
                }
                try? await Task.sleep(for: .milliseconds(300))
            }
        }
    }
}

private struct ShowBrowserButton: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        Button("Show Browser", systemImage: "safari") { app.showInspector(.browser) }
            .buttonStyle(.glass)
            .controlSize(.small)
    }
}
