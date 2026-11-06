import CoreGraphics
import Foundation

/// A sequence diagram with participants, messages, notes, activation bars and blocks
/// placed, ready to draw.
public struct SequenceGeometry: Sendable {
    public struct PlacedParticipant: Sendable {
        public var participant: SequenceDiagram.Participant
        public var centerX: Double
        public var header: CGRect
        public var label: WrappedText
    }

    public struct PlacedMessage: Sendable {
        public var message: SequenceDiagram.Message
        public var fromX: Double
        public var toX: Double
        public var y: Double
        public var isSelf: Bool
        public var label: WrappedText
        public var labelFrame: CGRect
        public var number: Int?
    }

    public struct PlacedNote: Sendable {
        public var frame: CGRect
        public var text: WrappedText
    }

    public struct PlacedBlock: Sendable {
        public struct Section: Sendable {
            public var label: WrappedText
            /// Top of the section; the first one is the block's top.
            public var y: Double
        }

        public var kind: SequenceDiagram.Block.Kind
        public var frame: CGRect
        public var sections: [Section]
        public var depth: Int
    }

    public var size: CGSize
    public var participants: [PlacedParticipant]
    public var lifelineTop: Double
    public var lifelineBottom: Double
    public var messages: [PlacedMessage]
    public var notes: [PlacedNote]
    public var activations: [CGRect]
    /// Outermost first.
    public var blocks: [PlacedBlock]
}

public enum SequenceLayout {
    static let participantLabelWidth = 150.0
    static let messageLabelWidth = 260.0
    static let noteWidth = 200.0
    static let barWidth = 10.0
    static let selfLoopWidth = 32.0

    public static func layout(_ diagram: SequenceDiagram, metrics: TextMetrics) -> SequenceGeometry {
        let count = diagram.participants.count
        let column = Dictionary(uniqueKeysWithValues: diagram.participants.enumerated().map { ($1.id, $0) })
        let labels = diagram.participants.map { WrappedText.wrap($0.label, maxWidth: participantLabelWidth, metrics: metrics) }
        let actorIcon = 18.0
        let headerSizes = diagram.participants.enumerated().map { index, participant in
            CGSize(width: max(labels[index].size.width + 28, 76), height: labels[index].size.height + 18 + (participant.isActor ? actorIcon : 0))
        }
        let headerHeight = headerSizes.map(\.height).max() ?? 0

        // MARK: Horizontal: gaps between neighbouring lifelines, widened for what sits between them.
        var gaps = (0..<max(count - 1, 0)).map { (headerSizes[$0].width + headerSizes[$0 + 1].width) / 2 + 28 }
        var leftExtra = 0.0, rightExtra = 0.0
        let numberWidth = diagram.autonumber ? 22.0 : 0

        func need(_ from: Int, _ to: Int, _ width: Double) {
            let lo = min(from, to), hi = max(from, to)
            guard hi > lo else { return }
            let have = gaps[lo..<hi].reduce(0, +)
            if have < width { gaps[hi - 1] += width - have }
        }
        func needRight(of index: Int, _ width: Double) {
            if index < count - 1 { gaps[index] = max(gaps[index], width) } else { rightExtra = max(rightExtra, width) }
        }
        func needLeft(of index: Int, _ width: Double) {
            if index > 0 { gaps[index - 1] = max(gaps[index - 1], width) } else { leftExtra = max(leftExtra, width) }
        }

        func measure(_ items: [SequenceDiagram.Item]) {
            for item in items {
                switch item {
                case .message(let message):
                    guard let a = column[message.from], let b = column[message.to] else { continue }
                    let label = WrappedText.wrap(message.text, maxWidth: messageLabelWidth, metrics: metrics)
                    if a == b {
                        needRight(of: a, label.size.width + selfLoopWidth + 24 + numberWidth)
                    } else {
                        need(a, b, label.size.width + 36 + numberWidth)
                    }
                case .note(let note):
                    let text = WrappedText.wrap(note.text, maxWidth: noteWidth, metrics: metrics)
                    let width = text.size.width + 24
                    let ids = note.participants.compactMap { column[$0] }
                    guard let first = ids.min(), let last = ids.max() else { continue }
                    switch note.placement {
                    case .rightOf: needRight(of: first, width + 24)
                    case .leftOf: needLeft(of: first, width + 24)
                    case .over:
                        if first == last {
                            needRight(of: first, width / 2 + 16)
                            needLeft(of: first, width / 2 + 16)
                        } else {
                            need(first, last, width - 40)
                        }
                    }
                case .block(let block):
                    for section in block.sections { measure(section.items) }
                case .activate, .deactivate:
                    break
                }
            }
        }
        measure(diagram.items)

        let margin = 12.0
        var centers: [Double] = []
        var x = margin + max(headerSizes.first.map { $0.width / 2 } ?? 0, leftExtra)
        for index in 0..<count {
            if index > 0 { x += gaps[index - 1] }
            centers.append(x)
        }

        // MARK: Vertical: one pass in reading order.
        var messages: [SequenceGeometry.PlacedMessage] = []
        var notes: [SequenceGeometry.PlacedNote] = []
        var activations: [CGRect] = []
        var blocks: [SequenceGeometry.PlacedBlock] = []
        var open: [Int: [Double]] = [:] // participant → start y of each active bar
        var y = headerHeight + 22
        var lastArrowY = y
        var number = 0

        func activationOffset(_ index: Int) -> Double {
            (open[index]?.isEmpty ?? true) ? 0 : barWidth / 2
        }
        func activate(_ index: Int, at start: Double) {
            open[index, default: []].append(start)
        }
        func deactivate(_ index: Int, at end: Double) {
            guard let start = open[index]?.popLast() else { return }
            let depth = Double(open[index]?.count ?? 0)
            activations.append(CGRect(x: centers[index] - barWidth / 2 + depth * 4, y: start, width: barWidth, height: max(end - start, 12)))
        }

        /// Returns the horizontal extent of what it placed, for sizing enclosing blocks.
        func place(_ items: [SequenceDiagram.Item], depth: Int) -> ClosedRange<Double>? {
            var extent: ClosedRange<Double>?
            func include(_ lo: Double, _ hi: Double) {
                extent = extent.map { min($0.lowerBound, lo)...max($0.upperBound, hi) } ?? lo...hi
            }
            for item in items {
                switch item {
                case .message(let message):
                    guard let a = column[message.from], let b = column[message.to] else { continue }
                    let label = WrappedText.wrap(message.text, maxWidth: messageLabelWidth, metrics: metrics)
                    let labelTop = y
                    let arrowY = y + label.size.height + 5
                    if diagram.autonumber { number += 1 }
                    if a == b {
                        let fromX = centers[a] + activationOffset(a)
                        let frame = CGRect(x: fromX + 8 + numberWidth, y: labelTop, width: label.size.width, height: label.size.height)
                        messages.append(.init(message: message, fromX: fromX, toX: fromX, y: arrowY, isSelf: true,
                                              label: label, labelFrame: frame, number: diagram.autonumber ? number : nil))
                        include(centers[a], max(frame.maxX, fromX + selfLoopWidth))
                        y = arrowY + 20 + 16
                    } else {
                        let rightward = b > a
                        let fromX = centers[a] + (rightward ? 1 : -1) * activationOffset(a)
                        let toX = centers[b] + (rightward ? -1 : 1) * activationOffset(b)
                        let mid = (fromX + toX) / 2
                        let frame = CGRect(x: mid - label.size.width / 2, y: labelTop, width: label.size.width, height: label.size.height)
                        messages.append(.init(message: message, fromX: fromX, toX: toX, y: arrowY, isSelf: false,
                                              label: label, labelFrame: frame, number: diagram.autonumber ? number : nil))
                        include(min(fromX, toX), max(fromX, toX))
                        y = arrowY + 16
                    }
                    lastArrowY = arrowY
                    if message.activatesTarget { activate(b, at: arrowY) }
                    if message.deactivatesSource { deactivate(a, at: arrowY) }

                case .note(let note):
                    let text = WrappedText.wrap(note.text, maxWidth: noteWidth, metrics: metrics)
                    let ids = note.participants.compactMap { column[$0] }
                    guard let first = ids.min(), let last = ids.max() else { continue }
                    let width = text.size.width + 24, height = text.size.height + 14
                    let frame: CGRect = switch note.placement {
                    case .rightOf: CGRect(x: centers[first] + 12, y: y, width: width, height: height)
                    case .leftOf: CGRect(x: centers[first] - 12 - width, y: y, width: width, height: height)
                    case .over:
                        first == last
                            ? CGRect(x: centers[first] - width / 2, y: y, width: width, height: height)
                            : CGRect(x: centers[first] - 20, y: y, width: max(centers[last] - centers[first] + 40, width), height: height)
                    }
                    notes.append(.init(frame: frame, text: text))
                    include(frame.minX, frame.maxX)
                    y += height + 14

                case .activate(let id):
                    if let index = column[id] { activate(index, at: lastArrowY) }
                case .deactivate(let id):
                    if let index = column[id] { deactivate(index, at: lastArrowY) }

                case .block(let block):
                    let top = y
                    let blockIndex = blocks.count
                    blocks.append(.init(kind: block.kind, frame: .zero, sections: [], depth: depth))
                    var sections: [SequenceGeometry.PlacedBlock.Section] = []
                    var inner: ClosedRange<Double>?
                    for (index, section) in block.sections.enumerated() {
                        let label = WrappedText.wrap(section.label, maxWidth: messageLabelWidth, metrics: metrics)
                        if index > 0 { y += 4 }
                        sections.append(.init(label: label, y: y))
                        y += max(label.size.height, metrics.lineHeight) + 14
                        if let placed = place(section.items, depth: depth + 1) {
                            inner = inner.map { min($0.lowerBound, placed.lowerBound)...max($0.upperBound, placed.upperBound) } ?? placed
                        }
                    }
                    y += 6
                    let span = inner ?? (centers.first ?? 0)...(centers.last ?? 0)
                    let pad = 24.0
                    var frame = CGRect(x: span.lowerBound - pad, y: top, width: span.upperBound - span.lowerBound + pad * 2, height: y - top)
                    // Wide enough for its own labels.
                    let tab = metrics.width(block.kind.rawValue) + 20
                    let widest = sections.map { $0.label.size.width }.max() ?? 0
                    frame.size.width = max(frame.width, tab + widest + 24)
                    blocks[blockIndex].frame = frame
                    blocks[blockIndex].sections = sections
                    include(frame.minX, frame.maxX)
                    y += 12
                }
            }
            return extent
        }
        _ = place(diagram.items, depth: 0)
        for (index, starts) in open {
            for _ in starts { deactivate(index, at: y - 8) }
        }

        let bottom = y
        var participants = diagram.participants.enumerated().map { index, participant in
            SequenceGeometry.PlacedParticipant(
                participant: participant, centerX: centers[index],
                header: CGRect(x: centers[index] - headerSizes[index].width / 2, y: 0, width: headerSizes[index].width, height: headerHeight),
                label: labels[index]
            )
        }

        // Normalize so nothing hangs off the left edge.
        var minX = participants.map(\.header.minX).min() ?? 0
        var maxX = participants.map(\.header.maxX).max() ?? 0
        for note in notes { minX = min(minX, note.frame.minX); maxX = max(maxX, note.frame.maxX) }
        for block in blocks { minX = min(minX, block.frame.minX); maxX = max(maxX, block.frame.maxX) }
        for message in messages { maxX = max(maxX, message.labelFrame.maxX, message.toX + (message.isSelf ? selfLoopWidth : 0)) }
        let dx = -minX
        for index in participants.indices {
            participants[index].centerX += dx
            participants[index].header = participants[index].header.offsetBy(dx: dx, dy: 0)
        }
        for index in messages.indices {
            messages[index].fromX += dx
            messages[index].toX += dx
            messages[index].labelFrame = messages[index].labelFrame.offsetBy(dx: dx, dy: 0)
        }
        for index in notes.indices { notes[index].frame = notes[index].frame.offsetBy(dx: dx, dy: 0) }
        for index in blocks.indices { blocks[index].frame = blocks[index].frame.offsetBy(dx: dx, dy: 0) }
        activations = activations.map { $0.offsetBy(dx: dx, dy: 0) }
        blocks.sort { $0.depth < $1.depth }

        return SequenceGeometry(
            size: CGSize(width: maxX - minX, height: bottom),
            participants: participants, lifelineTop: headerHeight, lifelineBottom: bottom,
            messages: messages, notes: notes, activations: activations, blocks: blocks
        )
    }
}
