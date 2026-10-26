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

/// The mark, slowly turning: Claude is working. Turned from the clock rather than a repeating
/// animation, for the same reason as `pulsing`, and still while no window can be seen.
struct SpinningClaudeMark: View {
    var size: CGFloat = 11
    /// Seconds a turn takes.
    private let period = 2.4

    var body: some View {
        TimelineView(.animation(minimumInterval: nil, paused: !WindowPresence.shared.isVisible)) { context in
            ClaudeMark(size: size)
                .rotationEffect(.degrees(context.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: period) / period * 360))
        }
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
