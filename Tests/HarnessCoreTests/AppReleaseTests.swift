import Foundation
import Testing
@testable import HarnessCore

struct AppReleaseTests {
    @Test func comparesVersions() throws {
        func v(_ text: String) throws -> SemanticVersion { try #require(SemanticVersion(text)) }
        #expect(try v("0.0.2") < v("0.0.3"))
        #expect(try v("0.0.9") < v("0.0.10"))
        #expect(try v("0.1") > v("0.0.12"))
        #expect(try v("v1.2.0") == v("1.2"))
        #expect(try v("1.0.0-beta.2") < v("1.0.0-beta.10"))
        #expect(try v("1.0.0-beta") < v("1.0.0"))
        #expect(try v("v0.0.2").description == "0.0.2")
        #expect(SemanticVersion("nightly") == nil)
        #expect(SemanticVersion("1..2") == nil)
    }

    @Test func parsesGitHubLatestRelease() throws {
        let json = """
        {"tag_name":"v0.0.3","name":"YourHarness 0.0.3","prerelease":false,"published_at":"2026-10-23T08:00:00Z",
         "html_url":"https://github.com/mdovhopo/yourharness/releases/tag/v0.0.3",
         "body":"### Highlights\\nUpdates in the app.\\n\\n### Install\\n1. Download it.",
         "assets":[
          {"name":"YourHarness-0.0.3.dmg.sha256","size":88,"browser_download_url":"https://example.com/YourHarness-0.0.3.dmg.sha256"},
          {"name":"YourHarness-0.0.3.dmg","size":3713053,"browser_download_url":"https://example.com/YourHarness-0.0.3.dmg"}],
         "author":{"login":"someone"},"new_field":[1,2]}
        """
        let release = try AppRelease.parse(gitHubLatest: Data(json.utf8))
        #expect(release.version == SemanticVersion("0.0.3"))
        #expect(release.title == "YourHarness 0.0.3")
        #expect(release.diskImage.absoluteString == "https://example.com/YourHarness-0.0.3.dmg")
        #expect(release.diskImageSize == 3713053)
        #expect(release.checksum?.absoluteString == "https://example.com/YourHarness-0.0.3.dmg.sha256")
        #expect(release.publishedAt != nil)
        #expect(release.whatsNew == "### Highlights\nUpdates in the app.")
    }

    @Test func rejectsReleasesItCantInstall() {
        #expect(throws: AppRelease.ParseError.notARelease) { try AppRelease.parse(gitHubLatest: Data(#"{"message":"Not Found"}"#.utf8)) }
        #expect(throws: AppRelease.ParseError.noDiskImage("v0.0.3")) {
            try AppRelease.parse(gitHubLatest: Data(#"{"tag_name":"v0.0.3","assets":[]}"#.utf8))
        }
        #expect(throws: AppRelease.ParseError.badVersion("latest")) {
            try AppRelease.parse(gitHubLatest: Data(#"{"tag_name":"latest","assets":[]}"#.utf8))
        }
    }

    @Test func readsChecksumFiles() {
        let digest = String(repeating: "a1", count: 32)
        #expect(AppRelease.digest(inChecksumFile: "\(digest)  YourHarness-0.0.3.dmg\n") == digest)
        #expect(AppRelease.digest(inChecksumFile: digest.uppercased()) == digest)
        #expect(AppRelease.digest(inChecksumFile: "not a digest") == nil)
        #expect(AppRelease.digest(inChecksumFile: "") == nil)
    }
}

struct ReleaseNotesTests {
    @Test func parsesGitHubReleaseList() throws {
        let json = """
        [{"tag_name":"v0.0.3","name":"YourHarness 0.0.3","body":"### Fixed\\n- A thing.\\n\\n### Install\\n1. Download.","html_url":"https://example.com/v0.0.3","published_at":"2026-10-23T08:00:00Z","assets":[]},
         {"tag_name":"v0.0.4-beta.1","prerelease":true,"body":"beta","assets":[]},
         {"tag_name":"v0.0.5","draft":true,"body":"draft"},
         {"tag_name":"nightly","body":"not a version"},
         {"tag_name":"v0.0.1","name":"","body":"First.","assets":[]},
         {"tag_name":"v0.0.2","name":"YourHarness 0.0.2","body":"## Install\\nOnly install notes.","assets":[]}]
        """
        let notes = try ReleaseNotes.parse(gitHubList: Data(json.utf8))
        #expect(notes.map(\.id) == ["0.0.3", "0.0.2", "0.0.1"])
        #expect(notes[0].text == "### Fixed\n- A thing.")
        #expect(notes[0].page?.absoluteString == "https://example.com/v0.0.3")
        #expect(notes[0].publishedAt != nil)
        #expect(notes[1].text == "")
        #expect(notes[2].title == "YourHarness 0.0.1")
        #expect(throws: AppRelease.ParseError.notARelease) { try ReleaseNotes.parse(gitHubList: Data(#"{"message":"Not Found"}"#.utf8)) }
    }

    @Test func anUpdatesNotesComeFromItsRelease() throws {
        let json = #"{"tag_name":"v0.0.3","body":"Notes.\n\n### Install\nSteps.","html_url":"https://example.com/r","assets":[{"name":"YourHarness-0.0.3.dmg","browser_download_url":"https://example.com/a.dmg"}]}"#
        let notes = try AppRelease.parse(gitHubLatest: Data(json.utf8)).releaseNotes
        #expect(notes.version == SemanticVersion("0.0.3"))
        #expect(notes.text == "Notes.")
        #expect(notes.page?.absoluteString == "https://example.com/r")
    }
}
