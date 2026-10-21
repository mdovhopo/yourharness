import Foundation
import HarnessCore

/// Local servers a thread started: from what its commands printed, and from the ports
/// that processes under its `claude` are listening on. Never a scan of every port: only
/// what this thread is running.
@MainActor
enum LocalServers {
    private static let pattern = /https?:\/\/(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\]):\d{2,5}[^\s'"`)\]>,]*/

    /// Newest first, one per port.
    static func found(in thread: ThreadModel) -> [String] {
        var seen = Set<Int>(), found: [String] = []
        for port in thread.listeningPorts where seen.insert(port).inserted {
            found.append("http://localhost:\(port)")
        }
        for url in printed(in: thread) {
            guard let port = URL(string: url)?.port, seen.insert(port).inserted else { continue }
            found.append(url)
        }
        return Array(found.prefix(6))
    }

    private static func printed(in thread: ThreadModel) -> [String] {
        var found: [String] = []
        for entry in thread.entries.reversed() where entry.kind == .tool && ["Bash", "BashOutput", "TaskOutput"].contains(entry.toolName) {
            guard let output = entry.toolResult else { continue }
            for match in output.matches(of: pattern) {
                var url = String(match.output).replacingOccurrences(of: "0.0.0.0", with: "localhost")
                while url.hasSuffix(".") { url.removeLast() }
                if URL(string: url)?.port != nil { found.append(url) }
            }
            if found.count >= 6 { break }
        }
        return found
    }

    /// Whether a link points at this Mac, so it opens in the thread's browser.
    static func isLocal(_ url: URL) -> Bool {
        guard ["http", "https"].contains(url.scheme?.lowercased() ?? "") else { return false }
        return ["localhost", "127.0.0.1", "0.0.0.0", "::1", "[::1]"].contains(url.host()?.lowercased() ?? "")
            || url.host()?.hasSuffix(".localhost") == true
    }

    /// TCP ports processes under `root` (the thread's `claude`, and what it started) listen on.
    nonisolated static func listeningPorts(under root: Int32) async -> [Int] {
        let ps = await CommandRunner.run(URL(fileURLWithPath: "/bin/ps"), arguments: ["-A", "-o", "pid=,ppid="],
                                         currentDirectory: URL(fileURLWithPath: "/"))
        var children: [Int32: [Int32]] = [:]
        for line in ps.stdout.split(whereSeparator: \.isNewline) {
            let fields = line.split(separator: " ").compactMap { Int32($0) }
            if fields.count == 2 { children[fields[1], default: []].append(fields[0]) }
        }
        var pids: [Int32] = [], queue = [root]
        while let pid = queue.popLast(), pids.count < 500 {
            pids.append(pid)
            queue += children[pid] ?? []
        }
        let lsof = await CommandRunner.run(URL(fileURLWithPath: "/usr/sbin/lsof"),
                                           arguments: ["-a", "-iTCP", "-sTCP:LISTEN", "-P", "-n", "-Fn", "-p", pids.map(String.init).joined(separator: ",")],
                                           currentDirectory: URL(fileURLWithPath: "/"))
        var ports: [Int] = []
        for line in lsof.stdout.split(whereSeparator: \.isNewline) where line.hasPrefix("n") {
            // n*:3000, n127.0.0.1:8765, n[::1]:5173
            if let port = line.split(separator: ":").last.flatMap({ Int($0) }), !ports.contains(port) { ports.append(port) }
        }
        return ports.sorted()
    }
}
