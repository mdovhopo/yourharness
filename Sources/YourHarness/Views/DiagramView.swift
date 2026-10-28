import AppKit
import HarnessCore
import SwiftUI

// MARK: - Layout cache

/// Parsed and laid-out diagrams, keyed by source. Layout is cheap but not free, and rows
/// in a lazy transcript are rebuilt as you scroll.
@MainActor
enum DiagramCache {
    enum Entry {
        case flowchart(FlowchartGeometry, title: String?)
        case sequence(SequenceGeometry, title: String?)
        case failed(String)
    }

    private static var entries: [String: Entry] = [:]
    private static var order: [String] = []

    static func entry(for source: String) -> Entry {
        if let cached = entries[source] { return cached }
        let entry: Entry = switch MermaidDiagram.parse(source) {
        case .success(.flowchart(let chart)): .flowchart(FlowchartLayout.layout(chart, metrics: DiagramStyle.metrics), title: nil)
        case .success(.sequence(let diagram)): .sequence(SequenceLayout.layout(diagram, metrics: DiagramStyle.metrics), title: diagram.title)
        case .failure(let error): .failed(error.message)
        }
        entries[source] = entry
        order.append(source)
        if order.count > 48 { entries[order.removeFirst()] = nil }
        return entry
    }
}

enum DiagramStyle {
    static let fontSize: CGFloat = 13
    static let font = Font.system(size: fontSize)

    static let metrics = TextMetrics(lineHeight: 17) { text in
        Double((text as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: DiagramStyle.fontSize)]).width.rounded(.up))
    }

    static let nodeFill = Color(nsColor: .textBackgroundColor)
    static let nodeStroke = Color.primary.opacity(0.22)
    static let edge = Color.primary.opacity(0.5)
    static let lifeline = Color.primary.opacity(0.18)
    static let groupFill = Color.primary.opacity(0.035)
    static let groupStroke = Color.primary.opacity(0.12)
    static let noteFill = Color.yellow.opacity(0.16)
    static let noteStroke = Color.yellow.opacity(0.55)
    static let padding: CGFloat = 16

    static func color(_ color: DiagramColor) -> Color {
        Color(.sRGB, red: color.red, green: color.green, blue: color.blue, opacity: color.alpha)
    }

    /// Author fills are often pale pastels picked for a white page. Laid over our node
    /// fill at partial strength they read as a tint in both light and dark mode, and the
    /// label stays legible.
    static func tint(_ fill: DiagramColor) -> Color {
        color(fill).opacity(min(fill.alpha, 0.32))
    }
}

// MARK: - Card

/// Takes the width it's offered and the height the diagram needs at that width, in the
/// same layout pass. Measuring the width first and rescaling after made every diagram
/// change height just after it scrolled in, and the transcript jumped.
private struct DiagramFrame: Layout {
    let natural: CGSize

    /// Shrink wide diagrams to the column, but not past where text gets hard to read.
    static func scale(natural: CGSize, width: CGFloat) -> CGFloat {
        guard natural.width > 0, width.isFinite, width > 0 else { return 1 }
        return min(1, max(0.75, (width - DiagramStyle.padding * 2) / natural.width))
    }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let full = natural.width + DiagramStyle.padding * 2
        let width = proposal.width.flatMap { $0.isFinite ? $0 : nil } ?? full
        return CGSize(width: width, height: natural.height * Self.scale(natural: natural, width: width) + DiagramStyle.padding * 2)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        subviews.first?.place(at: bounds.origin, proposal: ProposedViewSize(bounds.size))
    }
}

/// A ```mermaid block: drawn natively, with its source one click away.
struct MermaidBlockView: View {
    let source: String
    @State private var showSource = false
    @State private var hovering = false
    @State private var copied = false
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        let entry = DiagramCache.entry(for: source)
        switch entry {
        case .failed(let message):
            VStack(alignment: .leading, spacing: 6) {
                CodeBlockView(language: "mermaid", code: source)
                Label("Couldn't draw this diagram: \(message)", systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        default:
            card(entry)
        }
    }

    private func card(_ entry: DiagramCache.Entry) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            header(entry)
            if showSource {
                ScrollView(.horizontal) {
                    Text(source)
                        .font(.system(size: MarkdownMetrics.small, design: .monospaced))
                        .textSelection(.enabled)
                        .padding(14)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .scrollIndicators(.never)
                .transition(.opacity)
            } else {
                let size = DiagramCanvas.size(of: entry)
                DiagramFrame(natural: size) {
                    GeometryReader { proxy in
                        let scale = DiagramFrame.scale(natural: size, width: proxy.size.width)
                        ScrollView(.horizontal) {
                            DiagramCanvas(entry: entry)
                                .scaleEffect(scale, anchor: .topLeading)
                                .frame(width: size.width * scale, height: size.height * scale, alignment: .topLeading)
                                .padding(DiagramStyle.padding)
                        }
                        .scrollIndicators(.automatic)
                    }
                }
                .transition(.opacity)
            }
        }
        .card()
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.15), value: hovering)
        .animation(.smooth(duration: 0.2), value: showSource)
    }

    private func header(_ entry: DiagramCache.Entry) -> some View {
        HStack(spacing: 10) {
            Label(kindName(entry), systemImage: kindSymbol(entry))
                .font(.caption.weight(.medium))
                .foregroundStyle(.secondary)
            Spacer()
            HStack(spacing: 12) {
                Button(showSource ? "Show Diagram" : "Show Source", systemImage: showSource ? "point.3.connected.trianglepath.dotted" : "chevron.left.forwardslash.chevron.right") {
                    showSource.toggle()
                }
                .help(showSource ? "Show diagram" : "Show Mermaid source")
                Button(copied ? "Copied" : "Copy Source", systemImage: copied ? "checkmark" : "doc.on.doc") {
                    Pasteboard.copy(source)
                    copied = true
                    Task {
                        try? await Task.sleep(for: .seconds(1.5))
                        copied = false
                    }
                }
                .help("Copy Mermaid source")
                Menu {
                    Button("Copy Image", systemImage: "photo") { DiagramExport.copyImage(entry) }
                    Button("Save as PNG…", systemImage: "square.and.arrow.down") { DiagramExport.savePNG(entry) }
                } label: {
                    Image(systemName: "square.and.arrow.up")
                }
                .menuIndicator(.hidden)
                .fixedSize()
                .help("Export")
                Button("Open Large", systemImage: "arrow.up.left.and.arrow.down.right") {
                    openWindow(id: "diagram", value: source)
                }
                .help("Open in a large window")
            }
            .labelStyle(.iconOnly)
            .buttonStyle(.borderless)
            .opacity(hovering || showSource ? 1 : 0)
        }
        .font(.callout)
        .padding(.horizontal, 12)
        .padding(.top, 8)
        .padding(.bottom, 2)
    }

    private func kindName(_ entry: DiagramCache.Entry) -> String {
        switch entry {
        case .flowchart: "Flowchart"
        case .sequence(_, let title): title ?? "Sequence"
        case .failed: "Diagram"
        }
    }

    private func kindSymbol(_ entry: DiagramCache.Entry) -> String {
        switch entry {
        case .sequence: "arrow.left.arrow.right"
        default: "point.3.connected.trianglepath.dotted"
        }
    }
}

/// Shown while the diagram's code block is still streaming in: a half-written graph
/// would re-lay itself out on every token.
struct DiagramPlaceholder: View {
    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "point.3.connected.trianglepath.dotted")
            Text("Drawing diagram…")
        }
        .font(.callout)
        .foregroundStyle(.secondary)
        .pulsing(low: 0.45, period: 2)
        .frame(maxWidth: .infinity, minHeight: 64)
        .background(Color.primary.opacity(0.025), in: .rect(cornerRadius: 12, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(.separator.opacity(0.6), style: StrokeStyle(lineWidth: 1, dash: [4, 4]))
        }
    }
}

/// A diagram in its own window that fills most of the screen: starts fitted, zooms with
/// pinch or ⌘+ / ⌘−, and Esc closes it.
struct DiagramWindow: View {
    let source: String
    @Environment(\.dismiss) private var dismiss
    @State private var zoom: CGFloat = 1
    /// Until you zoom yourself, the diagram keeps fitting the window (it grows when the
    /// window goes full screen).
    @State private var userZoomed = false
    @GestureState private var pinch: CGFloat = 1
    @State private var viewport: CGSize = .zero

    var body: some View {
        let entry = DiagramCache.entry(for: source)
        let size = DiagramCanvas.size(of: entry)
        let scale = zoom * pinch
        ScrollView([.horizontal, .vertical]) {
            DiagramCanvas(entry: entry)
                .scaleEffect(scale, anchor: .topLeading)
                .frame(width: size.width * scale, height: size.height * scale, alignment: .topLeading)
                .padding(40)
                // Small diagrams sit in the middle instead of the top-left corner.
                .frame(minWidth: viewport.width, minHeight: viewport.height)
        }
        .onGeometryChange(for: CGSize.self) { $0.size } action: { newSize in
            viewport = newSize
            if !userZoomed, newSize.width > 0 { zoom = fitZoom(size) }
        }
        .gesture(MagnifyGesture().updating($pinch) { value, state, _ in state = value.magnification }
            .onEnded {
                zoom = min(4, max(0.25, zoom * $0.magnification))
                userZoomed = true
            })
        .background(Color(nsColor: .windowBackgroundColor))
        .toolbar {
            ToolbarItemGroup {
                Button("Zoom Out", systemImage: "minus.magnifyingglass") {
                    zoom = max(0.25, zoom / 1.25)
                    userZoomed = true
                }
                .keyboardShortcut("-")
                Text(scale, format: .percent.precision(.fractionLength(0)))
                    .monospacedDigit()
                    .frame(width: 48)
                Button("Zoom In", systemImage: "plus.magnifyingglass") {
                    zoom = min(4, zoom * 1.25)
                    userZoomed = true
                }
                .keyboardShortcut("=")
                Button("Fit", systemImage: "arrow.up.left.and.down.right.magnifyingglass") {
                    zoom = fitZoom(size)
                    userZoomed = false
                }
                .keyboardShortcut("0")
                    .help("Fit to window (⌘0)")
                Button("Save as PNG…", systemImage: "square.and.arrow.down") { DiagramExport.savePNG(entry) }
                Button("Done") { dismiss() }
                    .keyboardShortcut(.cancelAction)
            }
        }
        .navigationTitle(DiagramCanvas.title(of: entry))
        .background(FillsScreenOnAppear())
    }

    private func fitZoom(_ size: CGSize) -> CGFloat {
        guard size.width > 0, size.height > 0, viewport.width > 0 else { return 1 }
        return min(2, max(0.25, min((viewport.width - 80) / size.width, (viewport.height - 80) / size.height)))
    }
}

/// Grows its window to most of the screen it's on, centered, the first time it appears.
private struct FillsScreenOnAppear: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        DispatchQueue.main.async {
            guard let window = view.window, let screen = window.screen ?? NSScreen.main else { return }
            let visible = screen.visibleFrame
            let frame = visible.insetBy(dx: visible.width * 0.04, dy: visible.height * 0.04)
            window.setFrame(frame, display: true, animate: true)
        }
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {}
}

// MARK: - Export

@MainActor
enum DiagramExport {
    static func image(_ entry: DiagramCache.Entry) -> NSImage? {
        let content = DiagramCanvas(entry: entry, drawsAsynchronously: false)
            .padding(DiagramStyle.padding)
            .background(Color(nsColor: .windowBackgroundColor))
        let renderer = ImageRenderer(content: content)
        renderer.scale = 2
        return renderer.nsImage
    }

    static func copyImage(_ entry: DiagramCache.Entry) {
        guard let image = image(entry) else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.writeObjects([image])
    }

    static func savePNG(_ entry: DiagramCache.Entry) {
        guard let image = image(entry), let tiff = image.tiffRepresentation,
              let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:])
        else { return }
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.png]
        panel.nameFieldStringValue = "Diagram.png"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        try? png.write(to: url)
    }
}

// MARK: - Drawing

struct DiagramCanvas: View {
    let entry: DiagramCache.Entry
    /// Off for an image made from it, which is drawn once, right then.
    var drawsAsynchronously = true

    static func title(of entry: DiagramCache.Entry) -> String {
        switch entry {
        case .flowchart: "Flowchart"
        case .sequence(_, let title): title ?? "Sequence Diagram"
        case .failed: "Diagram"
        }
    }

    static func size(of entry: DiagramCache.Entry) -> CGSize {
        switch entry {
        case .flowchart(let geometry, _): geometry.size
        case .sequence(let geometry, _): geometry.size
        case .failed: .zero
        }
    }

    var body: some View {
        let size = Self.size(of: entry)
        // Drawn off the main thread: a big diagram is thousands of points of paths and text,
        // painted whole when it comes on screen (switching to a thread that ends in a few
        // of them spent a tenth of a second here).
        Canvas(rendersAsynchronously: drawsAsynchronously) { context, _ in
            switch entry {
            case .flowchart(let geometry, _): FlowchartPainter.draw(geometry, in: &context)
            case .sequence(let geometry, _): SequencePainter.draw(geometry, in: &context)
            case .failed: break
            }
        }
        .frame(width: size.width, height: size.height)
        .accessibilityElement()
        .accessibilityLabel(accessibilityText)
    }

    private var accessibilityText: String {
        switch entry {
        case .flowchart(let geometry, _):
            "Flowchart: " + geometry.edges.map { "\($0.edge.from) to \($0.edge.to)" }.joined(separator: ", ")
        case .sequence(let geometry, _):
            "Sequence diagram: " + geometry.messages.map(\.message.text).joined(separator: ", ")
        case .failed: "Diagram"
        }
    }
}

enum DiagramPaint {
    static func text(_ wrapped: WrappedText, in frame: CGRect, context: inout GraphicsContext,
                     color: Color = .primary, weight: Font.Weight = .regular, alignment: HorizontalAlignment = .center) {
        let lineHeight = DiagramStyle.metrics.lineHeight
        let top = frame.midY - Double(wrapped.lines.count) * lineHeight / 2
        for (index, line) in wrapped.lines.enumerated() {
            let y = top + lineHeight * (Double(index) + 0.5)
            let resolved = context.resolve(Text(line).font(.system(size: DiagramStyle.fontSize, weight: weight)).foregroundStyle(color))
            switch alignment {
            case .leading: context.draw(resolved, at: CGPoint(x: frame.minX, y: y), anchor: .leading)
            default: context.draw(resolved, at: CGPoint(x: frame.midX, y: y), anchor: .center)
            }
        }
    }

    /// A smooth curve through `points` (Catmull-Rom), ending short of `trimEnd`/`trimStart`
    /// so arrowheads sit on the outline instead of under the line.
    static func curve(_ points: [CGPoint], trimStart: Double = 0, trimEnd: Double = 0) -> Path {
        var points = points
        guard points.count >= 2 else { return Path() }
        if trimEnd > 0 { points[points.count - 1] = moved(points[points.count - 1], toward: points[points.count - 2], by: trimEnd) }
        if trimStart > 0 { points[0] = moved(points[0], toward: points[1], by: trimStart) }
        var path = Path()
        path.move(to: points[0])
        if points.count == 2 {
            path.addLine(to: points[1])
            return path
        }
        for index in 0..<(points.count - 1) {
            let p0 = points[max(index - 1, 0)], p1 = points[index], p2 = points[index + 1], p3 = points[min(index + 2, points.count - 1)]
            let c1 = CGPoint(x: p1.x + (p2.x - p0.x) / 6, y: p1.y + (p2.y - p0.y) / 6)
            let c2 = CGPoint(x: p2.x - (p3.x - p1.x) / 6, y: p2.y - (p3.y - p1.y) / 6)
            path.addCurve(to: p2, control1: c1, control2: c2)
        }
        return path
    }

    /// Smooth S-curves between consecutive points, with tangents along the flow axis at
    /// every point. Unlike a spline through the points, it can't overshoot or loop.
    static func flowCurve(_ points: [CGPoint], vertical: Bool, trimStart: Double = 0, trimEnd: Double = 0) -> Path {
        var points = points
        guard points.count >= 2 else { return Path() }
        if trimEnd > 0 { points[points.count - 1] = alongFlowStep(points[points.count - 1], from: points[points.count - 2], by: trimEnd, vertical: vertical) }
        if trimStart > 0 { points[0] = alongFlowStep(points[0], from: points[1], by: trimStart, vertical: vertical) }
        var path = Path()
        path.move(to: points[0])
        for (a, b) in zip(points, points.dropFirst()) {
            if vertical {
                let half = (b.y - a.y) / 2
                path.addCurve(to: b, control1: CGPoint(x: a.x, y: a.y + half), control2: CGPoint(x: b.x, y: b.y - half))
            } else {
                let half = (b.x - a.x) / 2
                path.addCurve(to: b, control1: CGPoint(x: a.x + half, y: a.y), control2: CGPoint(x: b.x - half, y: b.y))
            }
        }
        return path
    }

    /// A point just behind `tip` along the flow axis, for aiming an arrowhead.
    static func alongFlow(_ tip: CGPoint, from previous: CGPoint, vertical: Bool) -> CGPoint {
        vertical
            ? CGPoint(x: tip.x, y: tip.y - (tip.y >= previous.y ? 10 : -10))
            : CGPoint(x: tip.x - (tip.x >= previous.x ? 10 : -10), y: tip.y)
    }

    static func alongFlowStep(_ tip: CGPoint, from previous: CGPoint, by distance: Double, vertical: Bool) -> CGPoint {
        let span = vertical ? abs(tip.y - previous.y) : abs(tip.x - previous.x)
        let step = min(distance, span / 2)
        return vertical
            ? CGPoint(x: tip.x, y: tip.y - (tip.y >= previous.y ? step : -step))
            : CGPoint(x: tip.x - (tip.x >= previous.x ? step : -step), y: tip.y)
    }

    /// Straight segments with rounded corners, for loops.
    static func rounded(_ points: [CGPoint], trimEnd: Double = 0) -> Path {
        var points = points
        guard points.count >= 2 else { return Path() }
        if trimEnd > 0 { points[points.count - 1] = moved(points[points.count - 1], toward: points[points.count - 2], by: trimEnd) }
        var path = Path()
        path.move(to: points[0])
        for index in 1..<points.count - 1 {
            path.addArc(tangent1End: points[index], tangent2End: points[index + 1], radius: 8)
        }
        path.addLine(to: points[points.count - 1])
        return path
    }

    static func moved(_ point: CGPoint, toward target: CGPoint, by distance: Double) -> CGPoint {
        let dx = target.x - point.x, dy = target.y - point.y
        let length = (dx * dx + dy * dy).squareRoot()
        guard length > distance, length > 0 else { return point }
        return CGPoint(x: point.x + dx / length * distance, y: point.y + dy / length * distance)
    }

    /// Draws a head at `tip`, pointing away from `from`.
    static func head(_ kind: Flowchart.Arrow, tip: CGPoint, from: CGPoint, color: Color, context: inout GraphicsContext) {
        let dx = tip.x - from.x, dy = tip.y - from.y
        let length = max((dx * dx + dy * dy).squareRoot(), 0.001)
        let ux = dx / length, uy = dy / length
        switch kind {
        case .none:
            break
        case .arrow:
            let base = CGPoint(x: tip.x - ux * 9, y: tip.y - uy * 9)
            var path = Path()
            path.move(to: tip)
            path.addLine(to: CGPoint(x: base.x - uy * 4.5, y: base.y + ux * 4.5))
            path.addLine(to: CGPoint(x: base.x + uy * 4.5, y: base.y - ux * 4.5))
            path.closeSubpath()
            context.fill(path, with: .color(color))
        case .circle:
            let center = CGPoint(x: tip.x - ux * 4, y: tip.y - uy * 4)
            let circle = Path(ellipseIn: CGRect(x: center.x - 4, y: center.y - 4, width: 8, height: 8))
            context.fill(circle, with: .color(DiagramStyle.nodeFill))
            context.stroke(circle, with: .color(color), lineWidth: 1.4)
        case .cross:
            let center = CGPoint(x: tip.x - ux * 5, y: tip.y - uy * 5)
            var path = Path()
            path.move(to: CGPoint(x: center.x - 4, y: center.y - 4))
            path.addLine(to: CGPoint(x: center.x + 4, y: center.y + 4))
            path.move(to: CGPoint(x: center.x - 4, y: center.y + 4))
            path.addLine(to: CGPoint(x: center.x + 4, y: center.y - 4))
            context.stroke(path, with: .color(color), lineWidth: 1.6)
        }
    }
}

enum FlowchartPainter {
    static func draw(_ geometry: FlowchartGeometry, in context: inout GraphicsContext) {
        for group in geometry.subgraphs {
            let shape = Path(roundedRect: group.frame, cornerRadius: 12, style: .continuous)
            let style = group.subgraph.style
            // Nested boxes get lighter so a stack of them doesn't turn grey.
            context.fill(shape, with: .color(DiagramStyle.groupFill.opacity(group.depth == 0 ? 1 : 0.6)))
            if let fill = style.fill { context.fill(shape, with: .color(DiagramStyle.tint(fill))) }
            context.stroke(shape, with: .color(style.stroke.map(DiagramStyle.color) ?? DiagramStyle.groupStroke),
                           style: StrokeStyle(lineWidth: style.strokeWidth ?? 1, dash: style.dashed ? [6, 4] : []))
            let titleFrame = CGRect(x: group.frame.minX + 12, y: group.frame.minY + 6, width: group.title.size.width, height: group.title.size.height)
            DiagramPaint.text(group.title, in: titleFrame, context: &context, color: .secondary, weight: .semibold, alignment: .leading)
        }

        for edge in geometry.edges where edge.edge.line != .invisible && edge.points.count >= 2 {
            let vertical = edge.vertical
            let points = edge.points
            let width: Double = edge.edge.line == .thick ? 2.6 : 1.3
            let style = StrokeStyle(lineWidth: width, lineCap: .round, lineJoin: .round, dash: edge.edge.line == .dotted ? [4, 4] : [])
            let path: Path
            let endFrom: CGPoint, startFrom: CGPoint
            if edge.isSelfLoop {
                path = DiagramPaint.rounded(points, trimEnd: edge.edge.end == .arrow ? 7 : 0)
                endFrom = points[points.count - 2]
                startFrom = points[1]
            } else {
                path = DiagramPaint.flowCurve(points, vertical: vertical, trimStart: edge.edge.start == .arrow ? 7 : 0, trimEnd: edge.edge.end == .arrow ? 7 : 0)
                // Curves arrive along the flow, so heads point along it too.
                endFrom = DiagramPaint.alongFlow(points[points.count - 1], from: points[points.count - 2], vertical: vertical)
                startFrom = DiagramPaint.alongFlow(points[0], from: points[1], vertical: vertical)
            }
            context.stroke(path, with: .color(DiagramStyle.edge), style: style)
            DiagramPaint.head(edge.edge.end, tip: points[points.count - 1], from: endFrom, color: DiagramStyle.edge, context: &context)
            DiagramPaint.head(edge.edge.start, tip: points[0], from: startFrom, color: DiagramStyle.edge, context: &context)
        }

        for edge in geometry.edges {
            guard let frame = edge.labelFrame else { continue }
            let pill = Path(roundedRect: frame, cornerRadius: 6, style: .continuous)
            context.fill(pill, with: .color(DiagramStyle.nodeFill))
            DiagramPaint.text(edge.label, in: frame, context: &context, color: .secondary)
        }

        for node in geometry.nodes {
            let isDecision = node.node.shape == .diamond
            let outline = path(for: node.node.shape, in: node.frame)
            context.drawLayer { layer in
                layer.addFilter(.shadow(color: .black.opacity(0.06), radius: 2, y: 1))
                layer.fill(outline, with: .color(DiagramStyle.nodeFill))
            }
            let style = node.node.style
            if let fill = style.fill {
                context.fill(outline, with: .color(DiagramStyle.tint(fill)))
            } else if isDecision {
                context.fill(outline, with: .color(Color.accentColor.opacity(0.07)))
            }
            let stroke = style.stroke.map(DiagramStyle.color) ?? (isDecision ? Color.accentColor.opacity(0.55) : DiagramStyle.nodeStroke)
            context.stroke(outline, with: .color(stroke),
                           style: StrokeStyle(lineWidth: style.strokeWidth.map { min(max($0, 1), 4) } ?? 1.2, dash: style.dashed ? [5, 4] : []))
            for detail in details(for: node.node.shape, in: node.frame) {
                context.stroke(detail, with: .color(DiagramStyle.nodeStroke), lineWidth: 1.2)
            }
            DiagramPaint.text(node.label, in: node.frame, context: &context)
        }
    }

    static func path(for shape: Flowchart.Shape, in r: CGRect) -> Path {
        switch shape {
        case .rectangle, .subroutine:
            return Path(roundedRect: r, cornerRadius: 3)
        case .rounded:
            return Path(roundedRect: r, cornerRadius: 10, style: .continuous)
        case .stadium:
            return Path(roundedRect: r, cornerRadius: r.height / 2, style: .continuous)
        case .circle, .doubleCircle:
            return Path(ellipseIn: r)
        case .diamond:
            return polygon([CGPoint(x: r.midX, y: r.minY), CGPoint(x: r.maxX, y: r.midY), CGPoint(x: r.midX, y: r.maxY), CGPoint(x: r.minX, y: r.midY)])
        case .hexagon:
            let inset = r.height / 2.5
            return polygon([CGPoint(x: r.minX + inset, y: r.minY), CGPoint(x: r.maxX - inset, y: r.minY), CGPoint(x: r.maxX, y: r.midY),
                            CGPoint(x: r.maxX - inset, y: r.maxY), CGPoint(x: r.minX + inset, y: r.maxY), CGPoint(x: r.minX, y: r.midY)])
        case .parallelogram:
            let slant = r.height / 2
            return polygon([CGPoint(x: r.minX + slant, y: r.minY), CGPoint(x: r.maxX, y: r.minY), CGPoint(x: r.maxX - slant, y: r.maxY), CGPoint(x: r.minX, y: r.maxY)])
        case .parallelogramAlt:
            let slant = r.height / 2
            return polygon([CGPoint(x: r.minX, y: r.minY), CGPoint(x: r.maxX - slant, y: r.minY), CGPoint(x: r.maxX, y: r.maxY), CGPoint(x: r.minX + slant, y: r.maxY)])
        case .trapezoid:
            let slant = r.height / 2
            return polygon([CGPoint(x: r.minX + slant, y: r.minY), CGPoint(x: r.maxX - slant, y: r.minY), CGPoint(x: r.maxX, y: r.maxY), CGPoint(x: r.minX, y: r.maxY)])
        case .trapezoidAlt:
            let slant = r.height / 2
            return polygon([CGPoint(x: r.minX, y: r.minY), CGPoint(x: r.maxX, y: r.minY), CGPoint(x: r.maxX - slant, y: r.maxY), CGPoint(x: r.minX + slant, y: r.maxY)])
        case .flag:
            let notch = r.height / 2.5
            return polygon([CGPoint(x: r.minX, y: r.minY), CGPoint(x: r.maxX, y: r.minY), CGPoint(x: r.maxX, y: r.maxY), CGPoint(x: r.minX, y: r.maxY), CGPoint(x: r.minX + notch, y: r.midY)])
        case .cylinder:
            let cap = 8.0
            var path = Path()
            path.move(to: CGPoint(x: r.minX, y: r.minY + cap))
            path.addLine(to: CGPoint(x: r.minX, y: r.maxY - cap))
            path.addCurve(to: CGPoint(x: r.maxX, y: r.maxY - cap), control1: CGPoint(x: r.minX, y: r.maxY + cap / 3), control2: CGPoint(x: r.maxX, y: r.maxY + cap / 3))
            path.addLine(to: CGPoint(x: r.maxX, y: r.minY + cap))
            path.addCurve(to: CGPoint(x: r.minX, y: r.minY + cap), control1: CGPoint(x: r.maxX, y: r.minY - cap / 3), control2: CGPoint(x: r.minX, y: r.minY - cap / 3))
            path.closeSubpath()
            return path
        }
    }

    /// Inner strokes that finish a shape: subroutine bars, the cylinder's rim, the inner ring.
    static func details(for shape: Flowchart.Shape, in r: CGRect) -> [Path] {
        switch shape {
        case .subroutine:
            return [
                Path { $0.move(to: CGPoint(x: r.minX + 8, y: r.minY)); $0.addLine(to: CGPoint(x: r.minX + 8, y: r.maxY)) },
                Path { $0.move(to: CGPoint(x: r.maxX - 8, y: r.minY)); $0.addLine(to: CGPoint(x: r.maxX - 8, y: r.maxY)) },
            ]
        case .cylinder:
            let cap = 8.0
            return [Path { path in
                path.move(to: CGPoint(x: r.minX, y: r.minY + cap))
                path.addCurve(to: CGPoint(x: r.maxX, y: r.minY + cap), control1: CGPoint(x: r.minX, y: r.minY + cap * 1.33), control2: CGPoint(x: r.maxX, y: r.minY + cap * 1.33))
            }]
        case .doubleCircle:
            return [Path(ellipseIn: r.insetBy(dx: 5, dy: 5))]
        default:
            return []
        }
    }

    static func polygon(_ points: [CGPoint]) -> Path {
        var path = Path()
        path.addLines(points)
        path.closeSubpath()
        return path
    }
}

enum SequencePainter {
    static func draw(_ geometry: SequenceGeometry, in context: inout GraphicsContext) {
        // Block outlines underneath everything; their labels go on top, after the bars.
        for block in geometry.blocks {
            let outline = Path(roundedRect: block.frame, cornerRadius: 8, style: .continuous)
            if block.kind == .rect {
                context.fill(outline, with: .color(Color.accentColor.opacity(0.05)))
                continue
            }
            context.fill(outline, with: .color(DiagramStyle.groupFill.opacity(0.6)))
            context.stroke(outline, with: .color(DiagramStyle.groupStroke), lineWidth: 1)
            for section in block.sections.dropFirst() {
                var divider = Path()
                divider.move(to: CGPoint(x: block.frame.minX, y: section.y))
                divider.addLine(to: CGPoint(x: block.frame.maxX, y: section.y))
                context.stroke(divider, with: .color(DiagramStyle.groupStroke), style: StrokeStyle(lineWidth: 1, dash: [4, 3]))
            }
        }

        for participant in geometry.participants {
            var lifeline = Path()
            lifeline.move(to: CGPoint(x: participant.centerX, y: geometry.lifelineTop))
            lifeline.addLine(to: CGPoint(x: participant.centerX, y: geometry.lifelineBottom))
            context.stroke(lifeline, with: .color(DiagramStyle.lifeline), style: StrokeStyle(lineWidth: 1, dash: [4, 4]))
        }

        for bar in geometry.activations {
            let shape = Path(roundedRect: bar, cornerRadius: 2)
            context.fill(shape, with: .color(DiagramStyle.nodeFill))
            context.fill(shape, with: .color(Color.accentColor.opacity(0.12)))
            context.stroke(shape, with: .color(Color.accentColor.opacity(0.5)), lineWidth: 1)
        }

        for block in geometry.blocks where block.kind != .rect {
            drawBlockLabels(block, in: &context)
        }

        for participant in geometry.participants {
            let box = Path(roundedRect: participant.header, cornerRadius: 8, style: .continuous)
            context.drawLayer { layer in
                layer.addFilter(.shadow(color: .black.opacity(0.06), radius: 2, y: 1))
                layer.fill(box, with: .color(DiagramStyle.nodeFill))
            }
            context.stroke(box, with: .color(DiagramStyle.nodeStroke), lineWidth: 1.2)
            var labelFrame = participant.header
            if participant.participant.isActor {
                let icon = context.resolve(Image(systemName: "person.fill"))
                context.draw(icon, at: CGPoint(x: participant.header.midX, y: participant.header.minY + 15), anchor: .center)
                labelFrame.origin.y += 18
                labelFrame.size.height -= 18
            }
            DiagramPaint.text(participant.label, in: labelFrame, context: &context, weight: .medium)
        }

        for note in geometry.notes {
            let shape = Path(roundedRect: note.frame, cornerRadius: 6, style: .continuous)
            context.fill(shape, with: .color(DiagramStyle.nodeFill))
            context.fill(shape, with: .color(DiagramStyle.noteFill))
            context.stroke(shape, with: .color(DiagramStyle.noteStroke), lineWidth: 1)
            DiagramPaint.text(note.text, in: note.frame, context: &context)
        }

        for message in geometry.messages {
            let style = StrokeStyle(lineWidth: 1.3, lineCap: .round, dash: message.message.line == .dotted ? [4, 4] : [])
            let color = DiagramStyle.edge
            let head = arrow(for: message.message.head)
            if message.isSelf {
                let x = message.fromX, y = message.y
                let points = [CGPoint(x: x, y: y), CGPoint(x: x + 32, y: y), CGPoint(x: x + 32, y: y + 20), CGPoint(x: x, y: y + 20)]
                var path = Path()
                path.move(to: points[0])
                path.addLine(to: points[1])
                path.addLine(to: points[2])
                path.addLine(to: DiagramPaint.moved(points[3], toward: points[2], by: head == .arrow ? 7 : 0))
                context.stroke(path, with: .color(color), style: style)
                DiagramPaint.head(head, tip: points[3], from: points[2], color: color, context: &context)
                DiagramPaint.text(message.label, in: message.labelFrame, context: &context, alignment: .leading)
            } else {
                let start = CGPoint(x: message.fromX, y: message.y), end = CGPoint(x: message.toX, y: message.y)
                let path = DiagramPaint.curve([start, end], trimStart: message.message.bidirectional && head == .arrow ? 7 : 0, trimEnd: head == .arrow ? 7 : 0)
                context.stroke(path, with: .color(color), style: style)
                if message.message.head == .open {
                    openHead(tip: end, from: start, color: color, context: &context)
                } else {
                    DiagramPaint.head(head, tip: end, from: start, color: color, context: &context)
                }
                if message.message.bidirectional { DiagramPaint.head(head, tip: start, from: end, color: color, context: &context) }
                DiagramPaint.text(message.label, in: message.labelFrame, context: &context)
            }
            if let number = message.number {
                let direction: Double = message.toX >= message.fromX ? 1 : -1
                let center = CGPoint(x: message.fromX + direction * 10, y: message.y)
                context.fill(Path(ellipseIn: CGRect(x: center.x - 8, y: center.y - 8, width: 16, height: 16)), with: .color(.accentColor))
                let text = context.resolve(Text("\(number)").font(.system(size: 9, weight: .bold)).foregroundStyle(.white))
                context.draw(text, at: center, anchor: .center)
            }
        }
    }

    static func drawBlockLabels(_ block: SequenceGeometry.PlacedBlock, in context: inout GraphicsContext) {
        let frame = block.frame
        for (index, section) in block.sections.enumerated() {
            if index == 0 {
                let tabText = WrappedText.wrap(block.kind.rawValue, maxWidth: 200, metrics: DiagramStyle.metrics)
                let tab = CGRect(x: frame.minX, y: frame.minY, width: tabText.size.width + 16, height: DiagramStyle.metrics.lineHeight + 6)
                let tabShape = Path(roundedRect: tab, cornerRadii: RectangleCornerRadii(topLeading: 8, bottomTrailing: 6))
                context.fill(tabShape, with: .color(DiagramStyle.nodeFill))
                context.fill(tabShape, with: .color(DiagramStyle.groupStroke))
                DiagramPaint.text(tabText, in: tab, context: &context, color: .secondary, weight: .semibold)
                if !section.label.isEmpty {
                    label(bracketed(section.label), at: CGPoint(x: tab.maxX + 6, y: frame.minY + 2), in: &context)
                }
            } else if !section.label.isEmpty {
                label(bracketed(section.label), at: CGPoint(x: frame.minX + 8, y: section.y + 3), in: &context)
            }
        }
    }

    /// Section labels sit on a backing so lifelines and bars don't run through them.
    static func label(_ text: WrappedText, at origin: CGPoint, in context: inout GraphicsContext) {
        let frame = CGRect(origin: origin, size: text.size).insetBy(dx: -4, dy: -1)
        context.fill(Path(roundedRect: frame, cornerRadius: 4), with: .color(DiagramStyle.nodeFill.opacity(0.85)))
        DiagramPaint.text(text, in: frame.insetBy(dx: 4, dy: 1), context: &context, color: .secondary, alignment: .leading)
    }

    static func arrow(for head: SequenceDiagram.ArrowHead) -> Flowchart.Arrow {
        switch head {
        case .filled: .arrow
        case .cross: .cross
        case .none, .open: .none
        }
    }

    /// The async `-)` head: two strokes, no fill.
    static func openHead(tip: CGPoint, from: CGPoint, color: Color, context: inout GraphicsContext) {
        let direction: Double = tip.x >= from.x ? 1 : -1
        var path = Path()
        path.move(to: CGPoint(x: tip.x - direction * 8, y: tip.y - 5))
        path.addLine(to: tip)
        path.addLine(to: CGPoint(x: tip.x - direction * 8, y: tip.y + 5))
        context.stroke(path, with: .color(color), style: StrokeStyle(lineWidth: 1.4, lineCap: .round, lineJoin: .round))
    }

    static func bracketed(_ text: WrappedText) -> WrappedText {
        var text = text
        guard !text.lines.isEmpty else { return text }
        text.lines[0] = "[" + text.lines[0]
        text.lines[text.lines.count - 1] += "]"
        return text
    }
}
