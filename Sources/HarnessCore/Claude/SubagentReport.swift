import Foundation

/// What a sub-agent's Agent tool call hands back to the main agent.
public enum SubagentReport {
    /// The tool that starts a sub-agent; older CLIs call it Task.
    public static let toolNames: Set<String> = ["Agent", "Task"]

    /// The report without the CLI's hand-back frame, ready to show. Nil for the placeholder
    /// a background sub-agent returns right away ("Async agent launched…").
    public static func text(fromToolResult result: String) -> String? {
        if result.hasPrefix("Async agent launched") { return nil }
        guard result.hasPrefix("[Subagent hand-back]"),
              let start = result.range(of: "The report follows:\n")
        else { return result }
        // The frame indents every line of the report by two spaces.
        return result[start.upperBound...]
            .split(separator: "\n", omittingEmptySubsequences: false)
            .map { $0.hasPrefix("  ") ? $0.dropFirst(2) : $0 }
            .joined(separator: "\n")
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
