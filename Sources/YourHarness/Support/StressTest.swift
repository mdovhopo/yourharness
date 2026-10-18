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

    /// Appends a streaming assistant entry and feeds it tokens at ~60 Hz, to see how
    /// streaming behaves at the bottom of a long transcript.
    static func simulateStreaming(into thread: ThreadModel, duration: Duration = .seconds(15)) {
        let entry = TranscriptEntry(kind: .assistant, isStreaming: true)
        thread.appendForStress(entry)
        let words = assistantText(0).split(separator: " ", omittingEmptySubsequences: false)
        Task {
            let clock = ContinuousClock()
            let deadline = clock.now + duration
            var index = 0
            while clock.now < deadline {
                entry.text += words[index % words.count] + " "
                index += 1
                try? await Task.sleep(for: .milliseconds(16))
            }
            entry.isStreaming = false
        }
    }
}
