import Foundation
import Testing
import HarnessCore
@testable import HarnessBackend

/// The store turns a turn's events into rows and says what changed, once per change.
@Suite struct ThreadStoreTests {
    @Test func toolCallThenAnswer() {
        let store = ThreadStore()
        var patches = store.userMessage("Write it", thread: "t")
        let m1 = "m1", m2 = "m2"
        let events: [AgentEvent] = [
            .blockStarted(BlockRef(messageID: m1, index: 0), .toolUse(id: "T1", name: "Write")),
            .toolInputDelta(BlockRef(messageID: m1, index: 0), "{\"a\":"),
            .assistantMessage(messageID: m1, blocks: [.toolUse(id: "T1", name: "Write", inputJSON: "{\"a\":1}")], parentToolUseID: nil),
            .toolResults([ToolResult(toolUseID: "T1", content: "ok", isError: false)], parentToolUseID: nil),
            .blockStarted(BlockRef(messageID: m2, index: 0), .text),
            .textDelta(BlockRef(messageID: m2, index: 0), "Done"),
            .textDelta(BlockRef(messageID: m2, index: 0), "."),
            .assistantMessage(messageID: m2, blocks: [.text("Done.")], parentToolUseID: nil),
        ]
        for event in events { patches += store.apply(event, thread: "t") }
        let rows = store.entries("t")
        #expect(rows.map(\.kind) == [.user, .tool, .assistant])
        #expect(rows[1].toolResult == "ok" && rows[1].toolInput == "{\"a\":1}" && !rows[1].live)
        #expect(rows[2].text == "Done." && !rows[2].live)
        // Two adds after the user's, streamed text as appends, no update when nothing changed.
        let adds = patches.filter { if case .add = $0 { true } else { false } }.count
        let appends = patches.filter { if case .append = $0 { true } else { false } }.count
        #expect(adds == 3 && appends == 3)
    }
}
