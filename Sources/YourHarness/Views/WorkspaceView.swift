import AppKit
import HarnessCore
import SwiftUI

/// A project's repos and open PRs at a glance. Read-only: everything is discovered from
/// git and the forge CLIs.
struct WorkspaceView: View {
    @Environment(AppModel.self) private var app
    let project: Project
    let workspace: WorkspaceModel

    var body: some View {
        ScrollView {
            // Lazy all the way down: workspaces can hold hundreds of repos and PRs, and only
            // the rows on screen should exist.
            LazyVStack(alignment: .leading, spacing: 0) {
                WorkspaceHeader(project: project, workspace: workspace)
                    .padding(.bottom, 28)

                if let layout = workspace.layout, layout.kind == .plain {
                    ContentUnavailableView {
                        Label("No Git Repositories", systemImage: "folder")
                    } description: {
                        Text("Repos and pull requests show up here once this folder, or folders inside it, are git repositories.")
                    }
                    // The column's width, so it sits centred under the header, not at its leading edge.
                    .frame(maxWidth: .infinity)
                    .padding(.top, 40)
                } else if workspace.layout != nil {
                    PullRequestsSection(workspace: workspace)
                    RepositoriesSection(workspace: workspace)
                }
            }
            .frame(maxWidth: 880, alignment: .leading)
            .padding(.horizontal, 32)
            .padding(.vertical, 28)
            .frame(maxWidth: .infinity)
        }
        .scrollIndicators(.automatic)
        .onAppear { app.computeFootprints() }
        .animation(.smooth(duration: 0.3), value: workspace.worktrees)
        .animation(.smooth(duration: 0.3), value: workspace.pullRequests)
        .animation(.smooth(duration: 0.3), value: workspace.statuses)
        .animation(.smooth(duration: 0.3), value: workspace.layout)
    }
}

// MARK: - Header

private struct WorkspaceHeader: View {
    @Environment(AppModel.self) private var app
    let project: Project
    let workspace: WorkspaceModel

    private var subtitle: String {
        guard let layout = workspace.layout else { return "Looking for repositories…" }
        let cloned = workspace.clonedRepos.count
        let uncloned = workspace.unclonedRepos.count
        switch layout.kind {
        case .plain:
            return "Folder"
        case .repo:
            let branch = workspace.statuses[""]?.branch.map { " · \($0)" } ?? ""
            return (["Git repository\(branch)"] + worktreeSummary).joined(separator: " · ")
        case .folder, .shell:
            var parts = ["Workspace", cloned == 1 ? "1 repo" : "\(cloned) repos"]
            if uncloned > 0 { parts.append("\(uncloned) not cloned") }
            return (parts + worktreeSummary).joined(separator: " · ")
        }
    }

    /// "3 worktrees", and what needs a look: not on a branch, done with, missing.
    private var worktreeSummary: [String] {
        let all = workspace.clonedRepos.flatMap { repo in (workspace.worktrees[repo.id] ?? []).map { (repo, $0) } }
        guard !all.isEmpty else { return [] }
        var parts = [all.count == 1 ? "1 worktree" : "\(all.count) worktrees"]
        let detached = all.count { $0.1.isDetached && !$0.1.isMissing }
        let finished = all.count { workspace.isFinished($0.1, in: $0.0) }
        let missing = all.count { $0.1.isMissing }
        if detached > 0 { parts.append("\(detached) not on a branch") }
        if finished > 0 { parts.append("\(finished) done with") }
        if missing > 0 { parts.append("\(missing) missing") }
        return parts
    }

    var body: some View {
        HStack(alignment: .center, spacing: 14) {
            Image(systemName: ProjectIcon.symbol(for: workspace.layout?.kind))
                .font(.system(size: 22, weight: .medium))
                .foregroundStyle(.secondary)
                .frame(width: 44, height: 44)
                .card()

            VStack(alignment: .leading, spacing: 3) {
                Text(project.name)
                    .font(.title2.weight(.semibold))
                Text(subtitle)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .contentTransition(.opacity)
            }

            Spacer()

            RefreshButton(workspace: workspace)

            Button("New Thread", systemImage: "square.and.pencil") {
                withAnimation(SidebarMetrics.selectionSpring) { _ = app.newThread(in: project) }
            }
            .buttonStyle(.glass)
        }
        .contextMenu {
            Button("Show in Finder", systemImage: "folder") { app.revealInFinder(project) }
        }
    }
}

private struct RefreshButton: View {
    @Environment(AppModel.self) private var app
    let workspace: WorkspaceModel

    var body: some View {
        HStack(spacing: 8) {
            if let updated = workspace.pullsUpdatedAt ?? workspace.statusUpdatedAt {
                TimelineView(.everyMinute) { _ in
                    Text("Updated \(updated, format: .relative(presentation: .named))")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                }
            }
            Button {
                Task { await workspace.refresh(tools: app.workspaceTools, rescan: true, pullsMaxAge: 0) }
            } label: {
                Image(systemName: "arrow.clockwise")
                    .symbolEffect(.rotate, options: .repeat(.continuous), isActive: workspace.isRefreshing)
            }
            .buttonStyle(.glass)
            .disabled(workspace.isRefreshing)
            .help("Refresh repos and pull requests")
        }
    }
}

enum ProjectIcon {
    static func symbol(for kind: WorkspaceLayout.Kind?) -> String {
        switch kind {
        case .folder, .shell: "square.stack.3d.up.fill"
        case .repo, .plain, nil: "folder.fill"
        }
    }
}

// MARK: - Pull requests

private struct PullRequestsSection: View {
    let workspace: WorkspaceModel
    @State private var showOthers = false

    private var allUnreachable: Bool {
        !workspace.unreachable.isEmpty
            && workspace.unreachable.count >= Set(workspace.remotes.values.filter { $0.forge != nil }).count
    }

    /// Repos the signed-in account can't see, so their PRs are missing from the list.
    private var unreachableNote: String? {
        let count = workspace.unreachable.count
        guard count > 0 else { return nil }
        let total = Set(workspace.remotes.values.filter { $0.forge != nil }).count
        let forges = Set(workspace.unreachable.compactMap(\.forge))
        let account = forges.count == 1 ? forges.first.flatMap { workspace.forgeAccounts[$0] } : nil
        let who = account.map { "@\($0)" } ?? "your account"
        let repos = count == total ? (count == 1 ? "This repo isn't" : "These repos aren't") : "\(count) of \(total) repos aren't"
        return "\(repos) visible to \(who), so their pull requests can't be shown."
    }

    var body: some View {
        let sections = workspace.pullRequestSections
        let isEmpty = sections.needsReview.isEmpty && sections.mine.isEmpty && sections.others.isEmpty

        if workspace.hasForgeRemotes || !workspace.forgeErrors.isEmpty {
            Group {
                SectionTitle("Pull Requests", count: workspace.pullRequests.count)
                    .padding(.bottom, unreachableNote == nil ? 14 : 4)
                if let unreachableNote {
                    Label(unreachableNote, systemImage: "lock.fill")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .padding(.bottom, 14)
                }

                ForEach(workspace.forgeErrors.sorted(by: { $0.key.rawValue < $1.key.rawValue }), id: \.key) { forge, message in
                    ForgeNotice(forge: forge, message: message)
                        .padding(.bottom, 14)
                }

                if isEmpty {
                    if workspace.pullsUpdatedAt != nil, workspace.forgeErrors.isEmpty, !allUnreachable {
                        Text("No open pull requests.")
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .padding(.bottom, 14)
                    } else if workspace.isRefreshingPulls {
                        HStack(spacing: 8) {
                            ProgressView().controlSize(.small)
                            Text("Loading pull requests…").foregroundStyle(.secondary)
                        }
                        .font(.callout)
                        .padding(.bottom, 14)
                    }
                }

                let unsubmitted = workspace.unsubmittedStacks.filter { $0.stack.number == nil }
                if !unsubmitted.isEmpty {
                    GroupLabel(title: "Not Submitted", symbol: "square.stack", count: unsubmitted.count)
                        .foregroundStyle(.secondary)
                        .padding(.bottom, 8)
                    ForEach(unsubmitted, id: \.stack.id) { item in
                        LocalStackCard(stack: item.stack, repo: item.repo, workspace: workspace)
                            .padding(.bottom, 8)
                    }
                    Color.clear.frame(height: 10)
                }
                if !sections.needsReview.isEmpty {
                    PullRequestGroup(title: "Needs Your Review", symbol: "eye", stacks: sections.needsReview, workspace: workspace)
                }
                if !sections.mine.isEmpty {
                    PullRequestGroup(title: "Yours", symbol: "person", stacks: sections.mine, workspace: workspace)
                }
                if !sections.others.isEmpty {
                    // Busy repos have plenty of PRs that aren't yours; keep them out of the way
                    // unless there's nothing else to show.
                    let expanded = showOthers || (sections.needsReview.isEmpty && sections.mine.isEmpty)
                    Button {
                            withAnimation(.snappy(duration: 0.25)) { showOthers.toggle() }
                        } label: {
                            HStack(spacing: 6) {
                                Image(systemName: "chevron.right")
                                    .font(.system(size: 9, weight: .bold))
                                    .rotationEffect(.degrees(expanded ? 90 : 0))
                                GroupLabel(title: "Everyone Else", symbol: "person.2", count: sections.others.flatMap(\.all).count)
                            }
                            .contentShape(.rect)
                        }
                        .buttonStyle(.plain)
                        .foregroundStyle(.secondary)
                        .padding(.bottom, 8)

                    if expanded {
                        StackList(stacks: sections.others, workspace: workspace)
                    }
                }
            }
            Color.clear.frame(height: 16)
        }
    }
}

private struct PullRequestGroup: View {
    let title: String
    let symbol: String
    let stacks: [PullRequestStack]
    let workspace: WorkspaceModel

    var body: some View {
        GroupLabel(title: title, symbol: symbol, count: stacks.flatMap(\.all).count)
            .foregroundStyle(.secondary)
            .padding(.bottom, 8)
        StackList(stacks: stacks, workspace: workspace)
        Color.clear.frame(height: 10)
    }
}

private struct GroupLabel: View {
    let title: String
    let symbol: String
    let count: Int

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: symbol)
            Text(title)
            Text("\(count)")
                .monospacedDigit()
                .foregroundStyle(.tertiary)
        }
        .font(.subheadline.weight(.semibold))
    }
}

/// One card per stack. A lone PR is a stack of one.
private struct StackList: View {
    let stacks: [PullRequestStack]
    let workspace: WorkspaceModel

    var body: some View {
        ForEach(stacks) { stack in
                // A staircase: each PR a step in from its base, the lines joined up.
                let rows = StackGraph.layout(stack, lanes: .depth).rows
                let isStack = stack.isStacked || stack.number != nil
                VStack(alignment: .leading, spacing: 0) {
                    if isStack {
                        StackHeader(stack: stack)
                            .padding(.horizontal, 12)
                            .padding(.top, 9)
                            .padding(.bottom, 2)
                    }
                    ForEach(Array(rows.enumerated()), id: \.element.pullRequest.id) { index, row in
                        PullRequestRow(
                            pullRequest: row.pullRequest,
                            repoName: workspace.repo(for: row.pullRequest)?.name ?? row.pullRequest.remote.name,
                            graph: isStack ? GraphCell(row: row, laneCount: row.lane + 1, step: 22, dotY: nil) : nil
                        )
                        if index < rows.count - 1, !isStack {
                            Divider().padding(.leading, 40)
                        }
                    }
                    UnsubmittedLayers(stack: stack, below: rows.count)
                }
                .padding(.vertical, 3)
                .card()
                .padding(.bottom, 8)
        }
    }
}

private struct PullRequestRow: View {
    let pullRequest: PullRequest
    let repoName: String
    let graph: GraphCell?
    @State private var hovering = false

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            if let graph {
                // Out to the row's edges, so the lines meet the rows above and below.
                graph.padding(.vertical, -8)
            }
            Image(systemName: pullRequest.isDraft ? "circle.dashed" : "arrow.triangle.pull")
                .font(.callout.weight(.medium))
                .foregroundStyle(pullRequest.isDraft ? AnyShapeStyle(.secondary) : AnyShapeStyle(Color.green))
                .frame(width: 18)
                .help(pullRequest.isDraft ? "Draft" : "Open")

            VStack(alignment: .leading, spacing: 3) {
                Text(pullRequest.title)
                    .lineLimit(1)
                    .truncationMode(.tail)
                HStack(spacing: 5) {
                    Text(repoName).fontWeight(.medium)
                    Text("#\(pullRequest.number)").monospacedDigit()
                    Text("·")
                    Text(pullRequest.headBranch)
                        .font(.caption.monospaced())
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Image(systemName: "arrow.right").font(.system(size: 8, weight: .semibold))
                    Text(pullRequest.baseBranch)
                        .font(.caption.monospaced())
                        .lineLimit(1)
                    if !pullRequest.isMine, let author = pullRequest.author {
                        Text("·")
                        Text(author)
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }

            Spacer(minLength: 8)

            ChecksBadge(checks: pullRequest.checks)
            ReviewBadge(review: pullRequest.review)
            if let updated = pullRequest.updatedAt {
                TimelineView(.everyMinute) { context in
                    Text(ShortAge.string(from: updated, to: context.date))
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(.tertiary)
                        .frame(minWidth: 34, alignment: .trailing)
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background {
            if hovering {
                RoundedRectangle(cornerRadius: 9, style: .continuous)
                    .fill(Color.primary.opacity(0.05))
                    .padding(.horizontal, 4)
            }
        }
        .contentShape(.rect)
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: hovering)
        .onTapGesture { NSWorkspace.shared.open(pullRequest.url) }
        .pointerStyle(.link)
        .help("Open \(pullRequest.remote.forge?.requestNoun ?? "pull request") in your browser")
        .contextMenu {
            Button("Open in Browser", systemImage: "safari") { NSWorkspace.shared.open(pullRequest.url) }
            Button("Copy Link", systemImage: "link") { Pasteboard.copy(pullRequest.url.absoluteString) }
            Button("Copy Branch Name", systemImage: "arrow.triangle.branch") { Pasteboard.copy(pullRequest.headBranch) }
        }
    }
}

struct ChecksBadge: View {
    let checks: PullRequest.Checks

    var body: some View {
        switch checks {
        case .passing:
            Image(systemName: "checkmark.circle.fill").foregroundStyle(.green).help("Checks passing")
        case .failing:
            Image(systemName: "xmark.circle.fill").foregroundStyle(.red).help("Checks failing")
        case .pending:
            Image(systemName: "clock.fill").foregroundStyle(.orange).help("Checks running")
        case .none:
            EmptyView()
        }
    }
}

struct ReviewBadge: View {
    let review: PullRequest.Review

    var body: some View {
        let (text, color): (String?, Color) = switch review {
        case .approved: ("Approved", .green)
        case .changesRequested: ("Changes", .orange)
        case .required: ("Review", .secondary)
        case .none: (nil, .clear)
        }
        if let text {
            Chip(text, tint: color)
                .help(review == .changesRequested ? "Changes requested" : review == .required ? "Review required" : "Approved")
        }
    }
}

private struct ForgeNotice: View {
    let forge: Forge
    let message: String

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
            Text(LocalizedStringKey(message))
                .font(.callout)
                .foregroundStyle(.secondary)
            Spacer()
            if message.hasPrefix("Install") {
                Link("Get \(forge.cliName)", destination: forge.installURL)
                    .font(.callout)
            }
        }
        .padding(12)
        .card(tint: .orange)
    }
}

// MARK: - Repositories

private struct RepositoriesSection: View {
    @Environment(AppModel.self) private var app
    let workspace: WorkspaceModel
    @State private var showUncloned = false
    @State private var filter = ""

    /// A repo's row, then one per worktree, in one card.
    private struct Line: Identifiable {
        let repo: WorkspaceRepo
        let worktree: Worktree?
        var id: String { worktree?.path ?? repo.id }
    }

    /// Each repo, then its worktrees if it's open. While filtering, repos and worktrees whose
    /// name, branch or threads match, with matching worktrees shown even in a folded repo.
    private func lines(_ repos: [WorkspaceRepo], threads: [String: [ThreadModel]]) -> [Line] {
        let query = filter.trimmingCharacters(in: .whitespaces)
        func matches(_ texts: [String?]) -> Bool {
            texts.contains { $0?.localizedCaseInsensitiveContains(query) == true }
        }
        return repos.flatMap { repo -> [Line] in
            let worktrees = workspace.worktrees[repo.id] ?? []
            guard !query.isEmpty else {
                return [Line(repo: repo, worktree: nil)] + (app.worktreesExpanded(repo) ? worktrees.map { Line(repo: repo, worktree: $0) } : [])
            }
            let repoMatches = matches([repo.name, workspace.statuses[repo.id]?.branch]
                + (threads[WorkspaceModel.realPath(repo.url.path)] ?? []).map(\.title))
            let hits = worktrees.filter { worktree in
                matches([worktree.url.lastPathComponent, worktree.branch] + (threads[worktree.path] ?? []).map(\.title))
            }
            guard repoMatches || !hits.isEmpty else { return [] }
            return [Line(repo: repo, worktree: nil)] + hits.map { Line(repo: repo, worktree: $0) }
        }
    }

    /// Most used first (by how many threads worked in them), then by name. A shell's own
    /// repo stays on top: it's the frame the others sit in.
    private var sortedRepos: [WorkspaceRepo] {
        workspace.clonedRepos.sorted { a, b in
            let shellA = a.isRoot && workspace.layout?.kind == .shell, shellB = b.isRoot && workspace.layout?.kind == .shell
            if shellA != shellB { return shellA }
            let useA = app.usage(of: a), useB = app.usage(of: b)
            if useA != useB { return useA > useB }
            return a.name.localizedStandardCompare(b.name) == .orderedAscending
        }
    }

    var body: some View {
        let repos = sortedRepos
        let threads = workspace.threadsByCheckout(app.threads)
        let lines = lines(repos, threads: threads)
        let withWorktrees = repos.filter { !(workspace.worktrees[$0.id] ?? []).isEmpty }
        let anyExpanded = withWorktrees.contains(where: app.worktreesExpanded)
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            SectionTitle("Repositories", count: repos.count)
            Spacer()
            // Big workspaces: find a repo, worktree, branch or thread by name.
            if repos.count > 6 || workspace.allWorktrees.count > 8 {
                TextField("Filter", text: $filter, prompt: Text("Filter repos, worktrees, threads"))
                    .textFieldStyle(.roundedBorder)
                    .controlSize(.small)
                    .frame(maxWidth: 220)
            }
            if !withWorktrees.isEmpty {
                Button(anyExpanded ? "Collapse All" : "Expand All") {
                    withAnimation(.snappy(duration: 0.25)) { app.setWorktreesExpanded(!anyExpanded, for: withWorktrees) }
                }
                .buttonStyle(.borderless)
                .font(.callout)
                .help(anyExpanded ? "Fold every repo's worktrees" : "Show every repo's worktrees")
            }
        }
        .padding(.bottom, 12)

        if lines.isEmpty, !filter.isEmpty {
            Text("Nothing matches “\(filter)”.")
                .foregroundStyle(.secondary)
                .padding(.vertical, 8)
        }

        ForEach(Array(lines.enumerated()), id: \.element.id) { index, line in
            Group {
                if let worktree = line.worktree {
                    WorktreeRow(worktree: worktree, repo: line.repo, workspace: workspace, threads: threads[worktree.path] ?? [])
                } else {
                    RepoRow(repo: line.repo, workspace: workspace, threads: threads[WorkspaceModel.realPath(line.repo.url.path)] ?? [])
                }
            }
            .modifier(CardSegment(isFirst: index == 0, isLast: index == lines.count - 1))
        }

        let uncloned = workspace.unclonedRepos
        if !uncloned.isEmpty {
            Button {
                withAnimation(.snappy(duration: 0.25)) { showUncloned.toggle() }
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 9, weight: .bold))
                        .rotationEffect(.degrees(showUncloned ? 90 : 0))
                    GroupLabel(title: "Not Cloned", symbol: "icloud.and.arrow.down", count: uncloned.count)
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .padding(.top, 18)
            .padding(.bottom, 10)
            .help("Referenced by this workspace (for example in its compose file) but not on disk yet")

            if showUncloned {
                // A few columns of names: these can number in the hundreds.
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 190), spacing: 8, alignment: .leading)], alignment: .leading, spacing: 6) {
                    ForEach(uncloned) { repo in
                        Text(repo.name)
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .help(repo.relativePath)
                    }
                }
                .padding(.leading, 18)
            }
        }
    }
}

/// One row's slice of a rounded card, so a long list can be lazy and still read as one
/// grouped box: the outline's top and bottom edges only show on the first and last rows.
private struct CardSegment: ViewModifier {
    let isFirst: Bool
    let isLast: Bool
    private let radius: CGFloat = 12

    func body(content: Content) -> some View {
        let shape = UnevenRoundedRectangle(
            topLeadingRadius: isFirst ? radius : 0, bottomLeadingRadius: isLast ? radius : 0,
            bottomTrailingRadius: isLast ? radius : 0, topTrailingRadius: isFirst ? radius : 0,
            style: .continuous
        )
        content
            .padding(.top, isFirst ? 3 : 0)
            .padding(.bottom, isLast ? 3 : 0)
            .overlay(alignment: .bottom) {
                if !isLast { Divider().padding(.leading, 48) }
            }
            .background(.fill.quaternary, in: shape)
            .overlay {
                // Push the horizontal edges out of view on middle rows, then clip them off.
                shape.strokeBorder(.separator.opacity(0.6))
                    .padding(.top, isFirst ? 0 : -2)
                    .padding(.bottom, isLast ? 0 : -2)
            }
            .clipShape(shape)
    }
}

private struct RepoRow: View {
    @Environment(AppModel.self) private var app
    let repo: WorkspaceRepo
    let workspace: WorkspaceModel
    let threads: [ThreadModel]
    @State private var hovering = false
    @State private var addingWorktree = false
    @State private var addingStack = false

    private var status: RepoStatus? { workspace.statuses[repo.id] }
    private var remote: GitRemote? { workspace.remotes[repo.id] }
    private var pullCount: Int { workspace.pullRequests(for: repo).count }

    private var worktrees: [Worktree] { workspace.worktrees[repo.id] ?? [] }
    private var collapsed: Bool { !app.worktreesExpanded(repo) }

    var body: some View {
        HStack(spacing: 10) {
            // Folds this repo's worktrees; remembered.
            Group {
                if worktrees.isEmpty {
                    Color.clear
                } else {
                    Button(collapsed ? "Show Worktrees" : "Hide Worktrees", systemImage: "chevron.right") {
                        withAnimation(.snappy(duration: 0.25)) { app.setWorktreesExpanded(collapsed, for: [repo]) }
                    }
                    .labelStyle(.iconOnly)
                    .buttonStyle(.borderless)
                    .font(.system(size: 10, weight: .bold))
                    .foregroundStyle(.secondary)
                    .rotationEffect(.degrees(collapsed ? 0 : 90))
                    .help(collapsed ? "Show its \(worktrees.count) worktree\(worktrees.count == 1 ? "" : "s")" : "Hide its worktrees")
                }
            }
            .frame(width: 12)
            .padding(.trailing, -4)
            Image(systemName: repo.isRoot && workspace.layout?.kind == .shell ? "square.stack.3d.up" : "shippingbox")
                .foregroundStyle(.secondary)
                .frame(width: 18)
                .help(repo.isRoot && workspace.layout?.kind == .shell ? "The workspace repo itself" : "")

            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(repo.name)
                        .fontWeight(.medium)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    if workspace.unreachable.contains(where: { $0 == remote }) {
                        Image(systemName: "lock.fill")
                            .font(.caption2)
                            .foregroundStyle(.tertiary)
                            .help("\(remote?.forge?.displayName ?? "The host") couldn't find this repo with your account")
                    }
                    if collapsed, !worktrees.isEmpty {
                        Button {
                            withAnimation(.snappy(duration: 0.25)) { app.setWorktreesExpanded(true, for: [repo]) }
                        } label: {
                            WorktreeCountBadge(count: worktrees.count, needsLook: needsLook)
                        }
                        .buttonStyle(.plain)
                        .help(foldedSummary)
                    }
                }
                BranchLabel(status: status)
            }

            Spacer(minLength: 8)

            ThreadChips(threads: threads)
            if let status {
                SyncLabel(status: status)
                ChangesLabel(status: status)
            } else if workspace.isRefreshingStatus {
                ProgressView().controlSize(.mini)
            }
            if pullCount > 0 {
                Label("\(pullCount)", systemImage: "arrow.triangle.pull")
                    .font(.caption.weight(.medium).monospacedDigit())
                    .foregroundStyle(.secondary)
                    .help("\(pullCount) open \(remote?.forge?.requestNoun ?? "pull request")\(pullCount == 1 ? "" : "s")")
            }
            if let web = remote?.webURL {
                Button("Open on \(remote?.forge?.displayName ?? remote?.host ?? "Web")", systemImage: "arrow.up.forward.square") {
                    NSWorkspace.shared.open(web)
                }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .foregroundStyle(.secondary)
                .opacity(hovering ? 1 : 0)
                .help("Open on \(remote?.forge?.displayName ?? remote?.host ?? "the web")")
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .contentShape(.rect)
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: hovering)
        .contextMenu {
            Button("Show in Finder", systemImage: "folder") { NSWorkspace.shared.activateFileViewerSelecting([repo.url]) }
            if let web = remote?.webURL {
                Button("Open on \(remote?.forge?.displayName ?? remote?.host ?? "Web")", systemImage: "safari") { NSWorkspace.shared.open(web) }
            }
            Button("Open in Terminal", systemImage: "terminal") { app.openInTerminal(repo.url) }
            Button("Copy Path", systemImage: "doc.on.doc") { Pasteboard.copy(repo.url.path) }
            if let branch = status?.branch {
                Button("Copy Branch Name", systemImage: "arrow.triangle.branch") { Pasteboard.copy(branch) }
            }
            Divider()
            Button("New Worktree…", systemImage: "plus.square.on.square") { addingWorktree = true }
            if remote?.forge == .github {
                Button("New Stack from Branches…", systemImage: "square.stack") { addingStack = true }
            }
            if let project = app.project(id: workspace.projectID) {
                Button("New Thread Here", systemImage: "square.and.pencil") { app.newThread(in: project, workingIn: repo.url.path) }
            }
            if workspace.worktrees[repo.id]?.contains(where: \.isMissing) == true {
                Button("Forget Missing Worktrees", systemImage: "trash.slash") { Task { _ = await app.pruneWorktrees(of: repo, in: workspace) } }
            }
        }
        .sheet(isPresented: $addingWorktree) { NewWorktreeSheet(repo: repo, workspace: workspace) }
        .sheet(isPresented: $addingStack) { NewStackSheet(repo: repo, workspace: workspace) }
    }
}

/// How many worktrees a folded repo has, with a dot when one of them needs a look.
private struct WorktreeCountBadge: View {
    let count: Int
    let needsLook: Bool

    var body: some View {
        HStack(spacing: 3) {
            Image(systemName: WorktreeRow.symbol)
                .font(.system(size: 9, weight: .semibold))
            Text("\(count)")
                .font(.caption.weight(.medium).monospacedDigit())
            if needsLook {
                Circle().fill(.orange).frame(width: 5, height: 5)
            }
        }
        .foregroundStyle(.secondary)
        .padding(.horizontal, 6)
        .padding(.vertical, 1)
        .background(.quaternary.opacity(0.7), in: .capsule)
        .contentShape(.capsule)
    }
}

extension RepoRow {
    /// Something among its worktrees wants attention: changes, no branch, missing folder.
    fileprivate var needsLook: Bool {
        worktrees.contains { $0.isMissing || $0.isDetached || (workspace.worktreeStatuses[$0.path]?.isDirty ?? false) }
    }

    /// Folded: how many worktrees, and what among them needs a look.
    fileprivate var foldedSummary: String {
        var parts = [worktrees.count == 1 ? "1 worktree" : "\(worktrees.count) worktrees"]
        let changed = worktrees.count { (workspace.worktreeStatuses[$0.path]?.isDirty ?? false) && !$0.isMissing }
        let detached = worktrees.count { $0.isDetached && !$0.isMissing }
        let finished = worktrees.count { workspace.isFinished($0, in: repo) }
        let missing = worktrees.count(where: \.isMissing)
        if changed > 0 { parts.append("\(changed) with changes") }
        if detached > 0 { parts.append("\(detached) not on a branch") }
        if finished > 0 { parts.append("\(finished) done with") }
        if missing > 0 { parts.append("\(missing) missing") }
        return parts.joined(separator: " · ") + ". Click to show them."
    }
}

private struct BranchLabel: View {
    let status: RepoStatus?

    var body: some View {
        HStack(spacing: 4) {
            Image(systemName: "arrow.triangle.branch")
                .font(.system(size: 9, weight: .semibold))
            if let status {
                if let branch = status.branch {
                    Text(branch)
                } else if let oid = status.headOID {
                    Text("Detached at \(String(oid.prefix(7)))")
                } else {
                    Text("No commits yet")
                }
            } else {
                Text("—")
            }
        }
        .font(.caption.monospaced())
        .foregroundStyle(.secondary)
        .lineLimit(1)
        .truncationMode(.middle)
    }
}

private struct SyncLabel: View {
    let status: RepoStatus

    var body: some View {
        Group {
            if status.branch != nil, status.upstream == nil {
                Text("Local only")
                    .foregroundStyle(.tertiary)
                    .help("This branch isn't pushed")
            } else if status.ahead > 0 || status.behind > 0 {
                HStack(spacing: 6) {
                    if status.ahead > 0 {
                        Label("\(status.ahead)", systemImage: "arrow.up")
                            .help("\(status.ahead) commit\(status.ahead == 1 ? "" : "s") to push")
                    }
                    if status.behind > 0 {
                        Label("\(status.behind)", systemImage: "arrow.down")
                            .help("\(status.behind) commit\(status.behind == 1 ? "" : "s") to pull (as of the last fetch)")
                    }
                }
                .labelStyle(CompactLabelStyle())
                .foregroundStyle(.secondary)
            }
        }
        .font(.caption.weight(.medium).monospacedDigit())
    }
}

private struct ChangesLabel: View {
    let status: RepoStatus

    var body: some View {
        if status.conflicted > 0 {
            badge("\(status.conflicted) conflicted", color: .red)
        } else if status.isDirty {
            badge("\(status.changedFiles) changed", color: .orange)
                .help(detail)
        }
    }

    private var detail: String {
        var parts: [String] = []
        if status.staged > 0 { parts.append("\(status.staged) staged") }
        if status.modified > 0 { parts.append("\(status.modified) modified") }
        if status.untracked > 0 { parts.append("\(status.untracked) untracked") }
        return parts.joined(separator: ", ")
    }

    private func badge(_ text: String, color: Color) -> some View {
        HStack(spacing: 5) {
            Circle().fill(color).frame(width: 6, height: 6)
            Text(text)
        }
        .font(.caption.weight(.medium).monospacedDigit())
        .foregroundStyle(.secondary)
    }
}

private struct CompactLabelStyle: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 2) {
            configuration.icon.font(.system(size: 9, weight: .bold))
            configuration.title
        }
    }
}

// MARK: - Shared

private struct SectionTitle: View {
    let title: String
    let count: Int

    init(_ title: String, count: Int) {
        self.title = title
        self.count = count
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(title)
                .font(.title3.weight(.semibold))
            if count > 0 {
                Text("\(count)")
                    .font(.callout.weight(.medium).monospacedDigit())
                    .foregroundStyle(.tertiary)
                    .contentTransition(.numericText())
            }
        }
    }
}

enum Pasteboard {
    static func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }
}

// MARK: - Worktrees

/// A linked worktree under its repo: where it is, which branch it's on (or that it's on
/// none), what's in it, its PR, and the threads working in it.
private struct WorktreeRow: View {
    /// What stands for a worktree: another checkout beside the repo's own.
    static let symbol = "rectangle.on.rectangle"
    @Environment(AppModel.self) private var app
    let worktree: Worktree
    let repo: WorkspaceRepo
    let workspace: WorkspaceModel
    let threads: [ThreadModel]
    @State private var confirmingRemoval = false
    @State private var error: String?
    @State private var hovering = false
    @State private var removing = false

    private var status: RepoStatus? { workspace.worktreeStatuses[worktree.path] }
    private var pullRequest: PullRequest? { workspace.pullRequest(for: worktree, in: repo) }
    private var isFinished: Bool { workspace.isFinished(worktree, in: repo) }

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: worktree.isMissing ? "questionmark.folder" : Self.symbol)
                .foregroundStyle(worktree.isMissing ? AnyShapeStyle(.orange) : AnyShapeStyle(.tertiary))
                .frame(width: 18)

            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(worktree.url.lastPathComponent)
                        .fontWeight(.medium)
                        .lineLimit(1)
                    Text(location)
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                        .truncationMode(.head)
                        .layoutPriority(-1)
                    if worktree.isLocked {
                        Image(systemName: "lock.fill")
                            .font(.caption2)
                            .foregroundStyle(.tertiary)
                            .help(worktree.lockReason.map { "Locked: \($0)" } ?? "Locked: git won't remove or prune it")
                    }
                }
                WorktreeBranchLabel(worktree: worktree)
            }

            Spacer(minLength: 8)

            ThreadChips(threads: threads)
            if worktree.isMissing {
                StateBadge(text: "Folder missing", color: .orange)
                    .help("Its folder was moved or deleted. Forget Missing Worktrees (on the repo) cleans it up.")
            } else if let status {
                if isFinished {
                    StateBadge(text: status.upstreamGone ? "Remote branch deleted" : "Merged into \(defaultBranch)", color: .green)
                        .help("Its work has landed. If nothing in it is still needed, it can be removed.")
                } else if workspace.hasNoWork(worktree, in: repo), !status.isDirty {
                    Text("No commits yet")
                        .font(.caption.weight(.medium))
                        .foregroundStyle(.tertiary)
                        .help("Its branch points at a commit of \(defaultBranch): nothing has been committed on it")
                } else {
                    SyncLabel(status: status)
                }
                ChangesLabel(status: status)
            } else if workspace.isRefreshingStatus {
                ProgressView().controlSize(.mini)
            }
            if let pullRequest {
                Button("#\(pullRequest.number)") { NSWorkspace.shared.open(pullRequest.url) }
                    .buttonStyle(.borderless)
                    .font(.caption.weight(.medium).monospacedDigit())
                    .help(pullRequest.title)
            }
            removeButton
        }
        .padding(.leading, 38)
        .padding(.trailing, 12)
        .padding(.vertical, 7)
        .contentShape(.rect)
        .onHover { hovering = $0 }
        .opacity(removing ? 0.4 : 1)
        // The button only shows on hover; VoiceOver gets it as an action on the row.
        .accessibilityElement(children: .contain)
        .accessibilityAction(named: "Remove Worktree") { if !worktree.isLocked { requestRemoval() } }
        .contextMenu {
            Button("Show in Finder", systemImage: "folder") { NSWorkspace.shared.activateFileViewerSelecting([worktree.url]) }
                .disabled(worktree.isMissing)
            Button("Open in Terminal", systemImage: "terminal") { app.openInTerminal(worktree.url) }
                .disabled(worktree.isMissing)
            Button("Copy Path", systemImage: "doc.on.doc") { Pasteboard.copy(worktree.path) }
            if let branch = worktree.branch {
                Button("Copy Branch Name", systemImage: "arrow.triangle.branch") { Pasteboard.copy(branch) }
            }
            if let pullRequest {
                Button("Open #\(pullRequest.number)", systemImage: "arrow.triangle.pull") { NSWorkspace.shared.open(pullRequest.url) }
            }
            Divider()
            if let project = app.project(id: workspace.projectID), !worktree.isMissing {
                Button("New Thread Here", systemImage: "square.and.pencil") { app.newThread(in: project, workingIn: worktree.path) }
            }
            Button(risk == .none ? "Remove Worktree" : "Remove Worktree…", systemImage: "trash", role: .destructive, action: requestRemoval)
                .disabled(worktree.isLocked)
        }
        .confirmationDialog(removalTitle, isPresented: $confirmingRemoval) {
            if case .losesWork = risk {
                Button("Remove and Discard", role: .destructive) { remove() }
            } else {
                Button("Remove", role: .destructive) { remove() }
            }
        } message: {
            Text(removalMessage)
        }
        .alert("Couldn't remove the worktree", isPresented: Binding(get: { error != nil }, set: { if !$0 { error = nil } })) {
            Button("OK") { error = nil }
        } message: {
            Text(error ?? "")
        }
    }

    private var defaultBranch: String {
        let base = workspace.defaultBranches[repo.id] ?? "main"
        return base.hasPrefix("origin/") ? String(base.dropFirst("origin/".count)) : base
    }

    /// Inside the project: relative; elsewhere: with ~ for your home folder.
    private var location: String {
        let folder = (worktree.path as NSString).deletingLastPathComponent
        let root = WorkspaceModel.realPath(workspace.root.path)
        if folder == root { return "" }
        if folder.hasPrefix(root + "/") { return String(folder.dropFirst(root.count + 1)) + "/" }
        return folder.replacingOccurrences(of: NSHomeDirectory(), with: "~") + "/"
    }

    private var risk: WorkspaceModel.RemovalRisk { workspace.removalRisk(worktree, in: repo) }

    /// One click: always there for a worktree that's safe to remove, on hover otherwise.
    @ViewBuilder
    private var removeButton: some View {
        let safe = risk == .none
        Button("Remove", systemImage: worktree.isMissing ? "trash.slash" : "trash", action: requestRemoval)
            .labelStyle(.iconOnly)
            .buttonStyle(.borderless)
            .foregroundStyle(safe ? AnyShapeStyle(.secondary) : AnyShapeStyle(.tertiary))
            .opacity(hovering || safe || removing ? 1 : 0)
            .disabled(worktree.isLocked || removing)
            .help(removeHelp)
    }

    private var removeHelp: String {
        if worktree.isLocked { return "Locked\(worktree.lockReason.map { ": \($0)" } ?? ""). Unlock it with git worktree unlock to remove it." }
        if worktree.isMissing { return "Forget this worktree: its folder is already gone" }
        switch risk {
        case .none: return "Remove this worktree: nothing in it would be lost"
        case .keepsWork: return "Remove this worktree (asks first: its branch has work in progress)"
        case .losesWork: return "Remove this worktree (asks first: something in it would be lost)"
        }
    }

    /// Safe: just remove it. Work in it: ask, saying what happens to it.
    private func requestRemoval() {
        if risk == .none { remove() } else { confirmingRemoval = true }
    }

    private var removalTitle: String {
        if case .losesWork = risk { return "Discard the work in “\(worktree.url.lastPathComponent)”?" }
        return "Remove the worktree “\(worktree.url.lastPathComponent)”?"
    }

    private var removalMessage: String {
        var parts: [String] = []
        switch risk {
        case .keepsWork(let why), .losesWork(let why): parts.append(why)
        case .none: break
        }
        parts.append("Its folder is deleted.")
        if threads.count == 1 { parts.append("The thread that worked in it keeps its conversation.") }
        if threads.count > 1 { parts.append("The \(threads.count) threads that worked in it keep their conversations.") }
        return parts.joined(separator: " ")
    }

    private func remove() {
        removing = true
        Task {
            if worktree.isMissing {
                error = await app.pruneWorktrees(of: repo, in: workspace)
            } else {
                // Uncommitted changes need --force; git refuses otherwise.
                error = await app.removeWorktree(worktree, of: repo, in: workspace, force: status?.isDirty ?? false)
            }
            removing = false
        }
    }
}

/// The branch, or that HEAD isn't on one: commits made there belong to no branch.
private struct WorktreeBranchLabel: View {
    let worktree: Worktree

    var body: some View {
        HStack(spacing: 4) {
            if let branch = worktree.branch {
                Image(systemName: "arrow.triangle.branch")
                    .font(.system(size: 9, weight: .semibold))
                Text(branch)
                    .foregroundStyle(.secondary)
            } else {
                Image(systemName: "exclamationmark.triangle.fill")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(.orange)
                Text("Not on a branch\(worktree.headOID.map { " · \($0.prefix(7))" } ?? "")")
                    .foregroundStyle(.orange)
                    .help("HEAD is detached: new commits here aren't on any branch, and are easy to lose. Check out or create a branch in it.")
            }
        }
        .font(.caption.monospaced())
        .foregroundStyle(.secondary)
        .lineLimit(1)
        .truncationMode(.middle)
    }
}

private struct StateBadge: View {
    let text: String
    let color: Color

    var body: some View {
        HStack(spacing: 5) {
            Circle().fill(color).frame(width: 6, height: 6)
            Text(text)
        }
        .font(.caption.weight(.medium))
        .foregroundStyle(.secondary)
    }
}

/// The threads that work in a checkout, as one pill however many there are: their title
/// when it's a single active one, else how many are active and settled. Settled ones are
/// done with, so a checkout that only has those reads quiet. Click for the list.
struct ThreadChips: View {
    let threads: [ThreadModel]
    @State private var showing = false

    private var active: [ThreadModel] { threads.filter { !$0.isSettled }.sorted { $0.sortDate > $1.sortDate } }
    private var settled: [ThreadModel] { threads.filter(\.isSettled).sorted { ($0.settledAt ?? .distantPast) > ($1.settledAt ?? .distantPast) } }

    var body: some View {
        if !threads.isEmpty {
            Button { showing.toggle() } label: { label }
                .buttonStyle(.plain)
                .help(help)
                .popover(isPresented: $showing, arrowEdge: .bottom) {
                    ThreadList(active: active, settled: settled) { showing = false }
                }
        }
    }

    private var label: some View {
        let onlySettled = active.isEmpty
        return HStack(spacing: 5) {
            Image(systemName: onlySettled ? "archivebox" : "bubble.left")
                .font(.system(size: 10, weight: .semibold))
            if active.count == 1, settled.isEmpty, let thread = active.first {
                Text(thread.title)
                    .lineLimit(1)
                    .frame(maxWidth: 170, alignment: .leading)
                    .fixedSize(horizontal: true, vertical: false)
            } else {
                if !active.isEmpty { Text("\(active.count) active") }
                if !active.isEmpty, !settled.isEmpty { Text("·").foregroundStyle(.tertiary) }
                if !settled.isEmpty { Text("\(settled.count) settled").foregroundStyle(.tertiary) }
            }
            if active.contains(where: \.isWorking) {
                WorkingDot()
            }
        }
        .font(.caption.monospacedDigit())
        .foregroundStyle(onlySettled ? AnyShapeStyle(.tertiary) : AnyShapeStyle(.primary))
        .padding(.horizontal, 8)
        .padding(.vertical, 3)
        .background {
            if onlySettled {
                Capsule().strokeBorder(.quaternary, style: StrokeStyle(lineWidth: 1, dash: [3, 2]))
            } else {
                Capsule().fill(.quaternary.opacity(0.7))
            }
        }
        .contentShape(.capsule)
        .fixedSize()
    }

    private var help: String {
        var parts: [String] = []
        if !active.isEmpty { parts.append(active.count == 1 ? "1 active thread" : "\(active.count) active threads") }
        if !settled.isEmpty { parts.append(settled.count == 1 ? "1 settled" : "\(settled.count) settled") }
        return parts.joined(separator: ", ") + " work here. Click to see them."
    }
}

/// Pulses while a thread here is working.
private struct WorkingDot: View {
    @State private var dim = false

    var body: some View {
        Circle()
            .fill(.tint)
            .frame(width: 6, height: 6)
            .opacity(dim ? 0.35 : 1)
            .animation(.easeInOut(duration: 0.8).repeatForever(), value: dim)
            .onAppear { dim = true }
            .help("Working now")
    }
}

/// The threads of a checkout: active ones first, then settled ones, dimmed.
private struct ThreadList: View {
    @Environment(AppModel.self) private var app
    let active: [ThreadModel]
    let settled: [ThreadModel]
    let close: () -> Void

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 2) {
                if !active.isEmpty {
                    header("Active")
                    ForEach(active) { row($0, settled: false) }
                }
                if !settled.isEmpty {
                    header("Settled")
                        .padding(.top, active.isEmpty ? 0 : 8)
                    ForEach(settled) { row($0, settled: true) }
                }
            }
            .padding(8)
        }
        .frame(width: 320)
        .frame(maxHeight: 360)
        .fixedSize(horizontal: false, vertical: true)
    }

    private func header(_ title: String) -> some View {
        Text(title)
            .font(.caption.weight(.semibold))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 8)
            .padding(.top, 4)
    }

    private func row(_ thread: ThreadModel, settled: Bool) -> some View {
        Button {
            close()
            app.selectedThreadID = thread.id
        } label: {
            HStack(spacing: 8) {
                Image(systemName: settled ? "archivebox" : thread.isWorking ? "circle.dotted" : "bubble.left")
                    .foregroundStyle(settled ? AnyShapeStyle(.tertiary) : AnyShapeStyle(.secondary))
                    .frame(width: 16)
                Text(thread.title)
                    .lineLimit(1)
                    .foregroundStyle(settled ? AnyShapeStyle(.secondary) : AnyShapeStyle(.primary))
                Spacer(minLength: 8)
                Text((settled ? thread.settledAt : nil) ?? thread.sortDate, format: .relative(presentation: .numeric, unitsStyle: .narrow))
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.tertiary)
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 5)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .help(settled ? "Settled: done with. Opening it keeps it on the shelf." : "Open “\(thread.title)”")
    }
}

/// New Worktree…: a branch (new, or one that exists) checked out in its own folder.
struct NewWorktreeSheet: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    let repo: WorkspaceRepo
    let workspace: WorkspaceModel
    @State private var branch = ""
    @State private var base = ""
    @State private var location: URL?
    @State private var working = false
    @State private var error: String?
    @State private var startThread = true

    private var trimmed: String { branch.trimmingCharacters(in: .whitespaces) }
    private var target: URL { location ?? app.defaultWorktreeLocation(repo: repo, branch: trimmed.isEmpty ? "branch" : trimmed) }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("New Worktree of \(repo.name)")
                .font(.title2.weight(.semibold))
            Text("Another checkout of the repo, on its own branch, so work there doesn't touch this one. A thread can work in as many as it needs.")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Form {
                TextField("Branch", text: $branch, prompt: Text("feature/login"))
                TextField("Starting from", text: $base, prompt: Text(workspace.defaultBranches[repo.id] ?? "HEAD"))
                    .help("Used for a new branch. An existing branch is checked out as it is.")
                LabeledContent("Folder") {
                    HStack {
                        Text(target.path.replacingOccurrences(of: NSHomeDirectory(), with: "~"))
                            .font(.callout.monospaced())
                            .lineLimit(1)
                            .truncationMode(.head)
                        Button("Change…", action: chooseFolder)
                    }
                }
                Toggle("Start a thread in it", isOn: $startThread)
            }
            .formStyle(.grouped)
            if let error {
                Label(error, systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Create") { Task { await create() } }
                    .buttonStyle(.glassProminent)
                    .keyboardShortcut(.defaultAction)
                    .disabled(trimmed.isEmpty || working)
            }
        }
        .padding(20)
        .frame(width: 520)
    }

    private func chooseFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        panel.prompt = "Put It Here"
        panel.message = "Choose the folder to create the worktree in."
        if panel.runModal() == .OK, let url = panel.url {
            location = url.appending(path: trimmed.isEmpty ? "worktree" : trimmed.replacingOccurrences(of: "/", with: "-"))
        }
    }

    private func create() async {
        working = true
        defer { working = false }
        let start = base.trimmingCharacters(in: .whitespaces)
        let folder = target
        if let failure = await app.addWorktree(to: repo, in: workspace, branch: trimmed, from: start.isEmpty ? workspace.defaultBranches[repo.id] : start, at: folder) {
            error = failure
            return
        }
        if startThread, let project = app.project(id: workspace.projectID) {
            app.newThread(in: project, workingIn: folder.path)
        }
        dismiss()
    }
}
