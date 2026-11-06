import Foundation
import Testing
@testable import HarnessCore

struct PullRequestActivityTests {
    private func bash(_ command: String, result: String = "") -> PullRequestActivity? {
        let input = String(data: try! JSONSerialization.data(withJSONObject: ["command": command]), encoding: .utf8)!
        return PullRequestActivity.of(toolName: "Bash", inputJSON: input, result: result)
    }

    @Test func writingCommandsChangePullRequests() throws {
        let created = try #require(bash(#"gh pr create --title "Add login" --body "x" --base main"#,
                                        result: "https://github.com/acme/api/pull/57\n"))
        #expect(created.targets.contains(.currentBranch))
        #expect(created.targets.contains { if case .link(let link) = $0 { link.number == 57 } else { false } })
        #expect(bash("gh pr merge 57 --squash -R acme/api")?.targets == [.number(57, repo: "acme/api")])
        #expect(bash("cd repos/api && gh pr merge 58 --auto")?.targets == [.number(58, repo: nil)])
        #expect(bash("gh pr comment https://github.com/acme/web/pull/31 -b 'lgtm'")?.targets.count == 1)
        #expect(bash("glab mr merge !12 --repo acme/billing")?.targets == [.number(12, repo: "acme/billing")])
        #expect(bash("gh api repos/acme/api/pulls/60/merge -X PUT")?.targets == [.number(60, repo: "acme/api")])
        #expect(bash("gh api repos/acme/api/pulls/60/comments -f body=hi")?.targets == [.number(60, repo: "acme/api")])
    }

    /// `gh stack submit` prints every PR it opened or updated (from a real run, renamed).
    @Test func stackCommandsChangeTheirPullRequests() throws {
        let submitted = try #require(bash("gh stack submit --auto --open", result: """
        Pushing to origin...
        ✓ Created PR #1 (https://github.com/acme/api/pull/1) for stack-a
        ✓ Created PR #2 (https://github.com/acme/api/pull/2) for stack-b
        ✓ Stack created on GitHub with 2 PRs (stack #4)
        """))
        #expect(submitted.targets.count == 2)
        #expect(!submitted.targets.contains(.currentBranch))
        #expect(bash("gh stack merge 4 --squash --yes")?.targets == [.currentBranch])
        #expect(bash("gh stack view --json") == nil)
        #expect(bash("gh stack add layer-3") == nil)
    }

    @Test func lookingIsNotChanging() {
        #expect(bash("gh pr view 57 --json state") == nil)
        #expect(bash("gh pr list --author @me", result: "https://github.com/acme/api/pull/57") == nil)
        #expect(bash("gh api repos/acme/api/pulls/60") == nil)
        #expect(bash("git log --oneline") == nil)
        // Quoted, it's text, not a command.
        #expect(bash("echo 'gh pr merge 57'") == nil)
        #expect(PullRequestActivity.of(toolName: "WebFetch", inputJSON: #"{"url":"https://github.com/acme/api/pull/57"}"#, result: "") == nil)
    }

    @Test func forgeToolsThatChangeAPullRequest() throws {
        let merged = try #require(PullRequestActivity.of(toolName: "mcp__github__merge_pull_request",
                                                         inputJSON: #"{"owner":"acme","repo":"api","pullNumber":57}"#, result: "Merged"))
        #expect(merged.targets == [.number(57, repo: "acme/api")])
        #expect(PullRequestActivity.of(toolName: "mcp__github__get_pull_request", inputJSON: #"{"owner":"acme","repo":"api","pullNumber":57}"#, result: "") == nil)
        #expect(PullRequestActivity.of(toolName: "mcp__github__list_pull_requests", inputJSON: "{}", result: "") == nil)
    }

    @Test func writtenFoldersAreEditsAndCommits() {
        let cwd = "/tmp/project"
        let edited = ThreadFootprint.writtenFolders(toolName: "Edit", inputJSON: #"{"file_path":"/tmp/project/src/a.swift"}"#, workingDirectory: cwd)
        #expect(edited.count == 1 && edited[0].hasSuffix("/project/src"))
        #expect(ThreadFootprint.writtenFolders(toolName: "Read", inputJSON: #"{"file_path":"/tmp/project/src/a.swift"}"#, workingDirectory: cwd).isEmpty)
        #expect(ThreadFootprint.writtenFolders(toolName: "Bash", inputJSON: #"{"command":"git status"}"#, workingDirectory: cwd).isEmpty)
        #expect(!ThreadFootprint.writtenFolders(toolName: "Bash", inputJSON: #"{"command":"git -C /tmp commit -m x"}"#, workingDirectory: cwd).isEmpty)
        #expect(ThreadFootprint.writesGit("git add . && git commit -m 'x' && git push"))
        #expect(!ThreadFootprint.writesGit("git log && git diff"))
    }
}
