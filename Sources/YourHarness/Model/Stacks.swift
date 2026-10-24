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
