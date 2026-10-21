import Foundation
import Testing
@testable import HarnessCore

struct FileIndexTests {
    private let index = FileIndex(files: [
        "README.md",
        "Sources/YourHarness/Views/ThreadView.swift",
        "Sources/YourHarness/Views/ComposerView.swift",
        "Sources/YourHarness/Model/ThreadModel.swift",
        "Tests/HarnessCoreTests/ThreadTitleTests.swift",
        "docs/my notes.md",
    ])

    @Test func addsEachFolder() {
        #expect(index.paths.contains("Sources/"))
        #expect(index.paths.contains("Sources/YourHarness/Views/"))
        #expect(index.paths.count(where: { $0 == "Sources/" }) == 1)
    }

    @Test func ranksNamesBeforePaths() {
        #expect(index.matches("threadview").first == "Sources/YourHarness/Views/ThreadView.swift")
        let thread = index.matches("thread")
        #expect(Set(thread.prefix(2)) == ["Sources/YourHarness/Model/ThreadModel.swift", "Sources/YourHarness/Views/ThreadView.swift"])
        #expect(thread.contains("Tests/HarnessCoreTests/ThreadTitleTests.swift"))
        #expect(index.matches("views").first == "Sources/YourHarness/Views/")
        // Letters in order, as a quick abbreviation of the path.
        #expect(index.matches("srcvcomp").first == "Sources/YourHarness/Views/ComposerView.swift")
        #expect(index.matches("zzz").isEmpty)
    }

    @Test func nothingTypedShowsTheTop() {
        #expect(index.matches("", limit: 3) == ["README.md", "Sources/", "Tests/"])
    }

    @Test func mentionsAndTheirQuery() {
        #expect(FileIndex.mention("docs/my notes.md") == "@\"docs/my notes.md\"")
        #expect(FileIndex.mention("README.md") == "@README.md")
        #expect(FileIndex.trailingMention(in: "look at @Thre")?.query == "Thre")
        #expect(FileIndex.trailingMention(in: "@")?.query == "")
        #expect(FileIndex.trailingMention(in: "mail me@example") == nil)
        #expect(FileIndex.trailingMention(in: "see @README.md now") == nil)
    }

    @Test func walksAFolderWithoutBuildOutput() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("yh-files-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        for path in ["a.txt", "src/b.swift", "node_modules/x/index.js", ".git/HEAD"] {
            let url = root.appendingPathComponent(path)
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try Data().write(to: url)
        }
        #expect(Set(FileIndex.walk(root)) == ["a.txt", "src/b.swift"])
    }
}
