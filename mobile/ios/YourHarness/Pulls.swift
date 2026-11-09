// A project's open PRs, as its page on the Mac lists them: what waits for your review, yours,
// everyone else's, each stack together and drawn as a rail. Filters show or hide each section
// and drafts (kept between launches). A PR opens on GitHub.

import SwiftUI

struct PullsSheet: View {
    let project: Project
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL
    @AppStorage("pulls.review") private var showReview = true
    @AppStorage("pulls.mine") private var showMine = true
    @AppStorage("pulls.others") private var showOthers = true
    @AppStorage("pulls.drafts") private var showDrafts = true

    private static let sections: [(key: String, title: String)] = [("review", "Needs Your Review"), ("mine", "Yours"), ("others", "Others")]

    var body: some View {
        NavigationStack {
            content
                .navigationTitle("Pull Requests")
                .navigationSubtitle(project.name)
                .navigationBarTitleDisplayMode(.inline)
                .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done", role: .confirm) { dismiss() } } }
                .refreshable { store.askPulls(project.id, fresh: true) }
        }
        .task {
            // `YH_PHONE_PULLS_HIDE=others,drafts`: those filters off, as tapping them does.
            if ProcessInfo.processInfo.environment["YH_PHONE_PULLS"] != nil {
                let hide = ProcessInfo.processInfo.environment["YH_PHONE_PULLS_HIDE"] ?? ""
                let off = Set(hide.split(separator: ",").map(String.init))
                showReview = !off.contains("review"); showMine = !off.contains("mine")
                showOthers = !off.contains("others"); showDrafts = !off.contains("drafts")
            }
            store.askPulls(project.id)
        }
    }

    private var answer: PullsAnswer? { store.pulls[project.id] }

    private func shown(_ section: String) -> Bool {
        switch section {
        case "review": showReview
        case "mine": showMine
        default: showOthers
        }
    }

    private func visible(in section: String) -> [PullInfo] {
        (answer?.pulls ?? []).filter { $0.section == section && (showDrafts || !$0.draft) }
    }

    @ViewBuilder private var content: some View {
        if let answer {
            List {
                Section { filters }
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets())
                ForEach(Self.sections, id: \.key) { section in
                    let rows = visible(in: section.key)
                    if shown(section.key), !rows.isEmpty {
                        Section(section.title) {
                            ForEach(Array(rows.enumerated()), id: \.element.id) { i, pull in
                                Button { if let url = URL(string: pull.url) { openURL(url) } } label: {
                                    PullRow(pull: pull, stacked: Self.stacked(pull, rows: rows, at: i))
                                }
                                .buttonStyle(.plain)
                                .contextMenu {
                                    Button("Open on GitHub", systemImage: "safari") { if let url = URL(string: pull.url) { openURL(url) } }
                                    Button("Copy Link", systemImage: "link") { UIPasteboard.general.string = pull.url }
                                }
                            }
                        }
                    }
                }
                if !answer.errors.isEmpty {
                    Section { ForEach(answer.errors, id: \.self) { Label($0, systemImage: "exclamationmark.triangle").font(.footnote).foregroundStyle(.secondary) } }
                }
            }
            .listStyle(.insetGrouped)
            .overlay {
                if Self.sections.allSatisfy({ !shown($0.key) || visible(in: $0.key).isEmpty }) && answer.errors.isEmpty {
                    if answer.pulls.isEmpty {
                        ContentUnavailableView("No Open Pull Requests", systemImage: "arrow.triangle.pull", description: Text("Nothing open in this project's repos."))
                    } else {
                        ContentUnavailableView("All Filtered Out", systemImage: "line.3.horizontal.decrease.circle", description: Text("Turn a filter back on above."))
                    }
                }
            }
        } else {
            ProgressView("Asking GitHub…").frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    /// Where a PR sits in its stack, for the rail: whether one comes below and above it.
    static func stacked(_ pull: PullInfo, rows: [PullInfo], at i: Int) -> PullRow.Rail? {
        let mates = rows.filter { $0.stack == pull.stack }
        guard mates.count > 1 else { return nil }
        let below = i > 0 && rows[i - 1].stack == pull.stack
        let above = i + 1 < rows.count && rows[i + 1].stack == pull.stack
        return .init(below: below, above: above)
    }

    private var filters: some View {
        let count = { (s: String) in (answer?.pulls ?? []).filter { $0.section == s && (showDrafts || !$0.draft) }.count }
        return ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                Toggle(isOn: $showReview) { Text("Review · \(count("review"))") }
                Toggle(isOn: $showMine) { Text("Yours · \(count("mine"))") }
                Toggle(isOn: $showOthers) { Text("Others · \(count("others"))") }
                Toggle(isOn: $showDrafts) { Label("Drafts", systemImage: "pencil.and.outline") }
            }
            .toggleStyle(.button)
            .buttonBorderShape(.capsule)
            .controlSize(.small)
            .padding(.horizontal, 4)
            .padding(.vertical, 2)
        }
    }
}

/// A PR: its title, number, repo, author and age, and what stands between it and merging.
struct PullRow: View {
    struct Rail: Equatable { let below: Bool; let above: Bool }
    let pull: PullInfo
    let stacked: Rail?

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            if let stacked { rail(stacked) }
            VStack(alignment: .leading, spacing: 3) {
                Text(pull.title).font(.body).lineLimit(2)
                Text(subtitle).font(.footnote).foregroundStyle(.secondary).lineLimit(1)
                badges
            }
            Spacer(minLength: 0)
            checks
        }
        .padding(.vertical, 2)
        .contentShape(.rect)
        .accessibilityElement(children: .combine)
    }

    private var subtitle: String {
        var parts = ["#\(pull.number)", pull.repo]
        if let a = pull.author { parts.append(a) }
        if let at = pull.updatedAt.flatMap(Self.date) { parts.append(at.formatted(.relative(presentation: .named))) }
        return parts.joined(separator: " · ")
    }

    /// The stack's rail: a dot for this PR, a line to the ones below and above.
    private func rail(_ r: Rail) -> some View {
        VStack(spacing: 0) {
            Rectangle().fill(r.below ? Color.secondary.opacity(0.5) : .clear).frame(width: 2, height: 8)
            Circle().stroke(Color.accentColor, lineWidth: 2).frame(width: 9, height: 9)
            Rectangle().fill(r.above ? Color.secondary.opacity(0.5) : .clear).frame(width: 2).frame(maxHeight: .infinity)
        }
        .frame(width: 12)
        .padding(.leading, CGFloat(min(pull.depth, 4)) * 6)
        .accessibilityLabel("In a stack of PRs, level \(pull.depth + 1)")
    }

    @ViewBuilder private var badges: some View {
        let tags = Self.tags(pull)
        if !tags.isEmpty {
            HStack(spacing: 6) {
                ForEach(tags, id: \.text) { tag in
                    Text(tag.text)
                        .font(.caption2.weight(.semibold))
                        .padding(.horizontal, 6).padding(.vertical, 2)
                        .background(tag.color.opacity(0.15), in: .capsule)
                        .foregroundStyle(tag.color)
                }
            }
        }
    }

    static func tags(_ p: PullInfo) -> [(text: String, color: Color)] {
        var t: [(String, Color)] = []
        if p.draft { t.append(("Draft", .secondary)) }
        switch p.review ?? "" {
        case "approved": t.append(("Approved", .green))
        case "changesRequested": t.append(("Changes requested", .orange))
        default: break
        }
        if p.mergeStatus == "DIRTY" { t.append(("Conflicts", .red)) }
        if p.mergeStatus == "BEHIND" { t.append(("Behind", .orange)) }
        if p.inMergeQueue == true { t.append(("In merge queue", .purple)) } else if p.autoMerge == true { t.append(("Auto-merge", .purple)) }
        return t
    }

    @ViewBuilder private var checks: some View {
        switch pull.checks ?? "" {
        case "passing": Image(systemName: "checkmark.circle.fill").foregroundStyle(.green).accessibilityLabel("Checks passing")
        case "failing": Image(systemName: "xmark.circle.fill").foregroundStyle(.red).accessibilityLabel("Checks failing")
        case "pending": Image(systemName: "clock.fill").foregroundStyle(.yellow).accessibilityLabel("Checks running")
        default: EmptyView()
        }
    }

    private static let iso = ISO8601DateFormatter()
    static func date(_ s: String) -> Date? { iso.date(from: s) }
}
