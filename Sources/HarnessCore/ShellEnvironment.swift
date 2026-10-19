import Foundation

/// GUI apps launched from Finder/Dock don't inherit the user's shell PATH, so `claude`
/// (and every tool it runs) would be missing. Capture the login shell's environment once.
///
/// The shell starts from a minimal environment, like a fresh Terminal window, so nothing
/// the app itself inherited leaks into the agents it runs. That matters when the app was
/// launched from inside a Claude Code session: its session ID and messaging socket would
/// otherwise tie every thread's `claude` to that outer session.
public enum ShellEnvironment {
    private static let marker = "__YOURHARNESS_ENV__"

    /// Set by a running Claude Code session for its own children. Never pass these on.
    static let sessionVariables: Set<String> = [
        "CLAUDECODE", "CLAUDE_PID", "CLAUDE_EFFORT",
        "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SSE_PORT", "CLAUDE_CODE_EXECPATH",
        "CLAUDE_CODE_SESSION_ID", "CLAUDE_CODE_CHILD_SESSION", "CLAUDE_CODE_SESSION_ATTENDED",
        "CLAUDE_CODE_MESSAGING_SOCKET", "CLAUDE_CODE_MESSAGING_TOKEN",
    ]

    /// What a login shell gets from launchd; everything else comes from the user's profile.
    static let baseVariables: Set<String> = [
        "HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "LANG", "SSH_AUTH_SOCK", "__CF_USER_TEXT_ENCODING",
    ]

    public static func load() async -> [String: String] {
        let inherited = ProcessInfo.processInfo.environment
        let shell = URL(fileURLWithPath: inherited["SHELL"] ?? "/bin/zsh")
        let command = "printf '\(marker)'; env -0"
        var base = inherited.filter { baseVariables.contains($0.key) || $0.key.hasPrefix("LC_") }
        base["PATH"] = "/usr/bin:/bin:/usr/sbin:/sbin"

        // Interactive first (PATH is often set in .zshrc), then login-only if the rc files
        // hang or fail. CommandRunner SIGKILLs on timeout: interactive shells ignore SIGTERM.
        for (flags, timeout) in [("-ilc", 4.0), ("-lc", 3.0)] {
            let output = await CommandRunner.run(shell, arguments: [flags, command], environment: base, timeout: timeout)
            if let environment = parse(Data(output.stdout.utf8)) { return withoutSessionVariables(environment) }
        }
        return withoutSessionVariables(inherited)
    }

    static func withoutSessionVariables(_ environment: [String: String]) -> [String: String] {
        environment.filter { key, _ in
            !sessionVariables.contains(key) && !key.hasPrefix("CLAUDE_CODE_SESSION") && !key.hasPrefix("CLAUDE_CODE_MESSAGING")
        }
    }

    static func parse(_ data: Data) -> [String: String]? {
        let text = String(decoding: data, as: UTF8.self)
        guard let range = text.range(of: marker) else { return nil }
        var environment: [String: String] = [:]
        for entry in text[range.upperBound...].split(separator: "\0") {
            guard let equals = entry.firstIndex(of: "=") else { continue }
            environment[String(entry[..<equals])] = String(entry[entry.index(after: equals)...])
        }
        return environment.isEmpty ? nil : environment
    }

    /// Resolves an executable name against `PATH`, plus common install locations.
    public static func which(_ name: String, in environment: [String: String]) -> URL? {
        let home = environment["HOME"] ?? NSHomeDirectory()
        let paths = (environment["PATH"] ?? "").split(separator: ":").map(String.init)
            + ["\(home)/.local/bin", "\(home)/.claude/local", "/opt/homebrew/bin", "/usr/local/bin"]
        return paths
            .map { URL(fileURLWithPath: $0).appendingPathComponent(name) }
            .first { FileManager.default.isExecutableFile(atPath: $0.path) }
    }
}
