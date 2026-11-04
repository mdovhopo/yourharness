import AppKit
import HarnessCore

/// `mcp__harness__sign_in_to_mcp_server`: Claude asks you to sign in to an MCP server it
/// needs. Run from the app, Claude Code has no sign-in of its own (it tells Claude the session
/// can't), so without this Claude can only send you to the terminal. A card in the thread asks;
/// Sign In opens the page in your browser, and the call returns once the server is connected,
/// so Claude carries on in the same turn. See docs/mcp.md.
@MainActor
final class MCPSignInTool {
    nonisolated static let toolName = "mcp__harness__sign_in_to_mcp_server"
    /// How long a sign-in may take in the browser before Claude hears it didn't happen.
    nonisolated static let browserTimeout: Duration = .seconds(300)

    nonisolated static let definition = MCPTool(
        name: "sign_in_to_mcp_server",
        description: """
        Ask the user to sign in to an MCP server that needs authentication before its tools work (Claude Code \
        reports it as needing auth, or says the session is non-interactive and to use /mcp or a terminal). A card \
        in the conversation asks them; signing in opens the server's page in their browser. Returns once the server \
        is connected, so you can use its tools right after (they may arrive as deferred tools to load), or says \
        they declined. Use this instead of telling the user to run /mcp or open a terminal.
        """,
        inputSchema: MCPTool.schema([
            "server": ["type": "string", "description": "The server's name, as in its tools' mcp__<server>__ prefix or as Claude Code lists it."],
            "reason": ["type": "string", "description": "A short line on what you need it for, shown to the user."],
        ], required: ["server"]),
        alwaysLoad: true
    )

    private weak var thread: ThreadModel?

    init(thread: ThreadModel) {
        self.thread = thread
    }

    func signIn(_ args: JSONValue) async -> MCPToolResult {
        guard let thread else { return .error("This thread is gone.") }
        guard let asked = args["server"]?.string?.trimmingCharacters(in: .whitespaces), !asked.isEmpty else {
            return .error("Say which server in `server`.")
        }
        guard let session = await thread.liveSession() else { return .error("This thread has no Claude Code session to sign in through.") }
        let servers: [MCPServerStatus]
        do {
            servers = try await session.mcpStatus()
        } catch {
            return .error("Couldn't read the MCP servers: \(error)")
        }
        guard let server = Self.match(asked, in: servers) else {
            let names = servers.filter { $0.transport != "sdk" }.map { "\($0.name) (\($0.state))" }
            return .error("No MCP server named \"\(asked)\". This session has: \(names.isEmpty ? "none" : names.joined(separator: ", ")).")
        }
        switch server.state {
        case .connected:
            return .text("\(server.name) is already connected; its tools are ready.")
        case .disabled:
            return .error("\(server.name) is turned off for this project. The user can turn it on with /mcp.")
        case .failed:
            return .error("\(server.name) isn't failing for want of a sign-in: \(server.error ?? "it couldn't start"). The user can see it in /mcp.")
        default:
            break
        }

        if ProcessInfo.processInfo.environment["YH_MCP_BROWSER"] == "log" {
            FileHandle.standardError.write(Data("MCP sign-in asked: \(server.name)\n".utf8))
        }
        let request = MCPSignInRequest(server: server, reason: args["reason"]?.string)
        thread.mcpSignIn = request
        defer { if thread.mcpSignIn === request { thread.mcpSignIn = nil } }

        guard await request.decision() else {
            return .text("The user chose not to sign in to \(server.name) now. Carry on without it, or ask them in chat.")
        }
        do {
            let signIn = try await session.signInToMCPServer(server.name)
            if let url = signIn.url {
                request.page = url
                MCPServersModel.openInBrowser(url)
            }
        } catch {
            return .error("Couldn't start signing in to \(server.name): \(error)")
        }
        // Claude Code connects the server once the browser comes back to it.
        let deadline = ContinuousClock.now + Self.browserTimeout
        while ContinuousClock.now < deadline, !request.isCancelled {
            try? await Task.sleep(for: .seconds(1))
            if let now = try? await session.mcpStatus().first(where: { $0.name == server.name }) {
                if now.state == .connected {
                    let tools = now.tools.map(\.name)
                    return .text("Signed in: \(server.name) is connected"
                        + (tools.isEmpty ? "." : ", with \(tools.count) tools (mcp__\(SlashCommand.normalizedName(server.name))__…): \(tools.prefix(20).joined(separator: ", ")).")
                        + " If they aren't in your tool list yet, look them up with tool search.")
                }
                if now.state == .failed {
                    return .error("\(server.name) failed after signing in: \(now.error ?? "unknown error").")
                }
            }
        }
        return .text(request.isCancelled
            ? "The user cancelled signing in to \(server.name). Carry on without it, or ask them in chat."
            : "The sign-in to \(server.name) didn't finish within five minutes. Carry on without it; the user can sign in later with /mcp.")
    }

    /// By Claude Code's name, its normalized form (as in tool names), or without "claude.ai ".
    static func match(_ asked: String, in servers: [MCPServerStatus]) -> MCPServerStatus? {
        let candidates = servers.filter { $0.transport != "sdk" }
        let wanted = asked.hasPrefix("mcp__") ? String(asked.dropFirst(5).split(separator: "__").first ?? "") : asked
        func same(_ a: String, _ b: String) -> Bool { a.caseInsensitiveCompare(b) == .orderedSame }
        return candidates.first { same($0.name, wanted) }
            ?? candidates.first { same(SlashCommand.normalizedName($0.name), wanted) }
            ?? candidates.first { same($0.displayName, wanted) }
    }
}

/// The card's side of a sign-in Claude asked for.
@MainActor @Observable
final class MCPSignInRequest: Identifiable {
    let id = UUID()
    let server: MCPServerStatus
    let reason: String?
    /// Set once Sign In is chosen and Claude Code gave the page: waiting for the browser.
    var page: URL?
    private(set) var decided = false
    private(set) var isCancelled = false
    @ObservationIgnored private var answer: CheckedContinuation<Bool, Never>?
    @ObservationIgnored private var answered: Bool?

    init(server: MCPServerStatus, reason: String?) {
        self.server = server
        self.reason = reason
        // Development: `YH_MCP_SIGN_IN_ANSWER=allow|deny` answers for you.
        switch ProcessInfo.processInfo.environment["YH_MCP_SIGN_IN_ANSWER"] {
        case "allow": decide(true)
        case "deny": decide(false)
        default: break
        }
    }

    func decision() async -> Bool {
        if let answered { return answered }
        return await withCheckedContinuation { answer = $0 }
    }

    func decide(_ signIn: Bool) {
        guard !decided else { return }
        decided = true
        answered = signIn
        answer?.resume(returning: signIn)
        answer = nil
    }

    /// Stops waiting for the browser, or says no if nothing was chosen yet.
    func cancel() {
        isCancelled = true
        decide(false)
    }
}
