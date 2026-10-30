import Foundation
import UniformTypeIdentifiers

/// A file as the viewer shows it, read off the main thread: its text and where each line
/// starts, or why there's no text to show.
public struct FileSnapshot: Sendable {
    public enum Kind: Sendable, Equatable {
        case text
        case image
        case binary
        case folder
        case missing
        case unreadable(String)
    }

    public var kind: Kind
    public var text = ""
    /// UTF-16 offset of each line's start: the first is 0.
    public var lineStarts: [Int] = [0]
    /// Only the start of a big file is shown.
    public var isTruncated = false
    /// Lines past `FileReader.lineLimit` end early: a minified file's one line.
    public var hasCutLines = false
    public var size: Int64 = 0
    public var modified: Date?
    /// For `SyntaxHighlighter`; empty when it's plain text.
    public var language = ""

    public var lineCount: Int { lineStarts.count }

    /// The line a UTF-16 offset is on, from 1.
    public func line(at offset: Int) -> Int {
        var low = 0, high = lineStarts.count - 1
        while low < high {
            let mid = (low + high + 1) / 2
            if lineStarts[mid] <= offset { low = mid } else { high = mid - 1 }
        }
        return low + 1
    }

    /// Lines `line...endLine` (from 1) as a UTF-16 range, without the last line's break.
    public func range(ofLines line: Int, through endLine: Int? = nil) -> NSRange {
        let first = min(max(line, 1), lineStarts.count) - 1
        let last = min(max(endLine ?? line, line), lineStarts.count) - 1
        let start = lineStarts[first]
        let length = (text as NSString).length
        var end = last + 1 < lineStarts.count ? lineStarts[last + 1] : length
        if end > start, end <= length, (text as NSString).character(at: end - 1) == 10 { end -= 1 }
        return NSRange(location: start, length: max(end - start, 0))
    }
}

public enum FileReader {
    /// Past this, the start of the file is shown.
    public static let textLimit = 8 * 1024 * 1024
    /// Past this, the text is shown plain: highlighting would cost more than it shows.
    public static let highlightLimit = 1024 * 1024
    public static let highlightLineLimit = 20_000
    /// Past this a line is cut: AppKit lays out a line whole, however little of it shows, and
    /// a 2 MB line held the main thread for a second.
    public static let lineLimit = 5_000

    public static func read(_ url: URL, limit: Int = textLimit) -> FileSnapshot {
        let keys: Set<URLResourceKey> = [.isDirectoryKey, .fileSizeKey, .contentModificationDateKey]
        guard let values = try? url.resourceValues(forKeys: keys) else { return FileSnapshot(kind: .missing) }
        var snapshot = FileSnapshot(kind: .text)
        snapshot.size = Int64(values.fileSize ?? 0)
        snapshot.modified = values.contentModificationDate
        if values.isDirectory == true {
            snapshot.kind = .folder
            return snapshot
        }
        if let type = UTType(filenameExtension: url.pathExtension), type.conforms(to: .image), type != .svg {
            snapshot.kind = .image
            return snapshot
        }
        let data: Data
        do {
            data = try Data(contentsOf: url, options: .mappedIfSafe)
        } catch {
            snapshot.kind = .unreadable(error.localizedDescription)
            return snapshot
        }
        if data.prefix(8_000).contains(0) {
            snapshot.kind = .binary
            return snapshot
        }
        var shown = data
        if data.count > limit {
            // Cut at a line break, so no character is cut in half.
            let head = data.prefix(limit)
            let cut = head.lastIndex(of: 10).map { $0 + 1 } ?? head.endIndex
            shown = data.prefix(upTo: cut)
            snapshot.isTruncated = true
        }
        let decoded = String(data: shown, encoding: .utf8) ?? String(data: shown, encoding: .isoLatin1) ?? ""
        snapshot.text = cuttingLongLines(decoded, wasCut: &snapshot.hasCutLines)
        var starts = [0]
        var offset = 0
        for unit in snapshot.text.utf16 {
            offset += 1
            if unit == 10 { starts.append(offset) }
        }
        // A last line break doesn't start another line.
        if starts.count > 1, starts.last == offset { starts.removeLast() }
        snapshot.lineStarts = starts
        snapshot.language = language(for: url.lastPathComponent)
        return snapshot
    }

    /// The text with each line past the limit ended by "…". Most files have none, and come
    /// back as they are.
    static func cuttingLongLines(_ text: String, limit: Int = lineLimit, wasCut: inout Bool) -> String {
        var isLong = false
        var run = 0
        for byte in text.utf8 {
            if byte == 10 {
                run = 0
            } else {
                run += 1
                // Bytes are at least characters' worth; the second pass counts exactly.
                if run > limit { isLong = true; break }
            }
        }
        guard isLong else { return text }
        var result = ""
        result.reserveCapacity(text.utf8.count / 2)
        var didCut = false
        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            if !result.isEmpty || line.startIndex != text.startIndex { result.append("\n") }
            if line.utf16.count > limit {
                let end = line.index(line.startIndex, offsetBy: limit, limitedBy: line.endIndex) ?? line.endIndex
                result.append(contentsOf: line[..<end])
                result.append("…")
                didCut = true
            } else {
                result.append(contentsOf: line)
            }
        }
        wasCut = didCut
        return didCut ? result : text
    }

    /// The highlighter's language for a file name: its extension, or what the name says.
    public static func language(for name: String) -> String {
        let lowered = name.lowercased()
        switch lowered {
        case "dockerfile", "containerfile": return "dockerfile"
        case ".zshrc", ".bashrc", ".zprofile", ".bash_profile", ".profile": return "sh"
        case ".gitconfig", ".editorconfig", ".env": return "ini"
        case "package.resolved": return "json"
        default: break
        }
        if lowered.hasPrefix("dockerfile.") || lowered.hasSuffix(".dockerfile") { return "dockerfile" }
        if lowered.hasPrefix(".env.") { return "ini" }
        let ext = (lowered as NSString).pathExtension
        return SyntaxHighlighter.highlights(ext) ? ext : ""
    }
}
