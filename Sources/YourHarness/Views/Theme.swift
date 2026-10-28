import AppKit
import struct HarnessCore.SyntaxToken
import SwiftUI

/// One small set of surfaces, so the thread, the overview and the side panels read as one
/// window: a soft canvas matching the glass sidebars, recessed cards on it, and Liquid
/// Glass only for what floats above the content (the composer, prompts, buttons).
enum Theme {
    /// Pieces inside a card: code, thumbnails, rows.
    static let innerRadius: CGFloat = 8
    static let cardRadius: CGFloat = 12
    /// Floating glass: approval prompts, panels over content.
    static let floatingRadius: CGFloat = 18
}

extension ShapeStyle where Self == Color {
    /// Behind threads and overviews: the grey the glass sidebars show. Translucent over the
    /// window background, so it keeps the same wallpaper tint they do.
    static var canvas: Color { Color(nsColor: .canvas) }
}

private extension NSColor {
    static let canvas = NSColor(name: "canvas") { appearance in
        appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            ? NSColor(white: 1, alpha: 0.055)
            : NSColor(white: 0, alpha: 0.075)
    }
}

extension View {
    /// A card on the canvas: a slightly deeper fill and a hairline edge. `tint` colors the
    /// fill (a report, a warning); `border` replaces the edge (running, failed).
    func card(radius: CGFloat = Theme.cardRadius, tint: Color? = nil, border: AnyShapeStyle? = nil, lineWidth: CGFloat = 1) -> some View {
        let shape = RoundedRectangle(cornerRadius: radius, style: .continuous)
        return background(tint.map { AnyShapeStyle($0.opacity(0.09)) } ?? AnyShapeStyle(.fill.quaternary), in: shape)
            .overlay { shape.strokeBorder(border ?? AnyShapeStyle(.separator.opacity(0.6)), lineWidth: lineWidth) }
    }

    /// The canvas, under the toolbar too.
    func canvasBackground() -> some View {
        background(.canvas)
    }

    /// Fades down to `low` and back up every `period` seconds, for as long as it's shown.
    func pulsing(low: Double, period: Double) -> some View {
        PulsingHost(content: self, low: low, period: period)
    }
}

/// Faded by Core Animation, in the render server, with the content in a hosting view of its
/// own. Never a repeating SwiftUI animation: one of those never finishes, so SwiftUI stacks
/// every later animation of the view behind it (each new row moves the typing indicator), and
/// after hours of a busy thread a single frame took minutes. Nor worked out from the clock: a
/// `TimelineView` lays the whole window out on every frame.
private struct PulsingHost<Content: View>: NSViewRepresentable {
    let content: Content
    let low: Double
    let period: Double

    func makeNSView(context: Context) -> PulsingView { PulsingView() }

    func updateNSView(_ view: PulsingView, context: Context) {
        // The font, colors and tint it was given, which a hosting view doesn't inherit.
        view.hosting.rootView = AnyView(content.environment(\.self, context.environment))
        view.pulse(low: low, period: period)
    }

    func sizeThatFits(_ proposal: ProposedViewSize, nsView view: PulsingView, context: Context) -> CGSize? {
        view.hosting.sizeThatFits(in: proposal.replacingUnspecifiedDimensions(by: CGSize(width: 10_000, height: 10_000)))
    }
}

final class PulsingView: NSView {
    let hosting = NSHostingController(rootView: AnyView(EmptyView()))
    private var shape: (low: Double, period: Double)?

    init() {
        super.init(frame: .zero)
        wantsLayer = true
        hosting.sizingOptions = []
        addSubview(hosting.view)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func layout() {
        super.layout()
        hosting.view.frame = bounds
    }

    func pulse(low: Double, period: Double) {
        guard shape?.low != low || shape?.period != period else { return }
        shape = (low, period)
        layer?.removeAnimation(forKey: "pulse")
        start()
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        // A layer leaving its window loses its animations.
        start()
    }

    private func start() {
        guard window != nil, let shape, let layer, layer.animation(forKey: "pulse") == nil else { return }
        let fade = CABasicAnimation(keyPath: "opacity")
        fade.fromValue = 1
        fade.toValue = shape.low
        fade.duration = shape.period / 2
        fade.autoreverses = true
        fade.repeatCount = .infinity
        fade.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
        fade.isRemovedOnCompletion = false
        // In step with every other pulse of the same period.
        fade.timeOffset = CACurrentMediaTime().truncatingRemainder(dividingBy: shape.period)
        layer.add(fade, forKey: "pulse")
    }
}

/// A small capsule label: an agent type, a question's topic, a status.
struct Chip: View {
    let text: String
    var tint: Color?

    init(_ text: String, tint: Color? = nil) {
        self.text = text
        self.tint = tint
    }

    var body: some View {
        Text(text)
            .font(.caption.weight(.medium))
            .foregroundStyle(tint.map { AnyShapeStyle($0) } ?? AnyShapeStyle(.secondary))
            .padding(.horizontal, 7)
            .padding(.vertical, 2)
            .background(tint.map { AnyShapeStyle($0.opacity(0.14)) } ?? AnyShapeStyle(.fill.tertiary), in: .capsule)
            .lineLimit(1)
            .fixedSize()
    }
}

/// Code colors, after Xcode's default themes: pink keywords, red strings, grey comments,
/// blue numbers, teal types. Each has a light and a dark variant.
enum SyntaxPalette {
    /// One container per kind, so building a block's text doesn't make one per token.
    static func attributes(_ kind: SyntaxToken.Kind) -> AttributeContainer {
        containers[kind] ?? AttributeContainer()
    }

    /// The same colors for AppKit text; nil for plain.
    static func nsColor(_ kind: SyntaxToken.Kind) -> NSColor? {
        nsColors[kind]
    }

    private static let nsColors: [SyntaxToken.Kind: NSColor] = Dictionary(uniqueKeysWithValues: SyntaxToken.Kind.allCases.compactMap { kind in
        hex(kind).map { (kind, NSColor.syntax(light: $0.light, dark: $0.dark)) }
    })

    private static func hex(_ kind: SyntaxToken.Kind) -> (light: UInt32, dark: UInt32)? {
        switch kind {
        case .keyword: (0x9B2393, 0xFC5FA3)
        case .type: (0x0B6E78, 0x5DD8D0)
        case .string: (0xC41A16, 0xFC6A5D)
        case .comment: (0x5D6C79, 0x7F8C98)
        case .number: (0x1C00CF, 0xA59BFF)
        case .function: (0x6C36A9, 0xB281EB)
        case .attribute: (0x815F03, 0xD9A05B)
        case .property: (0x255E9E, 0x7CB1E8)
        case .variable: (0x3900A0, 0xD0A8FF)
        case .inserted: (0x1E7B34, 0x5DD27A)
        case .deleted: (0xB42318, 0xFF7B72)
        case .meta: (0x4B6CB7, 0x8FB0F0)
        }
    }

    private static let colors: [SyntaxToken.Kind: Color] = nsColors.mapValues { Color(nsColor: $0) }

    private static let containers: [SyntaxToken.Kind: AttributeContainer] = colors.mapValues { color in
        var container = AttributeContainer()
        container.foregroundColor = color
        return container
    }
}

private extension NSColor {
    static func syntax(light: UInt32, dark: UInt32) -> NSColor {
        NSColor(name: nil) { appearance in
            let hex = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? dark : light
            return NSColor(
                srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
                green: CGFloat((hex >> 8) & 0xFF) / 255,
                blue: CGFloat(hex & 0xFF) / 255,
                alpha: 1
            )
        }
    }
}
