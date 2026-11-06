import Foundation
import Testing
@testable import HarnessCore

/// Which files a turn changed, from git, however they were changed.
struct TurnChangesTests {
    @Test func seesEveryKindOfChangeAndLeavesTheRepoAlone() async throws {
        let repo = FileManager.default.temporaryDirectory.appending(path: "yh-changes-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: repo) }
        try FileManager.default.createDirectory(at: repo, withIntermediateDirectories: true)
        let gitURL = URL(fileURLWithPath: "/usr/bin/git")
        let env = ["GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t", "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t", "PATH": "/usr/bin:/bin"]
        @discardableResult
        func git(_ args: String...) async -> String {
            await CommandRunner.run(gitURL, arguments: ["-C", repo.path] + args, environment: env).stdout
        }
        func write(_ text: String, _ path: String) throws {
            let url = repo.appending(path: path)
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try Data(text.utf8).write(to: url)
        }
        await git("init", "-q", "-b", "main")
        try write("ignored.log\n", ".gitignore")
        for name in ["edited.swift", "untouched.swift", "removed.swift", "staged.swift"] { try write("one\n", "Sources/\(name)") }
        try write("draft\n", "notes.md")
        await git("add", ".gitignore", "Sources")
        await git("commit", "-q", "-m", "one")
        // Already changed before the turn: not the turn's work.
        try write("two\n", "Sources/staged.swift")
        await git("add", "Sources/staged.swift")
        let status = await git("status", "--porcelain")
        let index = try Data(contentsOf: repo.appending(path: ".git/index"))

        let client = Git(executable: gitURL, environment: env)
        let start = try #require(await client.snapshot(of: repo.appending(path: "Sources")))
        #expect(await git("status", "--porcelain") == status)
        #expect(try Data(contentsOf: repo.appending(path: ".git/index")) == index)

        // The turn: a shell edit, a new file, one deleted, one committed, an untracked file
        // changed, and a log nobody tracks.
        try write("one\nmore\n", "Sources/edited.swift")
        try write("new\n", "Sources/Created.swift")
        try FileManager.default.removeItem(at: repo.appending(path: "Sources/removed.swift"))
        try write("committed\n", "Sources/committed.swift")
        await git("add", "Sources/committed.swift")
        await git("commit", "-q", "-m", "two")
        try write("draft, longer\n", "notes.md")
        try write("noise\n", "ignored.log")

        let changed = try #require(await client.changes(since: start))
        let relative = changed.map { String($0.dropFirst(start.top.path.count + 1)) }
        #expect(relative == ["Sources/Created.swift", "Sources/committed.swift", "Sources/edited.swift", "Sources/removed.swift", "notes.md"])

        // Nothing since: nothing changed.
        let again = try #require(await client.snapshot(of: repo))
        #expect(await client.changes(since: again) == [])
    }

    @Test func outsideARepoThereIsNoSnapshot() async throws {
        let folder = FileManager.default.temporaryDirectory.appending(path: "yh-norepo-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: folder) }
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let client = Git(executable: URL(fileURLWithPath: "/usr/bin/git"), environment: ["PATH": "/usr/bin:/bin", "GIT_CEILING_DIRECTORIES": folder.deletingLastPathComponent().path])
        #expect(await client.snapshot(of: folder) == nil)
    }
}
