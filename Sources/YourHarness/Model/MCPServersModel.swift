import AppKit
import HarnessCore

/// What the `/mcp` sheet shows and does for one thread: its MCP servers, as Claude Code
/// reports them, kept current while the sheet is open (see docs/mcp.md).
///
/// It asks the thread's own session when it has one, so changes apply to it at once.
/// Otherwise it starts a Claude Code process of its own in the thread's folder, with no
/// conversation, and keeps it while the sheet is open: a sign-in's browser comes back to it.
@MainActor @Observable
final class MCPServersModel: Identifiable {
    let thread: ThreadModel
    nonisolated let id: UUID
    @ObservationIgnored private unowned let app: AppModel

    private(set) var servers: [MCPServerStatus] = []
    private(set) var loaded = false
    /// Why the list couldn't be read.
    private(set) var failure: String?
    /// Servers with a request on the way.
    private(set) var busy: Set<String> = []
    /// What went wrong with a server's last action.
    private(set) var errors: [String: String] = [:]
    /// Sign-ins waiting for the browser, by server.
    private(set) var signIns: [String: MCPSignIn] = [:]
    /// The server whose page is open.
    var selected: String?

    @ObservationIgnored private var ownSession: ClaudeSession?
    @ObservationIgnored private var polling: Task<Void, Never>?

    init(thread: ThreadModel, app: AppModel) {
        self.thread = thread
        self.id = thread.id
        self.app = app
    }

    var selectedServer: MCPServerStatus? { selected.flatMap { name in servers.first { $0.name == name } } }

    func open() {
        guard polling == nil else { return }
        polling = Task { [weak self] in
            while !Task.isCancelled {
                await self?.refresh()
                // Quicker while something is still connecting or a sign-in is out.
                let waiting = self.map { model in model.signIns.isEmpty == false || model.servers.contains { $0.state == .connecting } } ?? false
                try? await Task.sleep(for: waiting ? .seconds(1) : .seconds(3))
            }
        }
    }

    func close() {
        polling?.cancel()
        polling = nil
        if let ownSession {
            self.ownSession = nil
            Task { await ownSession.terminate() }
        }
    }

    func refresh() async {
        do {
            let session = try await session()
            // The app's own servers (memory, threads, the browser) aren't yours to manage.
            let servers = try await session.mcpStatus().filter { $0.transport != "sdk" }
            self.servers = servers
            loaded = true
            failure = nil
            // Signed in: the browser came back and the server connected.
            for name in signIns.keys where servers.first(where: { $0.name == name })?.state == .connected {
                signIns[name] = nil
            }
            thread.noteMCPServers(servers)
        } catch {
            failure = (error as? ClaudeControlError)?.description ?? error.localizedDescription
            loaded = true
        }
    }

    // MARK: - Actions

    func setEnabled(_ name: String, _ enabled: Bool) {
        perform(name) { try await $0.setMCPServer(name, enabled: enabled) }
    }

    func reconnect(_ name: String) {
        perform(name) { try await $0.reconnectMCPServer(name) }
    }

    /// Opens the server's sign-in page in the default browser, where your company's sign-in,
    /// passkeys and device checks are; Claude Code hears the browser come back.
    func signIn(_ name: String) {
        perform(name) { [weak self] session in
            let signIn = try await session.signInToMCPServer(name)
            guard let url = signIn.url else { return }
            self?.signIns[name] = signIn
            Self.openInBrowser(url)
        }
    }

    func openSignInAgain(_ name: String) {
        if let url = signIns[name]?.url { Self.openInBrowser(url) }
    }

    /// The address the browser ended on, for when it couldn't reach Claude Code.
    func finishSignIn(_ name: String, callbackURL: String) {
        let address = callbackURL.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !address.isEmpty else { return }
        perform(name) { try await $0.finishMCPSignIn(name, callbackURL: address) }
    }

    /// Development: `YH_MCP_BROWSER=log` prints the page instead, so a test never puts a
    /// browser on the screen.
    private static func openInBrowser(_ url: URL) {
        if ProcessInfo.processInfo.environment["YH_MCP_BROWSER"] == "log" {
            FileHandle.standardError.write(Data("MCP sign-in page: \(url.absoluteString)\n".utf8))
        } else {
            NSWorkspace.shared.open(url)
        }
    }

    func cancelSignIn(_ name: String) {
        signIns[name] = nil
    }

    func signOut(_ name: String) {
        perform(name) { try await $0.signOutOfMCPServer(name) }
    }

    private func perform(_ name: String, _ action: @escaping (ClaudeSession) async throws -> Void) {
        guard !busy.contains(name) else { return }
        busy.insert(name)
        errors[name] = nil
        Task {
            do {
                try await action(try await session())
            } catch {
                errors[name] = (error as? ClaudeControlError)?.description ?? error.localizedDescription
            }
            busy.remove(name)
            await refresh()
        }
    }

    static func openForDevelopment(app: AppModel, thread: ThreadModel) {
        let environment = ProcessInfo.processInfo.environment
        app.openMCPServers(for: thread)
        guard let model = app.mcpServersSheet else { return }
        Task {
            while !model.loaded { try? await Task.sleep(for: .milliseconds(200)) }
            FileHandle.standardError.write(Data("MCP servers: \(model.servers.map { "\($0.name)=\($0.state)" }.joined(separator: ", "))\(model.failure.map { " failure: \($0)" } ?? "")\n".utf8))
            if let name = environment["YH_MCP_SELECT"] { model.selected = name }
            if let name = environment["YH_MCP_SIGN_IN"] { model.signIn(name) }
        }
    }

    // MARK: - Session

    func session() async throws -> ClaudeSession {
        if let live = await thread.liveSession() { return live }
        if let ownSession, await ownSession.isRunning { return ownSession }
        var options = try await app.launchOptions(for: thread)
        // No conversation: this process only answers about servers.
        options.resumeSessionID = nil
        options.persistsSession = false
        let session = try ClaudeSession(options: options)
        try await session.initialize()
        ownSession = session
        return session
    }
}

extension MCPServerStatus {
    /// Where it's set up, for grouping, in the order the sheet lists them.
    enum Place: Int, CaseIterable, Comparable {
        case project, local, user, organization, claudeAI, session, other

        init(scope: String?) {
            switch scope {
            case "project": self = .project
            case "local": self = .local
            case "user": self = .user
            case "enterprise", "managed": self = .organization
            case "claudeai": self = .claudeAI
            case "dynamic": self = .session
            default: self = .other
            }
        }

        var title: String {
            switch self {
            case .project: "This project · shared, in .mcp.json"
            case .local: "This project · just you"
            case .user: "Everywhere"
            case .organization: "Your organization"
            case .claudeAI: "claude.ai connectors"
            case .session: "This app"
            case .other: "Other"
            }
        }

        static func < (a: Place, b: Place) -> Bool { a.rawValue < b.rawValue }
    }

    var place: Place { Place(scope: scope) }

    /// What the user knows it as: claude.ai connectors without their prefix.
    var displayName: String {
        name.hasPrefix("claude.ai ") ? String(name.dropFirst("claude.ai ".count)) : name
    }

    /// Signing in applies: a server on the web.
    var isRemote: Bool { ["http", "sse", "ws", "claudeai-proxy"].contains(transport ?? "") }
}
