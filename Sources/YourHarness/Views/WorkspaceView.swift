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
        let cloned = layout.clonedRepos.count
        let uncloned = layout.repos.count - cloned
        switch layout.kind {
        case .plain:
            return "Folder"
        case .repo:
            let branch = workspace.statuses[""]?.branch.map { " · \($0)" } ?? ""
            return "Git repository\(branch)"
        case .folder, .shell:
            var parts = ["Workspace", cloned == 1 ? "1 repo" : "\(cloned) repos"]
            if uncloned > 0 { parts.append("\(uncloned) not cloned") }
            return parts.joined(separator: " · ")
        }
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
                let rows = stack.flattened
                VStack(alignment: .leading, spacing: 0) {
                    if stack.isStacked {
                        Label("Stack of \(rows.count)", systemImage: "square.stack")
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 12)
                            .padding(.top, 9)
                            .padding(.bottom, 2)
                    }
                    ForEach(Array(rows.enumerated()), id: \.element.pullRequest.id) { index, row in
                        PullRequestRow(
                            pullRequest: row.pullRequest,
                            repoName: workspace.repo(for: row.pullRequest)?.name ?? row.pullRequest.remote.name,
                            rail: stack.isStacked ? Rail(depth: row.depth, isFirst: index == 0, isLast: index == rows.count - 1) : nil
                        )
                        if index < rows.count - 1, !stack.isStacked {
                            Divider().padding(.leading, 40)
                        }
                    }
                }
                .padding(.vertical, 3)
                .card()
                .padding(.bottom, 8)
        }
    }
}

/// Where a PR sits in its stack: drawn as a line through dots, bottom of the stack first.
private struct Rail {
    var depth: Int
    var isFirst: Bool
    var isLast: Bool
}

private struct PullRequestRow: View {
    let pullRequest: PullRequest
    let repoName: String
    let rail: Rail?
    @State private var hovering = false

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            if let rail {
                RailView(rail: rail)
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
        .help("Open \(pullRequest.remote.forge?.requestNoun ?? "pull request") in your browser")
        .contextMenu {
            Button("Open in Browser", systemImage: "safari") { NSWorkspace.shared.open(pullRequest.url) }
            Button("Copy Link", systemImage: "link") { Pasteboard.copy(pullRequest.url.absoluteString) }
            Button("Copy Branch Name", systemImage: "arrow.triangle.branch") { Pasteboard.copy(pullRequest.headBranch) }
        }
    }
}

private struct RailView: View {
    let rail: Rail

    var body: some View {
        ZStack {
            VStack(spacing: 0) {
                Rectangle().fill(rail.isFirst ? .clear : Color.secondary.opacity(0.35))
                Rectangle().fill(rail.isLast ? .clear : Color.secondary.opacity(0.35))
            }
            .frame(width: 2)
            Circle()
                .fill(.background)
                .strokeBorder(Color.secondary.opacity(0.6), lineWidth: 2)
                .frame(width: 9, height: 9)
        }
        .frame(width: 12)
        .padding(.leading, CGFloat(rail.depth) * 14)
        .padding(.vertical, -8)
    }
}

private struct ChecksBadge: View {
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

private struct ReviewBadge: View {
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
    let workspace: WorkspaceModel
    @State private var showUncloned = false

    var body: some View {
        let repos = workspace.clonedRepos
        SectionTitle("Repositories", count: repos.count)
            .padding(.bottom, 12)

        ForEach(Array(repos.enumerated()), id: \.element.id) { index, repo in
            RepoRow(repo: repo, workspace: workspace)
                .modifier(CardSegment(isFirst: index == 0, isLast: index == repos.count - 1))
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
                if !isLast { Divider().padding(.leading, 40) }
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
    let repo: WorkspaceRepo
    let workspace: WorkspaceModel
    @State private var hovering = false

    private var status: RepoStatus? { workspace.statuses[repo.id] }
    private var remote: GitRemote? { workspace.remotes[repo.id] }
    private var pullCount: Int { workspace.pullRequests(for: repo).count }

    var body: some View {
        HStack(spacing: 10) {
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
                }
                BranchLabel(status: status)
            }

            Spacer(minLength: 8)

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
            Button("Copy Path", systemImage: "doc.on.doc") { Pasteboard.copy(repo.url.path) }
            if let branch = status?.branch {
                Button("Copy Branch Name", systemImage: "arrow.triangle.branch") { Pasteboard.copy(branch) }
            }
        }
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
