// A PR's review page (protocol 6): what it is, whether it can merge (reviews, checks, conflicts),
// its description and conversation, and Merge, read and done through `gh` on the Mac. A PR's link
// tapped in a thread opens it, pushed over the thread; the browser is one button away.

import SwiftUI

/// A GitHub PR's or GitLab MR's address, as the Mac reads it: `https://host/owner/repo/pull/12`
/// whatever followed (`/files`, `#discussion…`).
struct PullLink: Hashable, Identifiable, Sendable {
    let url: String
    let repo: String
    let number: Int
    let gitlab: Bool
    var id: String { url }

    init?(_ url: URL) {
        guard let scheme = url.scheme?.lowercased(), scheme == "https" || scheme == "http", let host = url.host()?.lowercased(), !host.isEmpty else { return nil }
        let parts = url.path().split(separator: "/").map(String.init)
        let gitlab: Bool
        let at: Int
        if let i = parts.firstIndex(of: "merge_requests"), i >= 3, parts[i - 1] == "-" {
            gitlab = true; at = i
        } else if let i = parts.firstIndex(of: "pull"), i == 2 {
            gitlab = false; at = i
        } else {
            return nil
        }
        guard at + 1 < parts.count, let number = Int(parts[at + 1]), number > 0 else { return nil }
        let repo = parts[..<(gitlab ? at - 1 : at)].joined(separator: "/")
        self.repo = repo
        self.number = number
        self.gitlab = gitlab
        self.url = "https://\(host)/\(repo)\(gitlab ? "/-/merge_requests/" : "/pull/")\(number)"
    }

    init?(_ string: String) {
        guard let url = URL(string: string) else { return nil }
        self.init(url)
    }

    var forge: String { gitlab ? "GitLab" : "GitHub" }
}

struct PullScreen: View {
    let link: PullLink
    @Environment(Store.self) private var store
    @Environment(\.openURL) private var openURL
    @State private var confirming: (method: String, auto: Bool)?
    @State private var showsAllChecks = false
    private static let env = ProcessInfo.processInfo.environment

    private var page: Store.PullPage? { store.pullPages[link.url] }
    private var pull: PullDetail? { page?.detail }

    var body: some View {
        content
            .navigationTitle(pull.map { "#\($0.number)" } ?? "#\(link.number)")
            .navigationSubtitle(link.repo)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar(.hidden, for: .tabBar)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Menu("More", systemImage: "ellipsis") {
                        Button("Copy Link", systemImage: "link") { UIPasteboard.general.string = link.url }
                        ShareLink(item: URL(string: link.url)!)
                    }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Open on \(link.forge)", systemImage: "safari") { open(link.url) }
                }
            }
            .safeAreaInset(edge: .bottom, spacing: 0) { mergeBar }
            .refreshable { await refresh() }
            .confirmationDialog(confirmTitle, isPresented: Binding(get: { confirming != nil }, set: { if !$0 { confirming = nil } }), titleVisibility: .visible) {
                if let c = confirming {
                    Button(Self.methodTitle(c.method, auto: c.auto)) { merge(c.method, auto: c.auto) }
                }
            } message: {
                if let pull, let c = confirming {
                    Text(c.auto ? "It merges into \(pull.base) by itself once its checks and reviews allow." : "Into \(pull.base), on \(link.forge), from your Mac.")
                }
            }
            // Asked now, then again while checks run, so the page follows them.
            .task(id: link) {
                store.askPull(link)
                while !Task.isCancelled {
                    try? await Task.sleep(for: .seconds(20))
                    guard !Task.isCancelled, pull?.state == "open", (pull?.checks ?? []).contains(where: { $0.state == "pending" }) else { continue }
                    store.askPull(link)
                }
            }
            // `YH_PHONE_MERGE=<method>[:auto]`: that merge asked for and confirmed once the page is read.
            .task(id: pull?.number) {
                guard pull != nil, let wanted = Self.env["YH_PHONE_MERGE"], !Self.mergedOnce else { return }
                Self.mergedOnce = true
                let parts = wanted.split(separator: ":").map(String.init)
                confirming = (parts[0], parts.count > 1 && parts[1] == "auto")
                try? await Task.sleep(for: .seconds(2))
                Perf.shared.log("PULL merge pressed \(parts[0])")
                merge(parts[0], auto: parts.count > 1 && parts[1] == "auto")
                confirming = nil
            }
    }

    private static var mergedOnce = false

    private func refresh() async {
        store.askPull(link)
        for _ in 0..<100 where store.pullPages[link.url]?.loading == true {
            try? await Task.sleep(for: .milliseconds(100))
        }
    }

    private func open(_ url: String) {
        if let u = URL(string: url) { openURL(u) }
    }

    private func merge(_ method: String, auto: Bool) {
        UIImpactFeedbackGenerator(style: .medium).impactOccurred()
        store.mergePull(link, method: method, auto: auto)
    }

    @ViewBuilder private var content: some View {
        if let pull {
            ScrollViewReader { proxy in
            List {
                Section { header(pull) }
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets(top: 4, leading: 4, bottom: 8, trailing: 4))
                Section { status(pull) }
                checks(pull)
                if let body = pull.body, !body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    Section("Description") { PullMarkdown(source: body) }.id("description")
                }
                conversation(pull)
            }
            .listStyle(.insetGrouped)
            .listSectionSpacing(.compact)
            // `YH_PHONE_PULL_SCROLL=description|conversation`: scrolled there once read, for screenshots.
            .task {
                guard let to = Self.env["YH_PHONE_PULL_SCROLL"] else { return }
                try? await Task.sleep(for: .seconds(1))
                proxy.scrollTo(to, anchor: .top)
            }
            }
        } else if let error = page?.error {
            ContentUnavailableView {
                Label("Couldn't Read the PR", systemImage: "exclamationmark.triangle")
            } description: {
                Text(error)
            } actions: {
                Button("Try Again") { store.askPull(link) }
                Button("Open on \(link.forge)") { open(link.url) }
            }
        } else {
            ProgressView("Asking \(link.forge)…").frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    // MARK: Header

    private func header(_ p: PullDetail) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            stateBadge(p)
            Text(p.title).font(.title3.weight(.semibold)).textSelection(.enabled)
            HStack(spacing: 4) {
                if let a = p.author { Text(a).fontWeight(.medium) }
                Text(p.state == "merged" ? "merged" : "wants to merge")
                    .foregroundStyle(.secondary)
            }
            .font(.footnote)
            HStack(spacing: 4) {
                branch(p.head)
                Image(systemName: "arrow.right").font(.caption2).foregroundStyle(.secondary)
                branch(p.base)
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("From \(p.head) into \(p.base)")
            Text(stats(p)).font(.footnote).foregroundStyle(.secondary)
            if let labels = p.labels, !labels.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        ForEach(labels, id: \.self) { Pill(text: $0, color: .secondary) }
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func stateBadge(_ p: PullDetail) -> some View {
        let (text, icon, color): (String, String, Color) = switch (p.state, p.draft) {
        case ("merged", _): ("Merged", "arrow.triangle.merge", .purple)
        case ("closed", _): ("Closed", "xmark.circle", .red)
        case (_, true): ("Draft", "pencil.circle", .secondary)
        default: ("Open", "arrow.triangle.pull", .green)
        }
        return Label(text, systemImage: icon)
            .font(.caption.weight(.semibold))
            .padding(.horizontal, 8).padding(.vertical, 3)
            .background(color.opacity(0.15), in: .capsule)
            .foregroundStyle(color)
    }

    private func branch(_ name: String) -> some View {
        Text(name)
            .font(.caption.monospaced())
            .lineLimit(1).truncationMode(.middle)
            .padding(.horizontal, 6).padding(.vertical, 2)
            .background(Color.secondary.opacity(0.12), in: .rect(cornerRadius: 5))
    }

    private func stats(_ p: PullDetail) -> String {
        var parts: [String] = []
        if let a = p.additions, let d = p.deletions, a + d > 0 { parts.append("+\(a) −\(d)") }
        if let f = p.changedFiles, f > 0 { parts.append(f == 1 ? "1 file" : "\(f) files") }
        if let c = p.commits, c > 0 { parts.append(c == 1 ? "1 commit" : "\(c) commits") }
        if let at = (p.mergedAt ?? p.updatedAt).flatMap(PullRow.date) {
            parts.append((p.mergedAt != nil ? "merged " : "updated ") + at.formatted(.relative(presentation: .named)))
        }
        return parts.joined(separator: " · ")
    }

    // MARK: Status

    @ViewBuilder private func status(_ p: PullDetail) -> some View {
        switch p.review ?? "" {
        case "approved": StatusRow(icon: "checkmark.seal.fill", color: .green, title: "Approved")
        case "changesRequested": StatusRow(icon: "exclamationmark.bubble.fill", color: .orange, title: "Changes requested")
        case "required": StatusRow(icon: "person.badge.clock", color: .secondary, title: "Review required")
        default: StatusRow(icon: "person.2", color: .secondary, title: "No reviews required")
        }
        let checks = p.checks ?? []
        if !checks.isEmpty {
            let failing = checks.filter { $0.state == "failing" }.count
            let pending = checks.filter { $0.state == "pending" }.count
            let passing = checks.filter { $0.state == "passing" }.count
            let summary = [failing > 0 ? "\(failing) failing" : nil, pending > 0 ? "\(pending) running" : nil, passing > 0 ? "\(passing) passed" : nil].compactMap { $0 }.joined(separator: ", ")
            if failing > 0 {
                StatusRow(icon: "xmark.circle.fill", color: .red, title: "Checks failing", detail: summary)
            } else if pending > 0 {
                StatusRow(icon: "clock.fill", color: .yellow, title: "Checks running", detail: summary)
            } else {
                StatusRow(icon: "checkmark.circle.fill", color: .green, title: "Checks passed", detail: summary)
            }
        }
        if p.state == "open" {
            if p.autoMerge == true {
                StatusRow(icon: "arrow.triangle.merge", color: .purple, title: "Auto-merge on", detail: "Merges once its checks and reviews allow")
            }
            switch p.mergeStatus ?? "" {
            case "DIRTY": StatusRow(icon: "exclamationmark.triangle.fill", color: .red, title: "Conflicts", detail: "Resolve them before it can merge")
            case "BEHIND": StatusRow(icon: "arrow.down.circle", color: .orange, title: "Behind \(p.base)", detail: "Update the branch before it can merge")
            case "BLOCKED": StatusRow(icon: "lock.fill", color: .secondary, title: "Merging is blocked", detail: "Required reviews or checks aren't there yet")
            case "CLEAN", "HAS_HOOKS": StatusRow(icon: "checkmark.circle", color: .green, title: "Ready to merge")
            case "UNSTABLE": StatusRow(icon: "checkmark.circle", color: .orange, title: "Can merge", detail: "Some checks that aren't required failed")
            case "DRAFT": StatusRow(icon: "pencil.circle", color: .secondary, title: "Draft", detail: "Mark it ready for review on \(link.forge) to merge")
            default: EmptyView()
            }
        }
    }

    @ViewBuilder private func checks(_ p: PullDetail) -> some View {
        let all = p.checks ?? []
        if !all.isEmpty {
            // Failing and running ones always; passed and skipped behind Show All when there are many.
            let needs = all.filter { $0.state == "failing" || $0.state == "pending" }
            let shown = showsAllChecks || all.count <= 6 ? all : (needs.isEmpty ? Array(all.prefix(4)) : needs)
            Section("Checks") {
                ForEach(Array(shown.enumerated()), id: \.offset) { _, check in
                    if let u = check.url {
                        Button { open(u) } label: { CheckRow(check: check) }.buttonStyle(.plain)
                    } else {
                        CheckRow(check: check)
                    }
                }
                if shown.count < all.count {
                    Button("Show All \(all.count) Checks") { withAnimation { showsAllChecks = true } }
                        .font(.subheadline)
                }
            }
        }
    }

    // MARK: Conversation

    @ViewBuilder private func conversation(_ p: PullDetail) -> some View {
        let comments = (p.comments ?? []).filter { !$0.body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || $0.verdict == "approved" || $0.verdict == "changesRequested" }
        Section(comments.isEmpty ? "Conversation" : "Conversation · \(comments.count)") {
            if comments.isEmpty {
                Text("No comments yet.").font(.subheadline).foregroundStyle(.secondary)
            }
            ForEach(Array(comments.enumerated()), id: \.offset) { _, c in
                CommentRow(comment: c)
                    .id(c == comments.first ? "conversation" : "comment-\(c.at ?? "")-\(c.body.hashValue)")
                    .contextMenu {
                        if let u = c.url { Button("Open on \(link.forge)", systemImage: "safari") { open(u) } }
                        Button("Copy Text", systemImage: "doc.on.doc") { UIPasteboard.general.string = c.body }
                    }
            }
        }
    }

    // MARK: Merging

    private var confirmTitle: String {
        guard let c = confirming else { return "" }
        return c.auto ? "Merge #\(link.number) when ready?" : "\(Self.methodTitle(c.method, auto: false)) #\(link.number)?"
    }

    static func methodTitle(_ method: String, auto: Bool) -> String {
        let name = switch method {
        case "squash": "Squash and Merge"
        case "rebase": "Rebase and Merge"
        default: "Merge"
        }
        return auto ? "\(name) When Ready" : name
    }

    /// Whether GitHub would merge it now; else Merge When Ready (auto-merge) is offered first.
    private func mergeableNow(_ p: PullDetail) -> Bool {
        ["CLEAN", "HAS_HOOKS", "UNSTABLE", "UNKNOWN", ""].contains(p.mergeStatus ?? "")
    }

    @ViewBuilder private var mergeBar: some View {
        if let p = pull, p.state == "open", store.offersActions {
            let methods = (p.mergeMethods ?? ["squash", "merge", "rebase"]).filter { !$0.isEmpty }
            let method = methods.first ?? "squash"
            let blocked = p.draft || p.mergeStatus == "DIRTY"
            let now = mergeableNow(p)
            let merging = page?.merging == true
            VStack(spacing: 8) {
                if let error = page?.mergeError {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .font(.footnote)
                        .foregroundStyle(.red)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .textSelection(.enabled)
                }
                HStack(spacing: 10) {
                    Menu {
                        ForEach(methods, id: \.self) { m in
                            Button(Self.methodTitle(m, auto: false)) { confirming = (m, false) }
                        }
                        if !now && p.autoMerge != true && !link.gitlab {
                            Section("Once checks and reviews allow") {
                                ForEach(methods, id: \.self) { m in
                                    Button(Self.methodTitle(m, auto: true)) { confirming = (m, true) }
                                }
                            }
                        }
                    } label: {
                        HStack {
                            if merging { ProgressView().tint(.white) }
                            Text(merging ? "Merging…" : Self.methodTitle(method, auto: !now && p.autoMerge != true && !link.gitlab))
                                .fontWeight(.semibold)
                        }
                        .frame(maxWidth: .infinity)
                    } primaryAction: {
                        confirming = (method, !now && p.autoMerge != true && !link.gitlab)
                    }
                    .buttonStyle(.glassProminent)
                    .tint(now ? .green : .accentColor)
                    .controlSize(.large)
                    .disabled(blocked || merging || !store.canAct || (!now && p.autoMerge == true))
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
        }
    }
}

/// A line of the status section: an icon in its color, what it says, a detail under it.
private struct StatusRow: View {
    let icon: String
    let color: Color
    let title: String
    var detail: String?

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: icon).foregroundStyle(color).frame(width: 22).imageScale(.large)
            VStack(alignment: .leading, spacing: 1) {
                Text(title).font(.body)
                if let detail { Text(detail).font(.footnote).foregroundStyle(.secondary) }
            }
        }
        .accessibilityElement(children: .combine)
    }
}

private struct CheckRow: View {
    let check: PullCheck

    var body: some View {
        HStack(spacing: 10) {
            icon.frame(width: 20)
            VStack(alignment: .leading, spacing: 1) {
                Text(check.name).font(.subheadline).lineLimit(1)
                if let w = check.workflow { Text(w).font(.caption).foregroundStyle(.secondary).lineLimit(1) }
            }
            Spacer(minLength: 0)
            if check.url != nil { Image(systemName: "arrow.up.forward").font(.caption).foregroundStyle(.tertiary) }
        }
        .contentShape(.rect)
        .accessibilityElement(children: .combine)
        .accessibilityValue(check.state)
    }

    @ViewBuilder private var icon: some View {
        switch check.state {
        case "passing": Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
        case "failing": Image(systemName: "xmark.circle.fill").foregroundStyle(.red)
        case "skipped": Image(systemName: "minus.circle").foregroundStyle(.secondary)
        default: Image(systemName: "clock.fill").foregroundStyle(.yellow)
        }
    }
}

private struct CommentRow: View {
    let comment: PullComment

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Text(String((comment.author ?? "?").prefix(1)).uppercased())
                    .font(.caption.weight(.bold))
                    .frame(width: 24, height: 24)
                    .background(Self.color(for: comment.author ?? "").opacity(0.2), in: .circle)
                    .foregroundStyle(Self.color(for: comment.author ?? ""))
                    .accessibilityHidden(true)
                Text(comment.author ?? "Someone").font(.subheadline.weight(.semibold)).lineLimit(1)
                if let verdict = comment.verdict { verdictPill(verdict) }
                Spacer(minLength: 0)
                if let at = comment.at.flatMap(PullRow.date) {
                    Text(at, format: .relative(presentation: .named)).font(.caption).foregroundStyle(.secondary)
                }
            }
            if comment.kind == "code", let path = comment.path {
                Label(comment.line.map { "\(path):\($0)" } ?? path, systemImage: "chevron.left.forwardslash.chevron.right")
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1).truncationMode(.head)
            }
            if !comment.body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                PullMarkdown(source: comment.body)
            }
        }
        .padding(.vertical, 4)
    }

    @ViewBuilder private func verdictPill(_ v: String) -> some View {
        switch v {
        case "approved": Pill(text: "Approved", color: .green)
        case "changesRequested": Pill(text: "Changes requested", color: .orange)
        case "dismissed": Pill(text: "Dismissed", color: .secondary)
        default: Pill(text: "Reviewed", color: .secondary)
        }
    }

    private static let palette: [Color] = [.blue, .purple, .pink, .orange, .teal, .indigo, .mint, .brown]
    static func color(for name: String) -> Color {
        palette[Int(name.unicodeScalars.reduce(0) { ($0 &* 31 &+ $1.value) % 9973 }) % palette.count]
    }
}

private struct Pill: View {
    let text: String
    let color: Color

    var body: some View {
        Text(text)
            .font(.caption2.weight(.semibold))
            .lineLimit(1)
            .padding(.horizontal, 6).padding(.vertical, 2)
            .background(color.opacity(0.15), in: .capsule)
            .foregroundStyle(color)
    }
}

/// Markdown as GitHub writes it, parsed off the main thread: paragraphs, headings, lists, quotes,
/// code. HTML comments and tags (outside code) are left out; long text folds behind Show More.
struct PullMarkdown: View {
    let source: String
    @State private var blocks: [MDBlock]?
    @State private var expanded = false

    private var long: Bool { source.count > 900 }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Group {
                if let blocks {
                    VStack(alignment: .leading, spacing: 8) {
                        ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in view(block) }
                    }
                } else {
                    Text(source.prefix(400)).font(.subheadline).foregroundStyle(.secondary)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: long && !expanded ? 220 : nil, alignment: .topLeading)
            .clipped()
            .mask {
                if long && !expanded {
                    LinearGradient(stops: [.init(color: .black, location: 0.75), .init(color: .clear, location: 1)], startPoint: .top, endPoint: .bottom)
                } else {
                    Color.black
                }
            }
            if long {
                Button(expanded ? "Show Less" : "Show More") { withAnimation(.snappy) { expanded.toggle() } }
                    .font(.subheadline)
                    .buttonStyle(.borderless)
            }
        }
        .task(id: source) {
            let source = source
            blocks = await Task.detached(priority: .userInitiated) { Markdown.blocks(Self.cleaned(source)) }.value
        }
    }

    @ViewBuilder private func view(_ block: MDBlock) -> some View {
        switch block {
        case .text(let paragraphs):
            VStack(alignment: .leading, spacing: 6) {
                ForEach(Array(paragraphs.enumerated()), id: \.offset) { _, p in paragraph(p) }
            }
        case .code(_, let text):
            ScrollView(.horizontal, showsIndicators: false) {
                Text(text).font(.caption.monospaced()).padding(8).textSelection(.enabled)
            }
            .background(Color.secondary.opacity(0.1), in: .rect(cornerRadius: 8))
        case .rule:
            Divider()
        }
    }

    @ViewBuilder private func paragraph(_ p: MDParagraph) -> some View {
        switch p.kind {
        case .heading(let level):
            Text(p.text).font(level <= 2 ? .headline : .subheadline.weight(.semibold))
        case .item(let marker, let depth):
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(marker).foregroundStyle(.secondary)
                Text(p.text)
            }
            .font(.subheadline)
            .padding(.leading, CGFloat(depth - 1) * 14)
        case .quote:
            Text(p.text).font(.subheadline).foregroundStyle(.secondary)
                .padding(.leading, 10)
                .overlay(alignment: .leading) { Rectangle().fill(.quaternary).frame(width: 3) }
        case .body:
            Text(p.text).font(.subheadline)
        }
    }

    /// HTML comments and tags dropped outside fenced code (GitHub's templates are full of both);
    /// `<br>` becomes a line break and an image's tag its link.
    nonisolated static func cleaned(_ s: String) -> String {
        var out: [Substring] = []
        var fenced = false
        var text = s.replacingOccurrences(of: "\r\n", with: "\n")
        while let open = text.range(of: "<!--"), let close = text.range(of: "-->", range: open.upperBound..<text.endIndex) {
            text.removeSubrange(open.lowerBound..<close.upperBound)
        }
        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            if line.trimmingCharacters(in: .whitespaces).hasPrefix("```") { fenced.toggle(); out.append(line); continue }
            guard !fenced, line.contains("<") else { out.append(line); continue }
            var l = String(line)
            l = l.replacing(/<br\s*\/?>/.ignoresCase(), with: "\n")
            l = l.replacing(/<img[^>]*src="([^"]+)"[^>]*>/.ignoresCase()) { "[image](\($0.1))" }
            l = l.replacing(/<\/?[a-zA-Z][^>]*>/, with: "")
            out.append(Substring(l))
        }
        return out.joined(separator: "\n")
    }
}
