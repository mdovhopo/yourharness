import Foundation
import Testing
@testable import HarnessCore

struct GitHubStacksTests {
    private static let remote = GitRemote(host: "github.com", path: "acme/api", forge: .github)

    private static func pull(_ number: Int, head: String, base: String, stack: (number: Int, size: Int, position: Int)? = nil,
                             state: PullRequest.State = .open) -> PullRequest {
        var pull = PullRequest(remote: remote, number: number, title: head, url: URL(string: "https://github.com/acme/api/pull/\(number)")!,
                               author: nil, isDraft: false, headBranch: head, baseBranch: base, updatedAt: nil,
                               review: .none, checks: .none, isMine: true, reviewRequestedFromMe: false, state: state)
        pull.stack = stack.map { PullRequest.StackPlace(number: $0.number, size: $0.size, position: $0.position) }
        return pull
    }

    /// As GitHub answers for a PR in a stack (fields as the API names them).
    @Test func readsAPullRequestsPlaceInItsStack() {
        let node: [String: Any] = [
            "number": 2, "title": "Layer B", "url": "https://github.com/acme/api/pull/2", "isDraft": false,
            "headRefName": "stack-b", "baseRefName": "stack-a", "mergeStateStatus": "CLEAN",
            "stack": ["number": 4, "size": 3], "stackEntry": ["position": 2],
        ]
        let pull = GitHubClient.pullRequest(from: node, remote: Self.remote, login: "me")
        #expect(pull?.stack == PullRequest.StackPlace(number: 4, size: 3, position: 2))
        #expect(pull?.mergeStatus == "CLEAN")
        let alone = GitHubClient.pullRequest(from: node.merging(["stack": NSNull(), "stackEntry": NSNull()]) { $1 }, remote: Self.remote, login: "me")
        #expect(alone?.stack == nil)
    }

    @Test func asksForStacksOnlyOnGitHubCom() {
        #expect(GitHubClient.query(for: [Self.remote]).contains("stackEntry { position }"))
        let enterprise = GitRemote(host: "github.acme.com", path: "acme/api", forge: .github)
        #expect(!GitHubClient.query(for: [enterprise]).contains("stackEntry"))
    }

    /// GitHub's order wins over branches: after the bottom merged, the next one targets
    /// main, and it's still the same stack, merged layer included.
    @Test func groupsGitHubStacksByTheirOrder() {
        let pulls = [
            Self.pull(3, head: "stack-c", base: "stack-b", stack: (4, 3, 3)),
            Self.pull(9, head: "solo", base: "main"),
            Self.pull(2, head: "stack-b", base: "main", stack: (4, 3, 2)),
            Self.pull(1, head: "stack-a", base: "main", stack: (4, 3, 1), state: .merged),
        ]
        let stacks = PullRequestStack.build(pulls)
        #expect(stacks.count == 2)
        #expect(stacks[0].number == 4)
        #expect(stacks[0].all.map(\.number) == [1, 2, 3])
        #expect(stacks[1].all.map(\.number) == [9])
        #expect(stacks[1].number == nil)
    }

    /// `.git/gh-stack` before and after `gh stack submit` (from a real run, renamed).
    @Test func readsLocalStacks() {
        let json = #"""
        {"schemaVersion":1,"repository":"github.com:acme/api","stacks":[
          {"trunk":{"branch":"main","head":"c843b36"},"branches":[{"branch":"stack-a","base":"c843b36"},{"branch":"stack-b","base":"cf27059"}]},
          {"id":"2567809","number":4,"trunk":{"branch":"main","head":"c843b36"},"branches":[
            {"branch":"feat-a","head":"cf27059","base":"c843b36","pullRequest":{"number":1,"id":"PR_1","url":"https://github.com/acme/api/pull/1"}},
            {"branch":"feat-b","head":"da471a2","base":"cf27059"}],"futureField":true}]}
        """#
        let stacks = LocalStack.parse(Data(json.utf8))
        #expect(stacks.count == 2)
        #expect(stacks[0].number == nil && stacks[0].layers.map(\.branch) == ["stack-a", "stack-b"])
        #expect(stacks[1].number == 4 && stacks[1].trunk == "main")
        #expect(stacks[1].layers.map(\.pullRequestNumber) == [1, nil])
        #expect(LocalStack.parse(Data("not json".utf8)).isEmpty)
    }

    @Test func findsAWorktreesSharedGitFolder() throws {
        let root = FileManager.default.temporaryDirectory.appending(path: "gh-stack-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        let main = root.appending(path: "repo"), linked = root.appending(path: "linked")
        let own = main.appending(path: ".git/worktrees/linked")
        try FileManager.default.createDirectory(at: own, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: linked, withIntermediateDirectories: true)
        try "gitdir: \(own.path)\n".write(to: linked.appending(path: ".git"), atomically: true, encoding: .utf8)
        try "../..\n".write(to: own.appending(path: "commondir"), atomically: true, encoding: .utf8)
        #expect(LocalStack.gitFolder(of: main)?.standardizedFileURL.path == main.appending(path: ".git").path)
        #expect(LocalStack.gitFolder(of: linked)?.standardizedFileURL.path == main.appending(path: ".git").path)
    }

    @Test func saysWhatAFailedCommandMeans() {
        func failure(_ status: Int32, _ stderr: String = "") -> String {
            GitHubStacks.describe(CommandOutput(status: status, stdout: "", stderr: stderr, timedOut: false, launchFailed: false))
        }
        #expect(failure(9).contains("aren't turned on"))
        #expect(failure(3).contains("conflicts"))
        #expect(failure(1, "gh stack is available as an official extension.\nTo install it, run:").contains("isn't installed"))
        #expect(failure(1, "something\nX went wrong") == "X went wrong")
    }
}
