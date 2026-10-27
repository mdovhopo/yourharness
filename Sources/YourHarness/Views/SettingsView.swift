import AppKit
import HarnessCore
import SwiftUI

struct SettingsView: View {
    var body: some View {
        TabView {
            Tab("Agents", systemImage: "terminal") {
                AgentsSettingsView()
            }
            Tab("General", systemImage: "gearshape") {
                GeneralSettingsView()
            }
            Tab("Releases", systemImage: "shippingbox") {
                ReleasesSettingsView()
            }
        }
        .frame(width: 640, height: 560)
    }
}

// MARK: - Agents

private struct AgentsSettingsView: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        Form {
            ClaudeSection()
            GitToolsSection()

            Section {
                ForEach(AgentCLI.all.filter { !$0.isSupported }) { cli in
                    OtherAgentRow(cli: cli, status: app.cliStatuses[cli.id])
                }
            } header: {
                Text("Other Agents")
            } footer: {
                Text("Support for these is coming. YourHarness detects them now, so they're ready when it lands.")
                    .foregroundStyle(.secondary)
            }

            // Not in the toolbar: in Settings that's the tab bar, where it showed up as a tab.
            Section {
                LabeledContent("Installed or signed in just now?") {
                    Button {
                        Task { await app.refreshAgents() }
                    } label: {
                        if app.isRefreshingAgents {
                            ProgressView().controlSize(.small)
                        } else {
                            Text("Detect Again")
                        }
                    }
                    .disabled(app.isRefreshingAgents)
                    .help("Look for agents and tools again")
                }
            }
        }
        .formStyle(.grouped)
        .task { await app.refreshAgents() }
    }
}

/// The CLIs repo and pull request views drive. Their own logins are used as-is.
private struct GitToolsSection: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        Section {
            GitToolRow(name: "Git", tool: "git", symbol: "arrow.triangle.branch", installURL: URL(string: "https://git-scm.com/download/mac")!)
            ForEach([Forge.github, .gitlab], id: \.self) { forge in
                GitToolRow(
                    name: "\(forge.displayName) CLI",
                    tool: forge.cliName,
                    symbol: "arrow.triangle.pull",
                    installURL: forge.installURL,
                    account: app.forgeAccount(for: forge)
                )
            }
        } header: {
            Text("Git & Code Hosting")
        } footer: {
            Text("Pull requests come from `gh` and `glab`, signed in as you. YourHarness never sees their tokens.")
                .foregroundStyle(.secondary)
        }
    }
}

private struct GitToolRow: View {
    @Environment(AppModel.self) private var app
    let name: String
    let tool: String
    let symbol: String
    let installURL: URL
    var account: String?

    private var status: CLIStatus? { app.toolStatuses[tool] }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Image(systemName: symbol)
                .foregroundStyle(.secondary)
                .frame(width: 20)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(name)
                    if let account {
                        Chip(account, tint: .accentColor)
                            .help("Signed in as \(account)")
                    }
                }
                if let path = status?.path {
                    Text(path.path)
                        .font(.caption.monospaced())
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
            Spacer()
            if status?.state == .notFound {
                Link("Get", destination: installURL)
                    .font(.callout)
            }
            StatusLabel(status: status, refreshing: status == nil)
        }
    }
}

private struct ClaudeSection: View {
    @Environment(AppModel.self) private var app

    private var status: CLIStatus? { app.cliStatuses[AgentCLI.claude.id] }
    private var hasOverride: Bool { app.cliPathOverrides[AgentCLI.claude.id] != nil }

    var body: some View {
        Section {
            LabeledContent("Status") {
                StatusLabel(status: status, refreshing: app.isRefreshingAgents && status == nil)
            }

            LabeledContent("Location") {
                HStack(spacing: 8) {
                    Text(status?.path?.path ?? "Not found")
                        .font(.callout.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                    if let path = status?.path {
                        Button("Show in Finder", systemImage: "magnifyingglass") {
                            NSWorkspace.shared.activateFileViewerSelecting([path])
                        }
                        .labelStyle(.iconOnly)
                        .buttonStyle(.borderless)
                        .help("Show in Finder")
                    }
                }
            }

            HStack {
                Spacer()
                if hasOverride {
                    Button("Use Detected") { app.setExecutableOverride(nil, for: .claude) }
                }
                Button("Choose…") { app.presentChooseExecutable(for: .claude) }
            }

            if status?.state == .notFound {
                InstallHint(cli: .claude)
            }
        } header: {
            Label {
                Text(AgentCLI.claude.name)
            } icon: {
                ClaudeMark(size: 14)
            }
        }

        if status?.isReady == true {
            AccountSection()
        }
    }
}

private struct AccountSection: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        Section("Account") {
            if let account = app.claudeAccount {
                if account.loggedIn {
                    if let email = account.email {
                        LabeledContent("Signed in as", value: email)
                    }
                    if let plan = account.planName {
                        LabeledContent("Plan") {
                            Text(plan)
                                .fontWeight(.medium)
                                .padding(.horizontal, 8)
                                .padding(.vertical, 2)
                                .background(.tint.opacity(0.15), in: .capsule)
                        }
                    }
                    if let org = account.orgName {
                        LabeledContent("Organization", value: org)
                    }
                    if let method = account.signInMethod {
                        LabeledContent("Signed in with", value: method)
                    }
                } else {
                    SignInHint()
                }
            } else if app.isRefreshingAgents {
                LabeledContent("Account") { ProgressView().controlSize(.small) }
            } else {
                LabeledContent("Account", value: "Couldn't read sign-in status")
            }

            if let rateLimit = app.rateLimit, !rateLimit.windows.isEmpty {
                ForEach(rateLimit.windows) { window in
                    UsageBar(window: window)
                        .padding(.vertical, 2)
                }
            } else {
                LabeledContent("Usage", value: "Appears after your first message")
            }
        }
    }
}

private struct SignInHint: View {
    private let command = "claude auth login"

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Not signed in", systemImage: "person.crop.circle.badge.exclamationmark")
                .foregroundStyle(.orange)
            Text("Sign in once in Terminal and YourHarness uses that login. It never sees your credentials.")
                .font(.callout)
                .foregroundStyle(.secondary)
            CommandRow(command: command)
        }
    }
}

private struct InstallHint: View {
    let cli: AgentCLI

    var body: some View {
        HStack {
            Text("Install \(cli.name), then click Refresh.")
                .foregroundStyle(.secondary)
            Spacer()
            Link("Installation Guide", destination: cli.installURL)
        }
        .font(.callout)
    }
}

private struct CommandRow: View {
    let command: String
    @State private var copied = false

    var body: some View {
        HStack(spacing: 8) {
            Text(command)
                .font(.callout.monospaced())
                .textSelection(.enabled)
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(.quaternary.opacity(0.6), in: .rect(cornerRadius: 6))
            Button(copied ? "Copied" : "Copy") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(command, forType: .string)
                copied = true
            }
            Button("Open Terminal") {
                NSWorkspace.shared.open(URL(fileURLWithPath: "/System/Applications/Utilities/Terminal.app"))
            }
        }
    }
}

private struct OtherAgentRow: View {
    let cli: AgentCLI
    let status: CLIStatus?

    private var symbol: String {
        switch cli.id {
        case "codex": "chevron.left.forwardslash.chevron.right"
        case "cursor": "cursorarrow.rays"
        case "gemini": "diamond"
        case "opencode": "curlybraces"
        default: "terminal"
        }
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Image(systemName: symbol)
                .foregroundStyle(.secondary)
                .frame(width: 20)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(cli.name)
                    Chip("Soon")
                }
                if let path = status?.path {
                    Text(path.path)
                        .font(.caption.monospaced())
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
            Spacer()
            if status?.state == .notFound {
                Link("Get", destination: cli.installURL)
                    .font(.callout)
            }
            StatusLabel(status: status, refreshing: status == nil)
        }
    }
}

private struct StatusLabel: View {
    let status: CLIStatus?
    let refreshing: Bool

    var body: some View {
        switch status?.state {
        case .ready(let version):
            Label(Self.shortVersion(version), systemImage: "checkmark.circle.fill")
                .foregroundStyle(.green)
                .help(version)
        case .broken(let reason):
            Label("Not working", systemImage: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
                .help(reason)
        case .notFound:
            Text("Not installed")
                .foregroundStyle(.secondary)
        case nil:
            if refreshing { ProgressView().controlSize(.small) } else { Text("—").foregroundStyle(.secondary) }
        }
    }

    /// "2.1.302 (Claude Code)" → "2.1.302"
    private static func shortVersion(_ version: String) -> String {
        version.split(separator: " ").first.map(String.init) ?? version
    }
}

// MARK: - General

private struct GeneralSettingsView: View {
    @Environment(AppModel.self) private var app

    private var defaultModel: Binding<String?> {
        Binding(get: { app.defaults.model }, set: { app.setDefaultModel($0) })
    }

    private var defaultEffort: Binding<String?> {
        Binding(get: { app.defaults.effort }, set: { app.setDefaultEffort($0) })
    }

    private var defaultPermissionMode: Binding<PermissionMode> {
        Binding(get: { app.defaults.permissionMode ?? .standard }, set: { app.setDefaultPermissionMode($0) })
    }

    /// Writes to your own Claude Code settings, so it asks first.
    private func confirmTurnOffEverywhere() {
        let alert = NSAlert()
        alert.messageText = "Turn off Claude Code's memory everywhere?"
        alert.informativeText = "Sets autoMemoryEnabled to false in your Claude Code settings (\(app.claudeConfigDirectory.appending(path: "settings.json").path.replacingOccurrences(of: NSHomeDirectory(), with: "~"))), so claude keeps no memory files of its own in any folder, in YourHarness or in Terminal. YourHarness keeps the memory. Its existing files are left as they are; import them from a project's Memory page."
        alert.addButton(withTitle: "Turn Off")
        alert.addButton(withTitle: "Cancel")
        if alert.runModal() == .alertFirstButtonReturn { app.turnOffClaudeMemoryEverywhere() }
    }

    var body: some View {
        Form {
            Section {
                Picker(selection: Binding(get: { app.appearance }, set: { app.setAppearance($0) })) {
                    ForEach(Appearance.allCases) { appearance in
                        Text(appearance.label).tag(appearance)
                    }
                } label: {
                    Label("Appearance", systemImage: "circle.lefthalf.filled")
                }
                .pickerStyle(.segmented)
            } footer: {
                Text("System follows macOS, including its automatic switch between light and dark.")
                    .foregroundStyle(.secondary)
            }

            NotificationsSection()

            Section {
                Picker(selection: defaultModel) {
                    Text("Claude Code Default · \(app.modelDisplayName(for: nil))").tag(String?.none)
                    Divider()
                    ForEach(app.claudeModels.filter { $0.isAlias && $0.value != "default" }) { model in
                        Text(model.displayName).tag(Optional(model.value))
                    }
                    let versions = app.claudeModels.filter { !$0.isAlias }
                    if !versions.isEmpty {
                        Divider()
                        ForEach(versions) { model in
                            Text("\(model.displayName) (pinned)").tag(Optional(model.value))
                        }
                    }
                } label: {
                    Label {
                        Text("Model")
                    } icon: {
                        ClaudeMark(size: 14)
                    }
                }

                Picker(selection: defaultEffort) {
                    Text("Auto").tag(String?.none)
                    Divider()
                    ForEach(EffortLevel.allCases) { level in
                        Text(level.label).tag(Optional(level.rawValue))
                    }
                } label: {
                    Label("Reasoning effort", systemImage: "gauge.with.dots.needle.50percent")
                }

                Picker(selection: defaultPermissionMode) {
                    ForEach(PermissionMode.allCases) { mode in
                        Text(mode.label).tag(mode)
                    }
                } label: {
                    Label("Permissions for new projects", systemImage: "hand.raised")
                }
            } header: {
                Text("New Thread Defaults")
            } footer: {
                Text("Each thread can override these from the composer. Threads that haven't overridden them follow these defaults.")
                    .foregroundStyle(.secondary)
            }

            Section {
                Picker(selection: Binding(get: { app.autoSettleDays }, set: { app.setAutoSettleDays($0) })) {
                    Text("Never").tag(0)
                    Text("After 1 day").tag(1)
                    Text("After 3 days").tag(3)
                    Text("After 1 week").tag(7)
                    Text("After 2 weeks").tag(14)
                } label: {
                    Label("Settle idle threads", systemImage: "checkmark.circle")
                }
            } header: {
                Text("Threads")
            } footer: {
                Text("Settled threads move to a shelf at the end of their project and stop their Claude process; the conversation stays. Threads that are working or waiting on you never settle.")
                    .foregroundStyle(.secondary)
            }

            TitleGenerationSection()

            Section {
                Toggle(isOn: Binding(get: { app.memoryEnabled }, set: { app.setMemoryEnabled($0) })) {
                    Label("Give agents the workspace's memory", systemImage: "brain")
                }
                Toggle(isOn: Binding(get: { app.claudeCodeMemoryOff }, set: { app.setClaudeCodeMemoryOff($0) })) {
                    Label("Turn off Claude Code's own memory files", systemImage: "doc.on.doc")
                }
                .disabled(!app.memoryEnabled)
                LabeledContent {
                    if app.claudeMemoryOffEverywhere {
                        Text("Off for every folder").foregroundStyle(.secondary)
                    } else {
                        Button("Turn Off Everywhere…") { confirmTurnOffEverywhere() }
                    }
                } label: {
                    Label("Claude Code's memory outside the app", systemImage: "terminal")
                }
                .disabled(!app.memoryEnabled)
                LabeledContent {
                    MemoryAgentModelPicker(model: Binding(get: { app.memoryAgentModel }, set: { app.setMemoryAgentModel($0) }),
                                           effort: Binding(get: { app.memoryAgentEffort }, set: { app.setMemoryAgentEffort($0) }))
                } label: {
                    Label("Importer and Reconciler", systemImage: "wand.and.sparkles")
                }
            } header: {
                Text("Memory")
            } footer: {
                Text("Each project has its own memory, which every thread there starts with: an index of what's remembered (a line each, about 2,000 tokens at most) and tools to search, read and save. Memory tools don't ask for approval: they only touch the app's own database. Claude Code's memory files are always off in YourHarness's own threads when the second switch is on; a project's Memory page nudges you to bring in what it has and to turn it off for `claude` you run in Terminal too, so there's one memory, not two.")
                    .foregroundStyle(.secondary)
            }

            Section {
                Toggle(isOn: Binding(get: { app.planChecklist }, set: { app.setPlanChecklist($0) })) {
                    Label("Let Claude keep a plan checklist", systemImage: "checklist")
                }
            } header: {
                Text("Plans")
            } footer: {
                Text("For work with several steps, Claude lists them and ticks them off: the checklist shows in the thread and above the composer while it works. Claude Code leaves these task tools off for current models unless they're turned on here.")
                    .foregroundStyle(.secondary)
            }

            Section {
                Toggle(isOn: Binding(get: { app.browserTools }, set: { app.setBrowserTools($0) })) {
                    Label("Let Claude use the browser", systemImage: "safari")
                }
                Toggle(isOn: Binding(get: { app.browserAutoShow }, set: { app.setBrowserAutoShow($0) })) {
                    Label("Show the browser when Claude opens a page", systemImage: "sidebar.trailing")
                }
                .disabled(!app.browserTools)
            } header: {
                Text("Browser")
            } footer: {
                Text("Claude opens, reads, clicks and types in a browser built into the app, to test what it builds; you watch in the sidebar and can take control. Each project keeps its own cookies. Browser tools don't ask for approval: they only touch the app's own browser.")
                    .foregroundStyle(.secondary)
            }

            Section {
                Toggle(isOn: Binding(get: { app.diagramInstructions }, set: { app.setDiagramInstructions($0) })) {
                    Label("Let agents draw diagrams", systemImage: "point.3.connected.trianglepath.dotted")
                }
            } header: {
                Text("Diagrams")
            } footer: {
                Text("Mermaid flowcharts and sequence diagrams are always drawn natively. With this on, agents are also told they can use them (about 150 tokens per session).")
                    .foregroundStyle(.secondary)
            }

            Section {
                LabeledContent("Default folder") {
                    HStack(spacing: 8) {
                        Label(app.homeProject.path.replacingOccurrences(of: NSHomeDirectory(), with: "~"), systemImage: "house")
                            .font(.callout.monospaced())
                            .lineLimit(1)
                            .truncationMode(.middle)
                        Button("Change…") { app.presentChooseHomeFolder() }
                        if app.homeProject.path != NSHomeDirectory() {
                            Button("Reset") { app.setHomeFolder(nil) }
                        }
                    }
                }
            } header: {
                Text("Threads")
            } footer: {
                Text("New threads that aren't in a project run here.")
                    .foregroundStyle(.secondary)
            }

            Section {
                LabeledContent("Threads and settings") {
                    Button("Show in Finder") { app.store.revealInFinder() }
                }
                LabeledContent("Everything, as one file") {
                    HStack {
                        if let busy = app.stateTransfer.busy {
                            ProgressView().controlSize(.small)
                            Text(busy).foregroundStyle(.secondary)
                        }
                        Button("Export State…") { app.stateTransfer.presentExport(app: app) }
                        Button("Import State…") { app.stateTransfer.presentImport(app: app) }
                            .disabled(app.stateTransfer.pending != nil)
                    }
                    .disabled(app.stateTransfer.busy != nil)
                }
            } header: {
                Text("Data")
            } footer: {
                Text("Export State writes every project, thread, memory and setting, with details of this Mac and app, into one file: for another Mac, or for reproducing a bug exactly. Import State replaces everything you have with a file's, after asking.")
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
    }
}

private struct NotificationsSection: View {
    @Environment(AppModel.self) private var app
    @State private var isDenied = false

    var body: some View {
        Section {
            Toggle(isOn: Binding(get: { app.notifications }, set: { app.setNotifications($0) })) {
                Label("Notify when Claude finishes or needs you", systemImage: "bell.badge")
            }
            if isDenied, app.notifications {
                LabeledContent {
                    Button("Open System Settings") {
                        NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.Notifications-Settings.extension")!)
                    }
                } label: {
                    Label("Notifications are off for YourHarness in System Settings.", systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.secondary)
                }
            }
        } header: {
            Text("Notifications")
        } footer: {
            Text("Only for threads you aren't looking at: a turn that finishes while YourHarness is in the background, and Claude asking for approval, an answer or a plan review. Click one to open its thread. The Dock icon counts threads waiting for you.")
                .foregroundStyle(.secondary)
        }
        .task { isDenied = await app.notifier.isDenied() }
    }
}

/// Which CLI and model name new threads.
private struct TitleGenerationSection: View {
    @Environment(AppModel.self) private var app

    /// Codex models offered for titles; the first is T3 Code's pick for short text.
    private static let codexModels = ["gpt-6-luna", "gpt-6-astra", "gpt-5.6-sol", "gpt-5.6-terra"]

    private var settings: TitleGenerationSettings { app.titleGeneration }

    private var provider: Binding<TitleGenerationSettings.Provider?> {
        Binding(get: { settings.provider }, set: { app.setTitleGeneration(TitleGenerationSettings(provider: $0)) })
    }

    private var model: Binding<String?> {
        Binding(get: { settings.model }, set: { app.setTitleGeneration(TitleGenerationSettings(provider: settings.provider, model: $0)) })
    }

    var body: some View {
        Section {
            Picker(selection: provider) {
                ForEach(TitleGenerationSettings.Provider.allCases) { provider in
                    let ready = app.canGenerateTitles(with: provider)
                    Text(ready ? provider.cli.name : "\(provider.cli.name) (not installed)")
                        .tag(Optional(provider))
                        .selectionDisabled(!ready && settings.provider != provider)
                }
                Divider()
                Text("Off: use the first message").tag(TitleGenerationSettings.Provider?.none)
            } label: {
                Label("Generate with", systemImage: "text.badge.star")
            }

            if let current = settings.provider {
                Picker(selection: model) {
                    Text("Default · \(displayName(current.defaultModel, for: current))").tag(String?.none)
                    Divider()
                    ForEach(models(for: current).filter { $0 != current.defaultModel }, id: \.self) { value in
                        Text(displayName(value, for: current)).tag(Optional(value))
                    }
                } label: {
                    Label("Model", systemImage: "cpu")
                }
            }
        } header: {
            Text("Thread Titles")
        } footer: {
            VStack(alignment: .leading, spacing: 4) {
                Text(settings.provider == nil
                     ? "Threads are named after the first words of their first message. Rename one from its menu."
                     : "After your first message, a short request with no tools or project access names the thread. Regenerate or rename it from the thread's menu.")
                if let error = app.titleGenerationError, settings.provider != nil {
                    Label("Last title failed: \(error)", systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.orange)
                }
            }
            .foregroundStyle(.secondary)
        }
    }

    private func models(for provider: TitleGenerationSettings.Provider) -> [String] {
        switch provider {
        case .claude:
            app.claudeModels.filter { $0.value != "default" }.map(\.value)
        case .codex:
            Self.codexModels
        }
    }

    private func displayName(_ value: String, for provider: TitleGenerationSettings.Provider) -> String {
        switch provider {
        case .claude:
            guard let model = app.claudeModel(for: value) else { return value.capitalized }
            return model.isAlias ? model.displayName : "\(model.displayName) (pinned)"
        case .codex:
            return value
        }
    }
}
