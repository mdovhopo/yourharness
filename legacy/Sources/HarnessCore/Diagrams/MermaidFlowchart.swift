import CoreGraphics
import Foundation

/// A Mermaid `flowchart` / `graph`: nodes, edges and subgraphs. Node and subgraph styling
/// (`classDef`, `class`, `:::name`, `style`) is honored for fill, outline and dashes;
/// `linkStyle` and `click` are accepted and ignored.
public struct Flowchart: Sendable, Equatable {
    public enum Direction: Sendable, Equatable {
        case topBottom, bottomTop, leftRight, rightLeft
    }

    public enum Shape: Sendable, Equatable {
        case rectangle, rounded, stadium, subroutine, cylinder, circle, doubleCircle
        case diamond, hexagon, parallelogram, parallelogramAlt, trapezoid, trapezoidAlt, flag
    }

    public struct Node: Sendable, Equatable, Identifiable {
        public var id: String
        public var label: String
        public var shape: Shape
        public var style = ShapeStyle()
    }

    public enum Line: Sendable, Equatable {
        case solid, dotted, thick, invisible
    }

    public enum Arrow: Sendable, Equatable {
        case none, arrow, circle, cross
    }

    public struct Edge: Sendable, Equatable {
        public var from: String
        public var to: String
        public var label: String?
        public var line: Line
        public var start: Arrow
        public var end: Arrow
    }

    public struct Subgraph: Sendable, Equatable, Identifiable {
        public var id: String
        public var title: String
        public var nodeIDs: [String]
        public var parent: String?
        public var style = ShapeStyle()
        /// Its own `direction`, used when no edge crosses its border.
        public var direction: Direction?
    }

    public var direction: Direction
    public var nodes: [Node]
    public var edges: [Edge]
    public var subgraphs: [Subgraph]
}

public enum FlowchartParser {
    public static func parse(_ source: String) throws -> Flowchart {
        var statements = MermaidText.statements(source)
        guard let header = statements.first else { throw MermaidError("The diagram is empty.") }
        let headerWords = header.split(separator: " ")
        guard let keyword = headerWords.first, keyword == "flowchart" || keyword == "graph" else {
            throw MermaidError("Expected `flowchart` or `graph`.")
        }
        statements.removeFirst()

        var builder = Builder(direction: direction(headerWords.dropFirst().first.map(String.init)))
        for statement in statements {
            try builder.add(statement)
        }
        guard builder.subgraphStack.isEmpty else { throw MermaidError("A `subgraph` is missing its `end`.") }
        guard !builder.nodes.isEmpty else { throw MermaidError("The flowchart has no nodes.") }
        return builder.flowchart
    }

    static func direction(_ token: String?) -> Flowchart.Direction {
        switch token?.uppercased() {
        case "LR": .leftRight
        case "RL": .rightLeft
        case "BT": .bottomTop
        default: .topBottom
        }
    }

    // MARK: - Building

    struct Builder {
        var direction: Flowchart.Direction
        var nodes: [Flowchart.Node] = []
        var nodeIndex: [String: Int] = [:]
        var edges: [Flowchart.Edge] = []
        var subgraphs: [Flowchart.Subgraph] = []
        var subgraphStack: [Int] = []
        var classDefinitions: [String: ShapeStyle] = [:]
        var classes: [String: [String]] = [:]
        var inlineStyles: [String: ShapeStyle] = [:]

        func style(for id: String) -> ShapeStyle {
            var style = classDefinitions["default"] ?? ShapeStyle()
            for name in classes[id] ?? [] {
                if let definition = classDefinitions[name] { style = style.merged(with: definition) }
            }
            return style.merged(with: inlineStyles[id] ?? ShapeStyle())
        }

        var flowchart: Flowchart {
            // An edge to a subgraph lands on its first node: close to Mermaid's look, and
            // keeps the layout a plain graph.
            let subgraphTargets = Dictionary(uniqueKeysWithValues: subgraphs.compactMap { subgraph in
                subgraph.nodeIDs.first.map { (subgraph.id, $0) }
            })
            let resolved = edges.compactMap { edge -> Flowchart.Edge? in
                var edge = edge
                if nodeIndex[edge.from] == nil { edge.from = subgraphTargets[edge.from] ?? edge.from }
                if nodeIndex[edge.to] == nil { edge.to = subgraphTargets[edge.to] ?? edge.to }
                return nodeIndex[edge.from] != nil && nodeIndex[edge.to] != nil ? edge : nil
            }
            let styledNodes = nodes.map { node in
                var node = node
                node.style = style(for: node.id)
                return node
            }
            let styledSubgraphs = subgraphs.map { subgraph in
                var subgraph = subgraph
                // `default` is for nodes; subgraphs only take what's aimed at them.
                var style = ShapeStyle()
                for name in classes[subgraph.id] ?? [] {
                    if let definition = classDefinitions[name] { style = style.merged(with: definition) }
                }
                subgraph.style = style.merged(with: inlineStyles[subgraph.id] ?? ShapeStyle())
                return subgraph
            }
            return Flowchart(direction: direction, nodes: styledNodes, edges: resolved, subgraphs: styledSubgraphs)
        }

        mutating func add(_ statement: String) throws {
            let lower = statement.lowercased()
            if lower.hasPrefix("classdef ") {
                let rest = statement.dropFirst("classDef ".count).trimmingCharacters(in: .whitespaces)
                guard let space = rest.firstIndex(of: " ") else { return }
                let style = ShapeStyle.parse(String(rest[space...]))
                for name in rest[..<space].split(separator: ",") { classDefinitions[String(name)] = style }
                return
            }
            if lower.hasPrefix("class ") {
                let parts = statement.dropFirst("class ".count).split(separator: " ", omittingEmptySubsequences: true)
                guard parts.count >= 2 else { return }
                for id in parts[0].split(separator: ",") { classes[String(id), default: []].append(String(parts[1])) }
                return
            }
            if lower.hasPrefix("style ") {
                let rest = statement.dropFirst("style ".count).trimmingCharacters(in: .whitespaces)
                guard let space = rest.firstIndex(of: " ") else { return }
                let id = String(rest[..<space])
                inlineStyles[id] = (inlineStyles[id] ?? ShapeStyle()).merged(with: ShapeStyle.parse(String(rest[space...])))
                return
            }
            for ignored in ["linkstyle ", "click ", "acctitle", "accdescr"] where lower.hasPrefix(ignored) {
                return
            }
            if lower.hasPrefix("direction ") {
                let value = FlowchartParser.direction(String(statement.dropFirst(10)).trimmingCharacters(in: .whitespaces))
                if let current = subgraphStack.last { subgraphs[current].direction = value } else { direction = value }
                return
            }
            if lower == "end" {
                guard !subgraphStack.isEmpty else { throw MermaidError("`end` without a `subgraph`.") }
                subgraphStack.removeLast()
                return
            }
            if lower.hasPrefix("subgraph") {
                openSubgraph(String(statement.dropFirst("subgraph".count)).trimmingCharacters(in: .whitespaces))
                return
            }
            try parseChain(statement)
        }

        mutating func openSubgraph(_ rest: String) {
            var id = rest
            var title = rest
            // `subgraph id [Title]` or `subgraph id["Title"]`
            if let bracket = rest.firstIndex(of: "["), rest.hasSuffix("]") {
                id = String(rest[..<bracket]).trimmingCharacters(in: .whitespaces)
                title = MermaidText.clean(String(rest[rest.index(after: bracket)..<rest.index(before: rest.endIndex)]))
            } else {
                title = MermaidText.clean(rest)
                id = rest.contains(" ") || rest.contains("\"") ? "subgraph-\(subgraphs.count)" : rest
            }
            if id.isEmpty { id = "subgraph-\(subgraphs.count)" }
            let parent = subgraphStack.last.map { subgraphs[$0].id }
            subgraphs.append(Flowchart.Subgraph(id: id, title: title, nodeIDs: [], parent: parent))
            subgraphStack.append(subgraphs.count - 1)
        }

        /// `A[x] --> B & C -->|label| D`
        mutating func parseChain(_ statement: String) throws {
            var scanner = Scanner(Array(statement))
            var previous = try parseGroup(&scanner)
            guard !previous.isEmpty else { throw MermaidError("Couldn't read “\(statement)”.") }
            while true {
                scanner.skipSpaces()
                if scanner.isAtEnd { break }
                guard let link = scanner.parseLink() else {
                    throw MermaidError("Couldn't read “\(statement)”.")
                }
                let next = try parseGroup(&scanner)
                guard !next.isEmpty else { throw MermaidError("An arrow in “\(statement)” points nowhere.") }
                for from in previous {
                    for to in next {
                        edges.append(Flowchart.Edge(from: from, to: to, label: link.label, line: link.line, start: link.start, end: link.end))
                    }
                }
                previous = next
            }
        }

        /// One or more nodes joined by `&`.
        mutating func parseGroup(_ scanner: inout Scanner) throws -> [String] {
            var ids: [String] = []
            repeat {
                scanner.skipSpaces()
                guard let (id, shape, label, className) = scanner.parseNode() else { break }
                if let className { classes[id, default: []].append(className) }
                // A bare subgraph id is an edge to the subgraph, not a new node.
                if shape != nil || label != nil || nodeIndex[id] != nil || !subgraphs.contains(where: { $0.id == id }) {
                    declare(id: id, shape: shape, label: label)
                }
                ids.append(id)
                scanner.skipSpaces()
            } while scanner.consume("&")
            return ids
        }

        mutating func declare(id: String, shape: Flowchart.Shape?, label: String?) {
            if let index = nodeIndex[id] {
                if let shape { nodes[index].shape = shape }
                if let label { nodes[index].label = label }
            } else {
                nodeIndex[id] = nodes.count
                nodes.append(Flowchart.Node(id: id, label: label ?? id, shape: shape ?? .rectangle))
                if let current = subgraphStack.last, !subgraphs.contains(where: { $0.id == id }) {
                    subgraphs[current].nodeIDs.append(id)
                }
            }
        }
    }

    // MARK: - Scanning

    struct Link {
        var label: String?
        var line: Flowchart.Line
        var start: Flowchart.Arrow
        var end: Flowchart.Arrow
    }

    struct Scanner {
        let characters: [Character]
        var index = 0

        init(_ characters: [Character]) {
            self.characters = characters
        }

        var isAtEnd: Bool { index >= characters.count }
        var current: Character? { isAtEnd ? nil : characters[index] }

        func peek(_ offset: Int = 0) -> Character? {
            index + offset < characters.count ? characters[index + offset] : nil
        }

        mutating func skipSpaces() {
            while let character = current, character == " " || character == "\t" { index += 1 }
        }

        mutating func consume(_ text: String) -> Bool {
            let target = Array(text)
            guard index + target.count <= characters.count, Array(characters[index..<index + target.count]) == target else { return false }
            index += target.count
            return true
        }

        static func isIDCharacter(_ character: Character) -> Bool {
            character.isLetter || character.isNumber || character == "_" || character == "." || character == ":" || character == "$"
        }

        /// A node id, optionally followed by a shape with a label, and `:::class`.
        mutating func parseNode() -> (String, Flowchart.Shape?, String?, String?)? {
            let start = index
            while let character = current {
                if Self.isIDCharacter(character) {
                    index += 1
                } else if character == "-", let next = peek(1), Self.isIDCharacter(next), next != "." {
                    // Hyphens inside ids (`my-node`), but not the start of an arrow.
                    index += 1
                } else {
                    break
                }
            }
            // `id:::className` names a class without a shape.
            var id = String(characters[start..<index])
            var className: String?
            if let range = id.range(of: ":::") {
                className = String(id[range.upperBound...])
                id = String(id[..<range.lowerBound])
            }
            guard !id.isEmpty else { return nil }
            if id.hasSuffix(":") { id.removeLast() }

            guard let (shape, label) = parseShape() else { return (id, nil, nil, className.flatMap { $0.isEmpty ? nil : $0 }) }
            if consume(":::") {
                let nameStart = index
                while let character = current, Self.isIDCharacter(character) || character == "-" { index += 1 }
                className = String(characters[nameStart..<index])
            }
            return (id, shape, MermaidText.clean(label), className.flatMap { $0.isEmpty ? nil : $0 })
        }

        /// Openers, longest first so `([` wins over `(`.
        static let shapes: [(open: String, close: String, shape: Flowchart.Shape)] = [
            ("(((", ")))", .doubleCircle),
            ("([", "])", .stadium),
            ("[[", "]]", .subroutine),
            ("[(", ")]", .cylinder),
            ("((", "))", .circle),
            ("{{", "}}", .hexagon),
            ("[/", "/]", .parallelogram),
            ("[\\", "\\]", .parallelogramAlt),
            ("[/", "\\]", .trapezoid),
            ("[\\", "/]", .trapezoidAlt),
            ("[", "]", .rectangle),
            ("(", ")", .rounded),
            ("{", "}", .diamond),
            (">", "]", .flag),
        ]

        mutating func parseShape() -> (Flowchart.Shape, String)? {
            for candidate in Self.shapes {
                let saved = index
                guard consume(candidate.open) else { continue }
                if let label = readUntil(candidate.close) {
                    return (candidate.shape, label)
                }
                index = saved
            }
            return nil
        }

        /// Reads to `close`, honoring quotes; leaves the scanner after it.
        mutating func readUntil(_ close: String) -> String? {
            let closing = Array(close)
            var text = ""
            var inQuote = false
            while !isAtEnd {
                if characters[index] == "\"" { inQuote.toggle() }
                if !inQuote, index + closing.count <= characters.count, Array(characters[index..<index + closing.count]) == closing {
                    // `[/x/]` vs `[/x\]`: the closer must end the shape, not sit inside the text.
                    let after = index + closing.count
                    if after >= characters.count || !"])}/\\".contains(characters[after]) || close.count > 1 {
                        index = after
                        return text
                    }
                }
                text.append(characters[index])
                index += 1
            }
            return nil
        }

        /// `-->`, `---`, `-.->`, `==>`, `--o`, `--x`, `<-->`, with `|label|` or inline
        /// `-- label -->` text.
        mutating func parseLink() -> Link? {
            let saved = index
            var start: Flowchart.Arrow = .none
            if let character = current, "<ox".contains(character), let next = peek(1), next == "-" || next == "=" || next == "." {
                start = character == "<" ? .arrow : character == "o" ? .circle : .cross
                index += 1
            }
            guard let first = current, first == "-" || first == "=" || first == "~" else {
                index = saved
                return nil
            }

            var body = ""
            while let character = current, "-=.~".contains(character) {
                body.append(character)
                index += 1
            }
            // Inline label: `-- text -->`, `-. text .->`, `== text ==>`
            if current == " ", body == "--" || body == "-." || body == "==" {
                let restStart = index
                if let (label, closing) = readInlineLabel(opening: body) {
                    var link = Self.link(body: closing.body, end: closing.end)
                    link.start = start
                    link.label = label
                    return link
                }
                index = restStart
            }

            var end: Flowchart.Arrow = .none
            if let character = current {
                switch character {
                case ">":
                    end = .arrow
                    index += 1
                case "o", "x":
                    // `--o B` is a circle end; `--oops` would be a node.
                    if let next = peek(1), next == " " || next == "|" {
                        end = character == "o" ? .circle : .cross
                        index += 1
                    } else if peek(1) == nil {
                        end = character == "o" ? .circle : .cross
                        index += 1
                    }
                default:
                    break
                }
            }
            // Bare `--` with nothing after is not a link.
            if end == .none, body.count < 3, !body.contains(".") {
                index = saved
                return nil
            }
            var link = Self.link(body: body, end: end)
            link.start = start
            skipSpaces()
            if consume("|"), let label = readUntil("|") {
                link.label = MermaidText.clean(label)
            }
            return link
        }

        mutating func readInlineLabel(opening: String) -> (String, (body: String, end: Flowchart.Arrow))? {
            var text = ""
            while !isAtEnd {
                let character = characters[index]
                if "-=.".contains(character), text.hasSuffix(" ") || text.isEmpty {
                    var body = ""
                    let saved = index
                    while let symbol = current, "-=.".contains(symbol) {
                        body.append(symbol)
                        index += 1
                    }
                    var end: Flowchart.Arrow = .none
                    if current == ">" {
                        end = .arrow
                        index += 1
                    } else if let symbol = current, symbol == "o" || symbol == "x", peek(1) == " " {
                        end = symbol == "o" ? .circle : .cross
                        index += 1
                    }
                    if body.count >= 2 || end != .none {
                        let label = MermaidText.clean(text)
                        guard !label.isEmpty else { return nil }
                        return (label, (body: opening + body, end: end))
                    }
                    index = saved
                }
                text.append(character)
                index += 1
            }
            return nil
        }

        static func link(body: String, end: Flowchart.Arrow) -> Link {
            let line: Flowchart.Line = body.contains("~") ? .invisible
                : body.contains(".") ? .dotted
                : body.contains("=") ? .thick
                : .solid
            return Link(label: nil, line: line, start: .none, end: end)
        }
    }
}
