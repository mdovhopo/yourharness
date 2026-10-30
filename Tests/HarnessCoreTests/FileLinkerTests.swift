import Foundation
import Testing
@testable import HarnessCore

struct FileLinkerTests {
    private let linker = FileLinker(root: "/work/app", index: FileIndex(files: [
        "README.md",
        "Package.swift",
        "Sources/App/ThreadView.swift",
        "Sources/App/ComposerView.swift",
        "Sources/Core/README.md",
        "Sources/Core/Model.swift",
        "Tests/ModelTests.swift",
        "docs/storage.md",
        "a/Model.swift",
    ]))

    private func linked(_ text: String) -> [String] {
        linker.references(in: text).map { reference in
            let characters = Array(text)
            var shown = String(characters[reference.range]) + " → " + reference.path
            if let line = reference.line { shown += ":\(line)" }
            if let end = reference.endLine { shown += "-\(end)" }
            return shown
        }
    }

    @Test func exactPathsWithLines() {
        #expect(linked("See Sources/App/ThreadView.swift:12 for it.") == ["Sources/App/ThreadView.swift:12 → Sources/App/ThreadView.swift:12"])
        #expect(linked("Sources/App/ThreadView.swift:12-20") == ["Sources/App/ThreadView.swift:12-20 → Sources/App/ThreadView.swift:12-20"])
        #expect(linked("Sources/App/ThreadView.swift#L4-L9") == ["Sources/App/ThreadView.swift#L4-L9 → Sources/App/ThreadView.swift:4-9"])
        // Line and column: the column is dropped.
        #expect(linked("Package.swift:3:7") == ["Package.swift:3:7 → Package.swift:3"])
        #expect(linked("README.md.") == ["README.md → README.md"])
    }

    @Test func absoluteInsideTheRootOnly() {
        #expect(linked("open /work/app/docs/storage.md") == ["/work/app/docs/storage.md → docs/storage.md"])
        #expect(linked("open /etc/hosts and /work/other/README.md").isEmpty)
        #expect(linked("../README.md").isEmpty)
        #expect(linked("./docs/storage.md") == ["./docs/storage.md → docs/storage.md"])
    }

    @Test func namesWhenOnlyOneFileHasThem() {
        #expect(linked("ThreadView.swift") == ["ThreadView.swift → Sources/App/ThreadView.swift"])
        #expect(linked("Core/Model.swift:3") == ["Core/Model.swift:3 → Sources/Core/Model.swift:3"])
        // Two files named so: no guess.
        #expect(linked("Model.swift").isEmpty)
        // One at the root wins by being exact.
        #expect(linked("README.md") == ["README.md → README.md"])
    }

    @Test func foldersAndMentions() {
        #expect(linked("in Sources/App/") == ["Sources/App/ → Sources/App/"])
        #expect(linked("@docs/storage.md") == ["docs/storage.md → docs/storage.md"])
    }

    @Test func notPaths() {
        #expect(linked("Call FileIndex.matches here, see https://github.com/a/README.md").isEmpty)
        #expect(linked("FileIndex.matches, e.g. 1.5 or v0.0.12").isEmpty)
        #expect(linked("mail me@example.com").isEmpty)
        #expect(FileLinker.empty.references(in: "README.md").isEmpty)
    }

    @Test func offsetsCountCharacters() {
        let text = "👋🏽 ünïcode then ThreadView.swift:2"
        let reference = try! #require(linker.references(in: text).first)
        #expect(String(Array(text)[reference.range]) == "ThreadView.swift:2")
    }

    @Test func revisionFollowsTheFiles() {
        let same = FileLinker(root: "/work/app", files: ["a.swift"])
        #expect(same.revision == FileLinker(root: "/work/app", files: ["a.swift"]).revision)
        #expect(same.revision != FileLinker(root: "/work/app", files: ["a.swift", "b.swift"]).revision)
        #expect(same.revision != FileLinker(root: "/work/other", files: ["a.swift"]).revision)
    }

    @Test func linksRoundTrip() throws {
        let url = try #require(FileLink.url(path: "/work/my app/a.swift", line: 3, endLine: 5))
        let parsed = try #require(FileLink.parse(url))
        #expect(parsed.path == "/work/my app/a.swift")
        #expect(parsed.line == 3)
        #expect(parsed.endLine == 5)
        #expect(FileLink.parse(URL(string: "https://example.com")!) == nil)
    }

    /// Runs on every paragraph rendered: a long reply against a big index stays well under a frame.
    @Test func fastOnBigIndexes() {
        let files = (0..<50_000).map { "Sources/Module\($0 / 500)/File\($0).swift" }
        let big = FileLinker(root: "/work/app", files: files)
        let paragraph = "Changed Sources/Module3/File1700.swift:40 and File42.swift, see the notes in README.md. "
        let reply = String(repeating: paragraph, count: 200)
        let start = ContinuousClock.now
        let found = big.references(in: reply)
        let elapsed = ContinuousClock.now - start
        #expect(found.count == 400)
        #expect(elapsed < .milliseconds(50))
    }
}

struct FileReaderTests {
    private func file(_ contents: Data, name: String = "a.swift") throws -> URL {
        let folder = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let url = folder.appending(path: name)
        try contents.write(to: url)
        return url
    }

    @Test func textAndLines() throws {
        let snapshot = FileReader.read(try file(Data("one\ntwo\nthree\n".utf8)))
        #expect(snapshot.kind == .text)
        #expect(snapshot.lineStarts == [0, 4, 8])
        #expect(snapshot.language == "swift")
        #expect(snapshot.line(at: 5) == 2)
        #expect((snapshot.text as NSString).substring(with: snapshot.range(ofLines: 2)) == "two")
        #expect((snapshot.text as NSString).substring(with: snapshot.range(ofLines: 2, through: 9)) == "two\nthree")
    }

    @Test func binaryImagesFoldersAndMissing() throws {
        #expect(FileReader.read(try file(Data([1, 0, 2]), name: "x.bin")).kind == .binary)
        #expect(FileReader.read(try file(Data([1, 2]), name: "x.png")).kind == .image)
        let url = try file(Data())
        #expect(FileReader.read(url.deletingLastPathComponent()).kind == .folder)
        #expect(FileReader.read(url.appending(path: "nope")).kind == .missing)
    }

    @Test func bigFilesShowTheirStart() throws {
        let line = String(repeating: "x", count: 99) + "\n"
        let snapshot = FileReader.read(try file(Data(String(repeating: line, count: 100).utf8), name: "big.txt"), limit: 1_050)
        #expect(snapshot.isTruncated)
        #expect(snapshot.text.count == 1_000)
        #expect(snapshot.language == "")
    }

    @Test func languages() {
        #expect(FileReader.language(for: "Dockerfile") == "dockerfile")
        #expect(FileReader.language(for: "index.TS") == "ts")
        #expect(FileReader.language(for: "notes.md") == "")
    }
}
