import CoreGraphics
import Foundation

/// Layered ("Sugiyama") graph layout with clusters, the same family of algorithm dagre and
/// Graphviz's `dot` use: break cycles, assign layers, insert dummy nodes for long edges,
/// order each layer (barycenter sweeps plus adjacent swaps) while keeping every cluster
/// contiguous, then place nodes with weighted isotonic regression so edges run straight.
///
/// Clusters get invisible left and right border vertices on every layer they span. Borders
/// take part in ordering and placement like any vertex, which is what keeps each cluster a
/// clean rectangle that nothing else intrudes on.
///
/// Everything is computed top-to-bottom; other directions are a transform at the end.
public enum LayeredLayout {
    public enum Direction: Sendable {
        case topBottom, bottomTop, leftRight, rightLeft

        var isHorizontal: Bool { self == .leftRight || self == .rightLeft }
    }

    public enum Boundary: Sendable {
        case rectangle, ellipse, diamond
    }

    public struct Node: Sendable {
        public var size: CGSize
        public var boundary: Boundary
        /// The innermost cluster this node is in.
        public var cluster: Int?

        public init(size: CGSize, boundary: Boundary = .rectangle, cluster: Int? = nil) {
            self.size = size
            self.boundary = boundary
            self.cluster = cluster
        }
    }

    public struct Edge: Sendable {
        public var from: Int
        public var to: Int
        public var labelSize: CGSize?

        public init(from: Int, to: Int, labelSize: CGSize? = nil) {
            self.from = from
            self.to = to
            self.labelSize = labelSize
        }
    }

    public struct Cluster: Sendable {
        public var parent: Int?
        /// Drawn at the cluster's top-left, inside its box.
        public var titleSize: CGSize

        public init(parent: Int? = nil, titleSize: CGSize = .zero) {
            self.parent = parent
            self.titleSize = titleSize
        }
    }

    public struct Options: Sendable {
        public var nodeSpacing: Double = 36
        public var edgeSpacing: Double = 14
        public var rankSpacing: Double = 52
        public var clusterPadding: Double = 16
        public var clusterGap: Double = 28
        public var titleGap: Double = 8
        public var sweeps: Int = 12

        public init() {}
    }

    public struct Route: Sendable, Equatable {
        /// From the source's boundary to the target's boundary, through any bends.
        public var points: [CGPoint]
        public var labelCenter: CGPoint?
    }

    public struct Result: Sendable {
        /// By node index.
        public var frames: [CGRect]
        /// By edge index.
        public var routes: [Route]
        /// By cluster index; `.null` for clusters with no nodes.
        public var clusterFrames: [CGRect]
        public var size: CGSize
    }

    // MARK: - Internal graph

    enum Border: Equatable {
        case left, right
    }

    struct Vertex {
        var width: Double
        var height: Double
        var rank = 0
        var x = 0.0
        var isDummy: Bool
        /// Innermost cluster; for a border vertex, the cluster it bounds.
        var cluster: Int?
        var border: Border?
    }

    public static func layout(nodes: [Node], edges: [Edge], clusters: [Cluster] = [], direction: Direction, options: Options = Options()) -> Result {
        guard !nodes.isEmpty else {
            return Result(frames: [], routes: edges.map { _ in Route(points: [], labelCenter: nil) },
                          clusterFrames: clusters.map { _ in .null }, size: .zero)
        }
        let horizontal = direction.isHorizontal
        // Work top-to-bottom; for left-to-right, a node's width runs along the layer.
        func internalSize(_ size: CGSize) -> (Double, Double) {
            horizontal ? (size.height, size.width) : (size.width, size.height)
        }

        // Cluster tree helpers.
        let chains: [[Int]] = clusters.indices.map { cluster in
            var chain: [Int] = []
            var current: Int? = cluster
            while let index = current {
                chain.insert(index, at: 0)
                current = clusters[index].parent
            }
            return chain
        }
        func chain(_ cluster: Int?) -> [Int] { cluster.map { chains[$0] } ?? [] }
        func commonCluster(_ a: Int?, _ b: Int?) -> Int? {
            var shared: Int?
            for (p, q) in zip(chain(a), chain(b)) {
                guard p == q else { break }
                shared = p
            }
            return shared
        }
        /// Title room along the layers (vertical flow) or across them (horizontal flow).
        func titleAlong(_ cluster: Int) -> Double {
            horizontal ? 0 : clusters[cluster].titleSize.height + options.titleGap
        }
        func titleAcross(_ cluster: Int) -> Double {
            horizontal ? clusters[cluster].titleSize.height + options.titleGap : 0
        }

        var vertices = nodes.map { node -> Vertex in
            let (width, height) = internalSize(node.size)
            return Vertex(width: width, height: height, isDummy: false, cluster: node.cluster)
        }
        let hasLabels = edges.contains { $0.labelSize != nil && $0.from != $0.to }
        let minLength = hasLabels ? 2 : 1

        // 1. Break cycles: reverse edges that point back up a depth-first search.
        let realEdges = edges.enumerated().filter { $0.element.from != $0.element.to }
        var reversed = Set<Int>()
        do {
            var adjacency = Array(repeating: [(edge: Int, to: Int)](), count: nodes.count)
            for (index, edge) in realEdges { adjacency[edge.from].append((index, edge.to)) }
            var state = Array(repeating: 0, count: nodes.count) // 0 new, 1 on stack, 2 done
            func visit(_ node: Int) {
                state[node] = 1
                for (edge, next) in adjacency[node] {
                    if state[next] == 1 { reversed.insert(edge) } else if state[next] == 0 { visit(next) }
                }
                state[node] = 2
            }
            for node in nodes.indices where state[node] == 0 { visit(node) }
        }
        let directed: [(index: Int, from: Int, to: Int)] = realEdges.map { index, edge in
            reversed.contains(index) ? (index, edge.to, edge.from) : (index, edge.from, edge.to)
        }

        // 2. Ranks: longest path from the sources, then pull sources down next to their
        // first child so they don't float at the top.
        var successors = Array(repeating: [Int](), count: nodes.count)
        var predecessors = Array(repeating: [Int](), count: nodes.count)
        for edge in directed {
            successors[edge.from].append(edge.to)
            predecessors[edge.to].append(edge.from)
        }
        var indegree = predecessors.map(\.count)
        var queue = nodes.indices.filter { indegree[$0] == 0 }
        var topological: [Int] = []
        while !queue.isEmpty {
            let node = queue.removeFirst()
            topological.append(node)
            for next in successors[node] {
                vertices[next].rank = max(vertices[next].rank, vertices[node].rank + minLength)
                indegree[next] -= 1
                if indegree[next] == 0 { queue.append(next) }
            }
        }
        for node in topological.reversed() where predecessors[node].isEmpty && !successors[node].isEmpty {
            let lowest = successors[node].map { vertices[$0].rank }.min()! - minLength
            vertices[node].rank = max(vertices[node].rank, lowest)
        }

        // 3. Long edges become chains through dummy vertices, one per layer crossed. A dummy
        // belongs to the innermost cluster holding both ends, so it never cuts through a
        // box it has no business in.
        var chainsByEdge: [Int: [Int]] = [:]
        var labelVertex: [Int: Int] = [:]
        var down = Array(repeating: [Int](), count: nodes.count)
        var up = Array(repeating: [Int](), count: nodes.count)
        for edge in directed {
            var path = [edge.from]
            let span = vertices[edge.to].rank - vertices[edge.from].rank
            let labelRank = vertices[edge.from].rank + span / 2
            let home = commonCluster(vertices[edge.from].cluster, vertices[edge.to].cluster)
            for rank in stride(from: vertices[edge.from].rank + 1, to: vertices[edge.to].rank, by: 1) {
                var dummy = Vertex(width: 0, height: 0, rank: rank, isDummy: true, cluster: home)
                if rank == labelRank, let label = edges[edge.index].labelSize {
                    (dummy.width, dummy.height) = internalSize(label)
                    labelVertex[edge.index] = vertices.count
                }
                vertices.append(dummy)
                down.append([])
                up.append([])
                path.append(vertices.count - 1)
            }
            path.append(edge.to)
            for (a, b) in zip(path, path.dropFirst()) {
                down[a].append(b)
                up[b].append(a)
            }
            chainsByEdge[edge.index] = path
        }

        // 4. Cluster spans, and border vertices on every layer of each span.
        var span: [Int: ClosedRange<Int>] = [:]
        for vertex in vertices {
            for cluster in chain(vertex.cluster) {
                span[cluster] = span[cluster].map { min($0.lowerBound, vertex.rank)...max($0.upperBound, vertex.rank) } ?? vertex.rank...vertex.rank
            }
        }
        var borderUp: [Int: Int] = [:]
        var borderDown: [Int: Int] = [:]
        var leftBorders: [Int: [Int]] = [:], rightBorders: [Int: [Int]] = [:]
        for cluster in clusters.indices {
            guard let ranks = span[cluster] else { continue }
            for side in [Border.left, .right] {
                var previous: Int?
                for rank in ranks {
                    vertices.append(Vertex(width: 0, height: 0, rank: rank, isDummy: true, cluster: cluster, border: side))
                    down.append([])
                    up.append([])
                    let index = vertices.count - 1
                    if let previous {
                        borderUp[index] = previous
                        borderDown[previous] = index
                    }
                    previous = index
                    if side == .left { leftBorders[cluster, default: []].append(index) } else { rightBorders[cluster, default: []].append(index) }
                }
            }
        }

        // 5. Order within layers.
        let rankCount = (vertices.map(\.rank).max() ?? 0) + 1
        var layers = Array(repeating: [Int](), count: rankCount)
        do {
            // Initial order: depth-first from the sources, so related nodes start close.
            var placed = Array(repeating: false, count: vertices.count)
            func place(_ vertex: Int) {
                guard !placed[vertex] else { return }
                placed[vertex] = true
                layers[vertices[vertex].rank].append(vertex)
                for next in down[vertex] { place(next) }
            }
            for vertex in vertices.indices where up[vertex].isEmpty && vertices[vertex].border == nil { place(vertex) }
            for vertex in vertices.indices { place(vertex) }
        }
        // For counting crossings, cluster walls are edges too: an edge cutting through a
        // box crosses its walls, so the ordering steers it around instead.
        var walls = down
        for (upper, lower) in borderDown { walls[upper].append(lower) }
        let ordering = Ordering(vertices: vertices, chains: chains, up: up, down: down, walls: walls)
        layers = ordering.minimize(layers, sweeps: options.sweeps)

        // 6. Coordinates across each layer.
        func separation(_ a: Int, _ b: Int) -> Double {
            let va = vertices[a], vb = vertices[b]
            let gap: Double
            switch (va.border, vb.border) {
            case (.right, .left):
                gap = options.clusterGap + titleAcross(vb.cluster!)
            case (.left, _):
                // Into a cluster: its padding, plus its title when titles sit across.
                gap = options.clusterPadding + titleAcross(va.cluster!)
            case (_, .right):
                gap = options.clusterPadding
            case (.right, _):
                gap = options.nodeSpacing * 0.75
            case (_, .left):
                gap = options.nodeSpacing * 0.75 + titleAcross(vb.cluster!)
            default:
                gap = switch (va.isDummy, vb.isDummy) {
                case (true, true): options.edgeSpacing
                case (false, false): options.nodeSpacing
                default: (options.nodeSpacing + options.edgeSpacing) / 2
                }
            }
            return (va.width + vb.width) / 2 + gap
        }
        func placeLayer(_ layer: [Int], targets: [Double], weights: [Double]) {
            var offsets = [0.0]
            for position in layer.indices.dropFirst() {
                offsets.append(offsets[position - 1] + separation(layer[position - 1], layer[position]))
            }
            let solved = isotonic(targets: zip(targets, offsets).map { $0 - $1 }, weights: weights)
            for (position, vertex) in layer.enumerated() { vertices[vertex].x = solved[position] + offsets[position] }
        }
        for layer in layers where !layer.isEmpty {
            placeLayer(layer, targets: Array(repeating: 0, count: layer.count), weights: Array(repeating: 1, count: layer.count))
        }
        for iteration in 0..<20 {
            let useUp = iteration % 2 == 0
            let both = iteration >= 16
            for rank in (useUp ? Array(layers.indices) : Array(layers.indices.reversed())) {
                let layer = layers[rank]
                guard !layer.isEmpty else { continue }
                var targets: [Double] = [], weights: [Double] = []
                for vertex in layer {
                    if vertices[vertex].border != nil {
                        // Borders line up with themselves above and below: straight walls.
                        let neighbours = [borderUp[vertex], borderDown[vertex]].compactMap { $0 }
                        targets.append(neighbours.isEmpty ? vertices[vertex].x : neighbours.map { vertices[$0].x }.reduce(0, +) / Double(neighbours.count))
                        weights.append(neighbours.isEmpty ? 0.01 : 6)
                        continue
                    }
                    let neighbours = both ? up[vertex] + down[vertex] : (useUp ? up[vertex] : down[vertex])
                    if neighbours.isEmpty {
                        targets.append(vertices[vertex].x)
                        weights.append(0.01)
                    } else {
                        targets.append(median(neighbours.map { vertices[$0].x }.sorted()))
                        // Keep dummy chains (long edges) straight above all else.
                        weights.append(Double(neighbours.count) * (vertices[vertex].isDummy ? 4 : 1))
                    }
                }
                placeLayer(layer, targets: targets, weights: weights)
            }
        }

        // Square up: each cluster wall becomes one position shared by all its layers (the
        // widest point), then every layer's spacing is enforced in one longest-path pass
        // over the constraint graph. Clusters keep one order everywhere, so the graph is
        // acyclic, and the result has rectangular boxes with nothing intruding.
        let byDepth = clusters.indices.filter { span[$0] != nil }.sorted { chains[$0].count > chains[$1].count }
        if !byDepth.isEmpty {
            func variable(_ vertex: Int) -> Int {
                guard let side = vertices[vertex].border, let cluster = vertices[vertex].cluster else { return vertex }
                return vertices.count + cluster * 2 + (side == .left ? 0 : 1)
            }
            let count = vertices.count + clusters.count * 2
            var position = Array(repeating: 0.0, count: count)
            for (index, vertex) in vertices.enumerated() where vertex.border == nil { position[index] = vertex.x }
            for cluster in byDepth {
                position[vertices.count + cluster * 2] = leftBorders[cluster]!.map { vertices[$0].x }.min()!
                position[vertices.count + cluster * 2 + 1] = rightBorders[cluster]!.map { vertices[$0].x }.max()!
            }
            var after: [Int: [(Int, Double)]] = [:]
            var indegree = Array(repeating: 0, count: count)
            for layer in layers {
                for (a, b) in zip(layer, layer.dropFirst()) {
                    after[variable(a), default: []].append((variable(b), separation(a, b)))
                    indegree[variable(b)] += 1
                }
            }
            var ready = (0..<count).filter { indegree[$0] == 0 }
            var visited = 0
            while let current = ready.popLast() {
                visited += 1
                for (next, gap) in after[current] ?? [] {
                    position[next] = max(position[next], position[current] + gap)
                    indegree[next] -= 1
                    if indegree[next] == 0 { ready.append(next) }
                }
            }
            if visited == count {
                for index in vertices.indices { vertices[index].x = position[variable(index)] }
            }
        }
        let minX = vertices.map { $0.x - $0.width / 2 }.min() ?? 0
        for index in vertices.indices { vertices[index].x -= minX }

        // 7. Layer positions down the page, with room for cluster padding and titles where
        // clusters open and close.
        let rankSpacing = hasLabels ? options.rankSpacing / 2 : options.rankSpacing
        var layerHeight = Array(repeating: 0.0, count: rankCount)
        for vertex in vertices { layerHeight[vertex.rank] = max(layerHeight[vertex.rank], vertex.height) }
        var opening = Array(repeating: 0.0, count: rankCount), closing = Array(repeating: 0.0, count: rankCount)
        for (cluster, ranks) in span {
            opening[ranks.lowerBound] += options.clusterPadding + titleAlong(cluster)
            closing[ranks.upperBound] += options.clusterPadding
        }
        var layerY = Array(repeating: 0.0, count: rankCount)
        var y = 0.0
        for rank in 0..<rankCount {
            y += rank == 0
                ? opening[0] + layerHeight[0] / 2
                : layerHeight[rank - 1] / 2 + closing[rank - 1] + rankSpacing + opening[rank] + layerHeight[rank] / 2
            layerY[rank] = y
        }
        let internalHeight = y + (layerHeight.last ?? 0) / 2 + (closing.last ?? 0)

        // Cluster boxes in internal coordinates, inner ones first so parents can wrap them.
        var internalBoxes = Array(repeating: CGRect.null, count: clusters.count)
        for cluster in byDepth {
            let ranks = span[cluster]!
            let left = leftBorders[cluster]!.map { vertices[$0].x }.min()!
            let right = rightBorders[cluster]!.map { vertices[$0].x }.max()!
            var top = layerY[ranks.lowerBound] - layerHeight[ranks.lowerBound] / 2
            var bottom = layerY[ranks.upperBound] + layerHeight[ranks.upperBound] / 2
            for child in clusters.indices where clusters[child].parent == cluster && !internalBoxes[child].isNull {
                top = min(top, internalBoxes[child].minY)
                bottom = max(bottom, internalBoxes[child].maxY)
            }
            top -= options.clusterPadding + titleAlong(cluster)
            bottom += options.clusterPadding
            internalBoxes[cluster] = CGRect(x: left, y: top, width: right - left, height: bottom - top)
        }
        let internalWidth = max(vertices.map { $0.x + $0.width / 2 }.max() ?? 0, internalBoxes.filter { !$0.isNull }.map(\.maxX).max() ?? 0)

        // 8. Back to the requested direction.
        func point(_ x: Double, _ y: Double) -> CGPoint {
            switch direction {
            case .topBottom: CGPoint(x: x, y: y)
            case .bottomTop: CGPoint(x: x, y: internalHeight - y)
            case .leftRight: CGPoint(x: y, y: x)
            case .rightLeft: CGPoint(x: internalHeight - y, y: x)
            }
        }
        func center(_ vertex: Int) -> CGPoint { point(vertices[vertex].x, layerY[vertices[vertex].rank]) }
        func rect(_ box: CGRect) -> CGRect {
            guard !box.isNull else { return .null }
            let a = point(box.minX, box.minY), b = point(box.maxX, box.maxY)
            return CGRect(x: min(a.x, b.x), y: min(a.y, b.y), width: abs(a.x - b.x), height: abs(a.y - b.y))
        }

        let frames = nodes.indices.map { index -> CGRect in
            let c = center(index)
            return CGRect(x: c.x - nodes[index].size.width / 2, y: c.y - nodes[index].size.height / 2,
                          width: nodes[index].size.width, height: nodes[index].size.height)
        }
        var routes: [Route] = []
        for (index, edge) in edges.enumerated() {
            if edge.from == edge.to {
                routes.append(selfLoop(frames[edge.from], horizontal: horizontal, labelSize: edge.labelSize))
                continue
            }
            guard var path = chainsByEdge[index] else {
                routes.append(Route(points: [], labelCenter: nil))
                continue
            }
            if reversed.contains(index) { path.reverse() }
            var points = path.map(center)
            points[0] = clip(frames[edge.from], nodes[edge.from].boundary, toward: points[1])
            points[points.count - 1] = clip(frames[edge.to], nodes[edge.to].boundary, toward: points[points.count - 2])
            routes.append(Route(points: points, labelCenter: labelVertex[index].map(center)))
        }

        let size = horizontal ? CGSize(width: internalHeight, height: internalWidth) : CGSize(width: internalWidth, height: internalHeight)
        return Result(frames: frames, routes: routes, clusterFrames: internalBoxes.map(rect), size: size)
    }

    // MARK: - Crossing reduction

    /// Barycenter ordering with cluster constraints: within every layer each cluster's
    /// vertices stay together between its borders, and sibling clusters keep one relative
    /// order across all layers so their boxes can't cross.
    struct Ordering {
        let vertices: [Vertex]
        let chains: [[Int]]
        let up: [[Int]]
        let down: [[Int]]
        /// `down` plus cluster walls, for counting crossings.
        let walls: [[Int]]

        func minimize(_ initial: [[Int]], sweeps: Int) -> [[Int]] {
            var keys = clusterKeys(initial)
            var layers = initial.map { layer in
                let width = Double(max(layer.count - 1, 1))
                let fraction = Dictionary(uniqueKeysWithValues: layer.enumerated().map { ($1, Double($0) / width) })
                return arrange(layer, fraction: { fraction[$0]! }, clusterKeys: keys)
            }
            var best = layers
            var bestCrossings = totalCrossings(layers)
            // A few sweeps even at zero crossings: clusters still need settling.
            for sweep in 0..<sweeps where bestCrossings > 0 || sweep < 4 {
                let downward = sweep % 2 == 0
                let ranks = downward ? Array(layers.indices.dropFirst()) : Array(layers.indices.dropLast().reversed())
                keys = clusterKeys(layers)
                for rank in ranks {
                    // Every key is a 0…1 position, measured in the layer it comes from, so
                    // vertices and whole clusters compare on the same scale.
                    let fixed = downward ? layers[rank - 1] : layers[rank + 1]
                    let fixedWidth = Double(max(fixed.count - 1, 1))
                    let ownWidth = Double(max(layers[rank].count - 1, 1))
                    var position: [Int: Double] = [:]
                    for (index, vertex) in fixed.enumerated() { position[vertex] = Double(index) / fixedWidth }
                    var fraction: [Int: Double] = [:]
                    for (index, vertex) in layers[rank].enumerated() {
                        let neighbours = (downward ? up[vertex] : down[vertex]).compactMap { position[$0] }
                        // Vertices without neighbours on that side keep their place.
                        fraction[vertex] = neighbours.isEmpty ? Double(index) / ownWidth : neighbours.reduce(0, +) / Double(neighbours.count)
                    }
                    layers[rank] = arrange(layers[rank], fraction: { fraction[$0]! }, clusterKeys: keys)
                }
                transpose(&layers)
                let crossings = totalCrossings(layers)
                if crossings < bestCrossings || (crossings == bestCrossings && sweep < 4) {
                    bestCrossings = crossings
                    best = layers
                }
            }
            return best
        }

        func chain(_ cluster: Int?) -> [Int] { cluster.map { chains[$0] } ?? [] }

        /// Where each cluster sits on average over all its layers, as a fraction of layer
        /// width: one key per cluster, so siblings sort the same way everywhere.
        func clusterKeys(_ layers: [[Int]]) -> [Int: Double] {
            var sums: [Int: (total: Double, count: Double)] = [:]
            for layer in layers {
                let width = Double(max(layer.count - 1, 1))
                for (index, vertex) in layer.enumerated() where vertices[vertex].border == nil {
                    for cluster in chain(vertices[vertex].cluster) {
                        sums[cluster, default: (0, 0)].total += Double(index) / width
                        sums[cluster, default: (0, 0)].count += 1
                    }
                }
            }
            return sums.mapValues { $0.total / $0.count }
        }

        /// Sorts a layer by `fraction`, cluster by cluster: a cluster's members stay
        /// contiguous, flanked by its borders, and the cluster as a whole sorts by its
        /// global key.
        func arrange(_ layer: [Int], fraction: (Int) -> Double, clusterKeys: [Int: Double]) -> [Int] {
            func items(in parent: Int?, from members: [Int]) -> [Int] {
                var entries: [(key: Double, vertices: [Int])] = []
                var children: [Int: [Int]] = [:]
                var leftBorder: Int?, rightBorder: Int?
                let depth = parent.map { chains[$0].count } ?? 0
                for vertex in members {
                    let v = vertices[vertex]
                    if let side = v.border, v.cluster == parent {
                        if side == .left { leftBorder = vertex } else { rightBorder = vertex }
                        continue
                    }
                    let owners = chain(v.cluster)
                    if owners.count > depth {
                        children[owners[depth], default: []].append(vertex)
                    } else {
                        entries.append((fraction(vertex), [vertex]))
                    }
                }
                for (cluster, members) in children {
                    // Where the cluster sits overall, the same in every layer.
                    let local = members.map(fraction).reduce(0, +) / Double(members.count)
                    entries.append((clusterKeys[cluster] ?? local, items(in: cluster, from: members)))
                }
                let sorted = entries.enumerated().sorted { a, b in
                    a.element.key != b.element.key ? a.element.key < b.element.key : a.offset < b.offset
                }.flatMap(\.element.vertices)
                return (leftBorder.map { [$0] } ?? []) + sorted + (rightBorder.map { [$0] } ?? [])
            }
            return items(in: nil, from: layer)
        }

        /// Swaps neighbours in the same cluster wherever that removes crossings. Only the two
        /// vertices' own edges change when they swap, so each decision looks at just those.
        func transpose(_ layers: inout [[Int]]) {
            var positions = Array(repeating: 0, count: vertices.count)
            for layer in layers {
                for (index, vertex) in layer.enumerated() { positions[vertex] = index }
            }
            func pairCrossings(_ u: Int, _ v: Int, _ neighbours: [[Int]]) -> Int {
                var count = 0
                for a in neighbours[u] {
                    for b in neighbours[v] where positions[b] < positions[a] { count += 1 }
                }
                return count
            }
            var improved = true
            var passes = 0
            while improved, passes < 4 {
                improved = false
                passes += 1
                for rank in layers.indices where layers[rank].count > 1 {
                    for index in 0..<(layers[rank].count - 1) {
                        let u = layers[rank][index], v = layers[rank][index + 1]
                        guard vertices[u].border == nil, vertices[v].border == nil, vertices[u].cluster == vertices[v].cluster else { continue }
                        let now = pairCrossings(u, v, up) + pairCrossings(u, v, down)
                        let swapped = pairCrossings(v, u, up) + pairCrossings(v, u, down)
                        if swapped < now {
                            layers[rank].swapAt(index, index + 1)
                            positions[u] = index + 1
                            positions[v] = index
                            improved = true
                        }
                    }
                }
            }
        }

        func totalCrossings(_ layers: [[Int]]) -> Int {
            zip(layers, layers.dropFirst()).reduce(0) { $0 + LayeredLayout.crossings($1.0, $1.1, down: walls) }
        }
    }

    /// Edge crossings between two adjacent layers: inversions among the lower endpoints,
    /// counted with a Fenwick tree in O(E log V).
    static func crossings(_ upper: [Int], _ lower: [Int], down: [[Int]]) -> Int {
        guard lower.count > 1 else { return 0 }
        var position: [Int: Int] = [:]
        for (index, vertex) in lower.enumerated() { position[vertex] = index }
        var tree = Array(repeating: 0, count: lower.count + 1)
        var seen = 0
        var count = 0
        for vertex in upper {
            let ends = down[vertex].compactMap { position[$0] }.sorted()
            // Edges from the same vertex never cross each other: count, then insert.
            for end in ends {
                var index = end + 1, atMost = 0
                while index > 0 { atMost += tree[index]; index -= index & -index }
                count += seen - atMost
            }
            for end in ends {
                var index = end + 1
                while index <= lower.count { tree[index] += 1; index += index & -index }
                seen += 1
            }
        }
        return count
    }

    // MARK: - Geometry helpers

    /// Weighted isotonic regression (pool adjacent violators): the non-decreasing sequence
    /// closest to `targets`.
    static func isotonic(targets: [Double], weights: [Double]) -> [Double] {
        var blocks: [(value: Double, weight: Double, count: Int)] = []
        for (target, weight) in zip(targets, weights) {
            blocks.append((target, weight, 1))
            while blocks.count > 1, blocks[blocks.count - 2].value > blocks[blocks.count - 1].value {
                let last = blocks.removeLast()
                let previous = blocks.removeLast()
                let total = previous.weight + last.weight
                blocks.append(((previous.value * previous.weight + last.value * last.weight) / total, total, previous.count + last.count))
            }
        }
        return blocks.flatMap { Array(repeating: $0.value, count: $0.count) }
    }

    static func median(_ sorted: [Double]) -> Double {
        let middle = sorted.count / 2
        return sorted.count % 2 == 1 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2
    }

    /// Where the line from a node's center toward `target` leaves its outline.
    public static func clip(_ frame: CGRect, _ boundary: Boundary, toward target: CGPoint) -> CGPoint {
        let center = CGPoint(x: frame.midX, y: frame.midY)
        let dx = target.x - center.x, dy = target.y - center.y
        guard dx != 0 || dy != 0 else { return center }
        let a = frame.width / 2, b = frame.height / 2
        let t: Double = switch boundary {
        case .rectangle: min(dx == 0 ? .infinity : a / abs(dx), dy == 0 ? .infinity : b / abs(dy))
        case .ellipse: 1 / ((dx / a) * (dx / a) + (dy / b) * (dy / b)).squareRoot()
        case .diamond: 1 / (abs(dx) / a + abs(dy) / b)
        }
        return CGPoint(x: center.x + dx * min(t, 1), y: center.y + dy * min(t, 1))
    }

    static func selfLoop(_ frame: CGRect, horizontal: Bool, labelSize: CGSize?) -> Route {
        // Out one side and back in, away from the flow direction.
        if horizontal {
            let top = frame.minY
            let points = [
                CGPoint(x: frame.midX - frame.width / 4, y: top),
                CGPoint(x: frame.midX - frame.width / 4, y: top - 22),
                CGPoint(x: frame.midX + frame.width / 4, y: top - 22),
                CGPoint(x: frame.midX + frame.width / 4, y: top),
            ]
            let label = labelSize.map { CGPoint(x: frame.midX, y: top - 22 - $0.height / 2 - 2) }
            return Route(points: points, labelCenter: label)
        }
        let right = frame.maxX
        let points = [
            CGPoint(x: right, y: frame.midY - frame.height / 4),
            CGPoint(x: right + 24, y: frame.midY - frame.height / 4),
            CGPoint(x: right + 24, y: frame.midY + frame.height / 4),
            CGPoint(x: right, y: frame.midY + frame.height / 4),
        ]
        let label = labelSize.map { CGPoint(x: right + 28 + $0.width / 2, y: frame.midY) }
        return Route(points: points, labelCenter: label)
    }
}
