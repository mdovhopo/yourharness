import Foundation

/// GUI apps launched from Finder/Dock don't inherit the user's shell PATH, so `claude`
/// (and every tool it runs) would be missing. Capture the login shell's environment once.
public enum ShellEnvironment {
    private static let marker = "__YOURHARNESS_ENV__"

    public static func load() async -> [String: String] {
        let fallback = ProcessInfo.processInfo.environment
        let shell = URL(fileURLWithPath: fallback["SHELL"] ?? "/bin/zsh")
        let command = "printf '\(marker)'; env -0"

        // Interactive first (PATH is often set in .zshrc), then login-only if the rc files
        // hang or fail. CommandRunner SIGKILLs on timeout: interactive shells ignore SIGTERM.
        for (flags, timeout) in [("-ilc", 4.0), ("-lc", 3.0)] {
            let output = await CommandRunner.run(shell, arguments: [flags, command], timeout: timeout)
            if let environment = parse(Data(output.stdout.utf8)) { return environment }
        }
        return fallback
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
