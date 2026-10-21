import AppKit
import HarnessCore
import SwiftUI

/// The right sidebar's PRs tab: every pull request a thread is linked to, open ones first
/// (as stacks when they're stacked), then merged and closed ones. Kept current while it's
/// showing, so asking Claude to merge shows the merge happen.
struct PullRequestsPanel: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel

    var body: some View {
        let items = app.pullRequests(for: thread)
        let open = items.compactMap(\.pull).filter { $0.state == .open }
        let done = items.compactMap(\.pull).filter { $0.state != .open }
            .sorted { ($0.mergedAt ?? $0.updatedAt ?? .distantPast) > ($1.mergedAt ?? $1.updatedAt ?? .distantPast) }
        let unknown = items.filter { $0.pull == nil }
        Group {
            if items.isEmpty {
                ContentUnavailableView {
                    Label("No Pull Requests", systemImage: "arrow.triangle.pull")
                } description: {
                    Text("PRs show up here once this thread works on a branch that has one, or a PR link comes up in it, such as Claude running gh pr create.")
                }
                .frame(maxHeight: .infinity)
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 10) {
                        SyncStatus(thread: thread, openCount: open.count)
                        ForEach(PullRequestStack.build(open)) { stack in
                            StackCard(stack: stack)
                        }
                        if !done.isEmpty {
                            Text("Merged and Closed")
                                .font(.caption.weight(.semibold))
                                .foregroundStyle(.secondary)
                                .padding(.top, open.isEmpty ? 0 : 6)
                            VStack(spacing: 0) {
                                ForEach(done) { pull in
                                    PullRequestCardRow(pull: pull, graph: nil)
                                    if pull.id != done.last?.id { Divider().padding(.leading, 34) }
                                }
                            }
                            .card()
                        }
                        ForEach(unknown) { item in
                            UnknownPullRequestRow(item: item)
                        }
                    }
                    .padding(12)
                    .animation(.smooth(duration: 0.35), value: items.compactMap(\.pull))
                }
            }
        }
        // Kept current while it's on screen.
        .task(id: thread.id) {
            while !Task.isCancelled {
                await app.syncPullRequests(for: thread)
                try? await Task.sleep(for: .seconds(15))
            }
        }
    }
}

/// "3 open · updated 12 s ago", and a way to look now.
private struct SyncStatus: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    let openCount: Int
    @State private var syncing = false

    var body: some View {
        HStack(spacing: 8) {
            Text(openCount == 0 ? "None open" : openCount == 1 ? "1 open" : "\(openCount) open")
                .font(.callout.weight(.medium))
            Spacer()
            if let synced = app.pullsSyncedAtObserved[thread.id] {
                TimelineView(.periodic(from: .now, by: 5)) { context in
                    Text("Updated \(ShortAge.string(from: synced, to: context.date))")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                }
            }
            Button("Refresh", systemImage: "arrow.clockwise") {
                Task {
                    syncing = true
                    await app.syncPullRequests(for: thread, force: true)
                    syncing = false
                }
            }
            .labelStyle(.iconOnly)
            .buttonStyle(.borderless)
            .symbolEffect(.rotate, options: .repeat(.continuous), isActive: syncing)
            .disabled(syncing)
            .help("Check the PRs now")
        }
    }
}

/// One stack (or a single PR) in a card, drawn as a tree: bottom of the stack first, a line
/// of PRs in one lane, each fork opening a lane to its right (see `StackGraph`).
private struct StackCard: View {
    let stack: PullRequestStack

    var body: some View {
        let graph = StackGraph.layout(stack)
        VStack(alignment: .leading, spacing: 0) {
            if stack.isStacked {
                Label("Stack of \(graph.rows.count)", systemImage: "square.stack")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 12)
                    .padding(.top, 9)
                    .padding(.bottom, 2)
            }
            ForEach(graph.rows, id: \.pullRequest.id) { row in
                PullRequestCardRow(pull: row.pullRequest,
                                   graph: stack.isStacked ? GraphCell(row: row, laneCount: graph.laneCount) : nil)
            }
        }
        .padding(.bottom, stack.isStacked ? 3 : 0)
        .card()
    }
}

/// A row's slice of the stack's tree: the lines through it, and its PR's dot (with its
/// layer) in its lane, level with the title. Rows stack up into one unbroken drawing.
struct GraphCell: View {
    let row: StackGraph.Row
    let laneCount: Int
    static let lane: CGFloat = 18
    static let dotY: CGFloat = 18
    static let dot: CGFloat = 16

    static func x(_ lane: Int) -> CGFloat { CGFloat(lane) * Self.lane + Self.dot / 2 }

    var body: some View {
        ZStack(alignment: .topLeading) {
            Canvas { context, size in
                var path = Path()
                for segment in row.segments {
                    switch segment {
                    case .through(let lane):
                        path.move(to: CGPoint(x: Self.x(lane), y: 0))
                        path.addLine(to: CGPoint(x: Self.x(lane), y: size.height))
                    case .fromAbove(let lane):
                        path.move(to: CGPoint(x: Self.x(lane), y: 0))
                        path.addLine(to: CGPoint(x: Self.x(lane), y: Self.dotY))
                    case .toBelow(let lane):
                        path.move(to: CGPoint(x: Self.x(lane), y: Self.dotY))
                        path.addLine(to: CGPoint(x: Self.x(lane), y: size.height))
                    case .branch(let from, let to):
                        // Down the parent's lane, round the corner, across into the dot.
                        let px = Self.x(from), cx = Self.x(to), radius: CGFloat = 8
                        path.move(to: CGPoint(x: px, y: 0))
                        path.addLine(to: CGPoint(x: px, y: Self.dotY - radius))
                        path.addQuadCurve(to: CGPoint(x: px + radius, y: Self.dotY), control: CGPoint(x: px, y: Self.dotY))
                        path.addLine(to: CGPoint(x: cx, y: Self.dotY))
                    }
                }
                context.stroke(path, with: .style(.quaternary), style: StrokeStyle(lineWidth: 2, lineCap: .round))
            }
            Text("\(row.layer)")
                .font(.system(size: 9, weight: .bold).monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: Self.dot, height: Self.dot)
                .background(Circle().fill(.background))
                .overlay(Circle().strokeBorder(.tertiary, lineWidth: 1.5))
                .offset(x: CGFloat(row.lane) * Self.lane, y: Self.dotY - Self.dot / 2)
        }
        .frame(width: CGFloat(laneCount - 1) * Self.lane + Self.dot)
        .frame(maxHeight: .infinity)
    }
}

/// A PR in the narrow sidebar: what it is, where it's going, and what stands in its way.
struct PullRequestCardRow: View {
    let pull: PullRequest
    let graph: GraphCell?
    @State private var hovering = false

    private var finished: Bool { pull.state != .open }

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            if let graph { graph }
            HStack(alignment: .top, spacing: 10) {
            Image(systemName: symbol)
                .font(.callout.weight(.medium))
                .foregroundStyle(tint)
                .frame(width: 16)
                .padding(.top, 1)
                .contentTransition(.symbolEffect(.replace))
            VStack(alignment: .leading, spacing: 4) {
                Text(pull.title)
                    .fontWeight(.medium)
                    .foregroundStyle(finished ? AnyShapeStyle(.secondary) : AnyShapeStyle(.primary))
                    .lineLimit(2)
                HStack(spacing: 4) {
                    Text("\(pull.remote.name) #\(pull.number)").monospacedDigit()
                    Text("·")
                    Text(pull.headBranch).font(.caption.monospaced()).lineLimit(1).truncationMode(.middle)
                    Image(systemName: "arrow.right").font(.system(size: 7, weight: .bold))
                    Text(pull.baseBranch).font(.caption.monospaced()).lineLimit(1)
                }
                .font(.caption)
                .foregroundStyle(.secondary)
                HStack(spacing: 6) {
                    StateChip(pull: pull)
                    if pull.state == .open {
                        if let hint = mergeHint { Chip(hint.text, tint: hint.color) }
                        ReviewBadge(review: pull.review)
                        ChecksBadge(checks: pull.checks)
                    }
                    Spacer(minLength: 0)
                    if !pull.isMine, let author = pull.author {
                        Text(author).font(.caption).foregroundStyle(.tertiary).lineLimit(1)
                    }
                    if let date = pull.mergedAt ?? pull.updatedAt {
                        TimelineView(.everyMinute) { context in
                            Text(ShortAge.string(from: date, to: context.date))
                                .font(.caption.monospacedDigit())
                                .foregroundStyle(.tertiary)
                        }
                    }
                }
                .font(.caption)
            }
            }
            // In a stack the tree runs the row's whole height, so the text keeps the space.
            .padding(.vertical, graph == nil ? 0 : 9)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, graph == nil ? 9 : 0)
        .fixedSize(horizontal: false, vertical: true)
        .background {
            if hovering { Color.primary.opacity(0.04) }
        }
        .contentShape(.rect)
        .onHover { hovering = $0 }
        .onTapGesture { NSWorkspace.shared.open(pull.url) }
        .pointerStyle(.link)
        .help("Open \(pull.remote.forge?.requestNoun ?? "pull request") #\(pull.number) in your browser")
        .contextMenu {
            Button("Open in Browser", systemImage: "safari") { NSWorkspace.shared.open(pull.url) }
            Button("Copy Link", systemImage: "link") { Pasteboard.copy(pull.url.absoluteString) }
            Button("Copy Branch Name", systemImage: "arrow.triangle.branch") { Pasteboard.copy(pull.headBranch) }
        }
    }

    private var symbol: String {
        switch pull.state {
        case .merged: "arrow.triangle.merge"
        case .closed: "xmark.circle"
        case .open: pull.isDraft ? "circle.dashed" : "arrow.triangle.pull"
        }
    }

    private var tint: AnyShapeStyle {
        switch pull.state {
        case .merged: AnyShapeStyle(.purple)
        case .closed: AnyShapeStyle(.red)
        case .open: pull.isDraft ? AnyShapeStyle(.secondary) : AnyShapeStyle(.green)
        }
    }

    /// What GitHub says is in the way of merging, when it's something to act on.
    private var mergeHint: (text: String, color: Color)? {
        switch pull.mergeStatus {
        case "DIRTY": ("Conflicts", .red)
        case "BEHIND": ("Behind base", .orange)
        case "BLOCKED" where pull.review == .none && pull.checks != .failing: ("Blocked", .orange)
        default: nil
        }
    }
}

/// Open, Draft, Auto-merge, In merge queue, Merged, Closed.
private struct StateChip: View {
    let pull: PullRequest

    var body: some View {
        let (text, color): (String, Color) = switch pull.state {
        case .merged: ("Merged", .purple)
        case .closed: ("Closed", .red)
        case .open where pull.inMergeQueue: ("In merge queue", .purple)
        case .open where pull.autoMerge: ("Auto-merge", .purple)
        case .open where pull.isDraft: ("Draft", .gray)
        case .open: ("Open", .green)
        }
        Chip(text, tint: color)
    }
}

/// A link that came up in the thread but hasn't been (or can't be) looked up: on GitLab,
/// or with `gh` signed out.
private struct UnknownPullRequestRow: View {
    let item: AppModel.ThreadPullRequest

    var body: some View {
        Button {
            NSWorkspace.shared.open(item.url)
        } label: {
            HStack(spacing: 10) {
                Image(systemName: "arrow.triangle.pull")
                    .foregroundStyle(.tertiary)
                    .frame(width: 16)
                VStack(alignment: .leading, spacing: 2) {
                    Text("\(item.repo) #\(item.number)").fontWeight(.medium)
                    Text("Mentioned in the thread; its state isn't known yet")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
            }
            .padding(12)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .card()
        .help(item.url.absoluteString)
    }
}
