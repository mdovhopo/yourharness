import Foundation
import Testing
@testable import HarnessCore

struct GitRemoteTests {
    @Test(arguments: [
        "git@github.com:acme/api.git",
        "https://github.com/acme/api.git",
        "https://ghp_secret@github.com/acme/api",
        "ssh://git@github.com:22/acme/api.git",
    ])
    func parsesGitHubForms(_ url: String) throws {
        let remote = try #require(GitRemote(url: url))
        #expect(remote.host == "github.com")
        #expect(remote.path == "acme/api")
        #expect(remote.forge == .github)
        #expect(remote.owner == "acme")
        #expect(remote.name == "api")
    }

    @Test func dropsCredentials() throws {
        let remote = try #require(GitRemote(url: "https://user:token123@gitlab.example.com/group/sub/repo.git"))
        #expect(remote.forge == .gitlab)
        #expect(remote.path == "group/sub/repo")
        #expect(remote.owner == "group/sub")
        #expect(!"\(remote)".contains("token123"))
    }

    @Test func rejectsLocalPaths() {
        #expect(GitRemote(url: "/Users/me/repos/api") == nil)
        #expect(GitRemote(url: "../api") == nil)
    }
}

struct RepoStatusTests {
    @Test func parsesBranchAndCounts() {
        let output = """
        # branch.oid 1234abcd
        # branch.head feature/x
        # branch.upstream origin/feature/x
        # branch.ab +2 -1
        # stash 3
        1 M. N... 100644 100644 100644 aaa bbb Sources/A.swift
        1 .M N... 100644 100644 100644 aaa bbb Sources/B.swift
        1 MM N... 100644 100644 100644 aaa bbb Sources/C.swift
        2 R. N... 100644 100644 100644 aaa bbb R100 New.swift\tOld.swift
        u UU N... 100644 100644 100644 100644 aaa bbb ccc Conflict.swift
        ? notes.txt
        ? scratch/
        """
        let status = RepoStatus.parse(porcelainV2: output)
        #expect(status.branch == "feature/x")
        #expect(status.upstream == "origin/feature/x")
        #expect(status.ahead == 2)
        #expect(status.behind == 1)
        #expect(status.staged == 3)
        #expect(status.modified == 2)
        #expect(status.conflicted == 1)
        #expect(status.untracked == 2)
        #expect(status.stashes == 3)
        #expect(status.isDirty)
    }

    @Test func detachedAndClean() {
        let status = RepoStatus.parse(porcelainV2: "# branch.oid abc\n# branch.head (detached)\n")
        #expect(status.branch == nil)
        #expect(status.headOID == "abc")
        #expect(status.upstream == nil)
        #expect(!status.isDirty)
    }

    @Test func readsARealRepo() async throws {
        let git = URL(fileURLWithPath: "/usr/bin/git")
        let directory = try temporaryDirectory()
        _ = await CommandRunner.run(git, arguments: ["-C", directory.path, "init", "-q", "-b", "main"])
        try "hi".write(to: directory.appending(path: "a.txt"), atomically: true, encoding: .utf8)

        let status = try #require(await Git(executable: git, environment: ProcessInfo.processInfo.environment).status(of: directory))
        #expect(status.branch == "main")
        #expect(status.untracked == 1)
    }
}

struct WorkspaceScannerTests {
    @Test func plainRepoFolderAndShell() throws {
        let root = try temporaryDirectory()
        #expect(WorkspaceScanner.scan(root).kind == .plain)

        try makeRepo(root.appending(path: "repos/api"))
        try makeRepo(root.appending(path: "group/billing/web"))
        try makeRepo(root.appending(path: "node_modules/pkg"))   // skipped
        try makeRepo(root.appending(path: "repos/api/vendor/x")) // inside a repo: not descended
        let folder = WorkspaceScanner.scan(root)
        #expect(folder.kind == .folder)
        #expect(folder.repos.map(\.relativePath) == ["group/billing/web", "repos/api"])

        try makeRepo(root)
        let shell = WorkspaceScanner.scan(root)
        #expect(shell.kind == .shell)
        #expect(shell.repos.first?.isRoot == true)
        #expect(shell.repos.count == 3)
    }

    @Test func singleRepo() throws {
        let root = try temporaryDirectory()
        try makeRepo(root)
        let layout = WorkspaceScanner.scan(root)
        #expect(layout.kind == .repo)
        #expect(layout.repos.count == 1)
        #expect(!layout.isWorkspace)
    }

    @Test func composeContextsBecomeUnclonedRepos() throws {
        let root = try temporaryDirectory()
        try makeRepo(root)
        try makeRepo(root.appending(path: "services/api"))
        try FileManager.default.createDirectory(at: root.appending(path: "services/web"), withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: root.appending(path: "storage/mysql"), withIntermediateDirectories: true)
        try "Dockerfile".write(to: root.appending(path: "storage/mysql/Dockerfile"), atomically: true, encoding: .utf8)
        try """
        services:
          api:
            build:
              context: ./services/api/docker   # inside a cloned repo
          web:
            build:
              context: ./services/web          # empty placeholder
              target: develop
          worker:
            build: services/worker             # missing entirely
          mysql:
            build: ./storage/mysql             # shell-owned folder
          remote:
            build: https://github.com/acme/thing.git
          templated:
            build: ${CONTEXT}
        """.write(to: root.appending(path: "docker-compose.yml"), atomically: true, encoding: .utf8)

        let layout = WorkspaceScanner.scan(root)
        #expect(layout.kind == .shell)
        let uncloned = layout.repos.filter { !$0.isCloned }.map(\.relativePath)
        #expect(uncloned == ["services/web", "services/worker"])
    }

    @Test func normalizesPaths() {
        #expect(WorkspaceScanner.normalize("./services/a/") == "services/a")
        #expect(WorkspaceScanner.normalize("../outside") == nil)
        #expect(WorkspaceScanner.normalize(".") == nil)
        #expect(WorkspaceScanner.normalize("/abs") == nil)
    }
}

struct ForgeTests {
    private func fixture(_ name: String) throws -> String {
        let url = try #require(Bundle.module.url(forResource: name, withExtension: "json", subdirectory: "Fixtures"))
        return try String(contentsOf: url, encoding: .utf8)
    }

    private let api = GitRemote(host: "github.com", path: "acme/api", forge: .github)
    private let gone = GitRemote(host: "github.com", path: "acme/gone", forge: .github)

    @Test func parsesGitHubBatchWithMissingRepo() throws {
        let page = try #require(GitHubClient.parse(try fixture("github-pulls"), batch: [api, gone]))
        #expect(page.login == "me")
        #expect(page.unreachable == [gone])
        #expect(page.pullRequests.map(\.number) == [12, 13, 9])

        let first = page.pullRequests[0]
        #expect(first.isMine)
        #expect(first.review == .approved)
        #expect(first.checks == .passing)
        #expect(first.updatedAt != nil)

        #expect(page.pullRequests[1].isDraft)
        #expect(page.pullRequests[1].checks == .failing)

        let theirs = page.pullRequests[2]
        #expect(!theirs.isMine)
        #expect(theirs.reviewRequestedFromMe)
        #expect(theirs.checks == .none)
    }

    @Test func queryEscapesNames() {
        let query = GitHubClient.query(for: [GitRemote(host: "github.com", path: "acme/we\"ird", forge: .github)])
        #expect(query.contains(#"name: "we\"ird""#))
        #expect(query.contains("viewer { login }"))
    }

    @Test func parsesGitLabMergeRequests() throws {
        let hooks = GitRemote(host: "gitlab.com", path: "acme/platform/hooks", forge: .gitlab)
        let other = GitRemote(host: "gitlab.com", path: "acme/other", forge: .gitlab)
        let page = try #require(GitLabClient.parse(try fixture("gitlab-merge-requests"), batch: [hooks, other]))
        #expect(page.login == "me")
        #expect(page.unreachable == [other])
        let mr = try #require(page.pullRequests.first)
        #expect(mr.number == 41)
        #expect(mr.checks == .pending)
        #expect(mr.review == .required)
        #expect(mr.reviewRequestedFromMe)
    }

    @Test func garbageIsNotAPage() {
        #expect(GitHubClient.parse("gh: not logged in", batch: [api]) == nil)
    }

    @Test func buildsStacksFromBaseBranches() throws {
        let page = try #require(GitHubClient.parse(try fixture("github-pulls"), batch: [api, gone]))
        let stacks = PullRequestStack.build(page.pullRequests)
        #expect(stacks.count == 2)
        let stack = try #require(stacks.first { $0.pullRequest.number == 12 })
        #expect(stack.isStacked)
        #expect(stack.flattened.map(\.pullRequest.number) == [12, 13])
        #expect(stack.flattened.map(\.depth) == [0, 1])
    }

    @Test func sameBranchNamesInDifferentReposDontStack() {
        func pr(_ repo: String, _ number: Int, head: String, base: String) -> PullRequest {
            PullRequest(
                remote: GitRemote(host: "github.com", path: "acme/\(repo)", forge: .github), number: number, title: "",
                url: URL(string: "https://github.com/acme/\(repo)/pull/\(number)")!, author: nil, isDraft: false,
                headBranch: head, baseBranch: base, updatedAt: nil, review: .none, checks: .none,
                isMine: false, reviewRequestedFromMe: false
            )
        }
        let stacks = PullRequestStack.build([
            pr("api", 1, head: "feature", base: "main"),
            pr("web", 2, head: "feature-ui", base: "feature"),
            pr("web", 3, head: "a", base: "b"),
            pr("web", 4, head: "b", base: "a"), // cycle: still shown
        ])
        #expect(stacks.flatMap(\.all).count == 4)
        #expect(stacks.first { $0.pullRequest.number == 1 }?.isStacked == false)
    }
}

private func temporaryDirectory() throws -> URL {
    let url = FileManager.default.temporaryDirectory.appending(path: "yh-tests-\(UUID().uuidString)", directoryHint: .isDirectory)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url.resolvingSymlinksInPath()
}

private func makeRepo(_ url: URL) throws {
    try FileManager.default.createDirectory(at: url.appending(path: ".git"), withIntermediateDirectories: true)
}
