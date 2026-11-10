// A file's diff in a PR (protocol 6): its hunks line by line, added lines green and removed ones
// red, numbered as in the file, with the comments on its lines under them. Lines wrap, or not
// (kept between launches), and the next and previous files are a tap away. The Mac reads the
// diff (`gh api …/files`); the phone splits it into lines as the frame is decoded, off the main
// thread, and draws only the lines on screen.

import SwiftUI

struct PullDiffScreen: View {
    let link: PullLink
    let files: [PullFile]
    @Environment(Store.self) private var store
    @Environment(\.openURL) private var openURL
    @AppStorage("diff.wrap") private var wrap = true
    @State private var index: Int

    init(link: PullLink, files: [PullFile], start: PullFile) {
        self.link = link
        self.files = files
        _index = State(initialValue: files.firstIndex(of: start) ?? 0)
    }

    private var file: PullFile { files[index] }
    private var key: String { Store.patchKey(link.url, file.path) }
    private var page: Store.PatchPage? { store.patches[key] }

    var body: some View {
        content
            .navigationTitle((file.path as NSString).lastPathComponent)
            .navigationSubtitle(subtitle)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar(.hidden, for: .tabBar)
            .toolbar {
                ToolbarItemGroup(placement: .topBarTrailing) {
                    Button(wrap ? "Don't Wrap Lines" : "Wrap Lines", systemImage: wrap ? "arrow.left.and.right.text.vertical" : "text.word.spacing") { wrap.toggle() }
                    Menu("More", systemImage: "ellipsis") {
                        Button("Open on \(link.forge)", systemImage: "safari") { if let u = URL(string: link.url + (link.gitlab ? "/diffs" : "/files")) { openURL(u) } }
                        Button("Copy Path", systemImage: "doc.on.doc") { UIPasteboard.general.string = file.path }
                    }
                }
                if files.count > 1 {
                    ToolbarItemGroup(placement: .bottomBar) {
                        Button("Previous File", systemImage: "chevron.up") { index -= 1 }.disabled(index == 0)
                        Spacer()
                        Text("\(index + 1) of \(files.count)").font(.footnote).foregroundStyle(.secondary).monospacedDigit()
                        Spacer()
                        Button("Next File", systemImage: "chevron.down") { index += 1 }.disabled(index == files.count - 1)
                    }
                }
            }
            // Asked when shown, and again when the PR's files were read anew (diffs dropped).
            .task(id: "\(key)|\(page == nil)") {
                if page == nil, file.hasPatch != false { store.askPatch(link, path: file.path) }
            }
            // `YH_PHONE_DIFF_WRAP=0|1`, for screenshots.
            .task {
                if let w = ProcessInfo.processInfo.environment["YH_PHONE_DIFF_WRAP"] { wrap = w == "1" }
            }
    }

    private var subtitle: String {
        var parts: [String] = []
        let folder = (file.path as NSString).deletingLastPathComponent
        if !folder.isEmpty { parts.append(folder) }
        if let a = file.additions, let d = file.deletions { parts.append("+\(a) −\(d)") }
        return parts.joined(separator: " · ")
    }

    /// Comments on this file's lines, by the line they're on (in the file after), and the ones
    /// whose line isn't in the diff.
    private var comments: (byLine: [Int: [PullComment]], elsewhere: [PullComment]) {
        let mine = (store.pullPages[link.url]?.detail?.comments ?? []).filter { $0.kind == "code" && $0.path == file.path }
        let lines = Set(page?.lines?.compactMap(\.new) ?? [])
        var byLine: [Int: [PullComment]] = [:]
        var elsewhere: [PullComment] = []
        for c in mine {
            if let l = c.line, lines.contains(l) { byLine[l, default: []].append(c) } else { elsewhere.append(c) }
        }
        return (byLine, elsewhere)
    }

    @ViewBuilder private var content: some View {
        if file.hasPatch == false {
            unavailable("The forge doesn't show this file's diff: it's too big, or not text.")
        } else if let lines = page?.lines {
            if lines.isEmpty {
                ContentUnavailableView("No Changes to Show", systemImage: "doc", description: Text(file.status == "renamed" ? "Renamed, its contents unchanged." : "Nothing in this file's diff."))
            } else {
                PullDiffLines(lines: lines, comments: comments, wrap: wrap)
                    .id(key)
            }
        } else if let error = page?.error {
            unavailable(error)
        } else {
            ProgressView("Reading the diff…").frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private func unavailable(_ why: String) -> some View {
        ContentUnavailableView {
            Label("Can't Show This Diff", systemImage: "doc.text.magnifyingglass")
        } description: {
            Text(why)
        } actions: {
            Button("Open on \(link.forge)") { if let u = URL(string: link.url + (link.gitlab ? "/diffs" : "/files")) { openURL(u) } }
        }
    }
}

/// The lines, drawn lazily: a gutter with the line's number and its + or −, then the code.
/// Unwrapped, every line is as wide as the longest, so the colors run edge to edge as it scrolls.
private struct PullDiffLines: View {
    let lines: [DiffLine]
    let comments: (byLine: [Int: [PullComment]], elsewhere: [PullComment])
    let wrap: Bool

    private static let size: CGFloat = 12
    private static let font = Font.system(size: size, design: .monospaced)
    private static let charWidth = ("M" as NSString).size(withAttributes: [.font: UIFont.monospacedSystemFont(ofSize: size, weight: .regular)]).width

    var body: some View {
        let digits = max(3, String(lines.compactMap { $0.new ?? $0.old }.max() ?? 0).count)
        let gutter = CGFloat(digits) * Self.charWidth + 10
        GeometryReader { geo in
            let longest = CGFloat(lines.lazy.map { Self.columns($0.text) }.max() ?? 0)
            let codeWidth = wrap ? nil : max(geo.size.width - gutter - Self.charWidth * 2, longest * Self.charWidth + 16)
            ScrollView(wrap ? .vertical : [.vertical, .horizontal]) {
                LazyVStack(alignment: .leading, spacing: 0) {
                    if !comments.elsewhere.isEmpty {
                        VStack(alignment: .leading, spacing: 8) {
                            Text("On lines not in this diff").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                            ForEach(Array(comments.elsewhere.enumerated()), id: \.offset) { _, c in CommentRow(comment: c) }
                        }
                        .padding(12)
                        .frame(width: wrap ? nil : geo.size.width, alignment: .leading)
                    }
                    ForEach(lines.indices, id: \.self) { i in
                        let line = lines[i]
                        row(line, gutter: gutter, codeWidth: codeWidth)
                        if line.kind != .removed, let n = line.new, let said = comments.byLine[n] {
                            VStack(alignment: .leading, spacing: 8) {
                                ForEach(Array(said.enumerated()), id: \.offset) { _, c in CommentRow(comment: c) }
                            }
                            .padding(10)
                            .background(.regularMaterial, in: .rect(cornerRadius: 10))
                            .padding(.horizontal, 8).padding(.vertical, 6)
                            .frame(width: wrap ? nil : geo.size.width, alignment: .leading)
                        }
                    }
                }
                .padding(.bottom, 12)
            }
            .scrollIndicators(.automatic)
        }
    }

    /// How many columns a line takes, tabs as four.
    private static func columns(_ s: String) -> Int {
        s.count + s.reduce(0) { $1 == "\t" ? $0 + 3 : $0 }
    }

    @ViewBuilder private func row(_ line: DiffLine, gutter: CGFloat, codeWidth: CGFloat?) -> some View {
        switch line.kind {
        case .hunk:
            Text(Self.hunkTitle(line.text))
                .font(Self.font)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .padding(.horizontal, 8).padding(.vertical, 5)
                .frame(maxWidth: .infinity, alignment: .leading)
                .frame(width: codeWidth.map { $0 + gutter + Self.charWidth * 2 }, alignment: .leading)
                .background(Color.blue.opacity(0.08))
        case .note:
            Text(line.text).font(.caption2).italic().foregroundStyle(.secondary)
                .padding(.leading, gutter + Self.charWidth * 2).padding(.vertical, 2)
        default:
            HStack(alignment: .firstTextBaseline, spacing: 0) {
                Text(verbatim: (line.new ?? line.old).map(String.init) ?? "")
                    .foregroundStyle(.tertiary)
                    .frame(width: gutter - 6, alignment: .trailing)
                    .padding(.trailing, 6)
                Text(verbatim: line.kind == .added ? "+" : line.kind == .removed ? "−" : " ")
                    .foregroundStyle(line.kind == .added ? Color.green : line.kind == .removed ? Color.red : Color.clear)
                    .frame(width: Self.charWidth * 2, alignment: .center)
                Text(verbatim: line.text.replacingOccurrences(of: "\t", with: "    "))
                    .fixedSize(horizontal: !wrap, vertical: true)
                    .frame(width: codeWidth, alignment: .leading)
                    .frame(maxWidth: wrap ? .infinity : nil, alignment: .leading)
            }
            .font(Self.font)
            .padding(.vertical, 1)
            .background(Self.background(line.kind))
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("\(line.kind == .added ? "Added" : line.kind == .removed ? "Removed" : "Line") \((line.new ?? line.old).map(String.init) ?? ""): \(line.text)")
        }
    }

    private static func background(_ kind: DiffLine.Kind) -> Color {
        switch kind {
        case .added: Color.green.opacity(0.14)
        case .removed: Color.red.opacity(0.14)
        default: .clear
        }
    }

    /// `@@ -12,7 +12,8 @@ fn launch()` → `fn launch()` after the line it starts at, or just the line.
    private static func hunkTitle(_ s: String) -> String {
        let parts = s.split(separator: "@@", maxSplits: 2, omittingEmptySubsequences: false)
        guard parts.count >= 3 else { return s }
        let range = parts[1].trimmingCharacters(in: .whitespaces)
        let newStart = range.split(separator: " ").last.map { String($0.dropFirst().split(separator: ",").first ?? "") } ?? ""
        let context = parts[2].trimmingCharacters(in: .whitespaces)
        return context.isEmpty ? "Line \(newStart)" : "Line \(newStart) · \(context)"
    }
}
