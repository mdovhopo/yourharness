import HarnessCore
import SwiftUI

/// The memory as a map: a dot a memory (colored by kind, bigger the more it's read), a line
/// a link. Laid out by a small force simulation off the main thread, drawn with Canvas.
struct MemoryGraphView: View {
    let library: MemoryLibrary
    @State private var layout: GraphLayout?
    @State private var hovered: String?
    @State private var picked: String?

    private var nodes: [MemoryLibrary.Memory] { library.memories.filter { $0.status == .active } }

    /// What the layout depends on: which memories there are and how they're linked.
    private var shape: String {
        nodes.map { $0.id + ":" + $0.links.map(\.to).joined(separator: ",") }.joined(separator: ";")
    }

    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .topLeading) {
                if let layout {
                    canvas(layout, size: geometry.size)
                        .onContinuousHover { phase in
                            if case .active(let point) = phase { hovered = nearest(point, layout, geometry.size) } else { hovered = nil }
                        }
                        .onTapGesture { point in picked = nearest(point, layout, geometry.size) }
                } else {
                    ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
                }
                legend.padding(14)
                if let id = picked ?? hovered, let memory = library.byID[id] {
                    card(memory)
                        .frame(width: 300)
                        .padding(14)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomTrailing)
                }
            }
        }
        .task(id: shape) {
            let nodes = nodes.map(\.id)
            let index = Dictionary(uniqueKeysWithValues: nodes.enumerated().map { ($1, $0) })
            let edges = self.nodes.flatMap { memory in memory.links.compactMap { link in index[link.to].map { (index[memory.id]!, $0) } } }
            layout = await Task.detached(priority: .userInitiated) { GraphLayout(ids: nodes, edges: edges) }.value
        }
    }

    private func position(_ point: CGPoint, in size: CGSize) -> CGPoint {
        let inset: CGFloat = 60
        return CGPoint(x: inset + point.x * (size.width - inset * 2), y: inset + point.y * (size.height - inset * 2))
    }

    private func radius(_ memory: MemoryLibrary.Memory) -> CGFloat {
        6 + min(CGFloat(memory.recallCount).squareRoot() * 2.2, 14)
    }

    private func nearest(_ point: CGPoint, _ layout: GraphLayout, _ size: CGSize) -> String? {
        var best: (String, CGFloat)?
        for (id, spot) in zip(layout.ids, layout.points) {
            let p = position(spot, in: size)
            let distance = hypot(p.x - point.x, p.y - point.y)
            if distance < 22, distance < (best?.1 ?? .infinity) { best = (id, distance) }
        }
        return best?.0
    }

    private func canvas(_ layout: GraphLayout, size: CGSize) -> some View {
        let byID = library.byID
        let focus = picked ?? hovered
        let neighbours: Set<String> = focus.map { id in
            Set(layout.edges.flatMap { edge -> [String] in
                let a = layout.ids[edge.0], b = layout.ids[edge.1]
                return a == id ? [b] : b == id ? [a] : []
            })
        } ?? []
        return Canvas { context, size in
            for edge in layout.edges {
                var path = Path()
                path.move(to: position(layout.points[edge.0], in: size))
                path.addLine(to: position(layout.points[edge.1], in: size))
                let lit = focus != nil && (layout.ids[edge.0] == focus || layout.ids[edge.1] == focus)
                context.stroke(path, with: .color(.secondary.opacity(lit ? 0.8 : 0.25)), lineWidth: lit ? 2 : 1)
            }
            for (id, point) in zip(layout.ids, layout.points) {
                guard let memory = byID[id] else { continue }
                let center = position(point, in: size), r = radius(memory)
                let dimmed = focus != nil && id != focus && !neighbours.contains(id)
                let circle = Path(ellipseIn: CGRect(x: center.x - r, y: center.y - r, width: r * 2, height: r * 2))
                context.fill(circle, with: .color(MemoryKind.of(memory.kind).color.opacity(dimmed ? 0.25 : 0.9)))
                // A ring of the surface, so overlapping dots stay apart.
                context.stroke(circle, with: .color(Color(nsColor: .windowBackgroundColor)), lineWidth: 2)
                if memory.pinned {
                    context.stroke(Path(ellipseIn: CGRect(x: center.x - r - 3, y: center.y - r - 3, width: r * 2 + 6, height: r * 2 + 6)),
                                   with: .color(.primary.opacity(0.5)), lineWidth: 1)
                }
                let labelled = id == focus || neighbours.contains(id) || (focus == nil && (memory.recallCount >= 3 || layout.ids.count <= 24))
                if labelled {
                    let title = memory.title.count > 32 ? String(memory.title.prefix(30)) + "…" : memory.title
                    context.draw(Text(title).font(.caption).foregroundStyle(dimmed ? .tertiary : .secondary),
                                 at: CGPoint(x: center.x, y: center.y + r + 9), anchor: .center)
                }
            }
        }
    }

    private var legend: some View {
        VStack(alignment: .leading, spacing: 5) {
            ForEach(MemoryKind.allCases, id: \.self) { kind in
                HStack(spacing: 6) {
                    Circle().fill(kind.color).frame(width: 9, height: 9)
                    Text(kind.label).font(.caption)
                }
            }
            Text("Bigger: read more. Ringed: pinned.")
                .font(.caption2)
                .foregroundStyle(.secondary)
                .padding(.top, 2)
        }
        .padding(10)
        .background(.background.opacity(0.7), in: .rect(cornerRadius: Theme.innerRadius))
        .card(radius: Theme.innerRadius)
    }

    private func card(_ memory: MemoryLibrary.Memory) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Chip(MemoryKind.of(memory.kind).label, tint: MemoryKind.of(memory.kind).color)
                Spacer()
                Text("Read \(memory.recallCount)×").font(.caption.monospacedDigit()).foregroundStyle(.secondary)
            }
            Text(memory.title).font(.headline)
            if !memory.description.isEmpty { Text(memory.description).font(.callout).foregroundStyle(.secondary) }
            if picked == memory.id {
                Button("Open", systemImage: "arrow.right.circle") {
                    library.selection = memory.id
                    library.tab = .memories
                }
                .buttonStyle(.glass)
            }
        }
        .padding(14)
        .glassEffect(.regular, in: .rect(cornerRadius: Theme.floatingRadius))
    }
}

/// Fruchterman–Reingold in a unit square: linked memories pull together, everything pushes
/// apart, a little gravity keeps the loners in view. Seeded by ID, so it lays out the same
/// way every time.
struct GraphLayout: Sendable {
    let ids: [String]
    let edges: [(Int, Int)]
    let points: [CGPoint]

    init(ids: [String], edges: [(Int, Int)]) {
        self.ids = ids
        self.edges = edges
        let n = ids.count
        guard n > 0 else { points = []; return }
        var positions = ids.map { id -> CGPoint in
            var hash: UInt64 = 1469598103934665603
            for byte in id.utf8 { hash = (hash ^ UInt64(byte)) &* 1099511628211 }
            return CGPoint(x: Double(hash & 0xFFFF) / 65535, y: Double((hash >> 16) & 0xFFFF) / 65535)
        }
        let k = (1.0 / Double(n)).squareRoot() * 0.9
        let iterations = n > 300 ? 120 : 260
        var temperature = 0.1
        for _ in 0..<iterations {
            var shift = Array(repeating: CGPoint.zero, count: n)
            for i in 0..<n {
                for j in (i + 1)..<n {
                    var dx = positions[i].x - positions[j].x, dy = positions[i].y - positions[j].y
                    var distance = (dx * dx + dy * dy).squareRoot()
                    if distance < 1e-4 { dx = 0.001 * Double(i - j); dy = 0.001; distance = 0.0015 }
                    let force = k * k / distance
                    shift[i].x += dx / distance * force; shift[i].y += dy / distance * force
                    shift[j].x -= dx / distance * force; shift[j].y -= dy / distance * force
                }
            }
            for (a, b) in edges where a != b {
                let dx = positions[a].x - positions[b].x, dy = positions[a].y - positions[b].y
                let distance = max((dx * dx + dy * dy).squareRoot(), 1e-4)
                let force = distance * distance / k
                shift[a].x -= dx / distance * force; shift[a].y -= dy / distance * force
                shift[b].x += dx / distance * force; shift[b].y += dy / distance * force
            }
            for i in 0..<n {
                shift[i].x += (0.5 - positions[i].x) * 0.6 * k * 10
                shift[i].y += (0.5 - positions[i].y) * 0.6 * k * 10
                let length = max((shift[i].x * shift[i].x + shift[i].y * shift[i].y).squareRoot(), 1e-9)
                let step = min(length, temperature)
                positions[i].x += shift[i].x / length * step
                positions[i].y += shift[i].y / length * step
            }
            temperature *= 0.985
        }
        // Fitted to the square, keeping proportions.
        let xs = positions.map(\.x), ys = positions.map(\.y)
        let minX = xs.min()!, maxX = xs.max()!, minY = ys.min()!, maxY = ys.max()!
        let span = max(maxX - minX, maxY - minY, 1e-6)
        points = positions.map { CGPoint(x: n == 1 ? 0.5 : ($0.x - minX) / span + (1 - (maxX - minX) / span) / 2,
                                         y: n == 1 ? 0.5 : ($0.y - minY) / span + (1 - (maxY - minY) / span) / 2) }
    }
}
