import Foundation
import Testing
@testable import YourHarness

/// Where a resumed Claude Code session's thread goes unless you pick.
struct ResumeDestinationTests {
    private let home = Project(name: "Home", path: "/Users/me")
    private let workspace = Project(name: "work", path: "/Users/me/dev/work")
    private let repo = Project(name: "api", path: "/Users/me/dev/work/api/")

    private func suggestion(_ folder: String) -> String? {
        AppModel.project(containing: folder, in: [home, workspace, repo], home: home.id)?.name
    }

    @Test func theFoldersOwnProject() {
        #expect(suggestion("/Users/me/dev/work/api") == "api")
        #expect(suggestion("/Users/me/dev/work") == "work")
        #expect(suggestion("/Users/me") == "Home")
    }

    @Test func theInnermostWorkspaceHoldingIt() {
        #expect(suggestion("/Users/me/dev/work/docs") == "work")
        #expect(suggestion("/Users/me/dev/work/api/Sources") == "api")
        #expect(suggestion("/Users/me/dev/work/../work/docs/") == "work")
    }

    /// Home holds everything under it, so it isn't a suggestion for those.
    @Test func noneElsewhere() {
        #expect(suggestion("/Users/me/dev/other") == nil)
        #expect(suggestion("/Users/me/dev/workshop") == nil)
        #expect(suggestion("/tmp/project") == nil)
    }
}
