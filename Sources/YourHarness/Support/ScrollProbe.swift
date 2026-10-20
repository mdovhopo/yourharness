import Foundation
import SwiftUI

/// Development: `YH_SCROLL_PROBE=log` prints the transcript's scroll geometry as it
/// changes. `YH_SCROLL_PROBE=<points>` scrolls the open thread from the bottom to the
/// top in steps of that many points (negative: from the top down), the way a trackpad does, then prints to stderr how
/// often a row on screen moved by more or less than the scroll did: a visible jump. Off
/// unless set; nothing here touches a normal run.
struct ScrollProbe: ViewModifier {
    private static let step = ProcessInfo.processInfo.environment["YH_SCROLL_PROBE"].flatMap(Double.init)

    /// The transcript's own scroll position, so the probe scrolls it the same way.
    @Binding var position: ScrollPosition
    @State private var offset: CGFloat = 0
    @State private var viewport: CGFloat = 0
    /// The reported offset counts the toolbar inset; scroll targets don't.
    @State private var topInset: CGFloat = 0

    private static let logs = ProcessInfo.processInfo.environment["YH_SCROLL_PROBE"] == "log"

    func body(content: Content) -> some View {
        if Self.logs {
            content.onScrollGeometryChange(for: String.self) { g in
                "content \(Int(g.contentSize.height)) offset \(Int(g.contentOffset.y)) container \(Int(g.containerSize.height)) insets \(Int(g.contentInsets.top)),\(Int(g.contentInsets.bottom)) visible \(Int(g.visibleRect.minY))...\(Int(g.visibleRect.maxY)) bounds \(Int(g.bounds.minY))...\(Int(g.bounds.maxY))"
            } action: { _, line in
                FileHandle.standardError.write(Data((line + "\n").utf8))
            }
        } else if let step = Self.step {
            content
                .onScrollGeometryChange(for: [CGFloat].self) { [$0.containerSize.height, $0.contentOffset.y, $0.contentInsets.top] } action: { _, geometry in
                    viewport = geometry[0]
                    offset = geometry[1]
                    topInset = geometry[2]
                }
                .task { await run(step: step) }
        } else {
            content
        }
    }

    /// Up from the bottom for a positive step, down from the top for a negative one.
    private func run(step: CGFloat) async {
        try? await Task.sleep(for: .seconds(2))
        if step < 0 {
            withAnimation(.linear(duration: 0.03)) { position.scrollTo(edge: .top) }
            try? await Task.sleep(for: .seconds(1))
        }
        var steps = 0
        var jumps: [Int] = []
        while steps < 2000 {
            let start = offset, before = Rows.positions
            // Animated, so it reads as scrolling rather than following the bottom.
            withAnimation(.linear(duration: 0.03)) { position.scrollTo(y: start + topInset - step) }
            try? await Task.sleep(for: .milliseconds(80))
            let moved = start - offset
            guard abs(moved) > 0.5 else { break }
            steps += 1
            // The last step up stops short at the top.
            if offset <= -topInset + 0.5 { continue }
            // What you'd see: a row on screen should move by exactly the step. The offset
            // itself can shift more, when the stack corrects a height estimate above.
            for (id, y) in Rows.positions {
                guard let old = before[id], (0...viewport).contains(old), (0...viewport).contains(y) else { continue }
                let off = y - old - step
                if abs(off) > 2 { jumps.append(Int(off.rounded())) }
            }
        }
        let worst = jumps.sorted { abs($0) > abs($1) }.prefix(5)
        FileHandle.standardError.write(Data("scroll probe: \(steps) steps, \(jumps.count) jumps, worst \(Array(worst))\n".utf8))
    }

    /// Where each row on screen is, for the probe to compare between steps.
    @MainActor enum Rows {
        static var positions: [String: CGFloat] = [:]
    }

    struct Row: ViewModifier {
        let id: String

        func body(content: Content) -> some View {
            if ScrollProbe.step != nil {
                content
                    .onGeometryChange(for: CGFloat.self) { $0.frame(in: .scrollView(axis: .vertical)).minY } action: { Rows.positions[id] = $0 }
                    .onDisappear { Rows.positions[id] = nil }
            } else {
                content
            }
        }
    }
}
