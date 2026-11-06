import Foundation
import Testing
@testable import YourHarness

/// The Files tab's tabs: a preview tab links reuse, kept tabs, closing, and what's saved.
@MainActor
struct FilesModelTests {
    private func model(_ saved: SavedFiles? = nil) -> FilesModel {
        FilesModel(root: URL(fileURLWithPath: "/work/app", isDirectory: true), restoring: saved)
    }

    @Test func linksReuseThePreviewTab() {
        let files = model()
        files.open("a.swift", line: 3)
        files.open("b.swift")
        #expect(files.tabs.map(\.path) == ["b.swift"])
        #expect(files.tabs[0].isPreview)
        #expect(files.reveal?.path == "b.swift")
        // Kept, the next link opens beside it.
        files.keep("b.swift")
        files.open("c.swift")
        #expect(files.tabs.map(\.path) == ["b.swift", "c.swift"])
        files.open("d.swift", keep: true)
        #expect(files.tabs.map(\.path) == ["b.swift", "d.swift"])
        let previews = files.tabs.filter { $0.isPreview }
        #expect(previews.isEmpty)
        #expect(files.selectedPath == "d.swift")
    }

    @Test func reopeningSelectsAndRevealsAgain() {
        let files = model()
        files.open("a.swift", keep: true)
        files.open("b.swift", keep: true)
        let before = files.reveal?.serial ?? 0
        files.open("a.swift", line: 9)
        #expect(files.tabs.count == 2)
        #expect(files.selectedPath == "a.swift")
        #expect(files.reveal?.line == 9)
        #expect((files.reveal?.serial ?? 0) > before)
    }

    @Test func foldersShowInTheTree() {
        let files = model()
        files.showsTree = false
        files.open("Sources/")
        #expect(files.tabs.isEmpty)
        #expect(files.showsTree)
        #expect(files.treeReveal?.path == "Sources")
    }

    @Test func closingSelectsANeighbour() {
        let files = model()
        for name in ["a", "b", "c"] { files.open(name, keep: true) }
        files.select("b")
        files.close("b")
        #expect(files.selectedPath == "c")
        files.closeOthers("c")
        #expect(files.tabs.map(\.path) == ["c"])
    }

    @Test func oldestTabsGoPastTheLimit() {
        let files = model()
        for index in 0..<(FilesModel.tabLimit + 3) { files.open("f\(index)", keep: true) }
        #expect(files.tabs.count == FilesModel.tabLimit)
        #expect(files.tabs.first?.path == "f3")
    }

    @Test func savedAndRestored() throws {
        let files = model()
        files.open("a.swift", keep: true)
        files.open("b.swift")
        files.setExpanded("Sources", true)
        files.showsTree = false
        let data = try JSONEncoder().encode(files.saved)
        let restored = model(try JSONDecoder().decode(SavedFiles.self, from: data))
        #expect(restored.tabs == files.tabs)
        #expect(restored.selectedPath == "b.swift")
        #expect(restored.expanded == ["Sources"])
        #expect(!restored.showsTree)
        // Older threads have none.
        #expect(model().tabs.isEmpty)
        #expect(SavedFiles().isEmpty)
    }

    @Test func pathsOutsideTheRootStayClosed() {
        let files = model()
        #expect(files.relativePath(forAbsolute: "/work/app/src/a.swift") == "src/a.swift")
        #expect(files.relativePath(forAbsolute: "/work/app/../secrets.txt") == nil)
        #expect(files.relativePath(forAbsolute: "/work/application/a.swift") == nil)
    }
}

/// The files kept ready for going between tabs.
struct RecentFilesTests {
    private func loaded(_ name: String, bytes: Int = 10) throws -> (file: LoadedFile, url: URL) {
        let folder = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let url = folder.appending(path: name)
        try Data(repeating: 97, count: bytes).write(to: url)
        return (LoadedFile.load(url, path: name), url)
    }

    @Test func oldestGoPastTheLimits() throws {
        let recent = RecentFiles(limit: 2, textLimit: 100)
        recent.keep(try loaded("a").file)
        recent.keep(try loaded("b").file)
        recent.keep(try loaded("c").file)
        #expect(recent["a"] == nil)
        #expect(recent["b"] != nil)
        // A big one pushes the rest out, but stays itself.
        recent.keep(try loaded("big", bytes: 500).file)
        #expect(recent["big"] != nil)
        #expect(recent["c"] == nil)
    }

    @Test func closedTabsGo() throws {
        let recent = RecentFiles()
        recent.keep(try loaded("a").file)
        recent.keep(try loaded("b").file)
        recent.keepOnly(["b"])
        #expect(recent["a"] == nil)
        #expect(recent["b"] != nil)
    }

    @Test func aChangedFileIsNoLongerCurrent() throws {
        let (file, url) = try loaded("a")
        #expect(file.isCurrent(at: url))
        try Data(repeating: 98, count: 11).write(to: url)
        #expect(!file.isCurrent(at: url))
    }
}
