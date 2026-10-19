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
        /// From the source's outline to the target's, through any bends. Each step moves
        /// along the flow, so drawing it as flow-aligned curves never loops back.
        public var points: [CGPoint]
        public var label: WrappedText
        /// Centered on the edge; nil when the edge has no label.
        public var labelFrame: CGRect?
        public var isSelfLoop: Bool
        /// Whether its flow runs up and down; a block inside can flow differently.
        public var vertical: Bool
    }

    public struct PlacedSubgraph: Sendable {
        public var subgraph: Flowchart.Subgraph
        public var frame: CGRect
        public var title: WrappedText
        public var depth: Int
    }

    public var size: CGSize
    public var direction: Flowchart.Direction
    public var nodes: [PlacedNode]
    public var edges: [PlacedEdge]
    /// Outermost first, so they draw underneath their children.
    public var subgraphs: [PlacedSubgraph]
}

/// Subgraphs that something links into or out of are clusters in one layered layout:
/// their members rank with everything else, and border vertices keep each box a clean
/// rectangle (see `LayeredLayout`). A self-contained subgraph, with no edge crossing its
/// border, is laid out on its own, in its own `direction`, and placed as one block.
public enum FlowchartLayout {
    public static let maxLabelWidth = 190.0
    public static let maxEdgeLabelWidth = 150.0
    static let padding = 16.0
    static let titleGap = 8.0

    public static func layout(_ chart: Flowchart, metrics: TextMetrics) -> FlowchartGeometry {
        let nodeIndex = Dictionary(uniqueKeysWithValues: chart.nodes.enumerated().map { ($1.id, $0) })
        let clusterIndex = Dictionary(uniqueKeysWithValues: chart.subgraphs.enumerated().map { ($1.id, $0) })
        let clusterParent = chart.subgraphs.map { $0.parent.flatMap { clusterIndex[$0] } }
        let titles = chart.subgraphs.map { WrappedText.wrap($0.title, maxWidth: 320, metrics: metrics) }
        let labels = chart.nodes.map { WrappedText.wrap($0.label, maxWidth: maxLabelWidth, metrics: metrics) }
        let edgeLabels = chart.edges.map { edge in
            edge.label.map { WrappedText.wrap($0, maxWidth: maxEdgeLabelWidth, metrics: metrics) } ?? .empty
        }

        // The subgraph tree: every cluster a node sits in, outermost first.
        var nodeParent = Array(repeating: Int?.none, count: chart.nodes.count)
        for (index, subgraph) in chart.subgraphs.enumerated() {
            for id in subgraph.nodeIDs { if let node = nodeIndex[id] { nodeParent[node] = index } }
        }
        func chain(_ cluster: Int?) -> [Int] {
            var result: [Int] = []
            var current = cluster
            while let index = current {
                result.insert(index, at: 0)
                current = clusterParent[index]
            }
            return result
        }
        let nodeChains = chart.nodes.indices.map { chain(nodeParent[$0]) }
        func inside(_ node: Int, _ cluster: Int) -> Bool { nodeChains[node].contains(cluster) }

        // Self-contained subgraphs become blocks; only the outermost of nested ones.
        let selfContained = Set(chart.subgraphs.indices.filter { cluster in
            chart.nodes.indices.contains { inside($0, cluster) } && !chart.edges.contains { edge in
                inside(nodeIndex[edge.from]!, cluster) != inside(nodeIndex[edge.to]!, cluster)
            }
        })
        let blocks = selfContained.filter { cluster in !chain(clusterParent[cluster]).contains { selfContained.contains($0) } }.sorted()
        func block(of node: Int) -> Int? { nodeChains[node].first { blocks.contains($0) } }
        func isInsideBlock(_ cluster: Int) -> Bool { chain(cluster).contains { blocks.contains($0) } }

        // Lay each block out on its own.
        var blockGeometry: [Int: FlowchartGeometry] = [:]
        var blockEdges: [Int: [Int]] = [:]
        for cluster in blocks {
            let members = chart.nodes.indices.filter { inside($0, cluster) }
            let ids = Set(members.map { chart.nodes[$0].id })
            var subgraphs = chart.subgraphs.indices.filter { chain($0).contains(cluster) && $0 != cluster }.map { chart.subgraphs[$0] }
            for index in subgraphs.indices where subgraphs[index].parent == chart.subgraphs[cluster].id { subgraphs[index].parent = nil }
            let edges = chart.edges.indices.filter { ids.contains(chart.edges[$0].from) && ids.contains(chart.edges[$0].to) }
            blockEdges[cluster] = edges
            blockGeometry[cluster] = layout(Flowchart(
                direction: chart.subgraphs[cluster].direction ?? chart.direction,
                nodes: members.map { chart.nodes[$0] },
                edges: edges.map { chart.edges[$0] },
                subgraphs: subgraphs
            ), metrics: metrics)
        }

        // The layered graph at this level: free nodes, then one vertex per block.
        let freeNodes = chart.nodes.indices.filter { block(of: $0) == nil }
        let layoutClusters = chart.subgraphs.indices.filter { !isInsideBlock($0) && !blocks.contains($0) }
        let clusterSlot = Dictionary(uniqueKeysWithValues: layoutClusters.enumerated().map { ($1, $0) })
        func slot(_ cluster: Int?) -> Int? { cluster.flatMap { clusterSlot[$0] } }

        var layoutNodes: [LayeredLayout.Node] = freeNodes.map { node in
            LayeredLayout.Node(size: size(of: chart.nodes[node].shape, label: labels[node].size),
                               boundary: boundary(chart.nodes[node].shape), cluster: slot(nodeParent[node]))
        }
        var blockVertex: [Int: Int] = [:]
        for cluster in blocks {
            let inner = blockGeometry[cluster]!.size
            let title = titles[cluster]
            blockVertex[cluster] = layoutNodes.count
            layoutNodes.append(LayeredLayout.Node(
                size: CGSize(width: max(inner.width, title.size.width) + padding * 2, height: inner.height + padding * 2 + title.size.height + titleGap),
                cluster: slot(clusterParent[cluster])
            ))
        }
        let vertexOfNode = Dictionary(uniqueKeysWithValues: freeNodes.enumerated().map { ($1, $0) })
        // Edges inside blocks were laid out with the block; the rest are routed here.
        let levelEdges = chart.edges.indices.filter { block(of: nodeIndex[chart.edges[$0].from]!) == nil }
        let result = LayeredLayout.layout(
            nodes: layoutNodes,
            edges: levelEdges.map { index in
                let edge = chart.edges[index]
                let label = edgeLabels[index]
                return LayeredLayout.Edge(
                    from: vertexOfNode[nodeIndex[edge.from]!]!, to: vertexOfNode[nodeIndex[edge.to]!]!,
                    labelSize: label.isEmpty ? nil : CGSize(width: label.size.width + 10, height: label.size.height + 4)
                )
            },
            clusters: layoutClusters.map { cluster in
                LayeredLayout.Cluster(parent: slot(clusterParent[cluster]), titleSize: titles[cluster].size)
            },
            direction: layeredDirection(chart.direction)
        )

        // Nodes and subgraph boxes, with blocks' contents dropped into place.
        var nodes = Array(repeating: FlowchartGeometry.PlacedNode?.none, count: chart.nodes.count)
        for (vertex, node) in freeNodes.enumerated() {
            nodes[node] = .init(node: chart.nodes[node], frame: result.frames[vertex], label: labels[node])
        }
        var subgraphs: [FlowchartGeometry.PlacedSubgraph] = []
        for (slotIndex, cluster) in layoutClusters.enumerated() where !result.clusterFrames[slotIndex].isNull {
            subgraphs.append(.init(subgraph: chart.subgraphs[cluster], frame: result.clusterFrames[slotIndex], title: titles[cluster], depth: chain(cluster).count - 1))
        }
        var edges = Array(repeating: FlowchartGeometry.PlacedEdge?.none, count: chart.edges.count)
        for cluster in blocks {
            let frame = result.frames[blockVertex[cluster]!]
            let depth = chain(cluster).count - 1
            subgraphs.append(.init(subgraph: chart.subgraphs[cluster], frame: frame, title: titles[cluster], depth: depth))
            let inner = blockGeometry[cluster]!
            let dx = frame.minX + padding, dy = frame.minY + padding + titles[cluster].size.height + titleGap
            for placed in inner.nodes {
                var placed = placed
                placed.frame = placed.frame.offsetBy(dx: dx, dy: dy)
                nodes[nodeIndex[placed.node.id]!] = placed
            }
            for placed in inner.subgraphs {
                var placed = placed
                placed.frame = placed.frame.offsetBy(dx: dx, dy: dy)
                placed.depth += depth + 1
                placed.subgraph.parent = placed.subgraph.parent ?? chart.subgraphs[cluster].id
                subgraphs.append(placed)
            }
            // The block's edges come back in the order they went in.
            for (position, index) in blockEdges[cluster]!.enumerated() where position < inner.edges.count {
                var placed = inner.edges[position]
                placed.points = placed.points.map { CGPoint(x: $0.x + dx, y: $0.y + dy) }
                placed.labelFrame = placed.labelFrame?.offsetBy(dx: dx, dy: dy)
                edges[index] = placed
            }
        }

        // Edges at this level: the route, with its ends moved onto ports on the nodes.
        let vertical = chart.direction == .topBottom || chart.direction == .bottomTop
        var ports = PortAssigner(direction: chart.direction)
        for (position, index) in levelEdges.enumerated() {
            let edge = chart.edges[index]
            let points = result.routes[position].points
            let a = nodeIndex[edge.from]!, b = nodeIndex[edge.to]!
            guard a != b, points.count >= 2, let fromFrame = nodes[a]?.frame, let toFrame = nodes[b]?.frame else { continue }
            ports.add(PortKey(edge: index, outgoing: true), node: a, frame: fromFrame, toward: points[1])
            ports.add(PortKey(edge: index, outgoing: false), node: b, frame: toFrame, toward: points[points.count - 2])
        }
        let assigned = ports.resolve(shapes: chart.nodes.map { boundary($0.shape) })
        for (position, index) in levelEdges.enumerated() {
            let edge = chart.edges[index]
            let route = result.routes[position]
            var points = route.points
            if points.count >= 2, let start = assigned[PortKey(edge: index, outgoing: true)], let end = assigned[PortKey(edge: index, outgoing: false)] {
                points[0] = start.point
                points[points.count - 1] = end.point
            }
            let label = edgeLabels[index]
            var labelFrame: CGRect?
            if !label.isEmpty, !points.isEmpty {
                let center = route.labelCenter ?? midpoint(points)
                labelFrame = CGRect(x: center.x - label.size.width / 2 - 5, y: center.y - label.size.height / 2 - 2,
                                    width: label.size.width + 10, height: label.size.height + 4)
            }
            edges[index] = .init(edge: edge, points: points, label: label, labelFrame: labelFrame,
                                 isSelfLoop: edge.from == edge.to, vertical: vertical)
        }

        // Loops and labels can poke out; keep everything on the canvas.
        let placedNodes = nodes.compactMap { $0 }
        let placedEdges = edges.compactMap { $0 }
        var bounds = CGRect(origin: .zero, size: result.size)
        for node in placedNodes { bounds = bounds.union(node.frame) }
        for subgraph in subgraphs { bounds = bounds.union(subgraph.frame) }
        for edge in placedEdges {
            for point in edge.points { bounds = bounds.union(CGRect(origin: point, size: .zero)) }
            if let frame = edge.labelFrame { bounds = bounds.union(frame) }
        }
        let dx = -bounds.minX, dy = -bounds.minY
        return FlowchartGeometry(
            size: bounds.size,
            direction: chart.direction,
            nodes: placedNodes.map { node in
                var node = node
                node.frame = node.frame.offsetBy(dx: dx, dy: dy)
                return node
            },
            edges: placedEdges.map { edge in
                var edge = edge
                edge.points = edge.points.map { CGPoint(x: $0.x + dx, y: $0.y + dy) }
                edge.labelFrame = edge.labelFrame?.offsetBy(dx: dx, dy: dy)
                return edge
            },
            subgraphs: subgraphs
                .map { subgraph in
                    var subgraph = subgraph
                    subgraph.frame = subgraph.frame.offsetBy(dx: dx, dy: dy)
                    return subgraph
                }
                .sorted { $0.depth < $1.depth }
        )
    }

    // MARK: - Ports

    /// `end` is the side further along the flow (bottom when flowing down).
    enum Side { case start, end }

    struct PortKey: Hashable {
        var edge: Int
        var outgoing: Bool
    }

    struct Port {
        var point: CGPoint
        var side: Side
    }

    /// Edges leave and enter a node on the side facing where they go, spread out in the
    /// order of their other ends so they fan out instead of crossing at the node.
    struct PortAssigner {
        let direction: Flowchart.Direction
        var requests: [Int: [(key: PortKey, side: Side, cross: Double, frame: CGRect)]] = [:]

        init(direction: Flowchart.Direction) {
            self.direction = direction
        }

        var vertical: Bool { direction == .topBottom || direction == .bottomTop }
        var forward: Double { direction == .topBottom || direction == .leftRight ? 1 : -1 }

        mutating func add(_ key: PortKey, node: Int, frame: CGRect, toward point: CGPoint) {
            let along = vertical ? point.y - frame.midY : point.x - frame.midX
            let side: Side = along * forward >= 0 ? .end : .start
            requests[node, default: []].append((key, side, vertical ? point.x : point.y, frame))
        }

        func resolve(shapes: [LayeredLayout.Boundary]) -> [PortKey: Port] {
            var result: [PortKey: Port] = [:]
            for (node, list) in requests {
                for side in [Side.start, .end] {
                    let group = list.filter { $0.side == side }.sorted { $0.cross < $1.cross }
                    guard let frame = group.first?.frame else { continue }
                    let shape = shapes[node]
                    let crossSize = vertical ? frame.width : frame.height
                    // Rectangles have a whole side to share; pointy shapes only their tip.
                    let room = shape == .rectangle ? crossSize * 0.6 : shape == .ellipse ? crossSize * 0.35 : crossSize * 0.15
                    let step = group.count > 1 ? min(room / Double(group.count - 1), 22) : 0
                    let first = -step * Double(group.count - 1) / 2
                    let sign = (side == .end ? 1.0 : -1.0) * forward
                    for (position, request) in group.enumerated() {
                        let offset = first + step * Double(position)
                        let point: CGPoint
                        if shape == .rectangle {
                            point = vertical
                                ? CGPoint(x: frame.midX + offset, y: sign > 0 ? frame.maxY : frame.minY)
                                : CGPoint(x: sign > 0 ? frame.maxX : frame.minX, y: frame.midY + offset)
                        } else {
                            let toward = vertical
                                ? CGPoint(x: frame.midX + offset * 2, y: frame.midY + sign * frame.height)
                                : CGPoint(x: frame.midX + sign * frame.width, y: frame.midY + offset * 2)
                            point = LayeredLayout.clip(frame, shape, toward: toward)
                        }
                        result[request.key] = Port(point: point, side: side)
                    }
                }
            }
            return result
        }
    }

    // MARK: - Shapes

    static func layeredDirection(_ direction: Flowchart.Direction) -> LayeredLayout.Direction {
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
        case .hexagon, .parallelogram, .parallelogramAlt, .trapezoid, .trapezoidAlt:
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
