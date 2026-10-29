import Foundation
import Testing
@testable import HarnessCore

struct FileAttachmentTests {
    @Test func claudeIsToldWhereEachOneIs() {
        let note = FileAttachment.note(for: [("/tmp/a/report.pdf", false), ("/tmp/project", true)])
        #expect(note == """
        <attached-files>
        Attached (open them with Read, which takes text, PDFs and images; use a command only for other formats):
        - /tmp/a/report.pdf
        - /tmp/project/ (folder)
        </attached-files>
        """)
        #expect(FileAttachment.note(for: []).isEmpty)
    }

    @Test func foldersAndBigFilesStayWhereTheyAre() {
        let url = URL(fileURLWithPath: "/tmp/x")
        #expect(FileAttachment(url: url, isDirectory: false, size: 10).isCopied)
        #expect(!FileAttachment(url: url, isDirectory: true, size: nil).isCopied)
        #expect(!FileAttachment(url: url, isDirectory: false, size: FileAttachment.maxCopyBytes + 1).isCopied)
    }

    /// The session's history gives the message back as you typed it, so the transcript and
    /// the history still line up; files alone still make a message.
    @Test func historyShowsTheMessageWithoutTheNote() {
        let note = FileAttachment.note(for: [("/tmp/a.csv", false)])
        func line(_ uuid: String, parent: String?, text: String) -> String {
            let message: [String: Any] = ["role": "user", "content": [["type": "text", "text": text]]]
            let object: [String: Any] = ["type": "user", "uuid": uuid, "parentUuid": parent ?? NSNull(), "message": message,
                                         "timestamp": "2026-10-01T10:00:00.000Z", "sessionId": "s"]
            return String(decoding: try! JSONSerialization.data(withJSONObject: object), as: UTF8.self)
        }
        let data = Data([line("u1", parent: nil, text: "Sum the second column\n\n" + note),
                         line("u2", parent: "u1", text: note)].joined(separator: "\n").utf8)
        let archive = ClaudeSessionArchive.parse(data, sessionID: "s")
        #expect(archive.items == [.user("Sum the second column"), .user("[Image]")])
    }
}
