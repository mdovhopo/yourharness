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
            if model.isAdding {
                AddMCPServerPage(model: model)
            } else if let server = model.selectedServer {
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
        guard !model.isAdding, model.selected == nil, model.loaded, !model.servers.isEmpty else { return model.selected == nil && !model.isAdding ? 360 : 580 }
        let groups = Set(model.servers.map(\.place)).count
        return min(640, max(360, 150 + CGFloat(groups) * 38 + CGFloat(model.servers.count) * 54))
    }

    private var projectName: String {
        model.thread.flatMap { app.project(id: $0.projectID)?.name } ?? "this project"
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
                Button("Add Server…", systemImage: "plus") { model.isAdding = true }
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
    @State private var confirmingRemove = false
    @State private var removeError: String?

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
                    if let removeError {
                        Label(removeError, systemImage: "exclamationmark.triangle.fill")
                            .font(.callout).foregroundStyle(.red)
                    }
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
                if model.canRemove(server) {
                    Button("Remove Server…", role: .destructive) { confirmingRemove = true }
                        .confirmationDialog("Remove \(server.displayName)?", isPresented: $confirmingRemove) {
                            Button("Remove", role: .destructive) {
                                Task { removeError = await model.remove(server) }
                            }
                        } message: {
                            Text(server.place == .project
                                 ? "It's taken out of this project's .mcp.json, for everyone who uses it."
                                 : "Claude Code forgets it, in the terminal too.")
                        }
                }
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
                    model.thread?.draft = "The MCP server \"\(server.name)\" won't start"
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
                        .prominentButton(glass: glass)
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

// MARK: - A server asks you something

/// Above the composer, as an approval is: an MCP server's tool needs a few fields filled in,
/// or a page visited, before it goes on.
struct MCPElicitationCard: View {
    let elicitation: MCPElicitation
    let answer: (ClaudeSession.ElicitationAnswer) -> Void
    /// Off only where glass can't be drawn (the mockups, drawn off screen).
    var glass = true

    @State private var texts: [String: String] = [:]
    @State private var flags: [String: Bool] = [:]
    @State private var opened = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: elicitation.mode == .url ? "link" : "list.bullet.rectangle")
                    .font(.title3)
                    .foregroundStyle(.orange)
                    .frame(width: 24)
                VStack(alignment: .leading, spacing: 4) {
                    Text(elicitation.title ?? "\(elicitation.server) asks")
                        .font(.headline)
                    if !elicitation.message.isEmpty {
                        Text(elicitation.message)
                            .font(.callout)
                            .fixedSize(horizontal: false, vertical: true)
                            .textSelection(.enabled)
                    }
                    if elicitation.mode == .url, let host = elicitation.url?.host() {
                        Text("Opens \(host) in your browser.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            if elicitation.mode == .form, !elicitation.fields.isEmpty {
                Form {
                    ForEach(elicitation.fields) { field in fieldView(field) }
                }
                .formStyle(.columns)
                .padding(.leading, 36)
            }
            HStack(spacing: 8) {
                Spacer()
                Button("Decline") { answer(.decline) }
                    .keyboardShortcut(.delete, modifiers: .command)
                    .help("Decline (⌘⌫)")
                if elicitation.mode == .url {
                    if opened {
                        Button("Done") { answer(.accept(nil)) }
                            .keyboardShortcut(.return, modifiers: .command)
                            .prominentButton(glass: glass)
                    } else {
                        Button("Open Page…") {
                            if let url = elicitation.url { MCPServersModel.openInBrowser(url) }
                            opened = true
                        }
                        .keyboardShortcut(.return, modifiers: .command)
                        .prominentButton(glass: glass)
                        .disabled(elicitation.url == nil)
                    }
                } else {
                    Button("Send") { answer(.accept(content)) }
                        .keyboardShortcut(.return, modifiers: .command)
                        .prominentButton(glass: glass)
                        .disabled(content == nil)
                        .help("Send (⌘↩)")
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
        .onAppear(perform: fillDefaults)
    }

    @ViewBuilder private func fieldView(_ field: MCPElicitation.Field) -> some View {
        let label = field.title + (field.required ? "" : " (optional)")
        switch field.kind {
        case .boolean:
            Toggle(field.title, isOn: Binding(get: { flags[field.name] ?? false }, set: { flags[field.name] = $0 }))
                .help(field.description ?? "")
        case .choice(let options):
            Picker(label, selection: Binding(get: { texts[field.name] ?? "" }, set: { texts[field.name] = $0 })) {
                if !field.required || texts[field.name, default: ""].isEmpty { Text("—").tag("") }
                ForEach(options, id: \.value) { Text($0.label).tag($0.value) }
            }
            .help(field.description ?? "")
        case .number, .text:
            TextField(label, text: Binding(get: { texts[field.name] ?? "" }, set: { texts[field.name] = $0 }),
                      prompt: field.description.map(Text.init))
                .textFieldStyle(.roundedBorder)
        }
    }

    private func fillDefaults() {
        for field in elicitation.fields {
            switch (field.kind, field.defaultValue) {
            case (.boolean, let value?): flags[field.name] = value.bool ?? false
            case (.number, let value?): texts[field.name] = value.double.map { $0.rounded() == $0 ? String(Int($0)) : String($0) } ?? ""
            case (_, let value?): texts[field.name] = value.string ?? ""
            default: break
            }
        }
    }

    /// What goes back, or nil while a required field is empty or a number isn't one.
    private var content: JSONValue? {
        var values: [String: JSONValue] = [:]
        for field in elicitation.fields {
            switch field.kind {
            case .boolean:
                values[field.name] = .bool(flags[field.name] ?? false)
            case .number(let integer):
                let text = (texts[field.name] ?? "").trimmingCharacters(in: .whitespaces)
                if text.isEmpty {
                    if field.required { return nil }
                    continue
                }
                guard let number = Double(text.replacingOccurrences(of: ",", with: ".")), !integer || number.rounded() == number else { return nil }
                values[field.name] = .number(number)
            case .choice, .text:
                let text = texts[field.name] ?? ""
                if text.isEmpty {
                    if field.required { return nil }
                    continue
                }
                values[field.name] = .string(text)
            }
        }
        return .object(values)
    }
}

private extension View {
    /// Glass where it can be drawn; plain where it can't (the mockups, drawn off screen).
    @ViewBuilder func prominentButton(glass: Bool) -> some View {
        if glass { buttonStyle(.glassProminent) } else { buttonStyle(.borderedProminent) }
    }
}

// MARK: - Adding a server

/// A server's URL first (sign in after, in the browser), or a command, or pasted JSON; and
/// where it applies. Added through Claude Code's own `claude mcp add-json`.
private struct AddMCPServerPage: View {
    let model: MCPServersModel
    /// In Settings: only "Everywhere" makes sense there.
    var everywhereOnly: Bool { model.thread == nil }

    private enum Way: String, CaseIterable { case url = "URL", command = "Command", json = "JSON" }

    @State private var way = Way.url
    @State private var url = ""
    @State private var command = ""
    @State private var json = ""
    @State private var name = ""
    @State private var nameEdited = false
    @State private var scope = MCPScope.user
    @State private var headers: [Pair] = []
    @State private var environment: [Pair] = []
    @State private var clientID = ""
    @State private var clientSecret = ""
    @State private var callbackPort = ""
    @State private var showsAdvanced = false
    @State private var adding = false
    @State private var error: String?
    @State private var desktop: [String: JSONValue] = [:]
    @State private var desktopPicked: Set<String> = []
    @State private var showsDesktop = false

    struct Pair: Identifiable, Equatable {
        let id = UUID()
        var key = ""
        var value = ""
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    Button { model.isAdding = false } label: {
                        Label("MCP Servers", systemImage: "chevron.left")
                    }
                    .buttonStyle(.link)

                    VStack(alignment: .leading, spacing: 4) {
                        Text("Add an MCP Server").font(.headline)
                        Text(way == .url
                             ? "Paste the server's URL. If it needs you to sign in, your browser opens for it, with your company's sign-in if it uses one."
                             : way == .command ? "A program on your Mac that Claude Code starts." : "A server's config, or a whole `mcpServers` block from a README.")
                            .font(.callout).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }

                    Picker("", selection: $way) {
                        ForEach(Way.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .frame(width: 260)

                    switch way {
                    case .url: urlFields
                    case .command: commandFields
                    case .json: jsonField
                    }

                    if !everywhereOnly { scopePicker }

                    if let error {
                        Label(error, systemImage: "exclamationmark.triangle.fill")
                            .font(.callout).foregroundStyle(.red)
                            .fixedSize(horizontal: false, vertical: true)
                    }

                    if showsDesktop { desktopImport }
                }
                .padding(20)
            }
            Divider()
            HStack {
                Button("Import from Claude Desktop…") { loadDesktop() }
                    .buttonStyle(.link)
                Spacer()
                if adding { ProgressView().controlSize(.small) }
                Button("Cancel", role: .cancel) { model.isAdding = false }
                Button(way == .url ? "Add and Sign In" : "Add Server") { add() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(adding || !isReady)
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
        }
        .onChange(of: url) { suggestName() }
        .onChange(of: command) { suggestName() }
        .onAppear(perform: fillForDevelopment)
    }

    /// Development: `YH_MCP_ADD_URL` or `YH_MCP_ADD_COMMAND` fill the page in,
    /// `YH_MCP_ADD_SCOPE` picks where, `YH_MCP_ADD_SUBMIT=1` presses Add.
    private func fillForDevelopment() {
        let environment = ProcessInfo.processInfo.environment
        if let value = environment["YH_MCP_ADD_URL"] { way = .url; url = value }
        if let value = environment["YH_MCP_ADD_COMMAND"] { way = .command; command = value }
        if let value = environment["YH_MCP_ADD_SCOPE"].flatMap(MCPScope.init(rawValue:)) { scope = value }
        if environment["YH_MCP_ADD_SUBMIT"] == "1" {
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) { add() }
        }
    }

    // MARK: Fields

    private var urlFields: some View {
        VStack(alignment: .leading, spacing: 10) {
            TextField("", text: $url, prompt: Text("https://…/mcp"))
                .font(.body.monospaced())
                .textFieldStyle(.roundedBorder)
                .controlSize(.large)
            nameField
            DisclosureGroup("Advanced", isExpanded: $showsAdvanced) {
                VStack(alignment: .leading, spacing: 8) {
                    Text("Only if the server's docs ask for them.").font(.caption).foregroundStyle(.secondary)
                    pairs($headers, keyPrompt: "Header", valuePrompt: "Value", add: "Add Header")
                    TextField("OAuth client ID", text: $clientID).textFieldStyle(.roundedBorder)
                    SecureField("OAuth client secret", text: $clientSecret).textFieldStyle(.roundedBorder)
                    TextField("Callback port", text: $callbackPort).textFieldStyle(.roundedBorder).frame(width: 140)
                }
                .padding(.top, 6)
            }
            .font(.callout)
        }
    }

    private var commandFields: some View {
        VStack(alignment: .leading, spacing: 10) {
            TextField("", text: $command, prompt: Text("npx -y @modelcontextprotocol/server-github"))
                .font(.body.monospaced())
                .textFieldStyle(.roundedBorder)
                .controlSize(.large)
            nameField
            Text("Environment").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
            pairs($environment, keyPrompt: "NAME", valuePrompt: "Value", add: "Add Variable", secret: true)
        }
    }

    private var jsonField: some View {
        VStack(alignment: .leading, spacing: 10) {
            TextEditor(text: $json)
                .font(.callout.monospaced())
                .frame(minHeight: 140)
                .scrollContentBackground(.hidden)
                .padding(6)
                .card(radius: Theme.innerRadius)
            if MCPServerConfig.servers(fromJSON: json, fallbackName: "x")?.count == 1, !jsonHasNames {
                nameField
            }
        }
    }

    private var nameField: some View {
        HStack(spacing: 8) {
            Text("Name").foregroundStyle(.secondary)
            TextField("", text: Binding(get: { name }, set: { name = $0; nameEdited = true }), prompt: Text("server"))
                .textFieldStyle(.roundedBorder)
                .frame(width: 200)
            Spacer()
        }
        .font(.callout)
    }

    private func pairs(_ items: Binding<[Pair]>, keyPrompt: String, valuePrompt: String, add: String, secret: Bool = false) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(items) { $pair in
                HStack {
                    TextField("", text: $pair.key, prompt: Text(keyPrompt)).font(.callout.monospaced()).textFieldStyle(.roundedBorder)
                    if secret {
                        SecureField("", text: $pair.value, prompt: Text(valuePrompt)).textFieldStyle(.roundedBorder)
                    } else {
                        TextField("", text: $pair.value, prompt: Text(valuePrompt)).textFieldStyle(.roundedBorder)
                    }
                    Button("Remove", systemImage: "minus.circle") { items.wrappedValue.removeAll { $0.id == pair.id } }
                        .labelStyle(.iconOnly).buttonStyle(.borderless)
                }
            }
            Button(add, systemImage: "plus") { items.wrappedValue.append(Pair()) }
                .buttonStyle(.borderless).font(.callout)
        }
    }

    private var scopePicker: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("AVAILABLE IN").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
            Picker("", selection: $scope) {
                Text("Everywhere: every project, and Claude Code in the terminal").tag(MCPScope.user)
                Text("This project, just you").tag(MCPScope.local)
                Text("This project, for everyone (its .mcp.json, to commit)").tag(MCPScope.project)
            }
            .pickerStyle(.radioGroup)
            .labelsHidden()
        }
    }

    @ViewBuilder private var desktopImport: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("FROM CLAUDE DESKTOP").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
            if desktop.isEmpty {
                Text("Claude Desktop has no MCP servers, or isn't installed.").font(.callout).foregroundStyle(.secondary)
            } else {
                ForEach(desktop.keys.sorted(), id: \.self) { name in
                    Toggle(isOn: Binding(get: { desktopPicked.contains(name) }, set: { if $0 { desktopPicked.insert(name) } else { desktopPicked.remove(name) } })) {
                        VStack(alignment: .leading, spacing: 1) {
                            Text(name).font(.callout.weight(.medium))
                            Text(desktop[name].map(Self.summary) ?? "").font(.caption.monospaced()).foregroundStyle(.secondary).lineLimit(1)
                        }
                    }
                    .toggleStyle(.checkbox)
                }
                Button("Add \(desktopPicked.count) Server\(desktopPicked.count == 1 ? "" : "s")") { importDesktop() }
                    .disabled(desktopPicked.isEmpty || adding)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .card()
    }

    // MARK: Logic

    private var jsonHasNames: Bool {
        guard let value = JSONValue(json: json), case .object(let object) = value else { return false }
        return object["command"] == nil && object["url"] == nil
    }

    private var isReady: Bool {
        switch way {
        case .url: URL(string: url.trimmingCharacters(in: .whitespaces)).flatMap(\.scheme).map { ["http", "https"].contains($0) } == true && !trimmedName.isEmpty
        case .command: !MCPServerConfig.splitCommandLine(command).isEmpty && !trimmedName.isEmpty
        case .json: MCPServerConfig.servers(fromJSON: json, fallbackName: trimmedName.isEmpty ? "server" : trimmedName) != nil
        }
    }

    private var trimmedName: String { name.trimmingCharacters(in: .whitespaces) }

    private func suggestName() {
        guard !nameEdited else { return }
        switch way {
        case .url: name = URL(string: url.trimmingCharacters(in: .whitespaces)).map(MCPServerConfig.suggestedName(for:)) ?? ""
        case .command: name = command.isEmpty ? "" : MCPServerConfig.suggestedName(forCommand: command)
        case .json: break
        }
    }

    private static func values(_ pairs: [Pair]) -> [String: String] {
        Dictionary(pairs.filter { !$0.key.trimmingCharacters(in: .whitespaces).isEmpty }
            .map { ($0.key.trimmingCharacters(in: .whitespaces), $0.value) }) { $1 }
    }

    private static func summary(_ config: JSONValue) -> String {
        if let url = config["url"]?.string { return url }
        return ([config["command"]?.string ?? ""] + (config["args"]?.array ?? []).compactMap(\.string)).joined(separator: " ")
    }

    private func add() {
        let servers: [String: JSONValue]
        switch way {
        case .url:
            guard let address = URL(string: url.trimmingCharacters(in: .whitespaces)) else { return }
            servers = [trimmedName: MCPServerConfig.url(address, headers: Self.values(headers),
                                                        clientID: clientID.trimmingCharacters(in: .whitespaces),
                                                        callbackPort: Int(callbackPort.trimmingCharacters(in: .whitespaces))).json]
        case .command:
            let words = MCPServerConfig.splitCommandLine(command)
            guard let program = words.first else { return }
            servers = [trimmedName: MCPServerConfig.command(program, arguments: Array(words.dropFirst()), environment: Self.values(environment)).json]
        case .json:
            guard let parsed = MCPServerConfig.servers(fromJSON: json, fallbackName: trimmedName.isEmpty ? "server" : trimmedName) else { return }
            servers = parsed
        }
        run(servers, signIn: way == .url, clientSecret: way == .url ? clientSecret : nil)
    }

    private func loadDesktop() {
        desktop = ClaudeMCPConfig.claudeDesktopServers()
        desktopPicked = Set(desktop.keys)
        showsDesktop = true
    }

    private func importDesktop() {
        run(desktop.filter { desktopPicked.contains($0.key) }, signIn: false, clientSecret: nil)
    }

    private func run(_ servers: [String: JSONValue], signIn: Bool, clientSecret: String?) {
        adding = true
        error = nil
        Task {
            var failures: [String] = []
            for (name, config) in servers.sorted(by: { $0.key < $1.key }) {
                if let failure = await model.add(name: name, config: config, scope: everywhereOnly ? .user : scope,
                                                 clientSecret: clientSecret, signIn: signIn && servers.count == 1) {
                    failures.append(servers.count == 1 ? failure : "\(name): \(failure)")
                }
            }
            adding = false
            if !failures.isEmpty {
                error = failures.joined(separator: "\n")
                model.isAdding = true
            }
        }
    }
}
