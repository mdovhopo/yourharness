import CoreGraphics
import Foundation

/// A parsed Mermaid diagram of a kind we draw natively.
public enum MermaidDiagram: Sendable, Equatable {
    case flowchart(Flowchart)
    case sequence(SequenceDiagram)

    /// Whether we draw this kind at all; other Mermaid diagrams stay as code.
    public static func isSupported(_ source: String) -> Bool {
        guard let header = MermaidText.statements(source).first?.lowercased() else { return false }
        return header.hasPrefix("flowchart") || header.hasPrefix("graph") || header.hasPrefix("sequencediagram")
    }

    public static func parse(_ source: String) -> Result<MermaidDiagram, MermaidError> {
        let header = MermaidText.statements(stripFrontMatter(source)).first?.lowercased() ?? ""
        do {
            if header.hasPrefix("sequencediagram") {
                return .success(.sequence(try SequenceParser.parse(stripFrontMatter(source))))
            }
            if header.hasPrefix("flowchart") || header.hasPrefix("graph") {
                return .success(.flowchart(try FlowchartParser.parse(stripFrontMatter(source))))
            }
            return .failure(MermaidError("This kind of Mermaid diagram isn't drawn yet."))
        } catch let error as MermaidError {
            return .failure(error)
        } catch {
            return .failure(MermaidError(error.localizedDescription))
        }
    }

    /// `---\ntitle: …\n---` and `%%{init: …}%%` lines carry config we don't use.
    static func stripFrontMatter(_ source: String) -> String {
        var lines = source.components(separatedBy: .newlines)
        while let first = lines.first, first.trimmingCharacters(in: .whitespaces).isEmpty { lines.removeFirst() }
        if lines.first?.trimmingCharacters(in: .whitespaces) == "---",
           let close = lines.dropFirst().firstIndex(where: { $0.trimmingCharacters(in: .whitespaces) == "---" }) {
            lines.removeSubrange(0...close)
        }
        return lines.filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("%%{") }.joined(separator: "\n")
    }
}
