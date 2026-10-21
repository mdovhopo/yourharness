import AppKit
import HarnessCore
import SwiftUI

/// The repos and worktrees a thread works in, from the toolbar: as many as it touched,
/// with what state each is in. Hidden while it hasn't touched any.
struct ThreadWorktreesButton: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    let project: Project
    let workspace: WorkspaceModel
    @State private var showing = false

    var body: some View {
        let checkouts = workspace.checkouts(for: thread)
        if !checkouts.isEmpty {
            Button {
                showing.toggle()
            } label: {
                Label("\(checkouts.count)", systemImage: "arrow.triangle.branch")
                    .labelStyle(.titleAndIcon)
                    .monospacedDigit()
            }
            .help(checkouts.count == 1 ? "The checkout this thread works in" : "The \(checkouts.count) checkouts this thread works in")
            .popover(isPresented: $showing, arrowEdge: .bottom) {
                ThreadWorktreesList(thread: thread, project: project, workspace: workspace, checkouts: checkouts) { showing = false }
            }
        }
    }
}

private struct ThreadWorktreesList: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    let project: Project
    let workspace: WorkspaceModel
    let checkouts: [WorkspaceModel.Checkout]
    let close: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Works In")
                .font(.headline)
                .padding(.horizontal, 14)
                .padding(.top, 12)
                .padding(.bottom, 8)
            ForEach(checkouts) { checkout in
                CheckoutRow(thread: thread, workspace: workspace, checkout: checkout)
                if checkout.id != checkouts.last?.id { Divider().padding(.leading, 14) }
            }
            Divider()
            HStack {
                if thread.workingDirectory != nil {
                    Button("Back to the Project Folder") { thread.setWorkingDirectory(nil) }
                        .help("Run in \(project.name) again. The next message starts a new Claude session.")
                }
                Spacer()
                Button("All Worktrees") {
                    close()
                    app.showOverview(of: project)
                }
            }
            .buttonStyle(.borderless)
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
        }
        .frame(width: 400)
    }
}

private struct CheckoutRow: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    let workspace: WorkspaceModel
    let checkout: WorkspaceModel.Checkout

    private var status: RepoStatus? {
        checkout.worktree.map { workspace.worktreeStatuses[$0.path] } ?? workspace.statuses[checkout.repo.id]
    }
    private var runsHere: Bool { thread.workingDirectory.map(WorkspaceModel.realPath) == checkout.path }
    private var url: URL { URL(fileURLWithPath: checkout.path, isDirectory: true) }

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: checkout.worktree == nil ? "shippingbox" : "arrow.turn.down.right")
                .foregroundStyle(.secondary)
                .frame(width: 16)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(checkout.worktree.map { $0.url.lastPathComponent } ?? checkout.repo.name)
                        .fontWeight(.medium)
                    if checkout.worktree != nil {
                        Text("of \(checkout.repo.name)")
                            .foregroundStyle(.secondary)
                    }
                    if runsHere {
                        Text("Runs here")
                            .font(.caption2.weight(.semibold))
                            .padding(.horizontal, 5)
                            .padding(.vertical, 1)
                            .background(.tint.opacity(0.15), in: .capsule)
                    }
                }
                Text(stateLine)
                    .font(.caption)
                    .foregroundStyle(isAttention ? AnyShapeStyle(.orange) : AnyShapeStyle(.secondary))
                    .lineLimit(2)
            }
            Spacer(minLength: 6)
            Menu {
                Button("Show in Finder", systemImage: "folder") { NSWorkspace.shared.activateFileViewerSelecting([url]) }
                Button("Open in Terminal", systemImage: "terminal") { app.openInTerminal(url) }
                Button("Copy Path", systemImage: "doc.on.doc") { Pasteboard.copy(checkout.path) }
                if !runsHere {
                    Divider()
                    Button("Run This Thread Here", systemImage: "arrow.right.circle") { thread.setWorkingDirectory(checkout.path) }
                        .help("Claude starts here from the next message, in a new session")
                }
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
    }

    private var isAttention: Bool {
        checkout.worktree?.isDetached == true || checkout.worktree?.isMissing == true || (status?.conflicted ?? 0) > 0
    }

    /// Branch, then what's going on in it, in one line.
    private var stateLine: String {
        var parts: [String] = []
        if let worktree = checkout.worktree, worktree.isMissing { return "Folder missing" }
        if let branch = checkout.worktree?.branch ?? (checkout.worktree == nil ? status?.branch : nil) {
            parts.append(branch)
        } else {
            parts.append("Not on a branch")
        }
        if let status {
            if status.conflicted > 0 { parts.append("\(status.conflicted) conflicted") }
            else if status.isDirty { parts.append("\(status.changedFiles) changed") }
            if let worktree = checkout.worktree, workspace.isFinished(worktree, in: checkout.repo) {
                parts.append(status.upstreamGone ? "remote branch deleted" : "merged")
            } else if status.branch != nil, status.upstream == nil {
                parts.append("not pushed")
            } else {
                if status.ahead > 0 { parts.append("\(status.ahead) to push") }
                if status.behind > 0 { parts.append("\(status.behind) to pull") }
            }
        }
        if let worktree = checkout.worktree, let pr = workspace.pullRequest(for: worktree, in: checkout.repo) {
            parts.append("#\(pr.number)")
        } else if checkout.worktree == nil, let branch = status?.branch,
                  let pr = workspace.pullRequests(for: checkout.repo).first(where: { $0.headBranch == branch }) {
            parts.append("#\(pr.number)")
        }
        return parts.joined(separator: " · ")
    }
}

/// The pull requests a thread is linked to, from the toolbar: opens the right sidebar's PRs
/// tab, where they're listed in full. Hidden while there are none.
struct ThreadPullRequestsButton: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel

    var body: some View {
        let count = app.pullRequests(for: thread).count
        if count > 0 {
            Button {
                if app.showsInspector, app.inspectorPanel == .pullRequests {
                    app.setShowsInspector(false)
                } else {
                    app.showInspector(.pullRequests)
                }
            } label: {
                Label("\(count)", systemImage: "arrow.triangle.pull")
                    .labelStyle(.titleAndIcon)
                    .monospacedDigit()
            }
            .help(count == 1 ? "The pull request this thread is linked to" : "The \(count) pull requests this thread is linked to")
            .task(id: thread.pullRequestLinks) { await app.syncPullRequests(for: thread) }
        }
    }
}
