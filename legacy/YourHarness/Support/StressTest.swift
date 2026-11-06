import Foundation

/// Synthetic transcripts for measuring scroll performance and memory with realistic content.
@MainActor
enum StressTest {
    static func entries(count: Int) -> [TranscriptEntry] {
        (0..<count).map { index in
            switch index % 5 {
            case 0:
                return TranscriptEntry(kind: .user, text: "Message \(index): can you refactor the session reducer and add tests?")
            case 1:
                return TranscriptEntry(kind: .assistant, text: assistantText(index))
            case 2:
                let entry = TranscriptEntry(id: "tool-\(index)", kind: .tool)
                entry.toolName = "Bash"
                entry.toolInput = #"{"command":"swift test --filter ParserTests","description":"Run parser tests"}"#
                entry.toolResult = (0..<40).map { "Test case \($0) passed (0.00\($0 % 10)s)" }.joined(separator: "\n")
                return entry
            case 3:
                let entry = TranscriptEntry(id: "tool-\(index)", kind: .tool)
                entry.toolName = "Edit"
                entry.toolInput = #"{"file_path":"/tmp/project/Sources/Reducer.swift","old_string":"let a = 1\nlet b = 2","new_string":"let a = 1\nlet b = 3\nlet c = a + b"}"#
                entry.toolResult = "The file has been updated."
                return entry
            default:
                return TranscriptEntry(kind: .assistant, text: "Done. **\(index)** items processed, see `Reducer.swift`.")
            }
        }
    }

    private static func assistantText(_ index: Int) -> String {
        """
        ## Step \(index)

        Here's the plan. I'll keep the reducer **pure** and move side effects to the session layer:

        1. Parse each line into an `AgentEvent`
        2. Apply events to the transcript
        3. Persist when the turn settles

        ```swift
        func apply(_ event: AgentEvent) {
            switch event {
            case .textDelta(let ref, let text):
                blocks[ref]?.text += text
            default:
                break
            }
        }
        ```

        > Note: deltas only invalidate the row they touch.
        """
    }

    /// Every construct the renderer handles, for eyeballing styles and streaming.
    static let markdownSample = """
        The build is green and the parser work is done. Two things are left, and one needs your call.

        ## Current state

        **Repos** (checked just now)

        - `main` is 2 commits ahead of origin. Open PRs #4 (`table-parser`) and #5 are probably redundant now.
        - The fix landed in mdovhopo/yourharness#1, see https://github.com/mdovhopo/yourharness for details.
          It also needs a follow-up in the *docs*.
            - nested point with ~~old~~ new wording
        - [x] Parser handles tables
        - [ ] Streaming doesn't flicker

        | Area | Status | Owner |
        |---|:---:|--:|
        | 1. Block parser | Done, with tables and task lists | you |
        | 2. Inline styles (`code`, links) | Done, except images | me |
        | 3. Syntax highlighting | **Planned** | — |

        ### Next steps

        1. Profile a 10,000-entry transcript.
        2. Add syntax highlighting to code blocks.
        3. Check that `swift test` still passes on a clean checkout.

        ```swift
        let blocks = MarkdownParser.parse(text, streaming: true)
        ```

        > Note: tables, lists and code render natively; nothing is a web view.

        ---

        That's everything. Want me to start on step 1?
        """

    /// Appends a streaming assistant entry and feeds it the markdown sample in bursty,
    /// network-like chunks, to see how streaming looks at the bottom of a transcript.
    static func simulateStreaming(into thread: ThreadModel) {
        let entry = TranscriptEntry(kind: .assistant, isStreaming: true)
        thread.appendForStress(entry)
        let text = markdownSample
        Task {
            var index = text.startIndex
            while index < text.endIndex {
                let size = Int.random(in: 4...48)
                let end = text.index(index, offsetBy: size, limitedBy: text.endIndex) ?? text.endIndex
                entry.text += text[index..<end]
                index = end
                try? await Task.sleep(for: .milliseconds(Int.random(in: 30...140)))
            }
            entry.isStreaming = false
        }
    }
}
