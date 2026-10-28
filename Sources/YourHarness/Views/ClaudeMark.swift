import SwiftUI

extension Color {
    /// Claude's terracotta.
    static let claude = Color(red: 0xD9 / 255, green: 0x77 / 255, blue: 0x57 / 255)
}

/// Claude's starburst, drawn as a vector so it's crisp at every size.
/// To use an official brand asset instead, replace the body with an `Image`.
struct ClaudeMark: View {
    var size: CGFloat = 14

    var body: some View {
        StarburstShape()
            .fill(Color.claude)
            .frame(width: size, height: size)
            .accessibilityLabel("Claude")
    }
}

/// The mark, slowly turning: Claude is working. Core Animation turns it, in the render
/// server: a repeating SwiftUI animation never finishes, so SwiftUI stacks every later
/// animation of the view behind it, and turning it from the clock (a `TimelineView`) laid the
/// whole window out on every frame, half the main thread for as long as any thread worked.
struct SpinningClaudeMark: View {
    var size: CGFloat = 11

    var body: some View {
        SpinningMark()
            .frame(width: size, height: size)
            .accessibilityElement()
            .accessibilityLabel("Claude")
    }
}

private struct SpinningMark: NSViewRepresentable {
    func makeNSView(context: Context) -> SpinningMarkView { SpinningMarkView() }
    func updateNSView(_ view: SpinningMarkView, context: Context) {}
}

final class SpinningMarkView: NSView {
    /// Seconds a turn takes.
    private static let period = 2.4
    private let mark = CAShapeLayer()

    override var isFlipped: Bool { true }

    init() {
        super.init(frame: .zero)
        wantsLayer = true
        layer?.addSublayer(mark)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func layout() {
        super.layout()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        mark.frame = bounds
        mark.path = StarburstShape().path(in: bounds).cgPath
        mark.fillColor = NSColor(Color.claude).cgColor
        CATransaction.commit()
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        // A layer leaving its window loses its animations.
        guard window != nil, mark.animation(forKey: "spin") == nil else { return }
        let spin = CABasicAnimation(keyPath: "transform.rotation.z")
        spin.fromValue = 0
        spin.toValue = 2 * Double.pi
        spin.duration = Self.period
        spin.repeatCount = .infinity
        spin.isRemovedOnCompletion = false
        // Every mark at the same angle, wherever it is.
        spin.timeOffset = CACurrentMediaTime().truncatingRemainder(dividingBy: Self.period)
        mark.add(spin, forKey: "spin")
    }
}

private struct StarburstShape: Shape {
    /// Slightly irregular ray lengths give it the hand-drawn feel of the original.
    private static let rays: [CGFloat] = [1.0, 0.86, 0.95, 0.8, 0.98, 0.84, 0.92, 0.82, 1.0, 0.87, 0.94, 0.81]

    func path(in rect: CGRect) -> Path {
        let center = CGPoint(x: rect.midX, y: rect.midY)
        let radius = min(rect.width, rect.height) / 2
        let inner = radius * 0.2
        let count = Self.rays.count
        let halfWidth = Double.pi / Double(count) * 0.42

        var path = Path()
        for (index, length) in Self.rays.enumerated() {
            let angle = Double(index) / Double(count) * 2 * .pi - .pi / 2
            let tip = point(center, radius * length, angle)
            let left = point(center, inner, angle - halfWidth * 2.2)
            let right = point(center, inner, angle + halfWidth * 2.2)
            let shoulderLeft = point(center, radius * length * 0.55, angle - halfWidth * 0.55)
            let shoulderRight = point(center, radius * length * 0.55, angle + halfWidth * 0.55)

            path.move(to: left)
            path.addQuadCurve(to: tip, control: shoulderLeft)
            path.addQuadCurve(to: right, control: shoulderRight)
            path.closeSubpath()
        }
        path.addEllipse(in: CGRect(x: center.x - inner * 1.15, y: center.y - inner * 1.15,
                                   width: inner * 2.3, height: inner * 2.3))
        return path
    }

    private func point(_ center: CGPoint, _ distance: CGFloat, _ angle: Double) -> CGPoint {
        CGPoint(x: center.x + distance * cos(angle), y: center.y + distance * sin(angle))
    }
}
