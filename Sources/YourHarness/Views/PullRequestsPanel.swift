import AppKit
import HarnessCore
import SwiftUI

/// The right sidebar's PRs tab: every pull request a thread is linked to, open ones first
/// (as stacks when they're stacked), then merged and closed ones. Kept current while it's
/// showing, so asking Claude to merge shows the merge happen.
struct PullRequestsPanel: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    /// On screen: only then does it keep looking for changes.
    var isShown = true

    var body: some View {
        let items = app.pullRequests(for: thread)
        // What the thread worked on, as a tree; what only came up in it, below.
        let worked = items.filter(\.isWorkedOn)
        let mentioned = items.filter { !$0.isWorkedOn }
        // A GitHub stack still going keeps its merged layers, to show how far along it is.
        let pulls = worked.compactMap(\.pull)
        let live = Set(pulls.filter { $0.state == .open }.compactMap { $0.stack.map { [pull = $0] in "\(pull.remote.path)#\($0.number)" } })
        let inLiveStack = { (pull: PullRequest) in pull.stack.map { live.contains("\(pull.remote.path)#\($0.number)") } ?? false }
        let open = pulls.filter { $0.state == .open || inLiveStack($0) }
        let done = pulls.filter { $0.state != .open && !inLiveStack($0) }
            .sorted { ($0.mergedAt ?? $0.updatedAt ?? .distantPast) > ($1.mergedAt ?? $1.updatedAt ?? .distantPast) }
        let unknown = worked.filter { $0.pull == nil }
        Group {
            if items.isEmpty {
                ContentUnavailableView {
                    Label("No Pull Requests", systemImage: "arrow.triangle.pull")
                } description: {
                    Text("PRs show up here once this thread works on a branch that has one, or a PR link comes up in it, such as Claude running gh pr create.")
                } actions: {
                    SplitIntoStackButton(thread: thread)
                }
                .frame(maxHeight: .infinity)
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 10) {
                        SyncStatus(thread: thread, openCount: open.filter { $0.state == .open }.count)
                        ForEach(PullRequestStack.build(open)) { stack in
                            StackCard(stack: stack)
                        }
                        ForEach(localStacks, id: \.stack.id) { local in
                            LocalStackCard(stack: local.stack, repo: local.repo, workspace: local.workspace)
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
                        if !mentioned.isEmpty {
                            MentionedPullRequests(items: mentioned)
                                .padding(.top, worked.isEmpty ? 0 : 10)
                        }
                    }
                    .padding(12)
                    .animation(.smooth(duration: 0.35), value: items.compactMap(\.pull))
                }
            }
        }
        // Kept current while it's on screen.
        .task(id: "\(thread.id)\(isShown)") {
            guard isShown else { return }
            while !Task.isCancelled {
                await app.syncPullRequests(for: thread)
                try? await Task.sleep(for: .seconds(15))
            }
        }
    }
}

extension PullRequestsPanel {
    /// Stacks made with `gh stack` and not on GitHub yet, on a branch this thread works on.
    var localStacks: [(stack: LocalStack, repo: WorkspaceRepo, workspace: WorkspaceModel)] {
        guard let project = app.project(id: thread.projectID), let workspace = app.workspace(for: project) else { return [] }
        var found: [(LocalStack, WorkspaceRepo, WorkspaceModel)] = [], seen = Set<String>()
        for checkout in workspace.checkouts(for: thread) {
            guard let branch = workspace.branch(of: checkout) else { continue }
            for stack in workspace.localStacks[checkout.repo.id] ?? [] where stack.number == nil
                && stack.layers.contains(where: { $0.branch == branch }) && seen.insert(checkout.repo.id + stack.id).inserted {
                found.append((stack, checkout.repo, workspace))
            }
        }
        return found
    }
}

/// PRs that came up in the thread without it working on them: links you pasted, ones
/// Claude looked at or cited. Below the thread's own, in a plain list.
private struct MentionedPullRequests: View {
    let items: [AppModel.ThreadPullRequest]

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Mentioned")
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
                .help("Came up in this thread, but it didn't change them")
            VStack(spacing: 0) {
                ForEach(items) { item in
                    if let pull = item.pull {
                        PullRequestCardRow(pull: pull, graph: nil)
                    } else {
                        UnknownPullRequestRow(item: item, inCard: true)
                    }
                    if item.id != items.last?.id { Divider().padding(.leading, 34) }
                }
            }
            .card()
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
            SplitIntoStackButton(thread: thread, compact: true)
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
        let isStack = stack.isStacked || stack.number != nil
        VStack(alignment: .leading, spacing: 0) {
            if isStack {
                StackHeader(stack: stack)
                    .padding(.horizontal, 12)
                    .padding(.top, 9)
                    .padding(.bottom, 2)
            }
            ForEach(graph.rows, id: \.pullRequest.id) { row in
                PullRequestCardRow(pull: row.pullRequest,
                                   graph: isStack ? GraphCell(row: row, laneCount: graph.laneCount) : nil)
            }
            UnsubmittedLayers(stack: stack, below: graph.rows.count)
        }
        .padding(.bottom, isStack ? 3 : 0)
        .card()
    }
}

/// A row's slice of the stack's tree: the lines through it, and its PR's dot (with its
/// layer) in its lane, level with the title. Rows stack up into one unbroken drawing.
struct GraphCell: View {
    let row: StackGraph.Row
    /// How many lanes wide the cell is: the stack's, or (for a staircase) the row's own.
    let laneCount: Int
    /// Lane spacing.
    var step: CGFloat = 18
    /// Where the dot sits from the top; nil centres it, for one-line rows.
    var dotY: CGFloat? = 16
    static let dot: CGFloat = 16

    private func x(_ lane: Int) -> CGFloat { CGFloat(lane) * step + Self.dot / 2 }

    var body: some View {
        Canvas { context, size in
            let y = dotY ?? size.height / 2
            var path = Path()
            for segment in row.segments {
                switch segment {
                case .through(let lane):
                    path.move(to: CGPoint(x: x(lane), y: 0))
                    path.addLine(to: CGPoint(x: x(lane), y: size.height))
                case .fromAbove(let lane):
                    path.move(to: CGPoint(x: x(lane), y: 0))
                    path.addLine(to: CGPoint(x: x(lane), y: y))
                case .toBelow(let lane):
                    path.move(to: CGPoint(x: x(lane), y: y))
                    path.addLine(to: CGPoint(x: x(lane), y: size.height))
                case .branch(let from, let to):
                    // Down the parent's lane, round the corner, across into the dot.
                    let px = x(from), cx = x(to), radius = min(8, (cx - px) / 2)
                    path.move(to: CGPoint(x: px, y: 0))
                    path.addLine(to: CGPoint(x: px, y: y - radius))
                    path.addQuadCurve(to: CGPoint(x: px + radius, y: y), control: CGPoint(x: px, y: y))
                    path.addLine(to: CGPoint(x: cx, y: y))
                }
            }
            context.stroke(path, with: .style(.quaternary), style: StrokeStyle(lineWidth: 2, lineCap: .round))
        }
        .overlay(alignment: dotY == nil ? .leading : .topLeading) {
            // GitHub's own place in the stack when it has one: layers merged away still count.
            Text("\(row.pullRequest.stack?.position ?? row.layer)")
                .font(.system(size: 9, weight: .bold).monospacedDigit())
                .foregroundStyle(StateChip.color(row.pullRequest))
                .frame(width: Self.dot, height: Self.dot)
                .background(Circle().fill(.background))
                .overlay(Circle().strokeBorder(StateChip.color(row.pullRequest).opacity(0.8), lineWidth: 1.5))
                .offset(x: CGFloat(row.lane) * step, y: dotY.map { $0 - Self.dot / 2 } ?? 0)
        }
        .frame(width: CGFloat(laneCount - 1) * step + Self.dot)
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
        HStack(alignment: .top, spacing: 8) {
            // In a stack the dot carries the state; on its own, a small icon does.
            if let graph {
                graph
            } else {
                Image(systemName: symbol)
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(tint)
                    .frame(width: 16, height: 18)
                    .contentTransition(.symbolEffect(.replace))
            }
            VStack(alignment: .leading, spacing: 2) {
                // The title, with what to know about it on the same line.
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(pull.title)
                        .fontWeight(.medium)
                        .foregroundStyle(finished ? AnyShapeStyle(.secondary) : AnyShapeStyle(.primary))
                        .lineLimit(2)
                    Spacer(minLength: 4)
                    status
                }
                HStack(spacing: 4) {
                    Text("\(pull.remote.name) #\(pull.number)").monospacedDigit()
                    Text("·")
                    Text(pull.headBranch).font(.caption.monospaced()).lineLimit(1).truncationMode(.middle)
                    Image(systemName: "arrow.right").font(.system(size: 7, weight: .bold))
                    Text(pull.baseBranch).font(.caption.monospaced()).lineLimit(1)
                    if !pull.isMine, let author = pull.author {
                        Text("·")
                        Text(author).lineLimit(1)
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }
            // In a stack the tree runs the row's whole height, so the text keeps the space.
            .padding(.vertical, graph == nil ? 0 : 7)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, graph == nil ? 7 : 0)
        .fixedSize(horizontal: false, vertical: true)
        .background {
            if hovering { Color.primary.opacity(0.04) }
        }
        .contentShape(.rect)
        .onHover { hovering = $0 }
        .onTapGesture { NSWorkspace.shared.open(pull.url) }
        .pointerStyle(.link)
        .help("\(stateName). Open \(pull.remote.forge?.requestNoun ?? "pull request") #\(pull.number) in your browser")
        .contextMenu {
            Button("Open in Browser", systemImage: "safari") { NSWorkspace.shared.open(pull.url) }
            Button("Copy Link", systemImage: "link") { Pasteboard.copy(pull.url.absoluteString) }
            Button("Copy Branch Name", systemImage: "arrow.triangle.branch") { Pasteboard.copy(pull.headBranch) }
        }
    }

    /// Right of the title: a chip only when the state is worth saying (plain "Open" isn't),
    /// what blocks merging, review, checks, and age.
    @ViewBuilder
    private var status: some View {
        HStack(spacing: 5) {
            if let chip = StateChip.notable(pull) { Chip(chip.text, tint: chip.color) }
            if pull.state == .open {
                if let hint = mergeHint { Chip(hint.text, tint: hint.color) }
                ReviewBadge(review: pull.review)
                ChecksBadge(checks: pull.checks)
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
        .fixedSize()
    }

    private var stateName: String {
        StateChip.notable(pull)?.text ?? (pull.state == .open ? "Open" : pull.state.rawValue.capitalized)
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

/// Draft, Auto-merge, In merge queue, Merged, Closed: the states worth a chip. A plain open
/// PR needs none (the dot says it).
enum StateChip {
    static func notable(_ pull: PullRequest) -> (text: String, color: Color)? {
        switch pull.state {
        case .merged: ("Merged", .purple)
        case .closed: ("Closed", .red)
        case .open where pull.inMergeQueue: ("In merge queue", .purple)
        case .open where pull.autoMerge: ("Auto-merge", .purple)
        case .open where pull.isDraft: ("Draft", .gray)
        case .open: nil
        }
    }

    /// The colour the stack's dot takes: green open, grey draft, purple merging or merged.
    static func color(_ pull: PullRequest) -> Color {
        switch pull.state {
        case .merged: .purple
        case .closed: .red
        case .open where pull.inMergeQueue || pull.autoMerge: .purple
        case .open where pull.isDraft: .gray
        case .open: .green
        }
    }
}

/// A link that came up in the thread but hasn't been (or can't be) looked up: on GitLab,
/// or with `gh` signed out.
private struct UnknownPullRequestRow: View {
    let item: AppModel.ThreadPullRequest
    /// A row in a list's card, rather than a card of its own.
    var inCard = false

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
            .padding(inCard ? 10 : 12)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .modifier(CardUnless(inCard: inCard))
        .help(item.url.absoluteString)
    }
}

private struct CardUnless: ViewModifier {
    let inCard: Bool

    func body(content: Content) -> some View {
        if inCard { content } else { content.card() }
    }
}
