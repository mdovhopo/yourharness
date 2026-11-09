// What a transcript shows beyond text, fetched from the Mac as it's needed: pictures (yours, and
// the ones Claude showed you), Mermaid diagrams drawn as the Mac draws them, and a sub-agent's
// run. Each is asked for once and kept; views ask the caches and are told when it arrives.

import SwiftUI
import UIKit

// MARK: Pictures

/// A row's pictures, by "thread#seq#index": asked of the Mac once, kept while the app runs.
@MainActor
final class MediaCache {
    static let shared = MediaCache()
    private var images: [String: UIImage] = [:]
    private var failed: [String: String] = [:]
    private var waiting: [String: [(UIImage?) -> Void]] = [:]
    /// How to ask the Mac (the store sets it).
    var ask: ((String, Int, Int) -> Void)?

    static func key(_ thread: String, _ seq: Int, _ index: Int) -> String { "\(thread)#\(seq)#\(index)" }

    /// The picture now if it's here; otherwise `done` once it is (nil if it can't be had).
    func image(thread: String, seq: Int, index: Int, done: @escaping (UIImage?) -> Void) -> UIImage? {
        let key = Self.key(thread, seq, index)
        if let image = images[key] { return image }
        if failed[key] != nil {
            done(nil)
            return nil
        }
        let first = waiting[key] == nil
        waiting[key, default: []].append(done)
        if first { ask?(thread, seq, index) }
        return nil
    }

    func deliver(thread: String, seq: Int, index: Int, image: UIImage?, error: String?) {
        let key = Self.key(thread, seq, index)
        if let image { images[key] = image } else { failed[key] = error ?? "No picture." }
        for done in waiting.removeValue(forKey: key) ?? [] { done(image) }
    }
}

/// A picture the size of its frame, loaded from the Mac when first shown. Tap opens it.
final class PictureView: UIImageView {
    private var token = ""
    var onTap: (() -> Void)?

    override init(frame: CGRect) {
        super.init(frame: frame)
        contentMode = .scaleAspectFill
        clipsToBounds = true
        layer.cornerRadius = 12
        layer.cornerCurve = .continuous
        backgroundColor = .tertiarySystemFill
        isUserInteractionEnabled = true
        addGestureRecognizer(UITapGestureRecognizer(target: self, action: #selector(tapped)))
        isAccessibilityElement = true
        accessibilityLabel = "Picture"
        accessibilityTraits = [.image, .button]
    }

    required init?(coder: NSCoder) { fatalError() }

    @objc private func tapped() { onTap?() }

    func load(thread: String, seq: Int, index: Int) {
        let key = MediaCache.key(thread, seq, index)
        guard key != token else { return }
        token = key
        image = MediaCache.shared.image(thread: thread, seq: seq, index: index) { [weak self] image in
            guard let self, self.token == key else { return }
            self.image = image ?? UIImage(systemName: "photo.badge.exclamationmark")
            self.contentMode = image == nil ? .center : .scaleAspectFill
        }
        contentMode = .scaleAspectFill
    }
}

/// A row's pictures, one after another, full screen, pinch to zoom.
struct PictureViewer: View {
    let thread: String
    let seq: Int
    let count: Int
    @State var page: Int
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            TabView(selection: $page) {
                ForEach(0..<count, id: \.self) { i in
                    ZoomablePicture(thread: thread, seq: seq, index: i).tag(i)
                }
            }
            .tabViewStyle(.page(indexDisplayMode: count > 1 ? .automatic : .never))
            .background(.black)
            .navigationTitle(count > 1 ? "\(page + 1) of \(count)" : "Picture")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
        }
    }
}

private struct ZoomablePicture: View {
    let thread: String
    let seq: Int
    let index: Int
    @State private var image: UIImage?
    @State private var missing = false
    @State private var zoom: CGFloat = 1
    @GestureState private var pinch: CGFloat = 1

    var body: some View {
        Group {
            if let image {
                Image(uiImage: image)
                    .resizable()
                    .scaledToFit()
                    .scaleEffect(zoom * pinch)
                    .gesture(MagnifyGesture().updating($pinch) { v, s, _ in s = v.magnification }.onEnded { zoom = min(5, max(1, zoom * $0.magnification)) })
                    .onTapGesture(count: 2) { withAnimation(.snappy) { zoom = zoom > 1 ? 1 : 2.5 } }
                    .accessibilityLabel("Picture")
            } else if missing {
                ContentUnavailableView("Not on the Mac", systemImage: "photo.badge.exclamationmark")
            } else {
                ProgressView().tint(.white)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear {
            image = MediaCache.shared.image(thread: thread, seq: seq, index: index) { got in
                image = got
                missing = got == nil
            }
        }
    }
}

// MARK: Diagrams

/// Diagrams drawn by the Mac, by their source's key: read from any thread (rows are measured in
/// the background), asked for on the main thread.
final class DiagramCache: @unchecked Sendable {
    static let shared = DiagramCache()
    static let ready = Notification.Name("YHDiagramReady")
    private let lock = NSLock()
    private var drawings: [String: Drawing] = [:]
    private var errors: [String: String] = [:]
    private var asked: Set<String> = []
    /// How to ask the Mac (the store sets it; called on the main thread).
    nonisolated(unsafe) var ask: (@MainActor (String, String) -> Void)?

    /// A stable key for a source: FNV-1a, 64 bits.
    static func key(_ source: String) -> String {
        var h: UInt64 = 0xcbf29ce484222325
        for b in source.utf8 { h = (h ^ UInt64(b)) &* 0x100000001b3 }
        return String(h, radix: 16)
    }

    enum State { case drawn(Drawing), failed(String), pending }

    /// What's known of a diagram; asked for the first time it's pending.
    func state(_ source: String) -> (key: String, state: State) {
        let key = Self.key(source)
        lock.lock()
        defer { lock.unlock() }
        if let d = drawings[key] { return (key, .drawn(d)) }
        if let e = errors[key] { return (key, .failed(e)) }
        if !asked.contains(key) {
            asked.insert(key)
            DispatchQueue.main.async { MainActor.assumeIsolated { DiagramCache.shared.ask?(key, source) } }
        }
        return (key, .pending)
    }

    func drawing(_ key: String) -> Drawing? {
        lock.lock()
        defer { lock.unlock() }
        return drawings[key]
    }

    func deliver(key: String, drawing: Drawing?, error: String?) {
        lock.lock()
        if let drawing { drawings[key] = drawing } else { errors[key] = error ?? "This diagram couldn't be drawn." }
        lock.unlock()
        NotificationCenter.default.post(name: Self.ready, object: key)
    }
}

/// A diagram's shapes and labels, scaled to fit its frame (never above its own size), centred.
final class DiagramView: UIView {
    var drawing: Drawing? { didSet { setNeedsDisplay() } }
    var key = ""
    var onTap: (() -> Void)?
    /// Room around the drawing.
    static let padding: CGFloat = 12

    override init(frame: CGRect) {
        super.init(frame: frame)
        backgroundColor = .secondarySystemBackground
        layer.cornerRadius = 12
        layer.cornerCurve = .continuous
        contentMode = .redraw
        isUserInteractionEnabled = true
        addGestureRecognizer(UITapGestureRecognizer(target: self, action: #selector(tapped)))
        isAccessibilityElement = true
        accessibilityTraits = [.image, .button]
    }

    required init?(coder: NSCoder) { fatalError() }

    @objc private func tapped() { onTap?() }

    override func draw(_ rect: CGRect) {
        guard let d = drawing, let ctx = UIGraphicsGetCurrentContext() else {
            let s = NSAttributedString(string: "Drawing the diagram…", attributes: [.font: UIFont.preferredFont(forTextStyle: .footnote), .foregroundColor: UIColor.secondaryLabel])
            let size = s.size()
            s.draw(at: CGPoint(x: (bounds.width - size.width) / 2, y: (bounds.height - size.height) / 2))
            return
        }
        accessibilityLabel = "Diagram: \(d.kind)"
        let room = bounds.insetBy(dx: Self.padding, dy: Self.padding)
        let scale = min(1, room.width / max(d.width, 1), room.height / max(d.height, 1))
        let origin = CGPoint(x: room.midX - d.width * scale / 2, y: room.midY - d.height * scale / 2)
        DiagramView.paint(d, in: ctx, at: origin, scale: scale, traits: traitCollection)
        // Too small to read here: the tap opens it at its own size.
        if scale < 0.75 {
            let hint = NSAttributedString(string: "Tap to zoom", attributes: [.font: UIFont.preferredFont(forTextStyle: .caption2), .foregroundColor: UIColor.secondaryLabel])
            let size = hint.size()
            hint.draw(at: CGPoint(x: bounds.maxX - size.width - 10, y: bounds.maxY - size.height - 6))
        }
    }

    /// Paths, then labels, as the Mac paints them.
    static func paint(_ d: Drawing, in ctx: CGContext, at origin: CGPoint, scale: CGFloat, traits: UITraitCollection) {
        ctx.saveGState()
        ctx.translateBy(x: origin.x, y: origin.y)
        ctx.scaleBy(x: scale, y: scale)
        ctx.setLineJoin(.round)
        for p in d.paths {
            let path = Self.path(p.d)
            if let fill = p.fill {
                ctx.addPath(path)
                ctx.setFillColor(fill.color(traits).cgColor)
                ctx.fillPath()
            }
            if let stroke = p.stroke {
                ctx.addPath(path)
                ctx.setStrokeColor(stroke.color(traits).cgColor)
                ctx.setLineWidth(CGFloat(p.width ?? 1))
                ctx.setLineDash(phase: 0, lengths: (p.dashed ?? false) ? [4, 3] : [])
                ctx.strokePath()
            }
        }
        ctx.restoreGState()
        for l in d.labels {
            let weight: UIFont.Weight = l.weight >= 700 ? .bold : l.weight >= 600 ? .semibold : l.weight >= 500 ? .medium : .regular
            let font = UIFont.systemFont(ofSize: CGFloat(l.size) * scale, weight: weight)
            let style = NSMutableParagraphStyle()
            style.alignment = l.width == nil ? .left : .center
            style.lineBreakMode = .byClipping
            let attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: l.color.color(traits), .paragraphStyle: style]
            for (i, line) in l.lines.enumerated() {
                let top = origin.y + (l.y + Double(i) * l.lineHeight) * scale
                let lineHeight = l.lineHeight * scale
                // Centred in its line, as the Mac's text sits.
                let y = top + (lineHeight - font.lineHeight) / 2
                if let w = l.width {
                    NSString(string: line).draw(in: CGRect(x: origin.x + l.x * scale, y: y, width: w * scale, height: font.lineHeight + 2), withAttributes: attrs)
                } else {
                    NSString(string: line).draw(at: CGPoint(x: origin.x + l.x * scale, y: y), withAttributes: attrs)
                }
            }
        }
    }

    /// `M x y`, `L x y`, `A rx ry x y` (clockwise, the short way round), `Z`.
    static func path(_ d: String) -> CGPath {
        let path = CGMutablePath()
        let t = d.split(separator: " ")
        var i = 0
        var current = CGPoint.zero
        func num() -> CGFloat {
            defer { i += 1 }
            return i < t.count ? CGFloat(Double(t[i]) ?? 0) : 0
        }
        while i < t.count {
            let op = t[i]
            i += 1
            switch op {
            case "M":
                current = CGPoint(x: num(), y: num())
                path.move(to: current)
            case "L":
                current = CGPoint(x: num(), y: num())
                path.addLine(to: current)
            case "A":
                let rx = num(), ry = num()
                let end = CGPoint(x: num(), y: num())
                arc(path, from: current, to: end, rx: rx, ry: ry)
                current = end
            case "Z":
                path.closeSubpath()
            default:
                break
            }
        }
        return path
    }

    /// An SVG arc (no rotation, small arc, sweep 1) from `p0` to `p1`, as Core Graphics draws it.
    private static func arc(_ path: CGMutablePath, from p0: CGPoint, to p1: CGPoint, rx rx0: CGFloat, ry ry0: CGFloat) {
        guard rx0 > 0, ry0 > 0, p0 != p1 else { return path.addLine(to: p1) }
        var rx = rx0, ry = ry0
        let dx = (p0.x - p1.x) / 2, dy = (p0.y - p1.y) / 2
        // Radii too small for the chord grow until they reach.
        let lambda = (dx * dx) / (rx * rx) + (dy * dy) / (ry * ry)
        if lambda > 1 {
            rx *= sqrt(lambda)
            ry *= sqrt(lambda)
        }
        let num = rx * rx * ry * ry - rx * rx * dy * dy - ry * ry * dx * dx
        let den = rx * rx * dy * dy + ry * ry * dx * dx
        // Small arc with sweep 1: the centre's sign is negative.
        let coef = -sqrt(max(0, num / den))
        let cxp = coef * (rx * dy / ry)
        let cyp = coef * -(ry * dx / rx)
        let center = CGPoint(x: cxp + (p0.x + p1.x) / 2, y: cyp + (p0.y + p1.y) / 2)
        let angle = { (x: CGFloat, y: CGFloat) in atan2(y, x) }
        let start = angle((dx - cxp) / rx, (dy - cyp) / ry)
        var delta = angle((-dx - cxp) / rx, (-dy - cyp) / ry) - start
        if delta < 0 { delta += 2 * .pi }
        let transform = CGAffineTransform(translationX: center.x, y: center.y).scaledBy(x: rx, y: ry)
        path.addArc(center: .zero, radius: 1, startAngle: start, endAngle: start + delta, clockwise: false, transform: transform)
    }
}

extension Ink {
    /// The colour in this light or dark: a role as the phone's own, or the author's colour.
    func color(_ traits: UITraitCollection) -> UIColor {
        let base: UIColor
        if let rgba, rgba.count >= 3 {
            base = UIColor(red: rgba[0], green: rgba[1], blue: rgba[2], alpha: 1)
        } else {
            base = switch role {
            case "secondary": .secondaryLabel
            case "canvas": .secondarySystemBackground
            case "card": .tertiarySystemBackground
            case "accent": .tintColor
            default: .label
            }
        }
        let resolved = base.resolvedColor(with: traits)
        return resolved.withAlphaComponent(resolved.cgColor.alpha * alpha)
    }
}

/// A diagram at its own size, zoom and scroll, full screen.
final class DiagramViewer: UIViewController, UIScrollViewDelegate {
    private let drawing: Drawing
    private let scroll = UIScrollView()
    private let canvas = DiagramView()

    init(drawing: Drawing) {
        self.drawing = drawing
        super.init(nibName: nil, bundle: nil)
        title = drawing.kind
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .secondarySystemBackground
        navigationItem.rightBarButtonItem = UIBarButtonItem(systemItem: .done, primaryAction: UIAction { [weak self] _ in self?.dismiss(animated: true) })
        scroll.frame = view.bounds
        scroll.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        scroll.delegate = self
        scroll.maximumZoomScale = 4
        view.addSubview(scroll)
        let pad = DiagramView.padding
        canvas.frame = CGRect(x: 0, y: 0, width: drawing.width + pad * 2, height: drawing.height + pad * 2)
        canvas.layer.cornerRadius = 0
        canvas.drawing = drawing
        scroll.addSubview(canvas)
        scroll.contentSize = canvas.frame.size
    }

    private var zoomed = false

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        guard scroll.bounds.width > 0 else { return }
        // Zoomed out to see it whole; opened at its own size (or its height fitted), so a wide
        // one reads and scrolls sideways.
        let whole = min(1, scroll.bounds.width / canvas.bounds.width, scroll.bounds.height / canvas.bounds.height)
        scroll.minimumZoomScale = whole
        if !zoomed {
            zoomed = true
            scroll.zoomScale = min(1, scroll.bounds.height / canvas.bounds.height)
        }
        center()
    }

    func viewForZooming(in scrollView: UIScrollView) -> UIView? { canvas }
    func scrollViewDidZoom(_ scrollView: UIScrollView) { center() }

    private func center() {
        let x = max(0, (scroll.bounds.width - canvas.frame.width) / 2)
        let y = max(0, (scroll.bounds.height - canvas.frame.height) / 2)
        scroll.contentInset = UIEdgeInsets(top: y, left: x, bottom: y, right: x)
    }
}

// MARK: Sub-agents

/// A sub-agent's run: what it was asked, each step, its report; kept up to date while it runs.
struct SubagentSheet: View {
    let thread: String
    let seq: Int
    let store: Store
    @Environment(\.dismiss) private var dismiss

    private var run: SubagentRun? { store.subagentRuns["\(thread)#\(seq)"] }
    private var info: SubagentInfo? { run?.info ?? store.transcript?.row(seq)?.row.subagent }

    var body: some View {
        NavigationStack {
            List {
                if let info {
                    Section {
                        LabeledContent("Status", value: SubagentSheet.statusName(info.status))
                        if let type = info.agentType { LabeledContent("Agent", value: type) }
                        if let uses = info.toolUses { LabeledContent("Tool uses", value: uses.formatted()) }
                        if let tokens = info.tokens { LabeledContent("Tokens", value: tokens.formatted()) }
                        if let ms = info.durationMs { LabeledContent("Time", value: Duration.milliseconds(ms).formatted(.units(allowed: [.minutes, .seconds], width: .abbreviated))) }
                        if info.status == "running", let activity = info.activity {
                            Label(activity, systemImage: "ellipsis").foregroundStyle(.secondary)
                        }
                    }
                }
                if let prompt = run?.prompt, !prompt.isEmpty {
                    Section("Asked") {
                        Text(prompt).font(.callout).textSelection(.enabled).lineLimit(12)
                    }
                }
                if let steps = run?.steps, !steps.isEmpty {
                    Section("Steps · \(steps.count)") {
                        ForEach(Array(steps.enumerated()), id: \.offset) { _, step in StepRow(step: step) }
                    }
                }
                if let report = run?.report, !report.isEmpty {
                    Section("Report") {
                        Text(report).font(.callout).textSelection(.enabled)
                    }
                }
                if run == nil {
                    HStack { Spacer(); ProgressView(); Spacer() }
                }
            }
            .navigationTitle(info?.description ?? "Sub-agent")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
            .task {
                // Asked again every two seconds while it runs.
                while !Task.isCancelled {
                    store.askSubagent(thread: thread, seq: seq)
                    try? await Task.sleep(for: .seconds(2))
                    if let status = run?.info.status, status != "running" { break }
                }
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }

    static func statusName(_ s: String) -> String {
        switch s {
        case "running": "Working"
        case "completed": "Done"
        case "failed": "Failed"
        case "stopped": "Stopped"
        default: s.capitalized
        }
    }
}

private struct StepRow: View {
    let step: SubagentStep

    var body: some View {
        if let tool = step.tool {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    Image(systemName: step.isError == true ? "xmark.octagon" : "wrench.and.screwdriver")
                        .foregroundStyle(step.isError == true ? .red : .secondary)
                    Text(ToolSummary.displayName(tool)).font(.subheadline.weight(.semibold))
                    if let target = ToolSummary.target(tool: tool, input: step.text) {
                        Text(target).font(.footnote.monospaced()).foregroundStyle(.secondary).lineLimit(1)
                    }
                }
                if let result = step.result, !result.isEmpty {
                    Text(result).font(.caption.monospaced()).foregroundStyle(.secondary).lineLimit(4)
                }
            }
        } else {
            Text(step.text).font(.callout).lineLimit(8)
        }
    }
}
