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
    }

    let seq: Int
    let rev: Int
    let height: CGFloat
    let parts: [Part]
    let accessibility: String

    static let rowGap: CGFloat = 10
    static let codePadding = UIEdgeInsets(top: 8, left: 12, bottom: 10, right: 12)

    static func make(_ row: RowModel, width: CGFloat, fonts f: Fonts) -> RowLayout {
        var parts: [Part] = []
        var height: CGFloat
        var spoken = ""
        switch row.content {
        case .user(let text, let attachments):
            let s = NSMutableAttributedString(string: text.trimmingCharacters(in: .whitespacesAndNewlines),
                                              attributes: [.font: f.body, .foregroundColor: UIColor.label])
            if !attachments.isEmpty {
                s.append(NSAttributedString(string: "\n" + attachments.map(Self.attachmentLabel).joined(separator: "\n"),
                                            attributes: [.font: f.footnote, .foregroundColor: UIColor.secondaryLabel]))
            }
            let inner = measure(s, width: width - 48 - 28)
            let bubble = CGRect(x: width - inner.width - 28, y: 8, width: inner.width + 28, height: inner.height + 18)
            parts = [.bubble(bubble), .text(s, CGRect(x: bubble.minX + 14, y: bubble.minY + 9, width: inner.width, height: inner.height), lines: 0)]
            height = bubble.maxY + 8
            spoken = "You: " + text
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
        return RowLayout(seq: row.seq, rev: row.rev, height: height, parts: parts, accessibility: String(spoken.prefix(2000)))
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
