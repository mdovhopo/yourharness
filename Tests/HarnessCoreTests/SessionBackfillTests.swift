import Testing
@testable import HarnessCore

struct SessionBackfillTests {
    private typealias Item = ClaudeSessionArchive.Item

    private func archive(_ items: [Item]) -> ClaudeSessionArchive {
        ClaudeSessionArchive(sessionID: "s", items: items)
    }

    private func tool(_ id: String, result: String? = "ok") -> Item {
        .tool(id: id, name: "Bash", inputJSON: "{}", result: result, isError: false)
    }

    @Test func nothingMissingWhenTheyMatch() {
        let items: [Item] = [.user("hi"), .assistant("Hello"), tool("t1"), .assistant("Done")]
        #expect(SessionBackfill(known: items, archive: archive(items)).isEmpty)
    }

    /// Quit mid-reply: the reply was saved half-written and its tool call had no result.
    @Test func completesWhatWasCutOffAndAddsTheRest() {
        let known: [Item] = [.user("fix it"), tool("t1", result: nil), .assistant("Found the")]
        let session: [Item] = [.user("fix it"), tool("t1"), .assistant("Found the bug."), tool("t2"), .notice("Interrupted")]
        let backfill = SessionBackfill(known: known, archive: archive(session))
        #expect(backfill.completed == [1: tool("t1"), 2: .assistant("Found the bug.")])
        #expect(backfill.missing == [tool("t2"), .notice("Interrupted")])
    }

    /// Continued in the terminal with `claude --resume`.
    @Test func addsWhatHappenedElsewhere() {
        let known: [Item] = [.user("one"), .assistant("1"), .notice("Interrupted")]
        let session: [Item] = [.user("one"), .assistant("1"), .user("two"), .thinking("hm"), .assistant("2")]
        let backfill = SessionBackfill(known: known, archive: archive(session))
        #expect(backfill.completed.isEmpty)
        #expect(backfill.missing == [.user("two"), .thinking("hm"), .assistant("2")])
    }

    /// Only the newest common point matters, so long histories that were compacted or
    /// recorded differently further back still line up.
    @Test func anchorsOnTheNewestCommonMessage() {
        let known: [Item] = [.user("old"), .assistant("differs"), .user("/compact"), tool("t9"), .assistant("ok")]
        let session: [Item] = [.notice("Conversation compacted"), tool("t9"), .assistant("ok"), .user("next")]
        #expect(SessionBackfill(known: known, archive: archive(session)).missing == [.user("next")])
    }

    @Test func divergingHistoriesGiveNothing() {
        let known: [Item] = [.user("a"), .assistant("one thing")]
        let session: [Item] = [.user("a"), .assistant("something else"), .user("b")]
        #expect(SessionBackfill(known: known, archive: archive(session)).isEmpty)
        // Nothing in common at all, e.g. after /clear moved to another session.
        #expect(SessionBackfill(known: [.user("x")], archive: archive([.user("y")])).isEmpty)
    }

    @Test func transcriptAheadOfTheFileGivesNothing() {
        let known: [Item] = [.user("a"), .assistant("1"), .user("b")]
        #expect(SessionBackfill(known: known, archive: archive([.user("a"), .assistant("1")])).isEmpty)
    }
}
