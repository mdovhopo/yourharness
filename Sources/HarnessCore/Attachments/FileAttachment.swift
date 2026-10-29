import Foundation

/// Any file or folder sent with a message that isn't an image the model sees inline: a PDF,
/// a log, a CSV, a spreadsheet, a project folder. Claude gets its path and reads it with its
/// own tools, the way Claude Code does with a path dragged into the terminal: whatever its
/// Read tool takes (text, code, PDFs, notebooks, images) or a command can open.
public struct FileAttachment: Sendable, Equatable, Identifiable {
    public let id: UUID
    /// Where it was picked from.
    public let url: URL
    public let isDirectory: Bool
    /// Bytes; nil for a folder.
    public let size: Int64?

    public var name: String { url.lastPathComponent }

    /// Bigger files, and folders, are referred to where they are rather than copied.
    public static let maxCopyBytes: Int64 = 200_000_000

    public var isCopied: Bool { !isDirectory && (size ?? 0) <= Self.maxCopyBytes }

    public init(id: UUID = UUID(), url: URL, isDirectory: Bool, size: Int64?) {
        self.id = id
        self.url = url
        self.isDirectory = isDirectory
        self.size = size
    }

    public init?(contentsOf url: URL) {
        guard let values = try? url.resourceValues(forKeys: [.isDirectoryKey, .fileSizeKey, .isPackageKey]) else { return nil }
        // A package (an app, a bundle) is one thing to you, a folder to Claude.
        let isDirectory = values.isDirectory == true
        self.init(url: url, isDirectory: isDirectory, size: isDirectory ? nil : Int64(values.fileSize ?? 0))
    }

    /// What follows your message for Claude: where each one is. In a tag of its own, so
    /// the session's history gives back the message as you typed it.
    public static func note(for paths: [(path: String, isDirectory: Bool)]) -> String {
        guard !paths.isEmpty else { return "" }
        let lines = paths.map { "- \($0.path)\($0.isDirectory ? "/ (folder)" : "")" }
        return "<\(tag)>\nAttached (open them with Read, which takes text, PDFs and images; use a command only for other formats):\n" + lines.joined(separator: "\n") + "\n</\(tag)>"
    }

    public static let tag = "attached-files"
}
