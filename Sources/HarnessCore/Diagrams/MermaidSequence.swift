import CoreGraphics
import Foundation

/// A Mermaid `sequenceDiagram`: participants, messages, notes, activations and blocks
/// (`loop`, `alt`/`else`, `opt`, `par`/`and`, `critical`/`option`, `break`, `rect`).
public struct SequenceDiagram: Sendable, Equatable {
    public struct Participant: Sendable, Equatable, Identifiable {
        public var id: String
        public var label: String
        public var isActor: Bool
    }

    public enum LineStyle: Sendable, Equatable {
        case solid, dotted
    }

    public enum ArrowHead: Sendable, Equatable {
        /// `->` / `-->`
        case none
        /// `->>` / `-->>`
        case filled
        /// `-x` / `--x`
        case cross
        /// `-)` / `--)`: async
        case open
    }

    public struct Message: Sendable, Equatable {
        public var from: String
        public var to: String
        public var text: String
        public var line: LineStyle
        public var head: ArrowHead
        /// `<<->>`
        public var bidirectional: Bool
        public var activatesTarget: Bool
        public var deactivatesSource: Bool
    }

    public enum NotePlacement: Sendable, Equatable {
        case leftOf, rightOf, over
    }

    public struct Note: Sendable, Equatable {
        public var placement: NotePlacement
        public var participants: [String]
        public var text: String
    }

    public struct Block: Sendable, Equatable {
        public enum Kind: String, Sendable, Equatable {
            case loop, alt, opt, par, critical, `break`, rect
        }

        public struct Section: Sendable, Equatable {
            public var label: String
            public var items: [Item]
        }

        public var kind: Kind
        public var sections: [Section]
    }

    public indirect enum Item: Sendable, Equatable {
        case message(Message)
        case note(Note)
        case activate(String)
        case deactivate(String)
        case block(Block)
    }

    public var participants: [Participant]
    public var items: [Item]
    public var autonumber: Bool
    public var title: String?
}

public enum SequenceParser {
    public static func parse(_ source: String) throws -> SequenceDiagram {
        var lines = MermaidText.statements(source)
        guard lines.first?.lowercased().hasPrefix("sequencediagram") == true else {
            throw MermaidError("Expected `sequenceDiagram`.")
        }
        lines.removeFirst()

        var participants: [SequenceDiagram.Participant] = []
        var known: Set<String> = []
        func ensure(_ id: String, label: String? = nil, actor: Bool = false) {
            if known.insert(id).inserted {
                participants.append(.init(id: id, label: label ?? id, isActor: actor))
            } else if let label, let index = participants.firstIndex(where: { $0.id == id }) {
                participants[index].label = label
                participants[index].isActor = participants[index].isActor || actor
            }
        }

        var autonumber = false
        var title: String?
        // Each open block collects its sections; the root collects top-level items.
        var stack: [(kind: SequenceDiagram.Block.Kind?, sections: [SequenceDiagram.Block.Section])] = [(nil, [.init(label: "", items: [])])]
        func append(_ item: SequenceDiagram.Item) {
            let last = stack.count - 1
            stack[last].sections[stack[last].sections.count - 1].items.append(item)
        }

        for line in lines {
            let lower = line.lowercased()
            let word = lower.split(separator: " ").first.map(String.init) ?? lower

            switch word {
            case "participant", "actor":
                let rest = line.dropFirst(word.count).trimmingCharacters(in: .whitespaces)
                if let range = rest.range(of: " as ") {
                    ensure(String(rest[..<range.lowerBound]).trimmingCharacters(in: .whitespaces),
                           label: MermaidText.clean(String(rest[range.upperBound...])), actor: word == "actor")
                } else {
                    ensure(rest, actor: word == "actor")
                }
            case "autonumber":
                autonumber = true
            case "title":
                title = MermaidText.clean(String(line.dropFirst(5)))
            case "activate", "deactivate":
                let id = line.dropFirst(word.count).trimmingCharacters(in: .whitespaces)
                ensure(id)
                append(word == "activate" ? .activate(id) : .deactivate(id))
            case "note":
                guard let note = parseNote(line) else { throw MermaidError("Couldn't read “\(line)”.") }
                note.participants.forEach { ensure($0) }
                append(.note(note))
            case "loop", "alt", "opt", "par", "critical", "break", "rect":
                let kind = SequenceDiagram.Block.Kind(rawValue: word)!
                let label = kind == .rect ? "" : MermaidText.clean(String(line.dropFirst(word.count)))
                stack.append((kind, [.init(label: label, items: [])]))
            case "else", "and", "option":
                guard stack.count > 1 else { throw MermaidError("`\(word)` outside a block.") }
                stack[stack.count - 1].sections.append(.init(label: MermaidText.clean(String(line.dropFirst(word.count))), items: []))
            case "end":
                guard stack.count > 1 else { throw MermaidError("`end` without a block.") }
                let closed = stack.removeLast()
                if let kind = closed.kind {
                    append(.block(SequenceDiagram.Block(kind: kind, sections: closed.sections)))
                } else {
                    // A `box`: we don't draw it, but keep whatever was inside.
                    closed.sections.flatMap(\.items).forEach(append)
                }
            case "box":
                stack.append((nil, [.init(label: "", items: [])]))
            case "links", "link", "properties", "details", "create", "destroy":
                // Decoration we don't draw.
                break
            default:
                guard let message = parseMessage(line) else { throw MermaidError("Couldn't read “\(line)”.") }
                ensure(message.from)
                ensure(message.to)
                append(.message(message))
            }
        }
        guard stack.count == 1 else { throw MermaidError("A block is missing its `end`.") }
        guard !participants.isEmpty else { throw MermaidError("The diagram has no participants.") }
        return SequenceDiagram(participants: participants, items: stack[0].sections[0].items, autonumber: autonumber, title: title)
    }

    /// Longest arrows first, so `-->>` isn't read as `-->` followed by `>`.
    static let arrows: [(token: String, line: SequenceDiagram.LineStyle, head: SequenceDiagram.ArrowHead, both: Bool)] = [
        ("<<-->>", .dotted, .filled, true), ("<<->>", .solid, .filled, true),
        ("-->>", .dotted, .filled, false), ("->>", .solid, .filled, false),
        ("--x", .dotted, .cross, false), ("-x", .solid, .cross, false),
        ("--)", .dotted, .open, false), ("-)", .solid, .open, false),
        ("-->", .dotted, .none, false), ("->", .solid, .none, false),
    ]

    static func parseMessage(_ line: String) -> SequenceDiagram.Message? {
        guard let colon = line.firstIndex(of: ":") else { return nil }
        let head = String(line[..<colon])
        let text = MermaidText.clean(String(line[line.index(after: colon)...]))
        for arrow in arrows {
            guard let range = head.range(of: arrow.token) else { continue }
            let from = head[..<range.lowerBound].trimmingCharacters(in: .whitespaces)
            var to = head[range.upperBound...].trimmingCharacters(in: .whitespaces)
            var activates = false, deactivates = false
            if to.hasPrefix("+") {
                activates = true
                to.removeFirst()
            } else if to.hasPrefix("-") {
                deactivates = true
                to.removeFirst()
            }
            to = to.trimmingCharacters(in: .whitespaces)
            guard !from.isEmpty, !to.isEmpty else { return nil }
            return .init(from: from, to: to, text: text, line: arrow.line, head: arrow.head, bidirectional: arrow.both,
                         activatesTarget: activates, deactivatesSource: deactivates)
        }
        return nil
    }

    static func parseNote(_ line: String) -> SequenceDiagram.Note? {
        guard let colon = line.firstIndex(of: ":") else { return nil }
        let head = line[..<colon].trimmingCharacters(in: .whitespaces)
        let text = MermaidText.clean(String(line[line.index(after: colon)...]))
        let words = head.split(separator: " ", maxSplits: 3).map(String.init)
        guard words.count >= 3 else { return nil }
        let placement: SequenceDiagram.NotePlacement
        var target: String
        switch words[1].lowercased() {
        case "over":
            placement = .over
            target = words.dropFirst(2).joined(separator: " ")
        case "left", "right":
            guard words.count >= 4, words[2].lowercased() == "of" else { return nil }
            placement = words[1].lowercased() == "left" ? .leftOf : .rightOf
            target = words[3]
        default:
            return nil
        }
        let ids = target.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        guard !ids.isEmpty else { return nil }
        return .init(placement: placement, participants: ids, text: text)
    }
}
