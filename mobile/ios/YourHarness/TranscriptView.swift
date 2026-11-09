// The transcript: a UICollectionView, not a SwiftUI list (on the Mac, a lazy list pinned to
// the bottom stuttered under streaming). Each update touches only the rows it names: new rows
// are appended, changed ones reconfigured in place. It follows the bottom while you're there
// and reads earlier pages as you near the top.

import SwiftUI
import UIKit

struct TranscriptScreen: View {
    let threadID: String
    @Environment(Store.self) private var store
    @State private var model: TranscriptModel?

    var body: some View {
        Group {
            if let model {
                TranscriptRepresentable(model: model).ignoresSafeArea(.container, edges: .bottom)
            }
        }
        .navigationTitle(store.threads[threadID]?.title ?? "Thread")
        .navigationSubtitle(store.threads[threadID].flatMap(store.projectName) ?? "")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            if store.threads[threadID]?.active == true {
                ToolbarItem(placement: .topBarTrailing) {
                    Image(systemName: "circle.fill").font(.system(size: 8)).foregroundStyle(.green)
                        .symbolEffect(.pulse, options: .repeating)
                        .accessibilityLabel("Working")
                }
            }
        }
        .onAppear { model = store.open(threadID) }
        .onDisappear { if let model { store.close(model) } }
    }
}

struct TranscriptRepresentable: UIViewControllerRepresentable {
    let model: TranscriptModel

    func makeUIViewController(context: Context) -> TranscriptViewController {
        TranscriptViewController(model: model)
    }

    func updateUIViewController(_ controller: TranscriptViewController, context: Context) {}
}

final class TranscriptViewController: UIViewController, UICollectionViewDelegate {
    private let model: TranscriptModel
    private var collectionView: UICollectionView!
    private var dataSource: UICollectionViewDiffableDataSource<Int, Int>!
    private let styles = StyleCache()
    private var followsBottom = true

    init(model: TranscriptModel) {
        self.model = model
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        let item = NSCollectionLayoutItem(layoutSize: .init(widthDimension: .fractionalWidth(1), heightDimension: .estimated(60)))
        let group = NSCollectionLayoutGroup.vertical(layoutSize: item.layoutSize, subitems: [item])
        let section = NSCollectionLayoutSection(group: group)
        section.contentInsets = .init(top: 12, leading: 16, bottom: 24, trailing: 16)
        collectionView = UICollectionView(frame: view.bounds, collectionViewLayout: UICollectionViewCompositionalLayout(section: section))
        collectionView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        collectionView.backgroundColor = .systemBackground
        collectionView.delegate = self
        collectionView.allowsSelection = false
        collectionView.keyboardDismissMode = .interactive
        view.addSubview(collectionView)

        let user = UICollectionView.CellRegistration<UserCell, Int> { [unowned self] cell, _, seq in
            if case .user(let text, let attachments)? = model.row(seq)?.content { cell.configure(text: text, attachments: attachments) }
        }
        let reply = UICollectionView.CellRegistration<ReplyCell, Int> { [unowned self] cell, _, seq in
            guard let row = model.row(seq), case .reply(let blocks) = row.content else { return }
            cell.configure(blocks: blocks, styled: styles.styled(row, blocks))
        }
        let tool = UICollectionView.CellRegistration<ToolCell, Int> { [unowned self] cell, _, seq in
            if case .tool(let summary)? = model.row(seq)?.content { cell.configure(summary) }
        }
        let small = UICollectionView.CellRegistration<SmallCell, Int> { [unowned self] cell, _, seq in
            guard let content = model.row(seq)?.content else { return }
            cell.configure(content)
        }
        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { [unowned self] view, path, seq in
            switch model.row(seq)?.content {
            case .user?: view.dequeueConfiguredReusableCell(using: user, for: path, item: seq)
            case .reply?: view.dequeueConfiguredReusableCell(using: reply, for: path, item: seq)
            case .tool?: view.dequeueConfiguredReusableCell(using: tool, for: path, item: seq)
            default: view.dequeueConfiguredReusableCell(using: small, for: path, item: seq)
            }
        }
        model.onUpdate = { [weak self] update in self?.apply(update) }
        if model.loaded { apply(.reload) }
        // Text size changed: the styled text holds fonts, so make it again.
        registerForTraitChanges([UITraitPreferredContentSizeCategory.self]) { (self: Self, _: UITraitCollection) in
            self.styles.clear()
            self.apply(.reload)
        }
    }

    private func apply(_ update: TranscriptModel.Update) {
        var snapshot = dataSource.snapshot()
        switch update {
        case .reload:
            snapshot = NSDiffableDataSourceSnapshot()
            snapshot.appendSections([0])
            snapshot.appendItems(model.rows.map(\.seq))
            dataSource.applySnapshotUsingReloadData(snapshot)
            followsBottom = true
            scrollToBottom()
        case .changed(let appended, let changed):
            let follow = followsBottom
            if !changed.isEmpty { snapshot.reconfigureItems(changed) }
            if !appended.isEmpty { snapshot.appendItems(appended) }
            dataSource.apply(snapshot, animatingDifferences: false)
            if follow { scrollToBottom() }
        case .prepended(let n):
            let before = collectionView.contentSize.height
            let offset = collectionView.contentOffset.y
            let seqs = model.rows.prefix(n).map(\.seq)
            if let first = snapshot.itemIdentifiers.first {
                snapshot.insertItems(seqs, beforeItem: first)
            } else {
                snapshot.appendItems(seqs)
            }
            dataSource.apply(snapshot, animatingDifferences: false)
            collectionView.layoutIfNeeded()
            collectionView.contentOffset.y = offset + collectionView.contentSize.height - before
        }
    }

    private func scrollToBottom() {
        collectionView.layoutIfNeeded()
        let inset = collectionView.adjustedContentInset
        let bottom = collectionView.contentSize.height + inset.bottom - collectionView.bounds.height
        collectionView.contentOffset.y = max(-inset.top, bottom)
    }

    func scrollViewDidScroll(_ scrollView: UIScrollView) {
        guard scrollView.isTracking || scrollView.isDecelerating else { return }
        let inset = scrollView.adjustedContentInset
        let distance = scrollView.contentSize.height + inset.bottom - scrollView.bounds.height - scrollView.contentOffset.y
        followsBottom = distance < 80
        if scrollView.contentOffset.y + inset.top < 800 { model.loadEarlier() }
    }
}

// MARK: - Styling

/// Markdown made into attributed text, once per copy of a row.
@MainActor
final class StyleCache {
    private var cache: [Int: (rev: Int, texts: [NSAttributedString])] = [:]

    func styled(_ row: RowModel, _ blocks: [MDBlock]) -> [NSAttributedString] {
        if let hit = cache[row.seq], hit.rev == row.rev { return hit.texts }
        let texts = blocks.compactMap { block -> NSAttributedString? in
            if case .text(let paragraphs) = block { return Style.text(paragraphs) }
            return nil
        }
        cache[row.seq] = (row.rev, texts)
        if cache.count > 400 { cache.removeAll() }
        return texts
    }

    func clear() { cache.removeAll() }
}

enum Style {
    static var body: UIFont { .preferredFont(forTextStyle: .body) }
    static func mono(_ style: UIFont.TextStyle) -> UIFont {
        UIFontMetrics(forTextStyle: style).scaledFont(for: .monospacedSystemFont(ofSize: UIFont.preferredFont(forTextStyle: style).pointSize * 0.9, weight: .regular))
    }

    static func with(_ font: UIFont, _ traits: UIFontDescriptor.SymbolicTraits) -> UIFont {
        guard let d = font.fontDescriptor.withSymbolicTraits(font.fontDescriptor.symbolicTraits.union(traits)) else { return font }
        return UIFont(descriptor: d, size: 0)
    }

    static func text(_ paragraphs: [MDParagraph]) -> NSAttributedString {
        let out = NSMutableAttributedString()
        for (i, p) in paragraphs.enumerated() {
            let base: UIFont = switch p.kind {
            case .heading(1): with(.preferredFont(forTextStyle: .title2), .traitBold)
            case .heading(2): with(.preferredFont(forTextStyle: .title3), .traitBold)
            case .heading: .preferredFont(forTextStyle: .headline)
            default: body
            }
            let style = NSMutableParagraphStyle()
            style.paragraphSpacing = 10
            if case .heading = p.kind, i > 0 { style.paragraphSpacingBefore = 6 }
            var color = UIColor.label
            let s = NSMutableAttributedString()
            switch p.kind {
            case .item(let marker, let depth):
                let indent = CGFloat(depth) * 22
                style.headIndent = indent
                style.firstLineHeadIndent = indent - 22
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
                        font = UIFont.monospacedSystemFont(ofSize: base.pointSize * 0.9, weight: .regular)
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

// MARK: - Cells

@MainActor private func pin(_ view: UIView, in cell: UICollectionViewCell, insets: UIEdgeInsets = .init(top: 6, left: 0, bottom: 6, right: 0)) {
    view.translatesAutoresizingMaskIntoConstraints = false
    cell.contentView.addSubview(view)
    NSLayoutConstraint.activate([
        view.topAnchor.constraint(equalTo: cell.contentView.topAnchor, constant: insets.top),
        view.leadingAnchor.constraint(equalTo: cell.contentView.leadingAnchor, constant: insets.left),
        view.trailingAnchor.constraint(equalTo: cell.contentView.trailingAnchor, constant: -insets.right),
        view.bottomAnchor.constraint(equalTo: cell.contentView.bottomAnchor, constant: -insets.bottom).withPriority(.init(999)),
    ])
}

private extension NSLayoutConstraint {
    func withPriority(_ p: UILayoutPriority) -> NSLayoutConstraint {
        priority = p
        return self
    }
}

@MainActor private func label(_ style: UIFont.TextStyle, color: UIColor = .label, lines: Int = 0) -> UILabel {
    let l = UILabel()
    l.font = .preferredFont(forTextStyle: style)
    l.adjustsFontForContentSizeCategory = true
    l.textColor = color
    l.numberOfLines = lines
    return l
}

/// Your message: a bubble on the trailing side.
final class UserCell: UICollectionViewCell {
    private let bubble = UIView()
    private let text = label(.body)
    private let files = label(.footnote, color: .secondaryLabel)

    override init(frame: CGRect) {
        super.init(frame: frame)
        bubble.backgroundColor = .secondarySystemFill
        bubble.layer.cornerRadius = 18
        bubble.layer.cornerCurve = .continuous
        let stack = UIStackView(arrangedSubviews: [text, files])
        stack.axis = .vertical
        stack.spacing = 4
        stack.translatesAutoresizingMaskIntoConstraints = false
        bubble.addSubview(stack)
        bubble.translatesAutoresizingMaskIntoConstraints = false
        contentView.addSubview(bubble)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: bubble.topAnchor, constant: 9),
            stack.bottomAnchor.constraint(equalTo: bubble.bottomAnchor, constant: -9),
            stack.leadingAnchor.constraint(equalTo: bubble.leadingAnchor, constant: 14),
            stack.trailingAnchor.constraint(equalTo: bubble.trailingAnchor, constant: -14),
            bubble.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 10),
            bubble.bottomAnchor.constraint(equalTo: contentView.bottomAnchor, constant: -10).withPriority(.init(999)),
            bubble.trailingAnchor.constraint(equalTo: contentView.trailingAnchor),
            bubble.leadingAnchor.constraint(greaterThanOrEqualTo: contentView.leadingAnchor, constant: 48),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(text: String, attachments: [String]) {
        self.text.text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        files.text = attachments.isEmpty ? nil : attachments.map { "📎 \($0)" }.joined(separator: "\n")
        files.isHidden = attachments.isEmpty
    }
}

/// Claude's reply: text blocks and code blocks, stacked.
final class ReplyCell: UICollectionViewCell {
    private let stack = UIStackView()

    override init(frame: CGRect) {
        super.init(frame: frame)
        stack.axis = .vertical
        stack.spacing = 10
        stack.alignment = .fill
        pin(stack, in: self)
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(blocks: [MDBlock], styled: [NSAttributedString]) {
        // Reuse the views already there when the kinds line up: a streaming reply keeps its views.
        var views = stack.arrangedSubviews
        var texts = styled.makeIterator()
        for (i, block) in blocks.enumerated() {
            let existing = i < views.count ? views[i] : nil
            switch block {
            case .text:
                let l = (existing as? UILabel) ?? replace(at: i, in: &views, with: label(.body))
                l.attributedText = texts.next()
            case .code(let language, let text):
                let v = (existing as? CodeBlockView) ?? replace(at: i, in: &views, with: CodeBlockView())
                v.configure(language: language, text: text)
            case .rule:
                if !(existing is RuleView) { _ = replace(at: i, in: &views, with: RuleView()) }
            }
        }
        for v in views.dropFirst(blocks.count) {
            stack.removeArrangedSubview(v)
            v.removeFromSuperview()
        }
    }

    private func replace<V: UIView>(at i: Int, in views: inout [UIView], with view: V) -> V {
        if i < views.count {
            stack.removeArrangedSubview(views[i])
            views[i].removeFromSuperview()
            views[i] = view
        } else {
            views.append(view)
        }
        stack.insertArrangedSubview(view, at: i)
        return view
    }
}

final class CodeBlockView: UIView {
    private let scroll = UIScrollView()
    private let code = UILabel()
    private let language = label(.caption2, color: .secondaryLabel, lines: 1)

    override init(frame: CGRect) {
        super.init(frame: frame)
        backgroundColor = .secondarySystemBackground
        layer.cornerRadius = 10
        layer.cornerCurve = .continuous
        code.numberOfLines = 0
        code.font = Style.mono(.footnote)
        code.adjustsFontForContentSizeCategory = true
        scroll.showsHorizontalScrollIndicator = false
        scroll.alwaysBounceHorizontal = false
        for v in [scroll, code, language] as [UIView] { v.translatesAutoresizingMaskIntoConstraints = false }
        scroll.addSubview(code)
        addSubview(language)
        addSubview(scroll)
        NSLayoutConstraint.activate([
            language.topAnchor.constraint(equalTo: topAnchor, constant: 8),
            language.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12),
            language.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -12),
            scroll.topAnchor.constraint(equalTo: language.bottomAnchor, constant: 4),
            scroll.leadingAnchor.constraint(equalTo: leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: trailingAnchor),
            scroll.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -10),
            code.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor),
            code.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor),
            code.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor, constant: 12),
            code.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor, constant: -12),
            code.heightAnchor.constraint(equalTo: scroll.frameLayoutGuide.heightAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(language: String?, text: String) {
        self.language.text = (language?.isEmpty == false ? language! : "code")
        code.text = text
    }
}

final class RuleView: UIView {
    override init(frame: CGRect) {
        super.init(frame: frame)
        backgroundColor = .separator
        heightAnchor.constraint(equalToConstant: 1 / max(1, traitCollection.displayScale)).isActive = true
    }

    required init?(coder: NSCoder) { fatalError() }
}

/// A tool call, one line: its symbol, name and target.
final class ToolCell: UICollectionViewCell {
    private let icon = UIImageView()
    private let name = label(.footnote, lines: 1)
    private let target = label(.footnote, color: .secondaryLabel, lines: 1)

    override init(frame: CGRect) {
        super.init(frame: frame)
        icon.preferredSymbolConfiguration = .init(textStyle: .footnote)
        icon.contentMode = .center
        icon.setContentHuggingPriority(.required, for: .horizontal)
        name.font = Style.with(.preferredFont(forTextStyle: .footnote), .traitBold)
        name.setContentHuggingPriority(.required, for: .horizontal)
        name.setContentCompressionResistancePriority(.required, for: .horizontal)
        target.lineBreakMode = .byTruncatingMiddle
        let row = UIStackView(arrangedSubviews: [icon, name, target])
        row.spacing = 6
        row.alignment = .center
        pin(row, in: self, insets: .init(top: 3, left: 2, bottom: 3, right: 0))
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(_ s: ToolSummary) {
        icon.image = UIImage(systemName: s.failed ? "xmark.octagon" : s.symbol)
        icon.tintColor = s.failed ? .systemRed : .secondaryLabel
        name.text = s.name
        name.textColor = s.failed ? .systemRed : .secondaryLabel
        target.text = s.target
        target.font = s.target?.contains("/") == true || s.name == "Bash" ? Style.mono(.footnote) : .preferredFont(forTextStyle: .footnote)
        accessibilityLabel = [s.name, s.target, s.failed ? "failed" : nil].compactMap { $0 }.joined(separator: ", ")
        isAccessibilityElement = true
    }
}

/// Thinking, notices, and kinds this app doesn't know yet.
final class SmallCell: UICollectionViewCell {
    private let text = label(.footnote, color: .secondaryLabel)

    override init(frame: CGRect) {
        super.init(frame: frame)
        pin(text, in: self, insets: .init(top: 4, left: 2, bottom: 4, right: 0))
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(_ content: RowContent) {
        text.textAlignment = .natural
        text.numberOfLines = 2
        switch content {
        case .thinking(let t):
            let line = t.split(whereSeparator: \.isNewline).first.map(String.init) ?? ""
            text.attributedText = NSAttributedString(string: line.isEmpty ? "Thinking" : "Thinking · \(line)", attributes: [
                .font: Style.with(.preferredFont(forTextStyle: .footnote), .traitItalic), .foregroundColor: UIColor.secondaryLabel,
            ])
        case .notice(let t, let style):
            text.textAlignment = .center
            text.numberOfLines = 0
            text.attributedText = nil
            text.text = t
            text.textColor = style == "error" ? .systemRed : .secondaryLabel
        case .other(_, let t):
            text.attributedText = nil
            text.text = t
            text.textColor = .secondaryLabel
        default:
            text.text = nil
        }
    }
}
