// A row measured and laid out for a width: its height and where each piece goes. Made off
// the main thread (a page of earlier rows is measured before it's shown), so the main thread
// only sets frames: no Auto Layout, no self-sizing cells.

import UIKit

/// The fonts rows use at one text size. Made on the main thread, used from any.
struct Fonts: @unchecked Sendable {
    let category: UIContentSizeCategory
    let body, footnote, footnoteBold, footnoteItalic, caption, title2, title3, headline, mono: UIFont

    init(_ category: UIContentSizeCategory) {
        let traits = UITraitCollection(preferredContentSizeCategory: category)
        func font(_ style: UIFont.TextStyle) -> UIFont { .preferredFont(forTextStyle: style, compatibleWith: traits) }
        self.category = category
        body = font(.body)
        footnote = font(.footnote)
        footnoteBold = Style.with(footnote, .traitBold)
        footnoteItalic = Style.with(footnote, .traitItalic)
        caption = font(.caption2)
        title2 = Style.with(font(.title2), .traitBold)
        title3 = Style.with(font(.title3), .traitBold)
        headline = font(.headline)
        mono = .monospacedSystemFont(ofSize: footnote.pointSize * 0.95, weight: .regular)
    }
}

struct RowLayout: @unchecked Sendable {
    enum Part {
        case text(NSAttributedString, CGRect, lines: Int)
        case bubble(CGRect)
        case code(header: NSAttributedString, code: NSAttributedString, frame: CGRect, codeSize: CGSize)
        case rule(CGRect)
        case icon(symbol: String, tint: UIColor, CGRect)
        /// A sub-agent's card behind its lines.
        case card(CGRect)
        /// One of the row's pictures, loaded from the Mac when shown.
        case picture(index: Int, CGRect)
        /// A Mermaid diagram drawn as the Mac draws it (`DiagramCache`), by its source's key.
        case diagram(key: String, CGRect)
    }

    let seq: Int
    let rev: Int
    let height: CGFloat
    let parts: [Part]
    let accessibility: String
    /// Diagrams in it, by key: measured again once they're drawn.
    var diagramKeys: [String] = []

    static let rowGap: CGFloat = 10
    static let codePadding = UIEdgeInsets(top: 8, left: 12, bottom: 10, right: 12)

    static func make(_ row: RowModel, width: CGFloat, fonts f: Fonts) -> RowLayout {
        var parts: [Part] = []
        var height: CGFloat
        var spoken = ""
        var diagramKeys: [String] = []
        switch row.content {
        case .user(let text, let attachments):
            let pictures = Pictures.yours(attachments)
            let others = attachments.filter { !pictures.contains($0) }
            let s = NSMutableAttributedString(string: text.trimmingCharacters(in: .whitespacesAndNewlines),
                                              attributes: [.font: f.body, .foregroundColor: UIColor.label])
            if !others.isEmpty {
                s.append(NSAttributedString(string: (s.length > 0 ? "\n" : "") + others.map(Self.attachmentLabel).joined(separator: "\n"),
                                            attributes: [.font: f.footnote, .foregroundColor: UIColor.secondaryLabel]))
            }
            var y: CGFloat = 8
            if s.length > 0 {
                let inner = measure(s, width: width - 48 - 28)
                let bubble = CGRect(x: width - inner.width - 28, y: y, width: inner.width + 28, height: inner.height + 18)
                parts = [.bubble(bubble), .text(s, CGRect(x: bubble.minX + 14, y: bubble.minY + 9, width: inner.width, height: inner.height), lines: 0)]
                y = bubble.maxY + 6
            }
            // Your pictures under it, right-aligned.
            let grid = Self.grid(count: pictures.count, width: width - 48, side: 120, single: CGSize(width: 220, height: 160))
            for (i, r) in grid.frames.enumerated() {
                parts.append(.picture(index: i, r.offsetBy(dx: width - grid.width, dy: y)))
            }
            if !grid.frames.isEmpty { y += grid.height + 6 }
            height = y + 2
            spoken = "You: " + text + (pictures.isEmpty ? "" : ", \(pictures.count) picture\(pictures.count == 1 ? "" : "s")")
        case .agent(let t, let info):
            // A card: what it does, how it went, what it's doing now.
            let running = info?.status == "running" || (info == nil && t.running)
            let status = info?.status ?? (t.running ? "running" : t.failed ? "failed" : "completed")
            let pad: CGFloat = 12
            let title = NSAttributedString(string: info?.description ?? t.target ?? "Sub-agent", attributes: [.font: f.headline, .foregroundColor: UIColor.label])
            var facts = [running ? "Working" : SubagentSheet.statusName(status)]
            if let type = info?.agentType { facts.append(type) }
            if let uses = info?.toolUses, uses > 0 { facts.append("\(uses) tool use\(uses == 1 ? "" : "s")") }
            if let tokens = info?.tokens { facts.append(tokens >= 1000 ? "\(tokens / 1000)k tokens" : "\(tokens) tokens") }
            if let ms = info?.durationMs, !running { facts.append(ms >= 60_000 ? "\(ms / 60_000)m \(ms % 60_000 / 1000)s" : "\(max(1, ms / 1000))s") }
            let line = NSAttributedString(string: facts.joined(separator: " · "), attributes: [.font: f.footnote, .foregroundColor: UIColor.secondaryLabel])
            let inner = width - pad * 2 - 30
            let titleHeight = measure(title, width: inner, maxLines: 2, lineHeight: f.headline.lineHeight).height
            let lineHeight = ceil(f.footnote.lineHeight)
            var y: CGFloat = 6 + pad
            parts = [
                .card(.zero),
                .icon(symbol: "person.2", tint: .secondaryLabel, CGRect(x: pad, y: y, width: 22, height: ceil(f.headline.lineHeight))),
                .text(title, CGRect(x: pad + 30, y: y, width: inner, height: titleHeight), lines: 2),
            ]
            y += titleHeight + 2
            let tint: UIColor = status == "failed" ? .systemRed : status == "completed" ? .systemGreen : .secondaryLabel
            let symbol = running ? "ellipsis.circle" : status == "failed" ? "xmark.octagon.fill" : status == "completed" ? "checkmark.circle.fill" : "stop.circle"
            parts.append(.icon(symbol: symbol, tint: tint, CGRect(x: pad, y: y, width: 22, height: lineHeight)))
            parts.append(.text(line, CGRect(x: pad + 30, y: y, width: inner, height: lineHeight), lines: 1))
            y += lineHeight
            if running, let activity = info?.activity {
                let a = NSAttributedString(string: activity, attributes: [.font: f.footnoteItalic, .foregroundColor: UIColor.secondaryLabel])
                parts.append(.text(a, CGRect(x: pad + 30, y: y + 2, width: inner, height: lineHeight), lines: 1))
                y += lineHeight + 2
            }
            let card = CGRect(x: 0, y: 6, width: width, height: y + pad - 6)
            parts[0] = .card(card)
            height = card.maxY + 6
            spoken = "Sub-agent: " + title.string + ", " + line.string
        case .media(let t, let pictures):
            let lineHeight = ceil(f.footnote.lineHeight)
            let label = NSAttributedString(string: pictures.isEmpty ? "Showed you a file" : "Showed you \(pictures.count) picture\(pictures.count == 1 ? "" : "s")",
                                           attributes: [.font: f.footnoteBold, .foregroundColor: t.failed ? UIColor.systemRed : UIColor.secondaryLabel])
            parts = [
                .icon(symbol: "photo.on.rectangle", tint: .secondaryLabel, CGRect(x: 0, y: 4, width: 20, height: lineHeight)),
                .text(label, CGRect(x: 26, y: 4, width: width - 26, height: lineHeight), lines: 1),
            ]
            let grid = Self.grid(count: pictures.count, width: width, side: 150, single: CGSize(width: min(width, 320), height: 220))
            for (i, r) in grid.frames.enumerated() {
                parts.append(.picture(index: i, r.offsetBy(dx: 0, dy: lineHeight + 12)))
            }
            height = lineHeight + 12 + (grid.frames.isEmpty ? 0 : grid.height + 6)
            spoken = label.string
        case .reply(let blocks):
            var y: CGFloat = 4
            for block in blocks {
                switch block {
                case .text(let paragraphs):
                    let s = Style.text(paragraphs, fonts: f)
                    let size = measure(s, width: width)
                    parts.append(.text(s, CGRect(x: 0, y: y, width: width, height: size.height), lines: 0))
                    y += size.height + rowGap
                    spoken += s.string + "\n"
                case .code(let language, let text) where language?.lowercased() == "mermaid":
                    // Drawn as the Mac draws it once it arrives; a kind it doesn't draw stays code.
                    let (key, state) = DiagramCache.shared.state(text)
                    diagramKeys.append(key)
                    switch state {
                    case .drawn(let d):
                        let pad = DiagramView.padding
                        let scale = min(1, (width - pad * 2) / max(d.width, 1))
                        // Room under a small one for its "Tap to zoom".
                        let frame = CGRect(x: 0, y: y, width: width, height: ceil(d.height * scale) + pad * 2 + (scale < 0.75 ? 14 : 0))
                        parts.append(.diagram(key: key, frame))
                        y += frame.height + rowGap
                        spoken += "Diagram: \(d.kind)\n"
                    case .pending:
                        parts.append(.diagram(key: key, CGRect(x: 0, y: y, width: width, height: 120)))
                        y += 120 + rowGap
                        spoken += "Diagram\n"
                    case .failed:
                        let header = NSAttributedString(string: "mermaid", attributes: [.font: f.caption, .foregroundColor: UIColor.secondaryLabel])
                        let code = NSAttributedString(string: text, attributes: [.font: f.mono, .foregroundColor: UIColor.label])
                        let headerHeight = ceil(f.caption.lineHeight)
                        let codeSize = measure(code, width: .greatestFiniteMagnitude)
                        let p = codePadding
                        let frame = CGRect(x: 0, y: y, width: width, height: p.top + headerHeight + 4 + codeSize.height + p.bottom)
                        parts.append(.code(header: header, code: code, frame: frame, codeSize: codeSize))
                        y += frame.height + rowGap
                        spoken += "Code: " + text + "\n"
                    }
                case .code(let language, let text):
                    let header = NSAttributedString(string: language?.isEmpty == false ? language! : "code",
                                                    attributes: [.font: f.caption, .foregroundColor: UIColor.secondaryLabel])
                    let code = NSAttributedString(string: text, attributes: [.font: f.mono, .foregroundColor: UIColor.label])
                    let headerHeight = ceil(f.caption.lineHeight)
                    let codeSize = measure(code, width: .greatestFiniteMagnitude)
                    let p = codePadding
                    let frame = CGRect(x: 0, y: y, width: width, height: p.top + headerHeight + 4 + codeSize.height + p.bottom)
                    parts.append(.code(header: header, code: code, frame: frame, codeSize: codeSize))
                    y += frame.height + rowGap
                    spoken += "Code: " + text + "\n"
                case .rule:
                    parts.append(.rule(CGRect(x: 0, y: y + 4, width: width, height: 0.5)))
                    y += 9 + rowGap
                }
            }
            height = blocks.isEmpty ? 0 : y - rowGap + 6
            spoken = "Claude: " + spoken
        case .tool(let t):
            let line = NSMutableAttributedString(string: t.name, attributes: [.font: f.footnoteBold, .foregroundColor: t.failed ? UIColor.systemRed : UIColor.secondaryLabel])
            if let target = t.target {
                let monospaced = target.contains("/") || t.name == "Bash"
                line.append(NSAttributedString(string: "  " + target, attributes: [.font: monospaced ? f.mono : f.footnote, .foregroundColor: UIColor.secondaryLabel]))
            }
            let lineHeight = ceil(max(f.footnote.lineHeight, f.mono.lineHeight))
            parts = [
                .icon(symbol: t.failed ? "xmark.octagon" : t.symbol, tint: t.failed ? .systemRed : .secondaryLabel, CGRect(x: 0, y: 4, width: 20, height: lineHeight)),
                .text(line, CGRect(x: 26, y: 4, width: width - 26, height: lineHeight), lines: 1),
            ]
            height = lineHeight + 8
            spoken = [t.name, t.target, t.failed ? "failed" : nil].compactMap { $0 }.joined(separator: ", ")
        case .thinking(let text):
            let first = text.split(whereSeparator: \.isNewline).first.map(String.init) ?? ""
            let s = NSAttributedString(string: first.isEmpty ? "Thinking" : "Thinking · \(first)",
                                       attributes: [.font: f.footnoteItalic, .foregroundColor: UIColor.secondaryLabel])
            let size = measure(s, width: width, maxLines: 2, lineHeight: f.footnoteItalic.lineHeight)
            parts = [.text(s, CGRect(x: 0, y: 4, width: width, height: size.height), lines: 2)]
            height = size.height + 8
            spoken = s.string
        case .notice(let text, let style):
            let p = NSMutableParagraphStyle()
            p.alignment = .center
            let s = NSAttributedString(string: text, attributes: [.font: f.footnote, .paragraphStyle: p,
                                                                  .foregroundColor: style == "error" ? UIColor.systemRed : UIColor.secondaryLabel])
            let size = measure(s, width: width)
            parts = [.text(s, CGRect(x: 0, y: 6, width: width, height: size.height), lines: 0)]
            height = size.height + 12
            spoken = text
        case .other(_, let text):
            let s = NSAttributedString(string: text, attributes: [.font: f.footnote, .foregroundColor: UIColor.secondaryLabel])
            let size = measure(s, width: width, maxLines: 2, lineHeight: f.footnote.lineHeight)
            parts = text.isEmpty ? [] : [.text(s, CGRect(x: 0, y: 4, width: width, height: size.height), lines: 2)]
            height = text.isEmpty ? 0 : size.height + 8
            spoken = text
        }
        return RowLayout(seq: row.seq, rev: row.rev, height: height, parts: parts, accessibility: String(spoken.prefix(2000)), diagramKeys: diagramKeys)
    }

    /// Pictures in rows of squares `side` wide (one alone at `single`), left to right, wrapping.
    static func grid(count: Int, width: CGFloat, side: CGFloat, single: CGSize) -> (frames: [CGRect], width: CGFloat, height: CGFloat) {
        guard count > 0 else { return ([], 0, 0) }
        if count == 1 { return ([CGRect(origin: .zero, size: single)], single.width, single.height) }
        let gap: CGFloat = 6
        let across = max(1, min(count, Int((width + gap) / (side + gap))))
        let frames = (0..<count).map { i in CGRect(x: CGFloat(i % across) * (side + gap), y: CGFloat(i / across) * (side + gap), width: side, height: side) }
        let rows = (count + across - 1) / across
        return (frames, CGFloat(across) * (side + gap) - gap, CGFloat(rows) * (side + gap) - gap)
    }

    static func measure(_ s: NSAttributedString, width: CGFloat, maxLines: Int = 0, lineHeight: CGFloat = 0) -> CGSize {
        guard s.length > 0 else { return .zero }
        let r = s.boundingRect(with: CGSize(width: width, height: .greatestFiniteMagnitude), options: [.usesLineFragmentOrigin, .usesFontLeading], context: nil)
        var h = ceil(r.height)
        if maxLines > 0 { h = min(h, ceil(lineHeight * CGFloat(maxLines))) }
        return CGSize(width: ceil(r.width), height: h)
    }
}

enum Style {
    static func with(_ font: UIFont, _ traits: UIFontDescriptor.SymbolicTraits) -> UIFont {
        guard let d = font.fontDescriptor.withSymbolicTraits(font.fontDescriptor.symbolicTraits.union(traits)) else { return font }
        return UIFont(descriptor: d, size: 0)
    }

    static func text(_ paragraphs: [MDParagraph], fonts f: Fonts) -> NSAttributedString {
        let out = NSMutableAttributedString()
        for (i, p) in paragraphs.enumerated() {
            let base: UIFont = switch p.kind {
            case .heading(1): f.title2
            case .heading(2): f.title3
            case .heading: f.headline
            default: f.body
            }
            let style = NSMutableParagraphStyle()
            style.paragraphSpacing = 10
            if case .heading = p.kind, i > 0 { style.paragraphSpacingBefore = 6 }
            var color = UIColor.label
            let s = NSMutableAttributedString()
            switch p.kind {
            case .item(let marker, let depth):
                // The marker's room grows with the text size, so "10." never touches its words.
                let step = ceil(base.pointSize * 1.6)
                let indent = CGFloat(depth) * step
                style.headIndent = indent
                style.firstLineHeadIndent = indent - step
                style.tabStops = [NSTextTab(textAlignment: .left, location: indent)]
                // Tight within a list, a paragraph's space after its last item.
                if i + 1 < paragraphs.count, case .item = paragraphs[i + 1].kind { style.paragraphSpacing = 4 }
                s.append(NSAttributedString(string: "\(marker)\t", attributes: [.font: base, .foregroundColor: UIColor.secondaryLabel]))
            case .quote:
                style.headIndent = 14
                style.firstLineHeadIndent = 14
                color = .secondaryLabel
            default:
                break
            }
            for run in p.text.runs {
                var font = base
                var attributes: [NSAttributedString.Key: Any] = [.foregroundColor: color]
                if let inline = run.inlinePresentationIntent {
                    if inline.contains(.stronglyEmphasized) { font = with(font, .traitBold) }
                    if inline.contains(.emphasized) { font = with(font, .traitItalic) }
                    if inline.contains(.code) {
                        font = .monospacedSystemFont(ofSize: base.pointSize * 0.9, weight: .regular)
                        attributes[.backgroundColor] = UIColor.tertiarySystemFill
                    }
                    if inline.contains(.strikethrough) { attributes[.strikethroughStyle] = NSUnderlineStyle.single.rawValue }
                }
                if let link = run.link {
                    attributes[.foregroundColor] = UIColor.tintColor
                    attributes[.link] = link
                }
                attributes[.font] = font
                s.append(NSAttributedString(string: String(p.text[run.range].characters), attributes: attributes))
            }
            if i < paragraphs.count - 1 { s.append(NSAttributedString(string: "\n", attributes: [.font: base])) }
            s.addAttribute(.paragraphStyle, value: style, range: NSRange(location: 0, length: s.length))
            out.append(s)
        }
        return out
    }
}

extension RowLayout {
    /// How an attachment reads under your message: a picture as "Picture", a file by its name
    /// (pictures are kept on the Mac under a long made-up name).
    static func attachmentLabel(_ name: String) -> String {
        let lower = name.lowercased()
        if [".png", ".jpg", ".jpeg", ".gif", ".webp", ".heic"].contains(where: lower.hasSuffix), !name.contains("/") {
            return "🖼 Picture"
        }
        return "📎 " + (name.split(separator: "/").last.map(String.init) ?? name)
    }
}
