import AppKit
import SwiftUI

/// Development: `YH_MCP_DESIGNS=<folder>` draws mockups of the planned MCP views (see
/// docs/mcp.md) into PNGs there, light and dark, off screen, then quits. Fake servers, no
/// session: for deciding how it should look before building it.
@MainActor
enum MCPDesigns {
    static func runIfRequested() {
        guard let folder = ProcessInfo.processInfo.environment["YH_MCP_DESIGNS"], !folder.isEmpty else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) {
            try? FileManager.default.createDirectory(atPath: folder, withIntermediateDirectories: true)
            for design in designs {
                for dark in [false, true] {
                    render(design.view, width: design.width, dark: dark, canvas: design.canvas,
                           to: "\(folder)/\(design.name)-\(dark ? "dark" : "light").png")
                }
            }
            NSApp.terminate(nil)
        }
    }

    private struct Design {
        let name: String
        let width: CGFloat
        var canvas = false
        let view: AnyView
    }

    private static var designs: [Design] {
        [
            Design(name: "A-sheet", width: 600, view: AnyView(SheetDesign())),
            Design(name: "B-sidebar", width: 340, canvas: true, view: AnyView(SidebarDesign())),
            Design(name: "C-composer-popover", width: 720, canvas: true, view: AnyView(ComposerDesign())),
            Design(name: "D1-detail-connected", width: 600, view: AnyView(DetailDesign(server: MockServer.github))),
            Design(name: "D2-detail-sign-in", width: 600, view: AnyView(DetailDesign(server: MockServer.linear))),
            Design(name: "D3-detail-failed", width: 600, view: AnyView(DetailDesign(server: MockServer.postgres))),
            Design(name: "E-add-server", width: 560, view: AnyView(AddServerDesign())),
            Design(name: "E2-add-url", width: 560, view: AnyView(AddURLDesign(step: .typed))),
            Design(name: "E3-add-url-signing-in", width: 560, view: AnyView(AddURLDesign(step: .signingIn))),
            Design(name: "G-sign-in-in-thread", width: 720, canvas: true, view: AnyView(InThreadSignInDesign())),
            Design(name: "H-settings-everywhere", width: 680, view: AnyView(SettingsDesign())),
            Design(name: "F-project-approval", width: 720, canvas: true, view: AnyView(ApprovalDesign())),
        ]
    }

    private static func render(_ view: AnyView, width: CGFloat, dark: Bool, canvas: Bool, to path: String) {
        let root = view
            .frame(width: width)
            .background(canvas ? AnyShapeStyle(.canvas) : AnyShapeStyle(.clear))
            .background(Color(nsColor: .windowBackgroundColor))
            .environment(\.colorScheme, dark ? .dark : .light)
            .environment(\.controlActiveState, .key)
        let hosting = NSHostingView(rootView: root)
        let size = hosting.fittingSize
        hosting.frame = CGRect(origin: .zero, size: size)
        let window = KeyLookingWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: size.width, height: size.height),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
        window.contentView = hosting
        hosting.layoutSubtreeIfNeeded()
        window.displayIfNeeded()
        guard let bitmap = hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds) else { return }
        hosting.cacheDisplay(in: hosting.bounds, to: bitmap)
        let written = (try? bitmap.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))) != nil
        FileHandle.standardError.write(Data("mcp designs: \(written ? "wrote" : "couldn't write") \(path)\n".utf8))
        window.close()
    }
}

/// Drawn as the key window would be: blue switches and default buttons, not the greyed
/// look of a window in the background.
private final class KeyLookingWindow: NSWindow {
    override var isKeyWindow: Bool { true }
    override var isMainWindow: Bool { true }
}

// MARK: - Fake servers

private struct MockTool: Hashable {
    let name: String
    var readOnly = false
    var destructive = false
}

private struct MockServer: Identifiable {
    enum Status {
        case connected, needsSignIn, failed(String), off, connecting
    }

    enum Scope: String {
        case project = "This workspace · shared, in .mcp.json"
        case local = "This workspace · just you"
        case user = "Everywhere"
        case claudeAI = "claude.ai connectors"
        case session = "This thread only"
    }

    let name: String
    let status: Status
    let transport: String
    let scope: Scope
    let detail: String
    var version: String?
    var tools: [MockTool] = []
    var env: [(String, String)] = []
    var id: String { name }

    static let github = MockServer(
        name: "github", status: .connected, transport: "stdio", scope: .user,
        detail: "npx -y @modelcontextprotocol/server-github", version: "0.6.2",
        tools: [
            MockTool(name: "search_repositories", readOnly: true), MockTool(name: "get_file_contents", readOnly: true),
            MockTool(name: "list_issues", readOnly: true), MockTool(name: "get_pull_request", readOnly: true),
            MockTool(name: "create_issue"), MockTool(name: "create_pull_request"), MockTool(name: "add_issue_comment"),
            MockTool(name: "push_files"), MockTool(name: "create_branch"), MockTool(name: "merge_pull_request", destructive: true),
            MockTool(name: "delete_branch", destructive: true), MockTool(name: "update_issue"),
        ],
        env: [("GITHUB_TOKEN", "••••••••••••ab3f")])
    static let linear = MockServer(name: "linear", status: .needsSignIn, transport: "http", scope: .user,
                                   detail: "https://mcp.linear.app/mcp")
    static let postgres = MockServer(name: "postgres", status: .failed("spawn uvx ENOENT: the command uvx wasn't found"),
                                     transport: "stdio", scope: .project,
                                     detail: "uvx mcp-server-postgres postgresql://localhost/dev")
    static let sentry = MockServer(name: "sentry", status: .connected, transport: "http", scope: .local,
                                   detail: "https://mcp.sentry.dev/mcp", version: "1.4.0",
                                   tools: Array(repeating: MockTool(name: "x", readOnly: true), count: 14))
    static let playwright = MockServer(name: "playwright", status: .off, transport: "stdio", scope: .user,
                                       detail: "npx @playwright/mcp@latest")
    static let drive = MockServer(name: "Google Drive", status: .needsSignIn, transport: "claude.ai", scope: .claudeAI,
                                  detail: "drivemcp.googleapis.com")
    static let docs = MockServer(name: "Claude Docs", status: .connected, transport: "claude.ai", scope: .claudeAI,
                                 detail: "api.anthropic.com", version: "0.1.0",
                                 tools: Array(repeating: MockTool(name: "x"), count: 8))
    static let figma = MockServer(name: "figma", status: .connecting, transport: "http", scope: .project,
                                  detail: "https://mcp.figma.com/mcp")

    static let all = [github, linear, postgres, sentry, playwright, figma, drive, docs]

    var dotColor: Color {
        switch status {
        case .connected: .green
        case .needsSignIn: .orange
        case .failed: .red
        case .off: .secondary.opacity(0.5)
        case .connecting: .yellow
        }
    }

    var statusText: String {
        switch status {
        case .connected: "\(tools.count) tools"
        case .needsSignIn: "Needs sign-in"
        case .failed(let error): error
        case .off: "Off"
        case .connecting: "Connecting…"
        }
    }

    var isOn: Bool { if case .off = status { false } else { true } }
    var needsAttention: Bool {
        switch status {
        case .needsSignIn, .failed: true
        default: false
        }
    }
}

// MARK: - Pieces

private struct StatusDot: View {
    let color: Color
    var body: some View {
        Circle().fill(color).frame(width: 8, height: 8)
    }
}

private struct ServerGlyph: View {
    let server: MockServer
    var size: CGFloat = 28
    var body: some View {
        RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
            .fill(.fill.tertiary)
            .frame(width: size, height: size)
            .overlay {
                Image(systemName: server.scope == .claudeAI ? "cloud" : server.transport == "stdio" ? "terminal" : "globe")
                    .font(.system(size: size * 0.45, weight: .medium))
                    .foregroundStyle(.secondary)
            }
            .overlay(alignment: .bottomTrailing) {
                StatusDot(color: server.dotColor)
                    .padding(1.5)
                    .background(Color(nsColor: .windowBackgroundColor), in: .circle)
                    .offset(x: 3, y: 3)
            }
    }
}

/// The action a row offers, if any: what the server needs from you.
@ViewBuilder
private func rowAction(_ server: MockServer) -> some View {
    switch server.status {
    case .needsSignIn:
        Button("Sign In…") {}.buttonStyle(.borderedProminent).controlSize(.small)
    case .failed:
        Button("Retry") {}.controlSize(.small)
    default:
        EmptyView()
    }
}

private struct ServerRow: View {
    let server: MockServer
    var expanded = false
    var compact = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                ServerGlyph(server: server, size: compact ? 24 : 28)
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Text(server.name).font(.body.weight(.medium))
                        if !compact { Chip(server.transport) }
                    }
                    Text(server.statusText)
                        .font(.caption)
                        .foregroundStyle(server.needsAttention ? AnyShapeStyle(server.dotColor) : AnyShapeStyle(.secondary))
                        .lineLimit(1)
                }
                Spacer(minLength: 8)
                rowAction(server)
                Toggle("", isOn: .constant(server.isOn))
                    .toggleStyle(.switch)
                    .controlSize(.mini)
                    .labelsHidden()
                if !compact {
                    Image(systemName: expanded ? "chevron.down" : "chevron.right")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.tertiary)
                        .frame(width: 12)
                }
            }
            if expanded {
                ToolChips(tools: server.tools)
                    .padding(.leading, 38)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, compact ? 8 : 10)
    }
}

private struct ToolChips: View {
    let tools: [MockTool]
    var body: some View {
        FlowLayout(spacing: 5) {
            ForEach(tools, id: \.self) { tool in
                HStack(spacing: 4) {
                    if tool.destructive {
                        Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
                    } else if tool.readOnly {
                        Image(systemName: "eye").foregroundStyle(.secondary)
                    }
                    Text(tool.name).font(.caption.monospaced())
                }
                .font(.caption2)
                .padding(.horizontal, 7)
                .padding(.vertical, 3)
                .background(.fill.tertiary, in: .capsule)
            }
        }
    }
}

private struct FlowLayout: Layout {
    var spacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let rows = arrange(width: proposal.width ?? .infinity, subviews: subviews)
        return CGSize(width: proposal.width ?? rows.map(\.width).max() ?? 0,
                      height: rows.map(\.height).reduce(0, +) + spacing * CGFloat(max(rows.count - 1, 0)))
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var y = bounds.minY
        for row in arrange(width: bounds.width, subviews: subviews) {
            var x = bounds.minX
            for index in row.indices {
                let size = subviews[index].sizeThatFits(.unspecified)
                subviews[index].place(at: CGPoint(x: x, y: y), proposal: .unspecified)
                x += size.width + spacing
            }
            y += row.height + spacing
        }
    }

    private func arrange(width: CGFloat, subviews: Subviews) -> [(indices: [Int], width: CGFloat, height: CGFloat)] {
        var rows: [(indices: [Int], width: CGFloat, height: CGFloat)] = []
        var current: (indices: [Int], width: CGFloat, height: CGFloat) = ([], 0, 0)
        for index in subviews.indices {
            let size = subviews[index].sizeThatFits(.unspecified)
            if !current.indices.isEmpty, current.width + spacing + size.width > width {
                rows.append(current)
                current = ([], 0, 0)
            }
            current.width += (current.indices.isEmpty ? 0 : spacing) + size.width
            current.height = max(current.height, size.height)
            current.indices.append(index)
        }
        if !current.indices.isEmpty { rows.append(current) }
        return rows
    }
}

private struct MCPSectionLabel: View {
    let text: String
    var body: some View {
        Text(text)
            .font(.caption.weight(.semibold))
            .foregroundStyle(.secondary)
    }
}

private struct ServerCard: View {
    let servers: [MockServer]
    var expanded: String?
    var compact = false
    var body: some View {
        VStack(spacing: 0) {
            ForEach(servers) { server in
                ServerRow(server: server, expanded: server.name == expanded, compact: compact)
                if server.id != servers.last?.id { Divider().padding(.leading, compact ? 46 : 50) }
            }
        }
        .card()
    }
}

private func grouped(_ servers: [MockServer]) -> [(MockServer.Scope, [MockServer])] {
    let order: [MockServer.Scope] = [.project, .local, .user, .claudeAI, .session]
    return order.compactMap { scope in
        let matching = servers.filter { $0.scope == scope }
        return matching.isEmpty ? nil : (scope, matching)
    }
}

// MARK: - A: the /mcp sheet

private struct SheetDesign: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(alignment: .firstTextBaseline) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("MCP Servers").font(.headline)
                    Text("What Claude can reach in acme-workspace, in every repo and worktree under it. Switches apply to this workspace only.")
                        .font(.callout).foregroundStyle(.secondary)
                }
                Spacer()
                HStack(spacing: 6) {
                    Chip("4 on", tint: .green)
                    Chip("2 need you", tint: .orange)
                }
            }
            ForEach(grouped(MockServer.all), id: \.0) { scope, servers in
                VStack(alignment: .leading, spacing: 6) {
                    MCPSectionLabel(text: scope.rawValue)
                    ServerCard(servers: servers, expanded: "github")
                }
            }
            HStack {
                Button("Add Server…", systemImage: "plus") {}
                Spacer()
                Button("Done") {}.keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
    }
}

// MARK: - B: a tab in the right sidebar

private struct SidebarDesign: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Image(systemName: "puzzlepiece.extension")
                Text("MCP").font(.headline)
                Spacer()
                Button("Add", systemImage: "plus") {}.labelStyle(.iconOnly).buttonStyle(.borderless)
            }
            VStack(alignment: .leading, spacing: 8) {
                Label("2 servers need you", systemImage: "exclamationmark.circle.fill")
                    .font(.callout.weight(.medium))
                    .foregroundStyle(.orange)
                Text("Claude can't use Linear or Google Drive until you sign in, or postgres until it starts.")
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .card(tint: .orange)
            ForEach(grouped(MockServer.all), id: \.0) { scope, servers in
                VStack(alignment: .leading, spacing: 6) {
                    MCPSectionLabel(text: scope.rawValue)
                    ServerCard(servers: servers, compact: true)
                }
            }
        }
        .padding(14)
    }
}

// MARK: - C: a composer chip that opens a popover

private struct ComposerDesign: View {
    var body: some View {
        VStack(alignment: .trailing, spacing: 8) {
            // The popover, open over the transcript.
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    Text("MCP Servers").font(.headline)
                    Spacer()
                    Button("Manage…") {}.controlSize(.small)
                }
                VStack(spacing: 0) {
                    ForEach(MockServer.all.filter(\.needsAttention) + [MockServer.github, MockServer.sentry, MockServer.docs, MockServer.playwright]) { server in
                        ServerRow(server: server, compact: true)
                    }
                }
                .card()
            }
            .padding(14)
            .frame(width: 380)
            .background(Color(nsColor: .windowBackgroundColor), in: RoundedRectangle(cornerRadius: Theme.floatingRadius, style: .continuous))
            .overlay { RoundedRectangle(cornerRadius: Theme.floatingRadius, style: .continuous).strokeBorder(.separator) }
            .shadow(color: .black.opacity(0.18), radius: 16, y: 6)
            .padding(.trailing, 60)

            // The composer, roughly.
            VStack(alignment: .leading, spacing: 10) {
                Text("Ask Claude to…").foregroundStyle(.tertiary)
                    .frame(maxWidth: .infinity, minHeight: 36, alignment: .topLeading)
                HStack(spacing: 6) {
                    Chip("Opus 5.5")
                    Chip("High")
                    Chip("Ask before edits")
                    Spacer()
                    HStack(spacing: 4) {
                        Image(systemName: "puzzlepiece.extension")
                        Text("2 need sign-in")
                    }
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.orange)
                    .padding(.horizontal, 8).padding(.vertical, 3)
                    .background(.orange.opacity(0.14), in: .capsule)
                    Image(systemName: "arrow.up.circle.fill").font(.title2).foregroundStyle(.tint)
                }
            }
            .padding(12)
            .card(radius: Theme.floatingRadius)
        }
        .padding(20)
    }
}

// MARK: - D: one server

private struct DetailDesign: View {
    let server: MockServer

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 6) {
                Image(systemName: "chevron.left")
                Text("MCP Servers")
            }
            .font(.callout).foregroundStyle(.tint)

            HStack(spacing: 12) {
                ServerGlyph(server: server, size: 40)
                VStack(alignment: .leading, spacing: 3) {
                    Text(server.name).font(.title3.weight(.semibold))
                    HStack(spacing: 6) {
                        StatusDot(color: server.dotColor)
                        Text(headline).font(.callout).foregroundStyle(.secondary)
                    }
                }
                Spacer()
                Toggle("", isOn: .constant(server.isOn)).toggleStyle(.switch).labelsHidden()
            }

            switch server.status {
            case .needsSignIn: signIn
            case .failed(let error): failure(error)
            default: EmptyView()
            }

            VStack(alignment: .leading, spacing: 8) {
                fact("Available in", server.scope.rawValue)
                fact(server.transport == "stdio" ? "Command" : "Address", server.detail, mono: true)
                ForEach(server.env, id: \.0) { fact($0.0, $0.1, mono: true) }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .card()

            if !server.tools.isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    HStack {
                        MCPSectionLabel(text: "\(server.tools.count) TOOLS")
                        Spacer()
                        Label("reads only", systemImage: "eye").font(.caption2).foregroundStyle(.secondary)
                        Label("can't be undone", systemImage: "exclamationmark.triangle.fill").font(.caption2).foregroundStyle(.orange)
                    }
                    ToolChips(tools: server.tools)
                }
            }

            HStack {
                Button("Remove Server…", role: .destructive) {}
                Spacer()
                if case .connected = server.status {
                    if server.transport != "stdio" { Button("Sign Out") {} }
                    Button("Reconnect") {}
                }
            }
        }
        .padding(20)
    }

    private var headline: String {
        switch server.status {
        case .connected: "Connected" + (server.version.map { " · v\($0)" } ?? "")
        case .needsSignIn: "Needs sign-in"
        case .failed: "Couldn't start"
        case .off: "Off"
        case .connecting: "Connecting…"
        }
    }

    private var signIn: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Sign in to use \(server.name)").font(.callout.weight(.semibold))
            Text("Opens \(URL(string: server.detail)?.host() ?? server.detail) in your browser. Once you've approved it there, it connects here by itself.")
                .font(.callout).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack {
                Button("Sign In in Browser…") {}.buttonStyle(.borderedProminent)
                Spacer()
            }
            DisclosureGroup("Browser didn't come back?") {
                TextField("Paste the address the browser ended on", text: .constant(""))
                    .textFieldStyle(.roundedBorder)
                    .padding(.top, 4)
            }
            .font(.caption)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .card(tint: .orange)
    }

    private func failure(_ error: String) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Couldn't start", systemImage: "xmark.octagon.fill").font(.callout.weight(.semibold)).foregroundStyle(.red)
            Text(error).font(.callout.monospaced()).textSelection(.enabled)
            Text("Install uv (`brew install uv`) or change the command, then Retry.")
                .font(.caption).foregroundStyle(.secondary)
            HStack {
                Button("Retry") {}
                Button("Ask Claude to Fix It") {}
                Spacer()
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .card(tint: .red)
    }

    private func fact(_ label: String, _ value: String, mono: Bool = false) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(label).font(.callout).foregroundStyle(.secondary).frame(width: 110, alignment: .leading)
            Text(value).font(mono ? .callout.monospaced() : .callout).textSelection(.enabled)
            Spacer(minLength: 0)
        }
    }
}

// MARK: - E: adding a server

private struct AddServerDesign: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Add an MCP Server").font(.headline)
                Text("A program on your Mac, or a server's address.")
                    .font(.callout).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Picker("", selection: .constant(0)) {
                Text("Command").tag(0)
                Text("Address").tag(1)
                Text("Paste JSON").tag(2)
            }
            .pickerStyle(.segmented)
            .labelsHidden()

            Form {
                TextField("Name", text: .constant("github"))
                TextField("Command", text: .constant("npx -y @modelcontextprotocol/server-github"))
                    .font(.body.monospaced())
                LabeledContent("Environment") {
                    VStack(alignment: .leading, spacing: 6) {
                        HStack {
                            TextField("", text: .constant("GITHUB_TOKEN")).font(.callout.monospaced())
                            SecureField("", text: .constant("ghp_secret_token_here"))
                            Button("", systemImage: "minus.circle") {}.buttonStyle(.borderless)
                        }
                        Button("Add Variable", systemImage: "plus") {}.buttonStyle(.borderless).font(.callout)
                    }
                }
            }
            .formStyle(.columns)

            VStack(alignment: .leading, spacing: 8) {
                MCPSectionLabel(text: "AVAILABLE IN")
                scopeOption("This workspace, just you", "acme-workspace and every repo and worktree in it. Kept by YourHarness, secrets in your Keychain.", selected: true)
                scopeOption("This workspace, for everyone", "Written to .mcp.json in the workspace repo, to commit and share. Secrets stay out: each person is asked for their own.", selected: false)
                scopeOption("Everywhere", "Every project and workspace. Claude Code in the terminal sees it too.", selected: false)
            }

            HStack {
                Button("Import from Claude Desktop…") {}.buttonStyle(.link)
                Spacer()
                Button("Cancel", role: .cancel) {}
                Button("Add Server") {}.keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
    }

    private func scopeOption(_ title: String, _ detail: String, selected: Bool) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: selected ? "largecircle.fill.circle" : "circle")
                .foregroundStyle(selected ? AnyShapeStyle(.tint) : AnyShapeStyle(.secondary))
            VStack(alignment: .leading, spacing: 1) {
                Text(title).font(.callout)
                Text(detail).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

// MARK: - E2: adding a server by its URL alone

private struct AddURLDesign: View {
    enum Step { case typed, signingIn }
    let step: Step

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Add an MCP Server").font(.headline)
                Text("Paste the server's URL. If it needs you to sign in, your browser opens for it, with your company's sign-in if it uses one.")
                    .font(.callout).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            TextField("", text: .constant("https://mcp.acme-corp.com/mcp"), prompt: Text("https://…"))
                .font(.body.monospaced())
                .textFieldStyle(.roundedBorder)
                .controlSize(.large)

            HStack(spacing: 8) {
                Text("Name").foregroundStyle(.secondary)
                TextField("", text: .constant("acme-corp")).textFieldStyle(.roundedBorder).frame(width: 180)
                Spacer()
                Text("For").foregroundStyle(.secondary)
                Picker("", selection: .constant(0)) {
                    Text("This workspace, just you").tag(0)
                    Text("This workspace, for everyone").tag(1)
                    Text("Everywhere").tag(2)
                }
                .labelsHidden()
                .fixedSize()
            }
            .font(.callout)

            switch step {
            case .typed:
                DisclosureGroup("Advanced") {
                    VStack(alignment: .leading, spacing: 6) {
                        Text("Only if the server's docs ask for them.").font(.caption).foregroundStyle(.secondary)
                        TextField("OAuth client ID", text: .constant(""))
                        SecureField("OAuth client secret", text: .constant(""))
                        TextField("Callback port", text: .constant(""))
                        TextField("Header, e.g. Authorization: Bearer …", text: .constant(""))
                    }
                    .textFieldStyle(.roundedBorder)
                    .padding(.top, 6)
                }
                .font(.callout)
            case .signingIn:
                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("Waiting for you to sign in in your browser…").font(.callout.weight(.medium))
                    }
                    Text("Signing in at sso.acme-corp.com. This connects by itself once you're done there.")
                        .font(.caption).foregroundStyle(.secondary)
                    HStack {
                        Button("Open Again") {}.controlSize(.small)
                        Button("Copy Link") {}.controlSize(.small)
                        Spacer()
                        Button("Browser didn't come back?") {}.buttonStyle(.link).font(.caption)
                    }
                }
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .card()
            }

            HStack {
                Button("Command or JSON instead…") {}.buttonStyle(.link)
                Spacer()
                Button("Cancel", role: .cancel) {}
                if step == .typed {
                    Button("Add and Sign In") {}.buttonStyle(.borderedProminent)
                }
            }
        }
        .padding(20)
    }
}

// MARK: - G: Claude asks you to sign in, in the thread

private struct InThreadSignInDesign: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Find the open Linear issues assigned to me and draft a plan for this week.")
                .padding(.horizontal, 12).padding(.vertical, 8)
                .background(.fill.tertiary, in: RoundedRectangle(cornerRadius: Theme.cardRadius, style: .continuous))
                .frame(maxWidth: .infinity, alignment: .trailing)
            Text("Linear isn't signed in yet, so I've asked you to sign in. I'll carry on as soon as it's connected.")
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 10) {
                    ServerGlyph(server: MockServer.linear, size: 32)
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Sign in to linear").font(.headline)
                        Text("Claude needs it for this task. Your browser opens at mcp.linear.app, with your company's sign-in if it uses one.")
                            .font(.callout).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                HStack {
                    Toggle("Don't ask again in this thread", isOn: .constant(false)).toggleStyle(.checkbox).font(.caption)
                    Spacer()
                    Button("Not Now") {}
                    Button("Sign In…") {}.buttonStyle(.borderedProminent)
                }
            }
            .padding(14)
            .background(Color(nsColor: .windowBackgroundColor), in: RoundedRectangle(cornerRadius: Theme.floatingRadius, style: .continuous))
            .overlay { RoundedRectangle(cornerRadius: Theme.floatingRadius, style: .continuous).strokeBorder(.orange.opacity(0.5)) }
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("After Sign In: waiting for the browser… then it's connected, with 23 tools, and Claude carries on in this same turn.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .padding(20)
    }
}

// MARK: - H: Settings, servers everywhere and where they're off

private struct SettingsDesign: View {
    private let rows: [(MockServer, String?)] = [
        (MockServer.github, nil),
        (MockServer.linear, "Off in personal-site"),
        (MockServer.playwright, "On only in acme-workspace"),
    ]
    private let workspaces: [(String, Int, Int)] = [("acme-workspace", 3, 2), ("yourharness", 0, 1), ("personal-site", 1, 0)]

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            VStack(alignment: .leading, spacing: 4) {
                Text("MCP Servers").font(.title3.weight(.semibold))
                Text("Servers set up everywhere, and what each workspace adds or turns off.")
                    .font(.callout).foregroundStyle(.secondary)
            }
            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    MCPSectionLabel(text: "Everywhere")
                    Spacer()
                    Button("Add Server…", systemImage: "plus") {}.controlSize(.small)
                }
                VStack(spacing: 0) {
                    ForEach(rows, id: \.0.id) { server, exception in
                        HStack(spacing: 10) {
                            ServerGlyph(server: server, size: 26)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(server.name).font(.body.weight(.medium))
                                Text(server.detail).font(.caption.monospaced()).foregroundStyle(.secondary).lineLimit(1)
                            }
                            Spacer()
                            if let exception { Chip(exception) }
                            Image(systemName: "chevron.right").font(.caption.weight(.semibold)).foregroundStyle(.tertiary)
                        }
                        .padding(.horizontal, 12).padding(.vertical, 9)
                        if server.id != rows.last?.0.id { Divider().padding(.leading, 48) }
                    }
                }
                .card()
            }
            VStack(alignment: .leading, spacing: 6) {
                MCPSectionLabel(text: "Workspaces")
                VStack(spacing: 0) {
                    ForEach(workspaces, id: \.0) { name, own, shared in
                        HStack(spacing: 10) {
                            Image(systemName: "square.stack.3d.up").foregroundStyle(.secondary).frame(width: 26)
                            Text(name).font(.body.weight(.medium))
                            Spacer()
                            if own > 0 { Chip("\(own) just you") }
                            if shared > 0 { Chip("\(shared) in .mcp.json") }
                            Image(systemName: "chevron.right").font(.caption.weight(.semibold)).foregroundStyle(.tertiary)
                        }
                        .padding(.horizontal, 12).padding(.vertical, 9)
                        if name != workspaces.last?.0 { Divider().padding(.leading, 48) }
                    }
                }
                .card()
            }
            Toggle("Ask before a workspace's .mcp.json servers start", isOn: .constant(true))
            Toggle("Let Claude ask me to sign in to servers", isOn: .constant(true))
        }
        .padding(24)
    }
}

// MARK: - F: a project's .mcp.json, asked about

private struct ApprovalDesign: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                Image(systemName: "puzzlepiece.extension.fill").font(.title2).foregroundStyle(.orange)
                VStack(alignment: .leading, spacing: 2) {
                    Text("This project wants to start 2 MCP servers").font(.headline)
                    Text("Listed in yourharness/.mcp.json. They run on your Mac with your access, so only allow ones you trust.")
                        .font(.callout).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            VStack(spacing: 0) {
                approvalRow(MockServer.postgres)
                Divider().padding(.leading, 40)
                approvalRow(MockServer.figma)
            }
            .card()
            HStack {
                Text("Your choice is remembered for this project.").font(.caption).foregroundStyle(.secondary)
                Spacer()
                Button("Don't Allow") {}
                Button("Allow Both") {}.buttonStyle(.borderedProminent)
            }
        }
        .padding(16)
        .background(Color(nsColor: .windowBackgroundColor), in: RoundedRectangle(cornerRadius: Theme.floatingRadius, style: .continuous))
        .overlay { RoundedRectangle(cornerRadius: Theme.floatingRadius, style: .continuous).strokeBorder(.separator) }
        .padding(20)
    }

    private func approvalRow(_ server: MockServer) -> some View {
        HStack(spacing: 10) {
            Toggle("", isOn: .constant(true)).toggleStyle(.checkbox).labelsHidden()
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(server.name).font(.body.weight(.medium))
                    Chip(server.transport)
                }
                Text(server.detail).font(.caption.monospaced()).foregroundStyle(.secondary)
            }
            Spacer()
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }
}
