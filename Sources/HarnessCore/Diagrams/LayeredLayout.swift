import CoreGraphics
import Foundation

/// Layered ("Sugiyama") graph layout, the same family of algorithm dagre and Graphviz's
/// `dot` use: break cycles, assign layers, insert dummy nodes for long edges, reduce
/// crossings with barycenter sweeps plus adjacent swaps, then place nodes in each layer
/// with weighted isotonic regression so edges run as straight as the spacing allows.
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
        /// Nodes sharing a group are kept next to each other within each layer.
        public var group: Int?

        public init(size: CGSize, boundary: Boundary = .rectangle, group: Int? = nil) {
            self.size = size
            self.boundary = boundary
            self.group = group
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

    public struct Options: Sendable {
        public var nodeSpacing: Double = 36
        public var edgeSpacing: Double = 14
        public var rankSpacing: Double = 52
        public var sweeps: Int = 10
        /// Room kept between a group's members and the nodes around it, for its box.
        public var groupClearance: Double = 24

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
        public var size: CGSize
    }

    // MARK: - Internal graph

    struct Vertex {
        var width: Double
        var height: Double
        var rank = 0
        var x = 0.0
        var isDummy: Bool
        var group: Int?
    }

    public static func layout(nodes: [Node], edges: [Edge], direction: Direction, options: Options = Options()) -> Result {
        guard !nodes.isEmpty else { return Result(frames: [], routes: edges.map { _ in Route(points: [], labelCenter: nil) }, size: .zero) }
        let horizontal = direction.isHorizontal
        // Work top-to-bottom; for left-to-right, a node's width runs along the layer.
        func internalSize(_ size: CGSize) -> (Double, Double) {
            horizontal ? (size.height, size.width) : (size.width, size.height)
        }

        var vertices = nodes.map { node -> Vertex in
            let (width, height) = internalSize(node.size)
            return Vertex(width: width, height: height, isDummy: false, group: node.group)
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

        // 3. Long edges become chains through dummy vertices, one per layer crossed.
        var chains: [Int: [Int]] = [:]
        var labelVertex: [Int: Int] = [:]
        var down = Array(repeating: [Int](), count: nodes.count)
        var up = Array(repeating: [Int](), count: nodes.count)
        func connect(_ a: Int, _ b: Int) {
            down[a].append(b)
            up[b].append(a)
        }
        for edge in directed {
            var chain = [edge.from]
            let span = vertices[edge.to].rank - vertices[edge.from].rank
            let labelRank = vertices[edge.from].rank + span / 2
            for rank in stride(from: vertices[edge.from].rank + 1, to: vertices[edge.to].rank, by: 1) {
                var dummy = Vertex(width: 0, height: 0, rank: rank, isDummy: true, group: nil)
                if rank == labelRank, let label = edges[edge.index].labelSize {
                    (dummy.width, dummy.height) = internalSize(label)
                    labelVertex[edge.index] = vertices.count
                }
                // Dummies between two members of the same group stay inside it.
                if let group = vertices[edge.from].group, group == vertices[edge.to].group { dummy.group = group }
                vertices.append(dummy)
                down.append([])
                up.append([])
                chain.append(vertices.count - 1)
            }
            chain.append(edge.to)
            for (a, b) in zip(chain, chain.dropFirst()) { connect(a, b) }
            chains[edge.index] = chain
        }

        // 4. Order within layers.
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
            for vertex in vertices.indices where up[vertex].isEmpty { place(vertex) }
            for vertex in vertices.indices { place(vertex) }
        }
        layers = minimizeCrossings(layers, vertices: vertices, up: up, down: down, sweeps: options.sweeps)

        // 5. Coordinates across each layer.
        func separation(_ a: Int, _ b: Int) -> Double {
            let gap: Double = switch (vertices[a].isDummy, vertices[b].isDummy) {
            case (true, true): options.edgeSpacing
            case (false, false): options.nodeSpacing
            default: (options.nodeSpacing + options.edgeSpacing) / 2
            }
            return (vertices[a].width + vertices[b].width) / 2 + gap
        }
        for layer in layers {
            var x = 0.0
            for (position, vertex) in layer.enumerated() {
                if position > 0 { x += separation(layer[position - 1], vertex) }
                vertices[vertex].x = x
            }
        }
        for iteration in 0..<16 {
            let useUp = iteration % 2 == 0
            let order = useUp ? Array(layers.indices) : Array(layers.indices.reversed())
            for rank in order {
                let layer = layers[rank]
                guard !layer.isEmpty else { continue }
                var targets: [Double] = []
                var weights: [Double] = []
                for vertex in layer {
                    let neighbours = iteration >= 14 ? up[vertex] + down[vertex] : (useUp ? up[vertex] : down[vertex])
                    if neighbours.isEmpty {
                        targets.append(vertices[vertex].x)
                        weights.append(0.01)
                    } else {
                        let xs = neighbours.map { vertices[$0].x }.sorted()
                        targets.append(median(xs))
                        // Keep dummy chains (long edges) straight above all else.
                        weights.append(Double(neighbours.count) * (vertices[vertex].isDummy ? 4 : 1))
                    }
                }
                var offsets = [0.0]
                for position in 1..<layer.count {
                    offsets.append(offsets[position - 1] + separation(layer[position - 1], layer[position]))
                }
                let solved = isotonic(targets: zip(targets, offsets).map { $0 - $1 }, weights: weights)
                for (position, vertex) in layer.enumerated() {
                    vertices[vertex].x = solved[position] + offsets[position]
                }
            }
        }
        separateGroups(&vertices, layers: layers, options: options)
        let minX = vertices.map { $0.x - $0.width / 2 }.min() ?? 0
        for index in vertices.indices { vertices[index].x -= minX }

        // 6. Layer positions down the page.
        let rankSpacing = hasLabels ? options.rankSpacing / 2 : options.rankSpacing
        var layerHeight = Array(repeating: 0.0, count: rankCount)
        for vertex in vertices { layerHeight[vertex.rank] = max(layerHeight[vertex.rank], vertex.height) }
        var layerY = Array(repeating: 0.0, count: rankCount)
        var y = 0.0
        for rank in 0..<rankCount {
            if rank > 0 {
                // Empty layers (only possible with labels) still keep some room.
                y += layerHeight[rank - 1] / 2 + rankSpacing + layerHeight[rank] / 2
            } else {
                y = layerHeight[0] / 2
            }
            layerY[rank] = y
        }
        let internalWidth = vertices.map { $0.x + $0.width / 2 }.max() ?? 0
        let internalHeight = y + (layerHeight.last ?? 0) / 2

        // 7. Back to the requested direction.
        func point(_ x: Double, _ y: Double) -> CGPoint {
            switch direction {
            case .topBottom: CGPoint(x: x, y: y)
            case .bottomTop: CGPoint(x: x, y: internalHeight - y)
            case .leftRight: CGPoint(x: y, y: x)
            case .rightLeft: CGPoint(x: internalHeight - y, y: x)
            }
        }
        func center(_ vertex: Int) -> CGPoint { point(vertices[vertex].x, layerY[vertices[vertex].rank]) }

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
            guard var chain = chains[index] else {
                routes.append(Route(points: [], labelCenter: nil))
                continue
            }
            if reversed.contains(index) { chain.reverse() }
            var points = chain.map(center)
            points[0] = clip(frames[edge.from], nodes[edge.from].boundary, toward: points[1])
            points[points.count - 1] = clip(frames[edge.to], nodes[edge.to].boundary, toward: points[points.count - 2])
            routes.append(Route(points: points, labelCenter: labelVertex[index].map(center)))
        }

        let size = horizontal ? CGSize(width: internalHeight, height: internalWidth) : CGSize(width: internalWidth, height: internalHeight)
        return Result(frames: frames, routes: routes, size: size)
    }

    // MARK: - Groups

    /// A group's box spans every layer from its first member to its last, so nodes outside
    /// the group in those layers are pushed clear of it, to whichever side they already lean.
    static func separateGroups(_ vertices: inout [Vertex], layers: [[Int]], options: Options) {
        let groups = Set(vertices.compactMap(\.group))
        guard !groups.isEmpty else { return }
        let clearance = options.groupClearance
        func separation(_ a: Int, _ b: Int) -> Double {
            let gap: Double = switch (vertices[a].isDummy, vertices[b].isDummy) {
            case (true, true): options.edgeSpacing
            case (false, false): options.nodeSpacing
            default: (options.nodeSpacing + options.edgeSpacing) / 2
            }
            return (vertices[a].width + vertices[b].width) / 2 + gap
        }
        for _ in 0..<2 {
            for group in groups.sorted() {
                let members = vertices.indices.filter { vertices[$0].group == group }
                guard let left = members.map({ vertices[$0].x - vertices[$0].width / 2 }).min(),
                      let right = members.map({ vertices[$0].x + vertices[$0].width / 2 }).max()
                else { continue }
                let center = (left + right) / 2
                // The box only covers the layers its members are in.
                let ranks = members.map { vertices[$0].rank }
                guard let top = ranks.min(), let bottom = ranks.max() else { continue }
                for layer in layers[top...bottom] {
                    let firstMember = layer.firstIndex { vertices[$0].group == group }
                    let lastMember = layer.lastIndex { vertices[$0].group == group }
                    // Decide each outsider's side: by order when the layer has members, else by position.
                    var leftSide: [Int] = [], rightSide: [Int] = []
                    for (position, vertex) in layer.enumerated() where vertices[vertex].group != group {
                        let goesLeft = firstMember.map { position < $0 } ?? (vertices[vertex].x < center)
                        if goesLeft { leftSide.append(position) } else if lastMember.map({ position > $0 }) ?? true { rightSide.append(position) }
                    }
                    // Right side: push right, keeping everything after it separated.
                    var floor = right + clearance
                    for position in rightSide.sorted() {
                        let vertex = layer[position]
                        floor = max(floor, position > 0 ? vertices[layer[position - 1]].x + separation(layer[position - 1], vertex) - vertices[vertex].width / 2 : floor)
                        if vertices[vertex].x - vertices[vertex].width / 2 < floor { vertices[vertex].x = floor + vertices[vertex].width / 2 }
                        floor = -.infinity
                    }
                    for position in layer.indices.dropFirst() {
                        let minimum = vertices[layer[position - 1]].x + separation(layer[position - 1], layer[position])
                        if vertices[layer[position]].x < minimum { vertices[layer[position]].x = minimum }
                    }
                    // Left side: push left, keeping everything before it separated.
                    var ceiling = left - clearance
                    for position in leftSide.sorted(by: >) {
                        let vertex = layer[position]
                        if vertices[vertex].x + vertices[vertex].width / 2 > ceiling { vertices[vertex].x = ceiling - vertices[vertex].width / 2 }
                        ceiling = .infinity
                    }
                    for position in layer.indices.dropLast().reversed() {
                        let maximum = vertices[layer[position + 1]].x - separation(layer[position], layer[position + 1])
                        if vertices[layer[position]].x > maximum { vertices[layer[position]].x = maximum }
                    }
                }
            }
        }
    }

    // MARK: - Crossing reduction

    static func minimizeCrossings(_ initial: [[Int]], vertices: [Vertex], up: [[Int]], down: [[Int]], sweeps: Int) -> [[Int]] {
        var layers = initial
        var best = layers
        var bestCrossings = totalCrossings(layers, down: down)
        guard bestCrossings > 0 || vertices.contains(where: { $0.group != nil }) else { return layers }

        for sweep in 0..<sweeps {
            let downward = sweep % 2 == 0
            let ranks = downward ? Array(1..<max(layers.count, 1)) : Array((0..<max(layers.count - 1, 0)).reversed())
            for rank in ranks where rank >= 0 && rank < layers.count {
                let fixed = downward ? layers[rank - 1] : layers[rank + 1]
                var position: [Int: Double] = [:]
                for (index, vertex) in fixed.enumerated() { position[vertex] = Double(index) }
                let current = layers[rank]
                let keys = current.enumerated().map { index, vertex -> (Int, Double) in
                    let neighbours = (downward ? up[vertex] : down[vertex]).compactMap { position[$0] }
                    // Vertices without neighbours on that side keep their place.
                    return (vertex, neighbours.isEmpty ? Double(index) : neighbours.reduce(0, +) / Double(neighbours.count))
                }
                layers[rank] = keys.enumerated().sorted { a, b in
                    a.element.1 != b.element.1 ? a.element.1 < b.element.1 : a.offset < b.offset
                }.map(\.element.0)
                layers[rank] = groupContiguously(layers[rank], vertices: vertices)
            }
            transpose(&layers, vertices: vertices, up: up, down: down)
            let crossings = totalCrossings(layers, down: down)
            if crossings < bestCrossings {
                bestCrossings = crossings
                best = layers
            }
            if bestCrossings == 0 { break }
        }
        return best.map { groupContiguously($0, vertices: vertices) }
    }

    /// Members of a group move together, to where the group sits on average.
    static func groupContiguously(_ layer: [Int], vertices: [Vertex]) -> [Int] {
        guard layer.contains(where: { vertices[$0].group != nil }) else { return layer }
        var slots: [(key: Double, members: [Int])] = []
        var groupSlot: [Int: Int] = [:]
        for (index, vertex) in layer.enumerated() {
            if let group = vertices[vertex].group {
                if let slot = groupSlot[group] {
                    slots[slot].members.append(vertex)
                    let count = Double(slots[slot].members.count)
                    slots[slot].key += (Double(index) - slots[slot].key) / count
                } else {
                    groupSlot[group] = slots.count
                    slots.append((Double(index), [vertex]))
                }
            } else {
                slots.append((Double(index), [vertex]))
            }
        }
        return slots.enumerated().sorted { a, b in
            a.element.key != b.element.key ? a.element.key < b.element.key : a.offset < b.offset
        }.flatMap(\.element.members)
    }

    /// Swaps neighbours wherever that removes crossings. Only the two vertices' own edges
    /// change when they swap, so each decision looks at just those.
    static func transpose(_ layers: inout [[Int]], vertices: [Vertex], up: [[Int]], down: [[Int]]) {
        var positions = Array(repeating: 0, count: vertices.count)
        for layer in layers {
            for (index, vertex) in layer.enumerated() { positions[vertex] = index }
        }
        /// Crossings between u's and v's edges into one neighbouring layer, with u left of v.
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
                    if vertices[u].group != vertices[v].group { continue }
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

    static func totalCrossings(_ layers: [[Int]], down: [[Int]]) -> Int {
        zip(layers, layers.dropFirst()).reduce(0) { $0 + crossings($1.0, $1.1, down: down) }
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
