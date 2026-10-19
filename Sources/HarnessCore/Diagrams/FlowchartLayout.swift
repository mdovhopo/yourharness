import CoreGraphics
import Foundation

/// A flowchart with every node, edge and subgraph placed, ready to draw.
public struct FlowchartGeometry: Sendable {
    public struct PlacedNode: Sendable {
        public var node: Flowchart.Node
        public var frame: CGRect
        public var label: WrappedText
    }

    public struct PlacedEdge: Sendable {
        public var edge: Flowchart.Edge
        public var points: [CGPoint]
        public var label: WrappedText
        /// Centered on the edge; nil when the edge has no label.
        public var labelFrame: CGRect?
    }

    public struct PlacedSubgraph: Sendable {
        public var subgraph: Flowchart.Subgraph
        public var frame: CGRect
        public var title: WrappedText
        public var depth: Int
    }

    public var size: CGSize
    public var nodes: [PlacedNode]
    public var edges: [PlacedEdge]
    /// Outermost first, so they draw underneath their children.
    public var subgraphs: [PlacedSubgraph]
}

public enum FlowchartLayout {
    public static let maxLabelWidth = 190.0
    public static let maxEdgeLabelWidth = 150.0
    static let subgraphPadding = 14.0

    public static func layout(_ chart: Flowchart, metrics: TextMetrics) -> FlowchartGeometry {
        let labels = chart.nodes.map { WrappedText.wrap($0.label, maxWidth: maxLabelWidth, metrics: metrics) }
        let index = Dictionary(uniqueKeysWithValues: chart.nodes.enumerated().map { ($1.id, $0) })

        // Keep each top-level subgraph's members together in every layer.
        var topLevel: [String: Int] = [:]
        let subgraphIndex = Dictionary(uniqueKeysWithValues: chart.subgraphs.enumerated().map { ($1.id, $0) })
        func root(of subgraph: Flowchart.Subgraph) -> Int {
            var current = subgraph
            while let parent = current.parent, let next = subgraphIndex[parent] { current = chart.subgraphs[next] }
            return subgraphIndex[current.id] ?? 0
        }
        for subgraph in chart.subgraphs {
            for id in subgraph.nodeIDs { topLevel[id] = root(of: subgraph) }
        }

        // Subgraph titles need headroom above the first layer inside them.
        let extraRankSpacing = chart.subgraphs.isEmpty ? 0 : metrics.lineHeight + 10
        let layoutNodes = chart.nodes.enumerated().map { offset, node in
            LayeredLayout.Node(size: size(of: node.shape, label: labels[offset].size), boundary: boundary(node.shape), group: topLevel[node.id])
        }
        let edgeLabels = chart.edges.map { edge in
            edge.label.map { WrappedText.wrap($0, maxWidth: maxEdgeLabelWidth, metrics: metrics) } ?? .empty
        }
        let layoutEdges = chart.edges.enumerated().map { offset, edge in
            LayeredLayout.Edge(
                from: index[edge.from]!, to: index[edge.to]!,
                labelSize: edgeLabels[offset].isEmpty ? nil : CGSize(width: edgeLabels[offset].size.width + 10, height: edgeLabels[offset].size.height + 4)
            )
        }
        var options = LayeredLayout.Options()
        options.rankSpacing += extraRankSpacing
        if !chart.subgraphs.isEmpty { options.nodeSpacing += subgraphPadding }
        let result = LayeredLayout.layout(nodes: layoutNodes, edges: layoutEdges, direction: direction(chart.direction), options: options)

        var nodes = chart.nodes.enumerated().map { offset, node in
            FlowchartGeometry.PlacedNode(node: node, frame: result.frames[offset], label: labels[offset])
        }
        var edges = chart.edges.enumerated().map { offset, edge -> FlowchartGeometry.PlacedEdge in
            let route = result.routes[offset]
            let label = edgeLabels[offset]
            var labelFrame: CGRect?
            if !label.isEmpty {
                let center = route.labelCenter ?? midpoint(route.points)
                labelFrame = CGRect(x: center.x - label.size.width / 2 - 5, y: center.y - label.size.height / 2 - 2,
                                    width: label.size.width + 10, height: label.size.height + 4)
            }
            return FlowchartGeometry.PlacedEdge(edge: edge, points: route.points, label: label, labelFrame: labelFrame)
        }

        // Subgraph boxes: deepest first, so parents can wrap their children's boxes.
        func depth(_ subgraph: Flowchart.Subgraph) -> Int {
            var depth = 0
            var current = subgraph
            while let parent = current.parent, let next = subgraphIndex[parent] {
                depth += 1
                current = chart.subgraphs[next]
            }
            return depth
        }
        var boxes: [String: CGRect] = [:]
        var placedSubgraphs: [FlowchartGeometry.PlacedSubgraph] = []
        for subgraph in chart.subgraphs.sorted(by: { depth($0) > depth($1) }) {
            var rects = subgraph.nodeIDs.compactMap { index[$0].map { result.frames[$0] } }
            rects += chart.subgraphs.filter { $0.parent == subgraph.id }.compactMap { boxes[$0.id] }
            guard var frame = rects.first else { continue }
            for rect in rects.dropFirst() { frame = frame.union(rect) }
            let title = WrappedText.wrap(subgraph.title, maxWidth: max(frame.width, maxLabelWidth), metrics: metrics)
            frame = frame.insetBy(dx: -subgraphPadding, dy: -subgraphPadding)
            frame.origin.y -= title.size.height + 6
            frame.size.height += title.size.height + 6
            frame.size.width = max(frame.width, title.size.width + subgraphPadding * 2)
            boxes[subgraph.id] = frame
            placedSubgraphs.append(FlowchartGeometry.PlacedSubgraph(subgraph: subgraph, frame: frame, title: title, depth: depth(subgraph)))
        }
        placedSubgraphs.sort { $0.depth < $1.depth }

        // Shift everything so subgraph boxes and loops never hang off the top or left.
        var bounds = CGRect(origin: .zero, size: result.size)
        for subgraph in placedSubgraphs { bounds = bounds.union(subgraph.frame) }
        for edge in edges {
            for point in edge.points { bounds = bounds.union(CGRect(origin: point, size: .zero)) }
            if let frame = edge.labelFrame { bounds = bounds.union(frame) }
        }
        let dx = -bounds.minX, dy = -bounds.minY
        if dx != 0 || dy != 0 {
            for index in nodes.indices { nodes[index].frame = nodes[index].frame.offsetBy(dx: dx, dy: dy) }
            for index in edges.indices {
                edges[index].points = edges[index].points.map { CGPoint(x: $0.x + dx, y: $0.y + dy) }
                edges[index].labelFrame = edges[index].labelFrame?.offsetBy(dx: dx, dy: dy)
            }
            for index in placedSubgraphs.indices { placedSubgraphs[index].frame = placedSubgraphs[index].frame.offsetBy(dx: dx, dy: dy) }
        }
        return FlowchartGeometry(size: bounds.size, nodes: nodes, edges: edges, subgraphs: placedSubgraphs)
    }

    static func direction(_ direction: Flowchart.Direction) -> LayeredLayout.Direction {
        switch direction {
        case .topBottom: .topBottom
        case .bottomTop: .bottomTop
        case .leftRight: .leftRight
        case .rightLeft: .rightLeft
        }
    }

    static func boundary(_ shape: Flowchart.Shape) -> LayeredLayout.Boundary {
        switch shape {
        case .circle, .doubleCircle: .ellipse
        case .diamond: .diamond
        default: .rectangle
        }
    }

    /// The outline that fits a label of `label` size, with breathing room.
    public static func size(of shape: Flowchart.Shape, label: CGSize) -> CGSize {
        let w = max(label.width, 20), h = max(label.height, 16)
        switch shape {
        case .rectangle, .rounded:
            return CGSize(width: w + 28, height: h + 18)
        case .stadium:
            return CGSize(width: w + 28 + h, height: h + 18)
        case .subroutine:
            return CGSize(width: w + 44, height: h + 18)
        case .cylinder:
            return CGSize(width: w + 28, height: h + 34)
        case .circle:
            let d = max(w, h) + 28
            return CGSize(width: d, height: d)
        case .doubleCircle:
            let d = max(w, h) + 40
            return CGSize(width: d, height: d)
        case .diamond:
            // Half-diagonals that contain the label's corners with a margin.
            let p = w / 2, q = h / 2
            return CGSize(width: 2 * (p + q + 14), height: 2 * (q + p * 0.5 + 12))
        case .hexagon:
            return CGSize(width: w + 28 + h, height: h + 18)
        case .parallelogram, .parallelogramAlt, .trapezoid, .trapezoidAlt:
            return CGSize(width: w + 28 + h, height: h + 18)
        case .flag:
            return CGSize(width: w + 40, height: h + 18)
        }
    }

    static func midpoint(_ points: [CGPoint]) -> CGPoint {
        guard let first = points.first, let last = points.last else { return .zero }
        if points.count % 2 == 1 { return points[points.count / 2] }
        if points.count == 2 { return CGPoint(x: (first.x + last.x) / 2, y: (first.y + last.y) / 2) }
        let a = points[points.count / 2 - 1], b = points[points.count / 2]
        return CGPoint(x: (a.x + b.x) / 2, y: (a.y + b.y) / 2)
    }
}
