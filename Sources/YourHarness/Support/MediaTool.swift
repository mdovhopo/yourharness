import Foundation
import HarnessCore
import UniformTypeIdentifiers

/// `mcp__harness__show_media`: the agent puts images and videos in the conversation (a
/// screenshot it took, a screen recording, a chart it made). The files are copied into the
/// thread, so they stay after the originals go. It only shows you files, so it doesn't ask.
struct HarnessToolServer: MCPToolServer {
    let name = "harness"
    let tools = [MediaTool.definition]
    let media: MediaTool

    func call(_ tool: String, arguments: JSONValue, toolUseID: String?) async -> MCPToolResult {
        guard tool == MediaTool.definition.name else { return .error("No tool named \(tool).") }
        return await media.show(arguments, toolUseID: toolUseID)
    }
}

@MainActor
final class MediaTool {
    nonisolated static let toolName = "mcp__harness__show_media"
    nonisolated static let maxFiles = 10
    nonisolated static let maxBytes = 500 * 1024 * 1024

    nonisolated static let definition = MCPTool(
        name: "show_media",
        description: """
        Show the user images or videos right in the conversation: screenshots, screen recordings, \
        simulator captures, charts or images you generated. Give file paths, absolute or relative to your \
        working directory. The files are copied into the conversation, so they stay after the originals are \
        deleted. Images: png, jpg, gif, heic, webp, tiff, bmp. Videos: mp4, mov, m4v.
        """,
        inputSchema: MCPTool.schema([
            "paths": ["type": "array", "items": ["type": "string"], "minItems": 1, "maxItems": .number(Double(maxFiles)),
                      "description": "The files to show, in order."],
            "path": ["type": "string", "description": "One file to show, instead of `paths`."],
            "caption": ["type": "string", "description": "Optional: a line about what they show."],
        ]),
        readOnly: true
    )

    private weak var thread: ThreadModel?
    private unowned let app: AppModel

    init(thread: ThreadModel, app: AppModel) {
        self.thread = thread
        self.app = app
    }

    enum Kind { case image, video }

    /// What a path is, by its extension.
    nonisolated static func kind(of path: String) -> Kind? {
        guard let type = UTType(filenameExtension: (path as NSString).pathExtension.lowercased()) else { return nil }
        if type.conforms(to: .movie) || type.conforms(to: .video) { return .video }
        if type.conforms(to: .image) { return .image }
        return nil
    }

    /// Where the call's `index`th file is kept: named like attachments, so deleting the
    /// thread deletes it.
    nonisolated static func storedName(thread: UUID, toolUseID: String, index: Int, path: String) -> String {
        "\(thread.uuidString)-media-\(toolUseID)-\(index).\((path as NSString).pathExtension.lowercased())"
    }

    /// The paths a call asked for, from its input: `paths`, or a single `path`.
    nonisolated static func paths(inputJSON: String) -> [String] {
        let input = (try? JSONSerialization.jsonObject(with: Data(inputJSON.utf8))) as? [String: Any]
        if let paths = input?["paths"] as? [String] { return paths }
        if let path = (input?["path"] ?? input?["paths"]) as? String { return [path] }
        return []
    }

    func show(_ args: JSONValue, toolUseID: String?) async -> MCPToolResult {
        guard let thread else { return .error("This thread is gone.") }
        guard let toolUseID else { return .error("This call has no ID to keep its files under; try again.") }
        let paths = args["paths"]?.array?.compactMap(\.string) ?? [args["path"]?.string ?? args["paths"]?.string].compactMap(\.self)
        guard !paths.isEmpty else { return .error("Give the files to show in `paths`.") }
        guard paths.count <= Self.maxFiles else { return .error("At most \(Self.maxFiles) files at a time.") }
        let base = URL(fileURLWithPath: thread.effectiveDirectory, isDirectory: true)
        var problems: [String] = [], shown = 0
        for (index, path) in paths.enumerated() {
            let expanded = (path as NSString).expandingTildeInPath
            let url = expanded.hasPrefix("/") ? URL(fileURLWithPath: expanded) : base.appending(path: expanded)
            guard Self.kind(of: path) != nil else {
                problems.append("\(path): not an image or video this can show")
                continue
            }
            let size = (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? Int) ?? nil
            guard let size else {
                problems.append("\(path): no such file")
                continue
            }
            guard size <= Self.maxBytes else {
                problems.append("\(path): over \(Self.maxBytes / 1024 / 1024) MB")
                continue
            }
            let destination = app.store.attachmentURL(Self.storedName(thread: thread.id, toolUseID: toolUseID, index: index, path: path))
            let copied = await Task.detached {
                try? FileManager.default.removeItem(at: destination)
                // A clone on APFS: instant, and no extra space until one of them changes.
                return (try? FileManager.default.copyItem(at: url, to: destination)) != nil
            }.value
            if copied { shown += 1 } else { problems.append("\(path): couldn't be copied") }
        }
        if shown == 0 { return .error("Nothing shown. " + problems.joined(separator: "; ")) }
        var text = shown == 1 ? "Shown to the user." : "Shown to the user: \(shown) files."
        if !problems.isEmpty { text += " Not shown: " + problems.joined(separator: "; ") + "." }
        return .text(text)
    }
}
