import AppKit
import HarnessCore
import SwiftUI

/// `/mcp`: a thread's MCP servers, what each needs from you, and a page per server.
/// See docs/mcp.md for the design.
struct MCPServersSheet: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    let model: MCPServersModel

    var body: some View {
        Group {
            if let server = model.selectedServer {
                MCPServerPage(model: model, server: server) { done in
                    if done { dismiss() } else { model.selected = nil }
                }
            } else {
                list
            }
        }
        .frame(width: 600, height: height)
        .onDisappear { model.close() }
    }

    /// As tall as the list, within reason: the sheet doesn't resize as servers connect.
    private var height: CGFloat {
        guard model.selected == nil, model.loaded, !model.servers.isEmpty else { return model.selected == nil ? 360 : 580 }
        let groups = Set(model.servers.map(\.place)).count
        return min(640, max(360, 150 + CGFloat(groups) * 38 + CGFloat(model.servers.count) * 54))
    }

    private var projectName: String {
        app.project(id: model.thread.projectID)?.name ?? "this project"
    }

    private var list: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("MCP Servers").font(.headline)
                    Text("What Claude can reach in \(projectName). Changes apply right away.")
                        .font(.callout).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer()
                summary
            }
            .padding([.horizontal, .top], 20)
            .padding(.bottom, 12)

            content
                .frame(maxWidth: .infinity, maxHeight: .infinity)

            Divider()
            HStack {
                Spacer()
                Button("Done") { dismiss() }
                    .keyboardShortcut(.defaultAction)
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
        }
    }

    @ViewBuilder private var summary: some View {
        let on = model.servers.filter { $0.state == .connected }.count
        let needing = model.servers.filter(\.state.needsAttention).count
        HStack(spacing: 6) {
            if on > 0 { Chip("\(on) on", tint: .green) }
            if needing > 0 { Chip("\(needing) need\(needing == 1 ? "s" : "") you", tint: .orange) }
        }
    }

    @ViewBuilder private var content: some View {
        if !model.loaded {
            VStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Asking Claude Code…").font(.callout).foregroundStyle(.secondary)
            }
        } else if let failure = model.failure, model.servers.isEmpty {
            ContentUnavailableView {
                Label("Couldn't Read Your MCP Servers", systemImage: "exclamationmark.triangle")
            } description: {
                Text(failure)
            } actions: {
                Button("Try Again") { Task { await model.refresh() } }
            }
        } else if model.servers.isEmpty {
            ContentUnavailableView {
                Label("No MCP Servers", systemImage: "puzzlepiece.extension")
            } description: {
                Text("Servers you add with `claude mcp add`, in a project's .mcp.json or as claude.ai connectors show up here.")
            }
        } else {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    ForEach(MCPServerStatus.Place.allCases, id: \.self) { place in
                        let servers = model.servers.filter { $0.place == place }
                        if !servers.isEmpty {
                            VStack(alignment: .leading, spacing: 6) {
                                Text(place.title)
                                    .font(.caption.weight(.semibold))
                                    .foregroundStyle(.secondary)
                                VStack(spacing: 0) {
                                    ForEach(servers) { server in
                                        MCPServerRow(model: model, server: server)
                                        if server.id != servers.last?.id { Divider().padding(.leading, 50) }
                                    }
                                }
                                .card()
                            }
                        }
                    }
                }
                .padding(.horizontal, 20)
                .padding(.bottom, 16)
            }
        }
    }
}

private struct MCPServerRow: View {
    let model: MCPServersModel
    let server: MCPServerStatus

    var body: some View {
        HStack(spacing: 10) {
            MCPServerGlyph(server: server)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(server.displayName).font(.body.weight(.medium)).lineLimit(1)
                    if let transport = server.transport, transport != "claudeai-proxy" { Chip(transport) }
                }
                MCPStatusLine(model: model, server: server)
            }
            Spacer(minLength: 8)
            if model.busy.contains(server.name) {
                ProgressView().controlSize(.small)
            } else {
                switch server.state {
                case .needsSignIn:
                    Button("Sign In…") { model.signIn(server.name) }
                        .buttonStyle(.borderedProminent).controlSize(.small)
                case .failed:
                    Button("Retry") { model.reconnect(server.name) }.controlSize(.small)
                default:
                    EmptyView()
                }
            }
            MCPServerSwitch(model: model, server: server)
            Image(systemName: "chevron.right")
                .font(.caption.weight(.semibold))
                .foregroundStyle(.tertiary)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .contentShape(.rect)
        .onTapGesture { model.selected = server.name }
    }
}

/// What a server is doing, in a line: its tools, what it needs, or why it failed.
private struct MCPStatusLine: View {
    let model: MCPServersModel
    let server: MCPServerStatus

    var body: some View {
        if let error = model.errors[server.name] {
            Text(error).font(.caption).foregroundStyle(.red).lineLimit(1)
        } else if model.signIns[server.name] != nil {
            Text("Waiting for you in the browser…").font(.caption).foregroundStyle(.orange)
        } else {
            Text(text).font(.caption).foregroundStyle(color).lineLimit(1)
        }
    }

    private var text: String {
        switch server.state {
        case .connected: server.tools.isEmpty ? "Connected" : "\(server.tools.count) tool\(server.tools.count == 1 ? "" : "s")"
        case .needsSignIn: "Needs sign-in"
        case .failed: server.error ?? "Couldn't start"
        case .disabled: "Off"
        case .connecting: "Connecting…"
        case .other(let state): state
        }
    }

    private var color: Color {
        switch server.state {
        case .needsSignIn: .orange
        case .failed: .red
        default: .secondary
        }
    }
}

private struct MCPServerSwitch: View {
    let model: MCPServersModel
    let server: MCPServerStatus

    var body: some View {
        Toggle("", isOn: Binding(get: { server.state != .disabled }, set: { model.setEnabled(server.name, $0) }))
            .toggleStyle(.switch)
            .controlSize(.mini)
            .labelsHidden()
            .disabled(model.busy.contains(server.name))
            .help(server.state == .disabled ? "Turn on for this project" : "Turn off for this project")
    }
}

struct MCPServerGlyph: View {
    let server: MCPServerStatus
    var size: CGFloat = 28

    var body: some View {
        RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
            .fill(.fill.tertiary)
            .frame(width: size, height: size)
            .overlay {
                Image(systemName: symbol)
                    .font(.system(size: size * 0.45, weight: .medium))
                    .foregroundStyle(.secondary)
            }
            .overlay(alignment: .bottomTrailing) {
                Circle().fill(server.state.color).frame(width: 8, height: 8)
                    .padding(1.5)
                    .background(Color(nsColor: .windowBackgroundColor), in: .circle)
                    .offset(x: 3, y: 3)
            }
    }

    private var symbol: String {
        if server.place == .claudeAI { return "cloud" }
        return server.isRemote ? "globe" : "terminal"
    }
}

extension MCPServerStatus.State {
    var color: Color {
        switch self {
        case .connected: .green
        case .needsSignIn: .orange
        case .failed: .red
        case .connecting: .yellow
        case .disabled, .other: .secondary.opacity(0.5)
        }
    }
}

// MARK: - A server's page

private struct MCPServerPage: View {
    @Environment(AppModel.self) private var app
    let model: MCPServersModel
    let server: MCPServerStatus
    /// true: close the sheet too.
    let back: (_ done: Bool) -> Void
    @State private var pastedAddress = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    Button { back(false) } label: {
                        Label("MCP Servers", systemImage: "chevron.left")
                    }
                    .buttonStyle(.link)

                    header

                    if server.state == .needsSignIn || model.signIns[server.name] != nil { signIn }
                    if server.state == .failed { failure }
                    if let error = model.errors[server.name] {
                        Label(error, systemImage: "exclamationmark.triangle.fill")
                            .font(.callout).foregroundStyle(.red)
                    }

                    facts

                    if !server.tools.isEmpty { tools }
                }
                .padding(20)
            }
            Divider()
            HStack {
                Spacer()
                if server.state == .connected {
                    if server.isRemote { Button("Sign Out") { model.signOut(server.name) } }
                    Button("Reconnect") { model.reconnect(server.name) }
                }
                Button("Done") { back(true) }.keyboardShortcut(.defaultAction)
            }
            .disabled(model.busy.contains(server.name))
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
        }
    }

    private var header: some View {
        HStack(spacing: 12) {
            MCPServerGlyph(server: server, size: 40)
            VStack(alignment: .leading, spacing: 3) {
                Text(server.displayName).font(.title3.weight(.semibold))
                HStack(spacing: 6) {
                    Circle().fill(server.state.color).frame(width: 8, height: 8)
                    Text(headline).font(.callout).foregroundStyle(.secondary)
                }
            }
            Spacer()
            if model.busy.contains(server.name) { ProgressView().controlSize(.small) }
            MCPServerSwitch(model: model, server: server)
                .controlSize(.regular)
        }
    }

    private var headline: String {
        switch server.state {
        case .connected: "Connected" + (server.serverVersion.map { " · v\($0)" } ?? "")
        case .needsSignIn: "Needs sign-in"
        case .failed: "Couldn't start"
        case .disabled: "Off in this project"
        case .connecting: "Connecting…"
        case .other(let state): state
        }
    }

    @ViewBuilder private var signIn: some View {
        let host = server.target.flatMap { URL(string: $0)?.host() }
        VStack(alignment: .leading, spacing: 10) {
            if let pending = model.signIns[server.name] {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Waiting for you to sign in in your browser…").font(.callout.weight(.medium))
                }
                Text("This connects by itself once you're done there.")
                    .font(.caption).foregroundStyle(.secondary)
                HStack {
                    Button("Open Again") { model.openSignInAgain(server.name) }.controlSize(.small)
                    if let url = pending.url {
                        Button("Copy Link") {
                            NSPasteboard.general.clearContents()
                            NSPasteboard.general.setString(url.absoluteString, forType: .string)
                        }
                        .controlSize(.small)
                    }
                    Button("Cancel") { model.cancelSignIn(server.name) }.controlSize(.small)
                    Spacer()
                }
                DisclosureGroup("Browser didn't come back?") {
                    VStack(alignment: .leading, spacing: 6) {
                        Text("Paste the address the browser ended on, even if the page didn't load.")
                            .font(.caption).foregroundStyle(.secondary)
                        HStack {
                            TextField("http://localhost:…/callback?code=…", text: $pastedAddress)
                                .textFieldStyle(.roundedBorder)
                                .font(.callout.monospaced())
                                .onSubmit { model.finishSignIn(server.name, callbackURL: pastedAddress) }
                            Button("Finish") { model.finishSignIn(server.name, callbackURL: pastedAddress) }
                                .disabled(pastedAddress.isEmpty)
                        }
                    }
                    .padding(.top, 4)
                }
                .font(.caption)
            } else {
                Text("Sign in to use \(server.displayName)").font(.callout.weight(.semibold))
                Text("Opens \(host ?? "the server's sign-in page") in your browser, with your company's sign-in if it uses one. Once you've approved it there, it connects here by itself.")
                    .font(.callout).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Button("Sign In in Browser…") { model.signIn(server.name) }
                    .buttonStyle(.borderedProminent)
                    .disabled(model.busy.contains(server.name))
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .card(tint: .orange)
    }

    private var failure: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Couldn't start", systemImage: "xmark.octagon.fill")
                .font(.callout.weight(.semibold)).foregroundStyle(.red)
            if let error = server.error {
                Text(error).font(.callout.monospaced()).textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                Button("Retry") { model.reconnect(server.name) }
                Button("Ask Claude to Fix It") {
                    model.thread.draft = "The MCP server \"\(server.name)\" won't start"
                        + (server.target.map { " (`\($0)`)" } ?? "")
                        + (server.error.map { ": \($0)" } ?? ".")
                        + " Find out why and fix it."
                    back(true)
                }
                Spacer()
            }
            .disabled(model.busy.contains(server.name))
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .card(tint: .red)
    }

    private var facts: some View {
        VStack(alignment: .leading, spacing: 8) {
            fact("Set up in", server.place.title)
            if let target = server.target {
                fact(server.isRemote ? "Address" : "Command", target, mono: true)
            }
            if let name = server.serverName, name != server.name {
                fact("Server", name + (server.serverVersion.map { " \($0)" } ?? ""))
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .card()
    }

    private func fact(_ label: String, _ value: String, mono: Bool = false) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(label).font(.callout).foregroundStyle(.secondary).frame(width: 90, alignment: .leading)
            Text(value).font(mono ? .callout.monospaced() : .callout).textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
    }

    private var tools: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("\(server.tools.count) TOOL\(server.tools.count == 1 ? "" : "S")")
                    .font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                Spacer()
                if server.tools.contains(where: \.readOnly) {
                    Label("only reads", systemImage: "eye").font(.caption2).foregroundStyle(.secondary)
                }
                if server.tools.contains(where: \.destructive) {
                    Label("can't be undone", systemImage: "exclamationmark.triangle.fill").font(.caption2).foregroundStyle(.orange)
                }
            }
            FlowLayout(spacing: 5) {
                ForEach(server.tools, id: \.self) { tool in
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
}

/// Lays its children out in rows, wrapping when the next doesn't fit.
struct FlowLayout: Layout {
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

// MARK: - Claude asks you to sign in

/// Above the composer, as an approval is: Claude needs a server you haven't signed in to.
struct MCPSignInCard: View {
    let request: MCPSignInRequest
    /// Off only where glass can't be drawn (the mockups, drawn off screen).
    var glass = true

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            MCPServerGlyph(server: request.server, size: 32)
            VStack(alignment: .leading, spacing: 4) {
                Text(request.page == nil ? "Sign in to \(request.server.displayName)?" : "Waiting for you to sign in to \(request.server.displayName)…")
                    .font(.headline)
                Text(detail)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            HStack(spacing: 8) {
                if let page = request.page {
                    ProgressView().controlSize(.small)
                    Button("Open Again") { MCPServersModel.openInBrowser(page) }
                    Button("Cancel") { request.cancel() }
                } else if request.decided {
                    ProgressView().controlSize(.small)
                } else {
                    Button("Not Now") { request.decide(false) }
                        .keyboardShortcut(.delete, modifiers: .command)
                        .help("Not Now (⌘⌫)")
                    Button("Sign In…") { request.decide(true) }
                        .keyboardShortcut(.return, modifiers: .command)
                        .buttonStyle(.glassProminent)
                        .help("Sign In (⌘↩)")
                }
            }
            .controlSize(.large)
        }
        .padding(14)
        .background {
            if glass {
                Color.clear.glassEffect(.regular.tint(.orange.opacity(0.12)), in: .rect(cornerRadius: Theme.floatingRadius))
            } else {
                Color.clear.card(radius: Theme.floatingRadius, tint: .orange)
            }
        }
    }

    private var detail: String {
        if request.page != nil {
            return "Finish in your browser; Claude carries on once it's connected."
        }
        let host = request.server.target.flatMap { URL(string: $0)?.host() }
        let need = request.reason.map { "Claude needs it to \($0.prefix(1).lowercased() + $0.dropFirst())" } ?? "Claude needs it for this task"
        return need.trimmingCharacters(in: CharacterSet(charactersIn: ".")) + ". Your browser opens"
            + (host.map { " at \($0)" } ?? "") + ", with your company's sign-in if it uses one."
    }
}
