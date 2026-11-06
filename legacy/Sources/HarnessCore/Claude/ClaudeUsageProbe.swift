import Foundation

/// Fetches current usage limits on demand. The CLI only reports limits alongside a real
/// request, so this sends the smallest one it can: Haiku, one word, no tools, no MCP,
/// none of the user's settings or hooks, and nothing saved to session history. About 600
/// tokens, a couple of seconds.
public enum ClaudeUsageProbe {
    public enum Failure: Error, Equatable {
        case timedOut
        case noReport(String)
    }

    static let arguments = [
        "-p", "ping",
        "--output-format", "stream-json", "--verbose",
        "--model", "haiku",
        "--tools", "",
        "--strict-mcp-config",
        "--setting-sources", "",
        "--no-session-persistence",
        "--system-prompt", "Reply with one word.",
    ]

    public static func run(executable: URL, environment: [String: String]) async -> Result<RateLimitInfo, Failure> {
        let output = await CommandRunner.run(
            executable, arguments: arguments, environment: environment,
            timeout: 45, outputLimit: 1024 * 1024
        )
        if let info = parse(output.stdout) { return .success(info) }
        if output.timedOut { return .failure(.timedOut) }
        let detail = CLIDiscovery.firstLine(output.stderr) ?? resultText(output.stdout) ?? "Claude didn't report usage."
        return .failure(.noReport(detail))
    }

    static func parse(_ stdout: String) -> RateLimitInfo? {
        var parser = ClaudeStreamParser()
        var latest: RateLimitInfo?
        for line in stdout.split(separator: "\n") {
            for case .rateLimit(let info) in parser.parse(line: Data(line.utf8)) where !info.windows.isEmpty {
                latest = info
            }
        }
        return latest
    }

    /// An error result ("Not logged in", "Credit balance too low") is the most useful thing to show.
    private static func resultText(_ stdout: String) -> String? {
        var parser = ClaudeStreamParser()
        for line in stdout.split(separator: "\n") {
            for case .turnCompleted(let result) in parser.parse(line: Data(line.utf8)) where result.isError {
                return result.resultText
            }
        }
        return nil
    }
}
