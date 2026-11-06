import Foundation
import Testing
@testable import HarnessCore

struct WorktreeTests {
    @Test func parsesWorktreeList() {
        let output = """
        worktree /code/api
        HEAD 1111111111111111111111111111111111111111
        branch refs/heads/main

        worktree /code/api-login
        HEAD 2222222222222222222222222222222222222222
        branch refs/heads/feature/login
        locked moving disks

        worktree /code/api-spike
        HEAD 3333333333333333333333333333333333333333
        detached

        worktree /code/api-gone
        HEAD 4444444444444444444444444444444444444444
        branch refs/heads/old
        prunable gitdir file points to non-existent location

        """
        let worktrees = Worktree.parse(porcelain: output)
        #expect(worktrees.map(\.path) == ["/code/api", "/code/api-login", "/code/api-spike", "/code/api-gone"])
        #expect(worktrees[0].isMain && worktrees[0].branch == "main")
        #expect(!worktrees[1].isMain && worktrees[1].branch == "feature/login")
        #expect(worktrees[1].isLocked && worktrees[1].lockReason == "moving disks")
        #expect(worktrees[2].isDetached && worktrees[2].headOID?.hasPrefix("3333") == true)
        #expect(worktrees[3].isMissing)
    }

    @Test func noticesADeletedUpstream() {
        let gone = RepoStatus.parse(porcelainV2: "# branch.oid abc\n# branch.head feat\n# branch.upstream origin/feat\n")
        #expect(gone.upstreamGone)
        let tracking = RepoStatus.parse(porcelainV2: "# branch.oid abc\n# branch.head feat\n# branch.upstream origin/feat\n# branch.ab +1 -0\n")
        #expect(!tracking.upstreamGone && tracking.ahead == 1)
        #expect(!RepoStatus.parse(porcelainV2: "# branch.oid abc\n# branch.head local\n").upstreamGone)
    }

    @Test func footprintFromToolCalls() {
        let cwd = "/work/acme"
        #expect(ThreadFootprint.folders(toolName: "Edit", inputJSON: #"{"file_path":"/work/acme-login/src/app.ts"}"#, workingDirectory: cwd) == ["/work/acme-login/src"])
        #expect(ThreadFootprint.folders(toolName: "Grep", inputJSON: #"{"pattern":"x","path":"repos/api"}"#, workingDirectory: cwd) == ["/work/acme/repos/api"])
        // A relative word counts only if its first part is there: `repos` is, `feature` isn't.
        let root = FileManager.default.temporaryDirectory.appending(path: "yh-fp-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: root.appending(path: "repos/web"), withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let base = ThreadFootprint.resolve(root.path, in: "/")!
        let bash = ThreadFootprint.folders(toolName: "Bash", inputJSON: #"{"command":"cd ../acme-spike && git -C repos/web status && git worktree add -b feature/login x; npm --prefix=/opt/x i https://x.dev/a"}"#, workingDirectory: base)
        #expect(bash == [(base as NSString).deletingLastPathComponent + "/acme-spike", base + "/repos/web", "/opt/x"])
        #expect(ThreadFootprint.folders(toolName: "Bash", inputJSON: #"{"command":"ls"}"#, workingDirectory: cwd).isEmpty)
        #expect(ThreadFootprint.folders(inResult: "Created worktree at /work/acme/.claude/worktrees/fix.") == ["/work/acme/.claude/worktrees/fix"])
        #expect(ThreadFootprint.resolve("/tmp/x/", in: "/") == "/private/tmp/x")
    }

    @Test func footprintMatchesWorktrees() {
        let folders = ThreadFootprint.merge(["/work/acme-login/src", "/work/acme"], into: ["/work/acme", "/old"])
        #expect(folders == ["/old", "/work/acme-login/src", "/work/acme"])
        #expect(ThreadFootprint.touches(folders, root: "/work/acme-login"))
        #expect(ThreadFootprint.touches(folders, root: "/work/acme"))
        #expect(!ThreadFootprint.touches(folders, root: "/work/acme-spike"))
        #expect(!ThreadFootprint.touches(["/work/acme-loginx"], root: "/work/acme-login"))
    }

    /// Against real git: a repo with a linked worktree on a branch and a detached one.
    @Test func realWorktrees() async throws {
        let root = FileManager.default.temporaryDirectory.appending(path: "yh-wt-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        let repo = root.appending(path: "repo")
        try FileManager.default.createDirectory(at: repo, withIntermediateDirectories: true)
        let gitURL = URL(fileURLWithPath: "/usr/bin/git")
        let env = ["GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t", "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t", "PATH": "/usr/bin:/bin"]
        for args in [["init", "-q", "-b", "main"], ["commit", "-q", "--allow-empty", "-m", "one"]] {
            _ = await CommandRunner.run(gitURL, arguments: ["-C", repo.path] + args, environment: env)
        }
        let git = Git(executable: gitURL, environment: env)
        #expect(await git.addWorktree(of: repo, at: root.appending(path: "feature"), newBranch: "feature", from: "main", existingBranch: nil) == nil)
        _ = await CommandRunner.run(gitURL, arguments: ["-C", repo.path, "worktree", "add", "-q", "--detach", root.appending(path: "spike").path], environment: env)
        let worktrees = try #require(await git.worktrees(of: repo))
        #expect(worktrees.count == 3)
        #expect(worktrees.first { $0.branch == "feature" } != nil)
        #expect(worktrees.contains { $0.isDetached })
        #expect(await git.defaultBranch(of: repo) == "main")
        #expect(await git.branchesMerged(into: "main", in: repo).contains("feature"))
        let feature = try #require(worktrees.first { $0.branch == "feature" })
        #expect(await git.removeWorktree(feature, of: repo, force: false) == nil)
        #expect(await git.worktrees(of: repo)?.count == 2)
    }
}

extension WorktreeTests {
    /// A fresh branch from an older main isn't "merged"; a branch merged with a merge commit is.
    @Test func mainLineTellsFreshFromMerged() async throws {
        let repo = FileManager.default.temporaryDirectory.appending(path: "yh-ml-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: repo) }
        try FileManager.default.createDirectory(at: repo, withIntermediateDirectories: true)
        let gitURL = URL(fileURLWithPath: "/usr/bin/git")
        let env = ["GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t", "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t", "PATH": "/usr/bin:/bin"]
        func git(_ args: String...) async { _ = await CommandRunner.run(gitURL, arguments: ["-C", repo.path] + args, environment: env) }
        await git("init", "-q", "-b", "main")
        await git("commit", "-q", "--allow-empty", "-m", "one")
        await git("branch", "fresh")
        await git("checkout", "-q", "-b", "work")
        await git("commit", "-q", "--allow-empty", "-m", "work")
        await git("checkout", "-q", "main")
        await git("merge", "-q", "--no-ff", "work", "-m", "merge")
        let client = Git(executable: gitURL, environment: env)
        let line = await client.mainLine("main", in: repo)
        let merged = await client.branchesMerged(into: "main", in: repo)
        let fresh = await CommandRunner.run(gitURL, arguments: ["-C", repo.path, "rev-parse", "fresh"], environment: env).stdout.trimmingCharacters(in: .whitespacesAndNewlines)
        let work = await CommandRunner.run(gitURL, arguments: ["-C", repo.path, "rev-parse", "work"], environment: env).stdout.trimmingCharacters(in: .whitespacesAndNewlines)
        #expect(merged.isSuperset(of: ["fresh", "work"]))
        #expect(line.contains(fresh))
        #expect(!line.contains(work))
    }
}
