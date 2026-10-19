import Foundation
import Testing
@testable import HarnessCore

struct ClaudeSessionArchiveTests {
    private static let id = "0f3c5a1e-7b2d-4c9e-8a61-2d4e6f8a0b1c"

    /// The shape of a real session file, sanitized: a rewound branch, parallel tool calls
    /// with sibling results, a compaction, a slash command, an interrupt and CLI noise.
    private static let fixture = """
    {"type":"permission-mode","permissionMode":"default","sessionId":"s"}
    {"parentUuid":null,"isSidechain":false,"type":"user","message":{"role":"user","content":"Why is the export slow?"},"uuid":"u1","timestamp":"2026-10-01T10:00:00.000Z","cwd":"/tmp/project","sessionId":"s"}
    {"parentUuid":"u1","isSidechain":false,"type":"attachment","attachment":{},"uuid":"att1","sessionId":"s"}
    {"parentUuid":"u1","isSidechain":false,"type":"assistant","message":{"id":"m1","model":"claude-opus-5-5","role":"assistant","content":[{"type":"thinking","thinking":"","signature":"x"}]},"uuid":"a1","timestamp":"2026-10-01T10:00:01.000Z","cwd":"/tmp/project","sessionId":"s"}
    {"parentUuid":"a1","isSidechain":false,"type":"assistant","message":{"id":"m1","model":"claude-opus-5-5","role":"assistant","content":[{"type":"text","text":"Let me look."}]},"uuid":"a2","timestamp":"2026-10-01T10:00:02.000Z","cwd":"/tmp/project","sessionId":"s"}
    {"parentUuid":"a2","isSidechain":false,"type":"assistant","message":{"id":"m1","model":"claude-opus-5-5","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/tmp/project/a.swift"}}]},"uuid":"a3","timestamp":"2026-10-01T10:00:03.000Z","cwd":"/tmp/project","sessionId":"s"}
    {"parentUuid":"a3","isSidechain":false,"type":"assistant","message":{"id":"m1","model":"claude-opus-5-5","role":"assistant","content":[{"type":"tool_use","id":"t2","name":"Grep","input":{"pattern":"export"}}]},"uuid":"a4","timestamp":"2026-10-01T10:00:03.500Z","cwd":"/tmp/project","sessionId":"s"}
    {"parentUuid":"a3","isSidechain":false,"type":"user","message":{"role":"user","content":[{"tool_use_id":"t1","type":"tool_result","content":"line 1"}]},"uuid":"r1","timestamp":"2026-10-01T10:00:04.000Z","cwd":"/tmp/project","sessionId":"s","toolUseResult":{}}
    {"parentUuid":"a4","isSidechain":false,"type":"user","message":{"role":"user","content":[{"tool_use_id":"t2","type":"tool_result","content":[{"type":"text","text":"no matches"}],"is_error":true}]},"uuid":"r2","timestamp":"2026-10-01T10:00:04.100Z","cwd":"/tmp/project","sessionId":"s","toolUseResult":{}}
    {"parentUuid":"r2","isSidechain":false,"type":"assistant","message":{"id":"m2","model":"claude-opus-5-5","role":"assistant","content":[{"type":"text","text":"It reads every row twice."}]},"uuid":"a5","timestamp":"2026-10-01T10:00:05.000Z","cwd":"/tmp/project","sessionId":"s"}
    {"parentUuid":"a5","isSidechain":false,"type":"user","message":{"role":"user","content":"Abandoned question"},"uuid":"dead1","timestamp":"2026-10-01T10:01:00.000Z","cwd":"/tmp/project","sessionId":"s"}
    {"parentUuid":"dead1","isSidechain":false,"type":"assistant","message":{"id":"m3","model":"claude-opus-5-5","role":"assistant","content":[{"type":"text","text":"Abandoned answer"}]},"uuid":"dead2","timestamp":"2026-10-01T10:01:01.000Z","cwd":"/tmp/project","sessionId":"s"}
    {"parentUuid":"a5","isSidechain":true,"type":"user","message":{"role":"user","content":"subagent prompt"},"uuid":"side1","timestamp":"2026-10-01T10:01:30.000Z","sessionId":"s"}
    {"parentUuid":"a5","isSidechain":false,"type":"user","message":{"role":"user","content":"<command-name>/compact</command-name>\\n<command-message>compact</command-message>\\n<command-args></command-args>"},"uuid":"c1","timestamp":"2026-10-01T10:02:00.000Z","cwd":"/tmp/project","sessionId":"s"}
    {"parentUuid":"c1","isSidechain":false,"type":"user","message":{"role":"user","content":"<local-command-stdout>Compacted</local-command-stdout>"},"uuid":"c2","timestamp":"2026-10-01T10:02:01.000Z","cwd":"/tmp/project","sessionId":"s"}
    {"parentUuid":null,"logicalParentUuid":"c2","isSidechain":false,"type":"system","subtype":"compact_boundary","content":"Conversation compacted","uuid":"b1","timestamp":"2026-10-01T10:02:02.000Z","cwd":"/tmp/project","sessionId":"s"}
    {"parentUuid":"b1","isSidechain":false,"type":"user","isCompactSummary":true,"isVisibleInTranscriptOnly":true,"message":{"role":"user","content":"This session is being continued from a previous conversation..."},"uuid":"s1","timestamp":"2026-10-01T10:02:03.000Z","cwd":"/tmp/project","sessionId":"s"}
    {"parentUuid":"s1","isSidechain":false,"type":"user","isMeta":true,"message":{"role":"user","content":"[Image: source: /tmp/x.png]"},"uuid":"meta1","timestamp":"2026-10-01T10:03:00.000Z","cwd":"/tmp/project","sessionId":"s"}
    {"parentUuid":"meta1","isSidechain":false,"type":"user","message":{"role":"user","content":[{"type":"text","text":"Make it read once<system-reminder>internal</system-reminder>"},{"type":"image","source":{}}]},"uuid":"u2","timestamp":"2026-10-01T10:03:01.000Z","cwd":"/tmp/project","sessionId":"s"}
    {"parentUuid":"u2","isSidechain":false,"type":"user","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]},"uuid":"i1","timestamp":"2026-10-01T10:03:02.000Z","cwd":"/tmp/project","sessionId":"s"}
    {"type":"ai-title","aiTitle":"Export performance","sessionId":"s"}
    {"type":"api-request-blob","hash":"h","message":{"content":"ignored"},"sessionId":"s"}
    {"type":"ai-title","aiTitle":"Speed up CSV export","sessionId":"s"}
    not json at all
    """

    private var archive: ClaudeSessionArchive {
        ClaudeSessionArchive.parse(Data(Self.fixture.utf8), sessionID: Self.id)
    }

    @Test func rebuildsTheLiveBranch() {
        #expect(archive.items == [
            .user("Why is the export slow?"),
            .assistant("Let me look."),
            .tool(id: "t1", name: "Read", inputJSON: #"{"file_path":"\/tmp\/project\/a.swift"}"#, result: "line 1", isError: false),
            .tool(id: "t2", name: "Grep", inputJSON: #"{"pattern":"export"}"#, result: "no matches", isError: true),
            .assistant("It reads every row twice."),
            .notice("/compact"),
            .notice("Conversation compacted"),
            .user("Make it read once"),
            .notice("Interrupted"),
        ])
    }

    @Test func readsSessionDetails() {
        let archive = archive
        #expect(archive.cwd == "/tmp/project")
        #expect(archive.title == "Speed up CSV export")
        #expect(archive.model == "claude-opus-5-5")
        #expect(archive.messageCount == 2)
        #expect(archive.firstMessage == "Why is the export slow?")
        #expect(!archive.hasCustomTitle)
        #expect(archive.startedAt == Date(timeIntervalSince1970: 1_790_848_800))
        #expect(archive.updatedAt == Date(timeIntervalSince1970: 1_790_848_800 + 182))
    }

    @Test func aRenameBeatsTheGeneratedTitle() {
        let data = Data((Self.fixture + "\n" + #"{"type":"custom-title","customTitle":"Export speed","sessionId":"s"}"#).utf8)
        let archive = ClaudeSessionArchive.parse(data, sessionID: Self.id)
        #expect(archive.title == "Export speed")
        #expect(archive.hasCustomTitle)
    }

    @Test func findsTheIDInWhateverWasPasted() {
        #expect(ClaudeSessionArchive.sessionID(in: "claude --resume 0F3C5A1E-7B2D-4C9E-8A61-2D4E6F8A0B1C ") == Self.id)
        #expect(ClaudeSessionArchive.sessionID(in: "~/.claude/projects/-tmp/\(Self.id).jsonl") == Self.id)
        #expect(ClaudeSessionArchive.sessionID(in: "not an id") == nil)
    }

    @Test func locatesTheFileByNameAcrossProjects() throws {
        let config = FileManager.default.temporaryDirectory.appendingPathComponent("archive-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: config) }
        let folder = config.appendingPathComponent("projects/-tmp-project")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: config.appendingPathComponent("projects/-tmp-other"), withIntermediateDirectories: true)
        let file = folder.appendingPathComponent("\(Self.id).jsonl")
        try Data(Self.fixture.utf8).write(to: file)

        let found = try #require(ClaudeSessionArchive.locate(sessionID: Self.id, configDirectory: config))
        #expect(found.standardizedFileURL == file.standardizedFileURL)
        #expect(ClaudeSessionArchive.locate(sessionID: UUID().uuidString.lowercased(), configDirectory: config) == nil)
        #expect(try ClaudeSessionArchive.load(from: found, sessionID: Self.id).items.count == 9)
        #expect(ClaudeSessionArchive.configDirectory(environment: ["CLAUDE_CONFIG_DIR": "/tmp/cfg"]).path == "/tmp/cfg")
    }

    /// Point at a real (large) session file to check the result and the time it takes:
    /// `YH_SESSION_FILE=<path> swift test --filter ClaudeSessionArchiveTests`
    @Test(.enabled(if: ProcessInfo.processInfo.environment["YH_SESSION_FILE"] != nil))
    func readsARealSession() throws {
        let url = URL(fileURLWithPath: ProcessInfo.processInfo.environment["YH_SESSION_FILE"]!)
        let started = ContinuousClock.now
        let archive = try ClaudeSessionArchive.load(from: url, sessionID: url.deletingPathExtension().lastPathComponent)
        let elapsed = ContinuousClock.now - started
        print("items \(archive.items.count), messages \(archive.messageCount), title \(archive.title ?? "-"), \(elapsed)")
        #expect(archive.messageCount > 0)
        #expect(elapsed < .seconds(5))
    }
}
