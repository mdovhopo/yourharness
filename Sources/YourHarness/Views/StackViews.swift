import HarnessCore
import SwiftUI

/// Above a stack's PRs: which stack it is, how far along, and what you can do with it:
/// merge all of it (or the layers up to one) on GitHub, and submit branches that have no
/// PR yet. A stack made without `gh stack` only gets its name.
struct StackHeader: View {
    @Environment(AppModel.self) private var app
    let stack: PullRequestStack
    @State private var confirming: MergeRequest?

    /// What a merge would take: the open layers up to and including the last one.
    struct MergeRequest: Identifiable {
        let pulls: [PullRequest]
        /// Nil for the whole stack.
        let upTo: PullRequest?
        var id: String { upTo?.id ?? "all" }
    }

    private var open: [PullRequest] { stack.all.filter { $0.state == .open } }

    /// Open layers from the bottom up to the first draft: GitHub merges no draft, nor anything above one.
    private var mergeable: [PullRequest] { Array(open.prefix { !$0.isDraft }) }

    /// Branches of this stack `gh stack` knows that don't have a PR yet.
    private var unsubmitted: (local: LocalStack, repo: WorkspaceRepo, workspace: WorkspaceModel)? {
        guard let number = stack.number, let clone = app.clone(of: stack.pullRequest.remote),
              let local = clone.workspace.localStack(number: number, remote: stack.pullRequest.remote),
              local.layers.contains(where: { $0.pullRequestNumber == nil }) else { return nil }
        return (local, clone.repo, clone.workspace)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Label(title, systemImage: "square.stack")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Spacer(minLength: 4)
                if stack.number != nil { actions }
            }
            if let error = app.stackErrors[stack.id] {
                StackError(text: error) { app.stackErrors[stack.id] = nil }
            }
        }
        .task(id: stack.number) {
            if stack.number != nil { await app.checkStacksInstalled() }
        }
        .confirmationDialog(confirmTitle, isPresented: Binding(get: { confirming != nil }, set: { if !$0 { confirming = nil } }),
                            presenting: confirming) { request in
            Button("Squash and Merge") {
                Task { await app.mergeStack(stack, upTo: request.upTo) }
            }
            Button("Cancel", role: .cancel) {}
        } message: { request in
            Text(request.pulls.map { "#\($0.number) \($0.title)" }.joined(separator: "\n")
                 + "\n\nGitHub merges them bottom up, each squashed into one commit. This can't be undone.")
        }
    }

    private var title: String {
        guard let number = stack.number else { return "Stack of \(stack.all.count)" }
        let size = max(stack.pullRequest.stack?.size ?? 0, stack.all.count)
        let merged = stack.all.filter { $0.state == .merged }.count
        if merged > 0 { return "Stack #\(number) · \(merged) of \(size) merged" }
        // Only open PRs listed (the repos view): the rest have merged or closed.
        return open.count < size ? "Stack #\(number) · \(open.count) of \(size) open" : "Stack #\(number) · \(size) PRs"
    }

    private var confirmTitle: String {
        guard let request = confirming else { return "" }
        let into = request.pulls.first?.baseBranch ?? "its base"
        return request.pulls.count == 1 ? "Squash and merge #\(request.pulls[0].number) into \(into)?"
            : "Squash and merge \(request.pulls.count) pull requests into \(into)?"
    }

    @ViewBuilder
    private var actions: some View {
        HStack(spacing: 6) {
            if let activity = app.stackActivity[stack.id] {
                ProgressView().controlSize(.mini)
                Text(activity).font(.caption).foregroundStyle(.secondary)
            } else if app.stacksInstalled == false {
                InstallStacksButton()
            } else {
                if let unsubmitted {
                    let count = unsubmitted.local.layers.filter { $0.pullRequestNumber == nil }.count
                    let checkout = unsubmitted.workspace.checkout(of: unsubmitted.repo, onAnyOf: unsubmitted.local.layers.map(\.branch))
                    Button("Submit") {
                        Task { await app.submitStack(unsubmitted.local, id: stack.id, in: unsubmitted.repo, of: unsubmitted.workspace) }
                    }
                    .disabled(checkout == nil)
                    .help(checkout == nil ? "Check out one of the stack's branches to submit it"
                          : count == 1 ? "Push the stack and open a PR for its new branch" : "Push the stack and open PRs for its \(count) new branches")
                }
                mergeControl
                StackMoreMenu(stack: stack)
            }
        }
        .controlSize(.small)
    }

    @ViewBuilder
    private var mergeControl: some View {
        let layers = mergeable
        if !open.isEmpty {
            if app.clone(of: stack.pullRequest.remote) == nil {
                Button("Merge") {}
                    .disabled(true)
                    .help("Merging runs gh stack in a clone of \(stack.pullRequest.remote.name); there isn't one in your projects")
            } else if layers.isEmpty {
                Button("Merge") {}
                    .disabled(true)
                    .help("The bottom PR is a draft: GitHub won't merge it")
            } else if layers.count == 1 {
                Button(open.count == 1 ? "Merge" : "Merge #\(layers[0].number)") {
                    confirming = MergeRequest(pulls: layers, upTo: open.count == 1 ? nil : layers[0])
                }
                .help("Squash and merge on GitHub")
            } else {
                Menu {
                    ForEach(Array(layers.enumerated().dropLast()), id: \.element.id) { index, pull in
                        Button("Merge Up to #\(pull.number)") {
                            confirming = MergeRequest(pulls: Array(layers.prefix(index + 1)), upTo: pull)
                        }
                    }
                } label: {
                    Text(layers.count == open.count ? "Merge Stack" : "Merge \(layers.count)")
                } primaryAction: {
                    confirming = MergeRequest(pulls: layers, upTo: layers.count == open.count ? nil : layers.last)
                }
                .menuStyle(.button)
                .fixedSize()
                .help(layers.count == open.count ? "Squash and merge the whole stack on GitHub, bottom up"
                      : "Squash and merge the layers below the first draft")
            }
        }
    }
}

/// `gh stack` isn't there: one click puts it there.
struct InstallStacksButton: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        if app.stackActivity["install"] != nil {
            ProgressView().controlSize(.mini)
        } else {
            Button("Install gh stack") { Task { await app.installStacks() } }
                .help("Stacks are merged and submitted with GitHub's gh stack extension: \(GitHubStacks.installCommand)")
        }
    }
}

/// What went wrong, in red, until dismissed.
struct StackError: View {
    let text: String
    let dismiss: () -> Void

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Image(systemName: "exclamationmark.triangle.fill")
            Text(text)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 4)
            Button("Dismiss", systemImage: "xmark", action: dismiss)
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
        }
        .font(.caption)
        .foregroundStyle(.red)
    }
}

/// A branch of a stack that has no PR yet: on top of the PRs, until it's submitted.
struct UnsubmittedLayerRow: View {
    let branch: String
    let layer: Int

    var body: some View {
        HStack(spacing: 8) {
            Text("\(layer)")
                .font(.system(size: 9, weight: .bold).monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: GraphCell.dot, height: GraphCell.dot)
                .overlay(Circle().strokeBorder(.secondary, style: StrokeStyle(lineWidth: 1.5, dash: [2.5, 2])))
            Text(branch)
                .font(.caption.monospaced())
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 4)
            Chip("Not submitted")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 7)
        .help("A branch of the stack with no pull request yet: Submit pushes it and opens one")
    }
}

/// A stack made with `gh stack` that isn't on GitHub yet: its branches, and Submit.
struct LocalStackCard: View {
    @Environment(AppModel.self) private var app
    let stack: LocalStack
    let repo: WorkspaceRepo
    let workspace: WorkspaceModel

    private var id: String { "local:\(repo.id):\(stack.id)" }

    var body: some View {
        let checkout = workspace.checkout(of: repo, onAnyOf: stack.layers.map(\.branch))
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 8) {
                    Label("\(repo.name) · stack of \(stack.layers.count), not submitted", systemImage: "square.stack")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    if let activity = app.stackActivity[id] {
                        ProgressView().controlSize(.mini)
                        Text(activity).font(.caption).foregroundStyle(.secondary)
                    } else if app.stacksInstalled == false {
                        InstallStacksButton()
                    } else {
                        Button("Submit") { Task { await app.submitStack(stack, id: id, in: repo, of: workspace) } }
                            .disabled(checkout == nil)
                            .help(checkout == nil ? "Check out one of the stack's branches to submit it"
                                  : "Push its branches and open a PR for each, into \(stack.trunk)")
                    }
                }
                .controlSize(.small)
                if let error = app.stackErrors[id] {
                    StackError(text: error) { app.stackErrors[id] = nil }
                }
            }
            .padding(.horizontal, 12)
            .padding(.top, 9)
            .padding(.bottom, 2)
            ForEach(Array(stack.layers.enumerated()), id: \.element.branch) { index, layer in
                UnsubmittedLayerRow(branch: layer.branch, layer: index + 1)
            }
        }
        .padding(.bottom, 3)
        .card()
        .task { await app.checkStacksInstalled() }
    }
}

/// A GitHub stack's branches that `gh stack` has locally but no PR yet, above its PRs.
struct UnsubmittedLayers: View {
    @Environment(AppModel.self) private var app
    let stack: PullRequestStack
    /// How many layers are drawn above already.
    let below: Int

    var body: some View {
        if let number = stack.number, let clone = app.clone(of: stack.pullRequest.remote),
           let local = clone.workspace.localStack(number: number, remote: stack.pullRequest.remote) {
            let pending = local.layers.enumerated().filter { $0.element.pullRequestNumber == nil }
            ForEach(pending, id: \.element.branch) { index, layer in
                UnsubmittedLayerRow(branch: layer.branch, layer: max(index + 1, below + 1))
            }
        }
    }
}

/// New Stack from Branches…: existing branches of a repo made a GitHub stack, bottom first,
/// with `gh stack init`, and submitted if you like.
struct NewStackSheet: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    let repo: WorkspaceRepo
    let workspace: WorkspaceModel
    @State private var branches: [String] = []
    /// Bottom first.
    @State private var chosen: [String] = []
    @State private var base = ""
    @State private var submit = true
    @State private var working = false
    @State private var error: String?

    private var trunk: String { base.isEmpty ? workspace.defaultBranches[repo.id] ?? "main" : base }

    /// Where `gh stack` runs: a checkout already on one of the branches, else the repo's
    /// own. Either way it ends up on the top branch.
    private var checkout: URL { workspace.checkout(of: repo, onAnyOf: chosen) ?? repo.url }

    private var checkoutIsDirty: Bool {
        if checkout == repo.url { return workspace.statuses[repo.id]?.isDirty ?? false }
        return workspace.worktreeStatuses[checkout.path]?.isDirty ?? false
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("New Stack in \(repo.name)")
                .font(.title2.weight(.semibold))
            Text("Existing branches, each a pull request on the one below it. Pick them bottom first: the first goes into \(trunk).")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack(alignment: .top, spacing: 12) {
                List {
                    ForEach(branches.filter { !chosen.contains($0) && $0 != trunk }, id: \.self) { branch in
                        Button {
                            chosen.append(branch)
                        } label: {
                            Label(branch, systemImage: "plus.circle").font(.callout.monospaced())
                        }
                        .buttonStyle(.plain)
                    }
                }
                .frame(height: 220)
                .overlay { if branches.isEmpty { ProgressView() } }
                List {
                    ForEach(Array(chosen.enumerated().reversed()), id: \.element) { index, branch in
                        HStack {
                            Text("\(index + 1)").font(.caption.monospacedDigit()).foregroundStyle(.secondary).frame(width: 16)
                            Text(branch).font(.callout.monospaced()).lineLimit(1)
                            Spacer()
                            Button("Remove", systemImage: "minus.circle") { chosen.removeAll { $0 == branch } }
                                .labelStyle(.iconOnly)
                                .buttonStyle(.borderless)
                        }
                    }
                    .onMove { from, to in
                        // Shown top first: move in that order, then flip back.
                        var shown = Array(chosen.reversed())
                        shown.move(fromOffsets: from, toOffset: to)
                        chosen = shown.reversed()
                    }
                }
                .frame(height: 220)
                .overlay {
                    if chosen.isEmpty {
                        Text("Click branches to add them,\nbottom first").multilineTextAlignment(.center).foregroundStyle(.tertiary)
                    }
                }
            }
            Form {
                TextField("Into", text: $base, prompt: Text(workspace.defaultBranches[repo.id] ?? "main"))
                Toggle("Submit to GitHub now (push and open the PRs)", isOn: $submit)
            }
            .formStyle(.grouped)
            if !chosen.isEmpty {
                Label("Runs in \(checkout.path.replacingOccurrences(of: NSHomeDirectory(), with: "~")), which ends up on \(chosen.last ?? "")",
                      systemImage: "arrow.triangle.branch")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            if checkoutIsDirty {
                Label("That checkout has uncommitted changes. Commit or stash them first.", systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.orange)
            }
            if let error {
                Label(error, systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                if app.stacksInstalled == false { InstallStacksButton() }
                Spacer()
                if working { ProgressView().controlSize(.small) }
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Create Stack") { Task { await create() } }
                    .buttonStyle(.glassProminent)
                    .keyboardShortcut(.defaultAction)
                    .disabled(chosen.isEmpty || working || checkoutIsDirty || app.stacksInstalled == false)
            }
        }
        .padding(20)
        .frame(width: 620)
        .task {
            await app.checkStacksInstalled()
            if let git = app.workspaceTools.git { branches = await git.localBranches(in: repo.url) }
        }
    }

    private func create() async {
        working = true
        defer { working = false }
        let into = base.trimmingCharacters(in: .whitespaces)
        error = await app.createStack(branches: chosen, base: into.isEmpty ? nil : into, in: checkout, of: workspace, submit: submit)
        if error == nil { dismiss() }
    }
}

/// A stack's other actions: a branch on top, and catching up with what merged below.
struct StackMoreMenu: View {
    @Environment(AppModel.self) private var app
    let stack: PullRequestStack
    @State private var naming = false
    @State private var name = ""

    /// A checkout on the stack's top branch, and the workspace it's in.
    private var top: (checkout: URL, workspace: WorkspaceModel)? {
        guard let clone = app.clone(of: stack.pullRequest.remote) else { return nil }
        var branches = stack.all.map(\.headBranch)
        if let number = stack.number, let local = clone.workspace.localStack(number: number, remote: stack.pullRequest.remote) {
            branches = local.layers.map(\.branch)
        }
        guard let last = branches.last, let checkout = clone.workspace.checkout(of: clone.repo, onAnyOf: [last]) else { return nil }
        return (checkout, clone.workspace)
    }

    private var anyCheckout: (checkout: URL, workspace: WorkspaceModel)? {
        guard let clone = app.clone(of: stack.pullRequest.remote),
              let checkout = clone.workspace.checkout(of: clone.repo, onAnyOf: stack.all.map(\.headBranch)) else { return nil }
        return (checkout, clone.workspace)
    }

    var body: some View {
        let top = top, any = anyCheckout
        Menu {
            Button("Add Branch on Top…", systemImage: "plus.square.on.square") { naming = true }
                .disabled(top == nil)
            Button("Sync with GitHub", systemImage: "arrow.triangle.2.circlepath") {
                if let any { Task { await app.syncStack(stack.id, in: any.checkout, of: any.workspace) } }
            }
            .disabled(any == nil)
            if top == nil || any == nil {
                Text("Check out one of the stack's branches (its top one, to add) to use these")
            }
        } label: {
            Image(systemName: "ellipsis")
        }
        .menuStyle(.button)
        .menuIndicator(.hidden)
        .fixedSize()
        .help("More for this stack")
        .alert("Add a Branch on Top", isPresented: $naming) {
            TextField("Branch", text: $name)
            Button("Add") {
                let branch = name.trimmingCharacters(in: .whitespaces)
                if let top, !branch.isEmpty { Task { await app.addBranch(branch, onTopOf: stack.id, in: top.checkout, of: top.workspace) } }
                name = ""
            }
            Button("Cancel", role: .cancel) { name = "" }
        } message: {
            Text("A new branch at the top branch's latest commit, checked out there. Commit to it, then Submit.")
        }
    }
}

/// Asks Claude to make the thread's work a stack of small PRs. Only with `gh stack` there,
/// and while Claude isn't busy.
struct SplitIntoStackButton: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    var compact = false

    var body: some View {
        if app.stacksInstalled == true, !thread.isSideChat {
            Button("Split into a Stack", systemImage: "square.stack") { app.splitIntoStack(thread) }
                .labelStyle(compact ? AnyLabelStyle(.iconOnly) : AnyLabelStyle(.titleAndIcon))
                .buttonStyle(.borderless)
                .disabled(thread.phase == .running)
                .help("Ask Claude to split this thread's work into a stack of small pull requests, with gh stack")
        }
    }
}

private struct AnyLabelStyle: LabelStyle {
    private let make: (Configuration) -> AnyView

    init(_ style: some LabelStyle) {
        make = { AnyView(style.makeBody(configuration: $0)) }
    }

    func makeBody(configuration: Configuration) -> some View { make(configuration) }
}
