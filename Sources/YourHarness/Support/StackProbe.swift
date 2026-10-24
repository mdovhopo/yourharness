import AppKit
import HarnessCore

/// Development: `YH_STACK_PROBE=<branch prefix>` runs GitHub stacks end to end at launch,
/// through the same calls the buttons make, in the first GitHub repo of the project whose
/// overview is open: it makes branches `<prefix>-1…3` (already there, one commit each) a
/// stack and submits it, adds `<prefix>-4` on top and submits that, merges up to layer 2,
/// checks a thread's PRs (and a dead link) update, syncs, merges the rest, checks an error
/// reads well, and with `YH_STACK_PROBE_SPLIT=1` has Claude split a thread's work into a
/// stack. It prints each step's PASS or FAIL to stderr, and with
/// `YH_STACK_PROBE_SHOTS=<folder>` draws the window after each one. It merges for real:
/// only for a sandbox repo. Off unless set.
@MainActor
enum StackProbe {
    static func runIfRequested(app: AppModel) {
        let environment = ProcessInfo.processInfo.environment
        guard let prefix = environment["YH_STACK_PROBE"], !prefix.isEmpty else { return }
        Task { await Run(app: app, prefix: prefix, shots: environment["YH_STACK_PROBE_SHOTS"],
                         split: environment["YH_STACK_PROBE_SPLIT"] == "1").go() }
    }

    private struct Failure: Error { let reason: String }

    @MainActor private final class Run {
        let app: AppModel
        let prefix: String
        let shots: String?
        let split: Bool
        var step = 0
        var failures = 0
        var workspace: WorkspaceModel!
        var repo: WorkspaceRepo!
        var remote: GitRemote!
        /// Every PR of the stack once it has four: the repos view lists only open ones.
        var layers: [PullRequest] = []

        init(app: AppModel, prefix: String, shots: String?, split: Bool) {
            self.app = app
            self.prefix = prefix
            self.shots = shots
            self.split = split
        }

        func say(_ line: String) {
            FileHandle.standardError.write(Data("STACK PROBE \(line)\n".utf8))
        }

        func go() async {
            try? await Task.sleep(for: .seconds(4))
            await check("find the repo") {
                guard let project = app.projects.first(where: { $0.id == app.overviewProjectID }) ?? app.projects.last,
                      let workspace = app.workspace(for: project) else { throw Failure(reason: "no project overview open") }
                try await wait("the repo and its remote", timeout: 30) {
                    await workspace.refresh(tools: self.app.workspaceTools)
                    return workspace.clonedRepos.contains { workspace.remotes[$0.id]?.forge == .github }
                }
                self.workspace = workspace
                self.repo = workspace.clonedRepos.first { workspace.remotes[$0.id]?.forge == .github }
                self.remote = workspace.remotes[repo.id]
                return "\(remote.path)"
            }
            guard repo != nil else { return finish() }

            await check("gh stack is installed") {
                await app.checkStacksInstalled()
                guard app.stacksInstalled == true else { throw Failure(reason: "it isn't") }
                return "yes"
            }

            let branches = (1...3).map { "\(prefix)-\($0)" }
            await check("create a stack from three branches and submit it") {
                if let error = await app.createStack(branches: branches, base: nil, in: repo.url, of: workspace, submit: true) {
                    throw Failure(reason: error)
                }
                let stack = try await stackOnGitHub(heads: branches, open: 3)
                return "stack #\(stack.number ?? 0): " + describe(stack)
            }

            await check("add a branch on top, commit to it, submit") {
                let stack = try await stackOnGitHub(heads: branches, open: 3)
                let top = "\(prefix)-4"
                guard let checkout = workspace.checkout(of: repo, onAnyOf: [branches[2]]) else { throw Failure(reason: "no checkout on \(branches[2])") }
                await app.addBranch(top, onTopOf: stack.id, in: checkout, of: workspace)
                if let error = app.stackErrors[stack.id] { throw Failure(reason: error) }
                try await commit(file: "\(top).txt", message: "Layer 4", in: checkout)
                await workspace.refreshStatus(tools: app.workspaceTools, minInterval: 0)
                guard let local = workspace.localStack(number: stack.number!, remote: remote) else { throw Failure(reason: "the local stack is gone") }
                await app.submitStack(local, id: stack.id, in: repo, of: workspace)
                if let error = app.stackErrors[stack.id] { throw Failure(reason: error) }
                let grown = try await stackOnGitHub(heads: branches + [top], open: 4)
                layers = grown.all
                return describe(grown)
            }

            await check("merge up to layer 2") {
                let stack = try await stackOnGitHub(heads: branches, open: 4)
                let second = stack.all.first { $0.stack?.position == 2 }!
                await app.mergeStack(stack, upTo: second)
                if let error = app.stackErrors[stack.id] { throw Failure(reason: error) }
                let states = try await wait("layers 1 and 2 merged, 3 onto main", timeout: 90) { () -> [PullRequest]? in
                    let pulls = await self.fetch(stack.all)
                    let merged = pulls.filter { $0.state == .merged }.compactMap(\.stack?.position).sorted()
                    let third = pulls.first { $0.stack?.position == 3 }
                    return merged == [1, 2] && third?.baseBranch == stack.pullRequest.baseBranch ? pulls : nil
                }
                return states.sorted { $0.stack!.position < $1.stack!.position }.map { "#\($0.number) \($0.state.rawValue) → \($0.baseBranch)" }.joined(separator: ", ")
            }

            await check("a thread's PRs update, a dead link among them") {
                let project = app.projects.first { $0.id == workspace.projectID }!
                let thread = app.newThread(in: project)
                let links = layers.map(\.url.absoluteString) + ["https://github.com/\(remote.path)/pull/99999"]
                thread.addWorkedPullRequestLinks(links)
                await app.syncPullRequests(for: thread, force: true)
                let shown = app.pullRequests(for: thread)
                let merged = shown.filter { $0.pull?.state == .merged }.count, open = shown.filter { $0.pull?.state == .open }.count
                guard merged == 2, open == 2 else { throw Failure(reason: "\(merged) merged, \(open) open of \(shown.count)") }
                app.showOverview(of: project)
                return "2 merged, 2 open, dead link left unknown"
            }

            await check("sync the stack") {
                let stack = try await stackOnGitHub(heads: branches + ["\(prefix)-4"], open: 2)
                guard let checkout = workspace.checkout(of: repo, onAnyOf: stack.all.map(\.headBranch)) else { throw Failure(reason: "no checkout on the stack") }
                await app.syncStack(stack.id, in: checkout, of: workspace)
                if let error = app.stackErrors[stack.id] { throw Failure(reason: error) }
                return "synced"
            }

            await check("merge the rest of the stack") {
                let stack = try await stackOnGitHub(heads: branches + ["\(prefix)-4"], open: 2)
                await app.mergeStack(stack, upTo: nil)
                if let error = app.stackErrors[stack.id] { throw Failure(reason: error) }
                let pulls = try await wait("every layer merged", timeout: 90) { () -> [PullRequest]? in
                    let pulls = await self.fetch(self.layers)
                    return pulls.count == 4 && pulls.allSatisfy { $0.state == .merged } ? pulls : nil
                }
                return pulls.map { "#\($0.number)" }.joined(separator: ", ") + " merged"
            }

            await check("an error reads well") {
                guard let tools = app.workspaceTools.stacks else { throw Failure(reason: "no gh") }
                let error = await tools.merge(stack: 99_999, upTo: nil, in: repo.url) ?? "(no error)"
                guard !error.isEmpty, !error.contains("exit"), error != "(no error)" else { throw Failure(reason: error) }
                return error
            }

            if split { await splitByClaude() }
            finish()
        }

        /// A thread makes a change on a new branch, then Split into a Stack asks Claude to stack it.
        func splitByClaude() async {
            await check("Claude splits a thread's work into a stack") {
                let project = app.projects.first { $0.id == workspace.projectID }!
                let before = Set(workspace.pullRequests.compactMap(\.stack?.number))
                let thread = app.newThread(in: project)
                thread.send("""
                In this repo, from an up-to-date main, make a branch \(prefix)-work. On it add three small text files, \
                notes/one.txt, notes/two.txt and notes/three.txt, each with one line, and commit them together in one \
                commit. Don't push.
                """)
                try await wait("the first turn", timeout: 300) { thread.phase == .idle && !thread.entries.isEmpty ? true : nil }
                app.splitIntoStack(thread)
                try await wait("Claude to start", timeout: 30) { thread.phase == .running ? true : nil }
                try await wait("the split turn", timeout: 600) { thread.phase == .idle ? true : nil }
                let stack = try await wait("a new stack on GitHub", timeout: 120) { () -> PullRequestStack? in
                    await self.workspace.refreshPulls(tools: self.app.workspaceTools, maxAge: 0)
                    return PullRequestStack.build(self.workspace.pullRequests).first { $0.number.map { !before.contains($0) } ?? false }
                }
                return "stack #\(stack.number ?? 0): " + describe(stack)
            }
        }

        func finish() {
            say(failures == 0 ? "done: all \(step) steps passed" : "done: \(failures) of \(step) steps failed")
        }

        func check(_ name: String, _ body: () async throws -> String) async {
            step += 1
            do {
                let detail = try await body()
                say("\(step) PASS \(name): \(detail)")
            } catch let failure as Failure {
                failures += 1
                say("\(step) FAIL \(name): \(failure.reason)")
            } catch {
                failures += 1
                say("\(step) FAIL \(name): \(error)")
            }
            try? await Task.sleep(for: .seconds(2))
            if let shots { WindowSnapshot.write(to: "\(shots)/\(String(format: "%02d", step)).png") }
        }

        /// The open stack on GitHub with these head branches, once the repos view lists `open` of its layers.
        func stackOnGitHub(heads: [String], open: Int) async throws -> PullRequestStack {
            try await wait("\(open) open layers of the stack on \(heads.first ?? "")", timeout: 90) { () -> PullRequestStack? in
                await self.workspace.refreshPulls(tools: self.app.workspaceTools, maxAge: 0)
                let stack = PullRequestStack.build(self.workspace.pullRequests).first { stack in
                    stack.number != nil && stack.all.contains { heads.contains($0.headBranch) }
                }
                return stack.flatMap { $0.all.count == open ? $0 : nil }
            }
        }

        /// PRs as GitHub has them now, merged or not.
        func fetch(_ pulls: [PullRequest]) async -> [PullRequest] {
            guard let github = app.workspaceTools.github else { return [] }
            return await github.fetch(links: pulls.map { PullRequestLink(remote: $0.remote, number: $0.number, url: $0.url) })
        }

        func describe(_ stack: PullRequestStack) -> String {
            stack.all.map { "\($0.stack?.position ?? 0):#\($0.number) \($0.headBranch)→\($0.baseBranch)" }.joined(separator: ", ")
        }

        func commit(file: String, message: String, in checkout: URL) async throws {
            try "\(message)\n".write(to: checkout.appending(path: file), atomically: true, encoding: .utf8)
            guard let git = app.workspaceTools.git else { throw Failure(reason: "no git") }
            for arguments in [["add", file], ["commit", "-m", message]] {
                let output = await CommandRunner.run(git.executable, arguments: ["-C", checkout.path] + arguments, environment: git.environment)
                guard output.succeeded else { throw Failure(reason: "git \(arguments[0]): \(output.stderr)") }
            }
        }

        /// Polls `condition` every few seconds until it gives a value.
        func wait<T>(_ what: String, timeout: TimeInterval, _ condition: () async -> T?) async throws -> T {
            let deadline = Date.now.addingTimeInterval(timeout)
            while Date.now < deadline {
                if let value = await condition() { return value }
                try? await Task.sleep(for: .seconds(3))
            }
            throw Failure(reason: "timed out waiting for \(what)")
        }
    }
}
