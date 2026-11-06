import Foundation
import Testing
@testable import HarnessCore

/// Uses the real `git` and `gh`. Run with `YH_INTEGRATION=1 swift test --filter WorkspaceIntegration`.
/// Set `YH_SCAN_PATH` to also time a scan of a workspace on your disk.
@Suite(.enabled(if: ProcessInfo.processInfo.environment["YH_INTEGRATION"] == "1"))
struct WorkspaceIntegrationTests {
    private var environment: [String: String] { ProcessInfo.processInfo.environment }

    @Test func scansAndReadsThisRepo() async throws {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let layout = WorkspaceScanner.scan(root)
        // A shell once a preset has cloned a sandbox repo into test-harness-root (gitignored).
        #expect(layout.kind == .repo || layout.kind == .shell)

        let git = Git(executable: URL(fileURLWithPath: "/usr/bin/git"), environment: environment)
        let status = try #require(await git.status(of: root))
        #expect(status.headOID != nil)
        let origin = try #require(await git.originURL(of: root))
        #expect(GitRemote(url: origin)?.forge == .github)
    }

    @Test func fetchesPullRequestsThroughGh() async throws {
        let gh = try #require(ShellEnvironment.which("gh", in: environment))
        let remotes = [
            GitRemote(host: "github.com", path: "cli/cli", forge: .github),
            GitRemote(host: "github.com", path: "mdovhopo/does-not-exist-yh", forge: .github),
        ]
        let started = ContinuousClock.now
        let snapshot = await GitHubClient(executable: gh, environment: environment).fetch(remotes)
        print("gh fetch: \(snapshot.pullRequests.count) PRs in \(ContinuousClock.now - started), error: \(snapshot.error ?? "none")")
        #expect(snapshot.error == nil)
        #expect(snapshot.accounts["github.com"] != nil)
        #expect(!snapshot.pullRequests.isEmpty)
        #expect(snapshot.unreachable == [remotes[1]])
    }

    @Test func timesAScan() async throws {
        guard let path = environment["YH_SCAN_PATH"] else { return }
        let started = ContinuousClock.now
        let layout = WorkspaceScanner.scan(URL(fileURLWithPath: path))
        let scanned = ContinuousClock.now - started

        let git = Git(executable: URL(fileURLWithPath: "/usr/bin/git"), environment: environment)
        let statusStart = ContinuousClock.now
        let statuses = await withTaskGroup(of: RepoStatus?.self) { group in
            var results: [RepoStatus?] = []
            var pending = layout.clonedRepos.makeIterator()
            for _ in 0..<8 { if let repo = pending.next() { group.addTask { await git.status(of: repo.url) } } }
            while let result = await group.next() {
                results.append(result)
                if let repo = pending.next() { group.addTask { await git.status(of: repo.url) } }
            }
            return results
        }
        print("scan: \(layout.kind), \(layout.clonedRepos.count) cloned, \(layout.repos.count - layout.clonedRepos.count) known, in \(scanned); status x\(statuses.count) in \(ContinuousClock.now - statusStart), dirty: \(statuses.compactMap { $0 }.filter(\.isDirty).count)")
    }
}
