import Foundation
import HarnessCore

extension WorkspaceModel {
    /// The local stack GitHub numbers `number` in the repo `remote` points at.
    func localStack(number: Int, remote: GitRemote) -> LocalStack? {
        guard let repo = repos.first(where: { remotes[$0.id] == remote }) else { return nil }
        return localStacks[repo.id]?.first { $0.number == number }
    }

    /// Stacks made with `gh stack` that have a branch without a PR yet, by repo.
    var unsubmittedStacks: [(repo: WorkspaceRepo, stack: LocalStack)] {
        clonedRepos.flatMap { repo in
            (localStacks[repo.id] ?? []).filter { $0.layers.contains { $0.pullRequestNumber == nil } }.map { (repo, $0) }
        }
    }

    /// A checkout (main or worktree) of the repo that's on one of `branches`: where `gh stack`
    /// knows which stack is meant.
    func checkout(of repo: WorkspaceRepo, onAnyOf branches: [String]) -> URL? {
        let wanted = Set(branches)
        if let branch = statuses[repo.id]?.branch, wanted.contains(branch) { return repo.url }
        return (worktrees[repo.id] ?? []).first { !$0.isMissing && $0.branch.map(wanted.contains) == true }.map(\.url)
    }
}

extension AppModel {
    /// The clone of the repo a PR is in, in any project.
    func clone(of remote: GitRemote) -> (workspace: WorkspaceModel, repo: WorkspaceRepo)? {
        for workspace in projects.compactMap(workspace(for:)) {
            if let repo = workspace.clonedRepos.first(where: { workspace.remotes[$0.id] == remote }) { return (workspace, repo) }
        }
        return nil
    }

    /// Asks once whether `gh stack` is installed.
    func checkStacksInstalled() async {
        guard stacksInstalled == nil, let stacks = workspaceTools.stacks else { return }
        stacksInstalled = await stacks.isInstalled()
    }

    func installStacks() async {
        guard let stacks = workspaceTools.stacks else { return }
        stackActivity["install"] = "Installing…"
        let output = await CommandRunner.run(stacks.executable, arguments: ["extension", "install", "github/gh-stack"],
                                             environment: stacks.environment, timeout: 120)
        stackActivity["install"] = nil
        stacksInstalled = output.succeeded
        if !output.succeeded { stackErrors["install"] = output.stderr.trimmingCharacters(in: .whitespacesAndNewlines) }
    }

    /// Squash-merges a GitHub stack on GitHub: all of it, or up to and including `pull`.
    func mergeStack(_ stack: PullRequestStack, upTo pull: PullRequest?) async {
        guard let number = stack.number, let tools = workspaceTools.stacks, let clone = clone(of: stack.pullRequest.remote) else { return }
        await run(stack.id, "Merging…", clone.workspace) {
            await tools.merge(stack: number, upTo: pull?.number, in: clone.repo.url)
        }
    }

    /// Pushes a stack's branches and opens PRs for the ones without, from a checkout on one of them.
    func submitStack(_ stack: LocalStack, id: String, in repo: WorkspaceRepo, of workspace: WorkspaceModel) async {
        guard let tools = workspaceTools.stacks,
              let checkout = workspace.checkout(of: repo, onAnyOf: stack.layers.map(\.branch)) else { return }
        await run(id, "Submitting…", workspace) {
            await tools.submit(in: checkout)
        }
    }

    /// Makes `branches` (bottom first) a stack onto `base` in `checkout`, which ends up on the
    /// top branch, and submits it when asked. Nil when it worked, else what went wrong.
    func createStack(branches: [String], base: String?, in checkout: URL, of workspace: WorkspaceModel, submit: Bool) async -> String? {
        guard let tools = workspaceTools.stacks else { return "gh isn't available." }
        if let error = await tools.create(branches: branches, base: base, in: checkout) { return error }
        if submit, let error = await tools.submit(in: checkout) {
            await workspace.refreshStatus(tools: workspaceTools, minInterval: 0)
            return "The stack was made, but submitting it failed: \(error)"
        }
        await workspace.refreshStatus(tools: workspaceTools, minInterval: 0)
        await workspace.refreshPulls(tools: workspaceTools, maxAge: 0)
        return nil
    }

    /// A new branch on top of a stack, in the checkout that's on its top branch.
    func addBranch(_ branch: String, onTopOf stackID: String, in checkout: URL, of workspace: WorkspaceModel) async {
        guard let tools = workspaceTools.stacks else { return }
        await run(stackID, "Adding \(branch)…", workspace) {
            await tools.add(branch: branch, in: checkout)
        }
    }

    /// Rebases the stack's branches on what's merged below, pushes them and updates the PRs.
    func syncStack(_ stackID: String, in checkout: URL, of workspace: WorkspaceModel) async {
        guard let tools = workspaceTools.stacks else { return }
        await run(stackID, "Syncing…", workspace) {
            await tools.sync(in: checkout)
        }
    }

    /// Asks Claude to turn the thread's work into a stack of small PRs.
    func splitIntoStack(_ thread: ThreadModel) {
        thread.send("""
        Split the work in this thread into a GitHub stack of small pull requests with `gh stack`. Group the \
        changes into layers that each make sense on their own and build on the one below, bottom first, one \
        branch per layer with its changes committed there (`gh stack init`, then `gh stack add` for each layer \
        above). Then run `gh stack submit --auto --open` and tell me the stack number and its PRs. Don't merge anything.
        """)
    }

    /// Runs a stack command with its button showing what it's doing, then looks again at
    /// the PRs, now and a little later (GitHub merges and retargets in the background).
    private func run(_ id: String, _ activity: String, _ workspace: WorkspaceModel, _ command: () async -> String?) async {
        guard stackActivity[id] == nil else { return }
        stackActivity[id] = activity
        stackErrors[id] = nil
        let error = await command()
        stackActivity[id] = nil
        if let error { stackErrors[id] = error }
        let tools = workspaceTools
        await workspace.refreshStatus(tools: tools, minInterval: 0)
        for delay in [0.0, 4.0, 15.0] {
            try? await Task.sleep(for: .seconds(delay))
            await workspace.refreshPulls(tools: tools, maxAge: 0)
            if let thread = selectedThread { await syncPullRequests(for: thread, force: true) }
        }
    }
}
