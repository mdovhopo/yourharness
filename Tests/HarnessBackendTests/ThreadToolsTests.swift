import Foundation
import Testing
import HarnessCore
@testable import HarnessBackend

/// The `threads` server an agent gets: what it lists from where, and renaming through the
/// page's own request.
@MainActor
@Suite struct ThreadToolsTests {
    private func text(_ result: MCPToolResult) -> String {
        result.content.compactMap { if case .text(let text) = $0 { text } else { nil } }.joined()
    }

    @Test func listAndRenameRespectTheCallersProject() async throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("threadtools-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let backend = Backend(dataDirectory: dir)
        let home = backend.library.addProject(path: dir.appendingPathComponent("home").path)
        let app = backend.library.addProject(path: dir.appendingPathComponent("app").path)
        let other = backend.library.addProject(path: dir.appendingPathComponent("other").path)
        backend.library.setSetting("homeProjectID", home)
        let homeThread = backend.library.addThread(projectID: home)
        let caller = backend.library.addThread(projectID: app)
        let sibling = backend.library.addThread(projectID: app)
        let elsewhere = backend.library.addThread(projectID: other)
        backend.library.updateThread(caller) { $0["title"] = "Caller work" }
        backend.library.updateThread(sibling) { $0["title"] = "Fix the parser" }
        backend.library.updateThread(elsewhere) { $0["title"] = "Somewhere else" }

        // From a project: only that project's threads.
        let server = backend.threadToolServer(for: caller)
        #expect(server.name == "threads")
        #expect(server.tools.map(\.name).contains("list_threads"))
        let listed = await server.call("list_threads", arguments: [:], toolUseID: nil)
        #expect(!listed.isError)
        let listing = text(listed)
        #expect(listing.hasPrefix("app: 2 threads:"))
        #expect(listing.contains("“Fix the parser”") && listing.contains("this thread"))
        #expect(!listing.contains("Somewhere else"))
        let wrongProject = await server.call("list_threads", arguments: ["project": "other"], toolUseID: nil)
        #expect(wrongProject.isError)

        // Rename a sibling by its short id: chosen, as from the sidebar.
        let short = String(sibling.lowercased().prefix(8))
        let renamed = await server.call("rename_thread", arguments: ["thread_id": .string(short), "title": "Parser rewrite"], toolUseID: nil)
        #expect(!renamed.isError)
        #expect(text(renamed) == "Renamed “Fix the parser” to “Parser rewrite”.")
        #expect(backend.library.thread(sibling)?["title"] as? String == "Parser rewrite")
        #expect(backend.library.thread(sibling)?["titleChosen"] as? Bool == true)

        // A thread in another project reads as not there.
        let outside = await server.call("rename_thread", arguments: ["thread_id": .string(elsewhere), "title": "Nope"], toolUseID: nil)
        #expect(outside.isError)
        #expect(backend.library.thread(elsewhere)?["title"] as? String == "Somewhere else")

        // From Home: everything.
        let all = text(await backend.threadToolServer(for: homeThread).call("list_threads", arguments: [:], toolUseID: nil))
        #expect(all.hasPrefix("All projects: 4 threads:"))
        #expect(all.contains("Somewhere else") && all.contains("Parser rewrite"))
        #expect(backend.threadInstructions(for: homeThread).contains("This thread is in Home"))
        #expect(backend.threadInstructions(for: caller).contains("the project app"))
    }
}
