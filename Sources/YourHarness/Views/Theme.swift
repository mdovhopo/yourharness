import AppKit
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
