// Automations tab on iPhone (protocol 12): inspect, trigger, pause/resume, create, edit and
// delete scheduled, Mac-event, file/repo and CLI automations running on the connected Mac.

import SwiftUI

private struct EditorPresentation: Identifiable {
    let id = UUID()
    let draft: AutomationDraft
}

struct AutomationsView: View {
    @Environment(Store.self) private var store
    @Environment(Fleet.self) private var fleet
    @State private var scopeFilter = "all"
    @State private var editor: EditorPresentation?
    @State private var deleting: AutomationInfo?
    @State private var pushedID: String?
    @State private var autoActionDone = false

    private static let env = ProcessInfo.processInfo.environment

    private var filtered: [AutomationInfo] {
        switch scopeFilter {
        case "all": store.automations
        case "global": store.automations.filter(\.isGlobal)
        default: store.automations.filter { !$0.isGlobal && $0.projectId == scopeFilter }
        }
    }

    private var globalAutomations: [AutomationInfo] {
        filtered.filter(\.isGlobal)
    }

    private var projectGroups: [(project: Project, items: [AutomationInfo])] {
        store.projects.compactMap { project in
            let items = filtered.filter { !$0.isGlobal && $0.projectId == project.id }
            return items.isEmpty ? nil : (project, items)
        }
    }

    private var scopeOptions: [(id: String, label: String)] {
        var out: [(String, String)] = [("all", "All")]
        if store.automations.contains(where: \.isGlobal) {
            out.append(("global", "Global"))
        }
        for p in store.projects where store.automations.contains(where: { !$0.isGlobal && $0.projectId == p.id }) {
            out.append((p.id, p.name))
        }
        return out
    }

    var body: some View {
        List {
            if store.state != .connected && store.automations.isEmpty {
                Section {
                    ConnectionRow(state: store.state, address: store.shownAddress)
                }
            }
            if store.automationsPaused {
                Section {
                    Button {
                        store.setAutomationsPaused(false)
                    } label: {
                        HStack(spacing: 10) {
                            Image(systemName: "pause.circle.fill")
                                .font(.title3)
                                .foregroundStyle(.orange)
                            VStack(alignment: .leading, spacing: 2) {
                                Text("All Automations Paused")
                                    .font(.subheadline.weight(.semibold))
                                    .foregroundStyle(.primary)
                                Text("Scheduled and event runs are on hold on \(store.displayName). Tap to resume.")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                            Spacer(minLength: 0)
                            Text("Resume")
                                .font(.caption.weight(.semibold))
                                .foregroundStyle(.orange)
                                .padding(.horizontal, 10)
                                .padding(.vertical, 5)
                                .background(Color.orange.opacity(0.14), in: .capsule)
                        }
                        .padding(.vertical, 2)
                        .contentShape(.rect)
                    }
                    .buttonStyle(.plain)
                    .disabled(!store.canAct)
                }
            }
            if scopeOptions.count > 2 {
                Section {
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 8) {
                            ForEach(scopeOptions, id: \.id) { opt in
                                let active = scopeFilter == opt.id
                                Button {
                                    scopeFilter = opt.id
                                } label: {
                                    Text(opt.label)
                                        .font(.caption.weight(.semibold))
                                        .foregroundStyle(active ? Color.white : Color.primary)
                                        .padding(.horizontal, 12)
                                        .padding(.vertical, 6)
                                        .background(active ? Brand.accent : Color(.tertiarySystemFill), in: .capsule)
                                }
                                .buttonStyle(.plain)
                            }
                        }
                        .padding(.vertical, 2)
                    }
                    .listRowInsets(EdgeInsets(top: 6, leading: 16, bottom: 6, trailing: 16))
                    .listRowBackground(Color.clear)
                }
            }
            if !globalAutomations.isEmpty {
                Section {
                    ForEach(globalAutomations) { row($0) }
                } header: {
                    Label("Global", systemImage: "globe")
                } footer: {
                    Text("Runs in Home across your Mac.")
                }
            }
            ForEach(projectGroups, id: \.project.id) { group in
                Section {
                    ForEach(group.items) { row($0) }
                } header: {
                    Label(group.project.name, systemImage: "folder")
                }
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle("Automations")
        .navigationDestination(item: $pushedID) { id in
            AutomationDetailView(automationID: id)
        }
        .overlay {
            if store.state == .connected && !store.offersAutomations {
                ContentUnavailableView(
                    "Update YourHarness on Mac",
                    systemImage: "bolt.slash",
                    description: Text("Automations on iPhone require a newer YourHarness on \(store.displayName).")
                )
            } else if store.offersAutomations && store.automations.isEmpty && store.state == .connected {
                ContentUnavailableView {
                    Label("No Automations Yet", systemImage: "bolt.fill")
                } description: {
                    Text("Run prompts on a schedule, when your Mac wakes or unlocks, or when files and PRs change.")
                } actions: {
                    Button("New Automation", systemImage: "plus") {
                        openNew()
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(!store.canAct)
                }
            }
        }
        .refreshable { store.reconnect() }
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    Button {
                        store.setAutomationsPaused(!store.automationsPaused)
                    } label: {
                        Label(
                            store.automationsPaused ? "Resume All Automations" : "Pause All Automations",
                            systemImage: store.automationsPaused ? "play.circle" : "pause.circle"
                        )
                    }
                    .disabled(!store.canAct)
                    Toggle(
                        "Hide Automation Threads",
                        systemImage: "eye.slash",
                        isOn: Binding(
                            get: { store.hideAutomationThreads },
                            set: { store.setHideAutomationThreads($0) }
                        )
                    )
                    .disabled(!store.canAct)
                    Divider()
                    Menu("Starter Templates", systemImage: "sparkles") {
                        ForEach(AutomationTemplate.all) { tpl in
                            Button(tpl.name, systemImage: tpl.icon) {
                                openNew(template: tpl)
                            }
                        }
                    }
                } label: {
                    Image(systemName: "ellipsis.circle")
                }
                .accessibilityLabel("Automation Options")
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button("New Automation", systemImage: "plus") {
                    openNew()
                }
                .disabled(!store.canAct || !store.offersAutomations)
            }
        }
        .sheet(item: $editor) { item in
            AutomationEditorSheet(initial: item.draft)
        }
        .confirmationDialog(
            "Delete “\(deleting?.name ?? "")”?",
            isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }),
            titleVisibility: .visible
        ) {
            Button("Delete", role: .destructive) {
                if let a = deleting { store.deleteAutomation(a.id) }
                deleting = nil
            }
        } message: {
            Text("Past run threads stay on your Mac; this automation will stop running.")
        }
        .alert("Couldn’t Complete Action", isPresented: Binding(get: { store.actionError != nil }, set: { if !$0 { store.actionError = nil } })) {
            Button("OK") { store.actionError = nil }
        } message: {
            Text(store.actionError ?? "")
        }
        // Test hooks: YH_PHONE_AUTO_SELECT=<name>, YH_PHONE_AUTO_NEW=1, YH_PHONE_AUTO_RUN=<name>
        .task(id: store.automations.count) {
            guard !autoActionDone, store.offersAutomations else { return }
            if let name = Self.env["YH_PHONE_AUTO_SELECT"], let match = store.automations.first(where: { $0.name == name || $0.id == name }) {
                autoActionDone = true
                pushedID = match.id
            } else if Self.env["YH_PHONE_AUTO_NEW"] == "1", store.canAct {
                autoActionDone = true
                openNew()
            } else if let name = Self.env["YH_PHONE_AUTO_RUN"], store.canAct, let match = store.automations.first(where: { $0.name == name || $0.id == name }) {
                autoActionDone = true
                try? await Task.sleep(for: .seconds(1))
                Perf.shared.log("PHONE run automation \(match.name)")
                store.runAutomation(match.id, openThread: true)
            }
        }
    }

    @ViewBuilder
    private func row(_ auto: AutomationInfo) -> some View {
        Button {
            pushedID = auto.id
        } label: {
            AutomationRow(automation: auto, pausedAll: store.automationsPaused)
        }
        .buttonStyle(.plain)
        .swipeActions(edge: .leading, allowsFullSwipe: true) {
            Button("Run Now", systemImage: "play.fill") {
                store.runAutomation(auto.id, openThread: true)
            }
            .tint(Brand.accent)
            .disabled(!store.canAct)
        }
        .swipeActions(edge: .trailing, allowsFullSwipe: false) {
            Button("Delete", systemImage: "trash", role: .destructive) {
                deleting = auto
            }
            .disabled(!store.canAct)
            Button(auto.enabled ? "Pause" : "Resume", systemImage: auto.enabled ? "pause.fill" : "play.fill") {
                store.toggleAutomation(auto.id, enabled: !auto.enabled)
            }
            .tint(auto.enabled ? .orange : .green)
            .disabled(!store.canAct)
        }
        .contextMenu {
            Button("Run Now", systemImage: "play.fill") { store.runAutomation(auto.id, openThread: true) }
                .disabled(!store.canAct)
            if let tid = auto.latestThreadID {
                Button("Open Last Thread", systemImage: "bubble.left.and.bubble.right") { store.show(tid) }
            }
            Button("Edit", systemImage: "pencil") { openEdit(auto) }
                .disabled(!store.canAct)
            Button(auto.enabled ? "Pause" : "Resume", systemImage: auto.enabled ? "pause.circle" : "play.circle") {
                store.toggleAutomation(auto.id, enabled: !auto.enabled)
            }
            .disabled(!store.canAct)
            Divider()
            Button("Delete", systemImage: "trash", role: .destructive) { deleting = auto }
                .disabled(!store.canAct)
        }
    }

    private func openNew(template: AutomationTemplate? = nil) {
        let defaultProject = store.projects.first(where: { $0.name != "Home" })?.id ?? store.projects.first?.id
        let draft: AutomationDraft
        if let tpl = template {
            draft = tpl.makeDraft(defaultProjectID: defaultProject)
        } else {
            draft = AutomationDraft(
                id: nil,
                name: "",
                prompt: "",
                isGlobal: true,
                projectId: defaultProject,
                triggerKind: "schedule",
                scheduleText: "daily 9:00",
                systemEvent: "macWake",
                systemApp: nil,
                systemPower: nil,
                watchPath: "",
                watchGlob: nil,
                repoEvent: "commitMade",
                repoBranch: nil,
                threadPolicy: "new_settle_previous",
                agent: nil,
                model: nil,
                effort: nil,
                mode: nil,
                workingDir: nil,
                notify: "always",
                cooldownSecs: 0,
                acPowerOnly: false,
                overlap: "skip",
                enabled: true
            )
        }
        editor = EditorPresentation(draft: draft)
    }

    private func openEdit(_ auto: AutomationInfo) {
        editor = EditorPresentation(draft: AutomationDraft(from: auto, fallbackProjectID: store.projects.first?.id))
    }
}

private struct AutomationRow: View {
    let automation: AutomationInfo
    let pausedAll: Bool

    private var statusColor: Color {
        if pausedAll || !automation.enabled || automation.isDone { return .secondary }
        switch automation.recentRuns.first?.status {
        case "waiting": return .orange
        case "error": return .red
        case "running": return .green
        default: return Brand.teal
        }
    }

    private var triggerIcon: String {
        switch automation.triggerKind {
        case "schedule": "clock"
        case "system": "desktopcomputer"
        case "file_watch": "folder.badge.gearshape"
        case "repo": "arrow.triangle.branch"
        default: "terminal"
        }
    }

    var body: some View {
        HStack(spacing: 11) {
            ZStack {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(statusColor.opacity(0.14))
                    .frame(width: 34, height: 34)
                Image(systemName: triggerIcon)
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(statusColor)
            }
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(automation.name)
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(automation.enabled && !pausedAll ? .primary : .secondary)
                        .lineLimit(1)
                    if !automation.enabled {
                        Text("Paused")
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 1.5)
                            .background(Color(.tertiarySystemFill), in: .capsule)
                    } else if let latest = automation.recentRuns.first, latest.status == "running" || latest.status == "waiting" {
                        Text(latest.status == "waiting" ? "Needs you" : "Running")
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(latest.status == "waiting" ? .orange : .green)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 1.5)
                            .background((latest.status == "waiting" ? Color.orange : Color.green).opacity(0.14), in: .capsule)
                    }
                }
                Text(subtitle)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 4)
            Image(systemName: "chevron.forward")
                .font(.footnote.weight(.semibold))
                .foregroundStyle(.tertiary)
        }
        .padding(.vertical, 2)
        .contentShape(.rect)
    }

    private var subtitle: String {
        var parts = [automation.triggerSummary]
        if automation.enabled && !pausedAll, let next = automation.nextRun, next > Date().timeIntervalSince1970 {
            let rel = AutomationFormatting.relative(next)
            parts.append("next \(rel)")
        } else if let last = automation.recentRuns.first {
            parts.append("last \(AutomationFormatting.relative(last.at))")
        }
        return parts.joined(separator: " · ")
    }
}

struct AutomationDetailView: View {
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    let automationID: String
    @State private var editor: EditorPresentation?
    @State private var confirmDelete = false
    @State private var copiedCLI = false

    private var automation: AutomationInfo? {
        store.automations.first { $0.id == automationID }
    }

    var body: some View {
        Group {
            if let auto = automation {
                detailList(auto)
            } else {
                ContentUnavailableView("Automation Removed", systemImage: "bolt.slash")
            }
        }
        .navigationTitle(automation?.name ?? "Automation")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar(.hidden, for: .tabBar)
        .toolbar {
            if let auto = automation {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Edit") {
                        editor = EditorPresentation(draft: AutomationDraft(from: auto, fallbackProjectID: store.projects.first?.id))
                    }
                    .disabled(!store.canAct)
                }
            }
        }
        .sheet(item: $editor) { item in
            AutomationEditorSheet(initial: item.draft)
        }
        .confirmationDialog("Delete “\(automation?.name ?? "")”?", isPresented: $confirmDelete, titleVisibility: .visible) {
            Button("Delete", role: .destructive) {
                if let auto = automation {
                    store.deleteAutomation(auto.id)
                    dismiss()
                }
            }
        } message: {
            Text("Past run threads stay on your Mac; this automation will stop running.")
        }
    }

    @ViewBuilder
    private func detailList(_ auto: AutomationInfo) -> some View {
        List {
            Section {
                VStack(alignment: .leading, spacing: 12) {
                    HStack(spacing: 8) {
                        scopeBadge(auto)
                        statusBadge(auto)
                        Spacer()
                    }
                    Text(auto.triggerSummary)
                        .font(.subheadline.weight(.medium))
                        .foregroundStyle(.primary)
                    if auto.enabled && !store.automationsPaused, let next = auto.nextRun, next > Date().timeIntervalSince1970 {
                        Label("Next run \(AutomationFormatting.relative(next))", systemImage: "calendar.badge.clock")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    HStack(spacing: 10) {
                        Button {
                            store.runAutomation(auto.id, openThread: true)
                        } label: {
                            HStack(spacing: 6) {
                                Image(systemName: "play.fill")
                                    .font(.caption.weight(.bold))
                                Text("Run Now")
                                    .font(.subheadline.weight(.semibold))
                            }
                            .foregroundStyle(.white)
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 9)
                            .background(Brand.accent, in: Capsule())
                        }
                        .buttonStyle(.plain)
                        .disabled(!store.canAct)
                        .opacity(store.canAct ? 1 : 0.45)

                        Button {
                            store.toggleAutomation(auto.id, enabled: !auto.enabled)
                        } label: {
                            HStack(spacing: 6) {
                                Image(systemName: auto.enabled ? "pause.fill" : "play.fill")
                                    .font(.caption.weight(.bold))
                                Text(auto.enabled ? "Pause" : "Resume")
                                    .font(.subheadline.weight(.semibold))
                            }
                            .foregroundStyle(.primary)
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 9)
                            .background(Color(.tertiarySystemFill), in: Capsule())
                        }
                        .buttonStyle(.plain)
                        .disabled(!store.canAct)
                        .opacity(store.canAct ? 1 : 0.45)
                    }
                }
                .padding(.vertical, 4)
            }

            Section("Prompt") {
                Text(auto.prompt)
                    .font(.callout)
                    .textSelection(.enabled)
                    .padding(.vertical, 2)
            }

            Section {
                if auto.recentRuns.isEmpty {
                    Text("No runs yet. Tap Run Now to test it on your Mac.")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                } else {
                    ForEach(auto.recentRuns) { run in
                        runRow(run)
                    }
                }
            } header: {
                HStack {
                    Text("Run History")
                    Spacer()
                    if !auto.recentRuns.isEmpty {
                        Text("\(auto.recentRuns.count)")
                            .font(.caption2.weight(.semibold))
                            .monospacedDigit()
                            .foregroundStyle(.secondary)
                    }
                }
            }

            Section("Configuration") {
                LabeledContent("Threads", value: AutomationFormatting.threadPolicyLabel(auto.threadPolicy))
                if let agent = auto.agent {
                    LabeledContent("Agent", value: store.agentName(agent))
                }
                if let model = auto.model {
                    LabeledContent("Model", value: model)
                }
                if let effort = auto.effort {
                    LabeledContent("Effort", value: effort.capitalized)
                }
                if let mode = auto.mode {
                    LabeledContent("Permissions", value: AutomationFormatting.modeLabel(mode))
                }
                LabeledContent("Notifications", value: AutomationFormatting.notifyLabel(auto.notify))
                LabeledContent("Overlap", value: AutomationFormatting.overlapLabel(auto.overlap))
                if let cd = auto.cooldownSecs, cd > 0 {
                    LabeledContent("Cooldown", value: AutomationFormatting.duration(Double(cd)))
                }
                if auto.acPowerOnly == true {
                    LabeledContent("Power", value: "AC power only")
                }
                if let slug = auto.slug, !slug.isEmpty {
                    Button {
                        UIPasteboard.general.string = "yh trigger \(slug)"
                        copiedCLI = true
                        Task {
                            try? await Task.sleep(for: .seconds(1.5))
                            copiedCLI = false
                        }
                    } label: {
                        LabeledContent("CLI Trigger") {
                            HStack(spacing: 5) {
                                Text("yh trigger \(slug)")
                                    .font(.caption.monospaced())
                                    .foregroundStyle(.secondary)
                                    .lineLimit(1)
                                Image(systemName: copiedCLI ? "checkmark" : "doc.on.doc")
                                    .font(.caption)
                                    .foregroundStyle(copiedCLI ? Color.green : Color.secondary)
                            }
                        }
                    }
                    .buttonStyle(.plain)
                }
            }

            Section {
                Button("Delete Automation", role: .destructive) {
                    confirmDelete = true
                }
                .disabled(!store.canAct)
            }
        }
        .listStyle(.insetGrouped)
    }

    @ViewBuilder
    private func runRow(_ run: AutomationRunInfo) -> some View {
        let content = HStack(spacing: 10) {
            Circle()
                .fill(runColor(run.status))
                .frame(width: 8, height: 8)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(runStatusLabel(run.status))
                        .font(.subheadline.weight(.medium))
                        .foregroundStyle(.primary)
                    if let reason = run.triggerReason, !reason.isEmpty {
                        Text("· \(reason)")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                }
                HStack(spacing: 6) {
                    Text(AutomationFormatting.relative(run.at))
                    if let d = run.durationSecs, d >= 1 {
                        Text("· \(AutomationFormatting.duration(d))")
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
                if let error = run.error, !error.isEmpty {
                    Text(error)
                        .font(.caption)
                        .foregroundStyle(run.status == "skipped" ? Color.secondary : Color.red)
                        .lineLimit(2)
                }
            }
            Spacer(minLength: 4)
            if run.threadId != nil {
                Image(systemName: "chevron.forward")
                    .font(.footnote.weight(.semibold))
                    .foregroundStyle(.tertiary)
            }
        }
        .padding(.vertical, 2)
        .contentShape(.rect)

        if let threadID = run.threadId {
            Button {
                store.show(threadID)
            } label: {
                content
            }
            .buttonStyle(.plain)
        } else {
            content
        }
    }

    private func scopeBadge(_ auto: AutomationInfo) -> some View {
        let name = auto.isGlobal ? "Global" : (store.projects.first(where: { $0.id == auto.projectId })?.name ?? "Workspace")
        let icon = auto.isGlobal ? "globe" : "folder"
        return HStack(spacing: 4) {
            Image(systemName: icon)
            Text(name)
        }
        .font(.caption.weight(.semibold))
        .foregroundStyle(auto.isGlobal ? Brand.accent : Brand.teal)
        .padding(.horizontal, 9)
        .padding(.vertical, 4)
        .background((auto.isGlobal ? Brand.accent : Brand.teal).opacity(0.14), in: .capsule)
    }

    private func statusBadge(_ auto: AutomationInfo) -> some View {
        let (label, color): (String, Color) = if store.automationsPaused {
            ("Paused Globally", .orange)
        } else if auto.isDone {
            ("Completed", .secondary)
        } else if auto.enabled {
            ("Active", .green)
        } else {
            ("Paused", .secondary)
        }
        return Text(label)
            .font(.caption.weight(.semibold))
            .foregroundStyle(color)
            .padding(.horizontal, 9)
            .padding(.vertical, 4)
            .background(color.opacity(0.14), in: .capsule)
    }

    private func runColor(_ status: String) -> Color {
        switch status {
        case "running": .green
        case "waiting": .orange
        case "error": .red
        case "skipped": .secondary
        default: Brand.teal
        }
    }

    private func runStatusLabel(_ status: String) -> String {
        switch status {
        case "running": "Running"
        case "waiting": "Waiting on you"
        case "error": "Failed"
        case "skipped": "Skipped"
        default: "Done"
        }
    }
}

struct AutomationEditorSheet: View {
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss

    @State private var name: String
    @State private var prompt: String
    @State private var isGlobal: Bool
    @State private var projectId: String
    @State private var triggerKind: String
    @State private var scheduleText: String
    @State private var systemEvent: String
    @State private var systemApp: String
    @State private var systemPower: String
    @State private var watchPath: String
    @State private var watchGlob: String
    @State private var repoEvent: String
    @State private var repoBranch: String
    @State private var threadPolicy: String
    @State private var agent: String
    @State private var model: String
    @State private var effort: String
    @State private var mode: String
    @State private var notify: String
    @State private var cooldownSecs: Int
    @State private var acPowerOnly: Bool
    @State private var overlap: String

    private let editingID: String?
    private let initialEnabled: Bool?

    init(initial: AutomationDraft) {
        editingID = initial.id
        initialEnabled = initial.enabled
        _name = State(initialValue: initial.name)
        _prompt = State(initialValue: initial.prompt)
        _isGlobal = State(initialValue: initial.isGlobal)
        _projectId = State(initialValue: initial.projectId ?? "")
        _triggerKind = State(initialValue: initial.triggerKind)
        _scheduleText = State(initialValue: initial.scheduleText ?? "daily 9:00")
        _systemEvent = State(initialValue: initial.systemEvent ?? "macWake")
        _systemApp = State(initialValue: initial.systemApp ?? "")
        _systemPower = State(initialValue: initial.systemPower ?? "")
        _watchPath = State(initialValue: initial.watchPath ?? "")
        _watchGlob = State(initialValue: initial.watchGlob ?? "")
        _repoEvent = State(initialValue: initial.repoEvent ?? "commitMade")
        _repoBranch = State(initialValue: initial.repoBranch ?? "")
        _threadPolicy = State(initialValue: initial.threadPolicy)
        _agent = State(initialValue: initial.agent ?? "")
        _model = State(initialValue: initial.model ?? "")
        _effort = State(initialValue: initial.effort ?? "")
        _mode = State(initialValue: initial.mode ?? "")
        _notify = State(initialValue: initial.notify ?? "always")
        _cooldownSecs = State(initialValue: initial.cooldownSecs)
        _acPowerOnly = State(initialValue: initial.acPowerOnly)
        _overlap = State(initialValue: initial.overlap ?? "skip")
    }

    private var canSave: Bool {
        !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && !prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && (isGlobal || !projectId.isEmpty)
            && (triggerKind != "schedule" || !scheduleText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
    }

    private var schedulePresets: [(String, String)] {
        [
            ("Every 30m", "every 30m"),
            ("Hourly", "hourly"),
            ("Daily 9:00", "daily 9:00"),
            ("Weekdays 8:30", "weekdays 8:30"),
            ("Fri 17:00", "every friday at 17:00"),
        ]
    }

    var body: some View {
        NavigationStack {
            Form {
                Section("Basics") {
                    TextField("Name (e.g. Morning Brief)", text: $name)
                    Picker("Scope", selection: $isGlobal) {
                        Label("Global (Home)", systemImage: "globe").tag(true)
                        Label("Workspace", systemImage: "folder").tag(false)
                    }
                    if !isGlobal {
                        Picker("Workspace", selection: $projectId) {
                            ForEach(store.projects) { project in
                                Text(project.name).tag(project.id)
                            }
                        }
                    }
                }

                Section("Trigger") {
                    Picker("When", selection: $triggerKind) {
                        Text("Schedule").tag("schedule")
                        Text("Mac Event").tag("system")
                        Text("File Watch").tag("file_watch")
                        Text("Git / PR").tag("repo")
                        Text("Manual / CLI").tag("manual")
                    }
                    if triggerKind == "schedule" {
                        TextField("daily 9:00, every 30m, cron 0 9 * * 1-5", text: $scheduleText)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .font(.body.monospaced())
                        ScrollView(.horizontal, showsIndicators: false) {
                            HStack(spacing: 8) {
                                ForEach(schedulePresets, id: \.1) { label, value in
                                    Button(label) { scheduleText = value }
                                        .font(.caption.weight(.medium))
                                        .padding(.horizontal, 10)
                                        .padding(.vertical, 5)
                                        .background(scheduleText == value ? Brand.accent.opacity(0.18) : Color(.tertiarySystemFill), in: .capsule)
                                        .foregroundStyle(scheduleText == value ? Brand.accent : Color.primary)
                                        .buttonStyle(.plain)
                                }
                            }
                        }
                    } else if triggerKind == "system" {
                        Picker("Event", selection: $systemEvent) {
                            Text("Mac wakes from sleep").tag("macWake")
                            Text("Screen unlocks").tag("screenUnlock")
                            Text("App launches").tag("appLaunched")
                            Text("App quits").tag("appQuit")
                            Text("Power source changes").tag("powerChanged")
                            Text("Network comes online").tag("networkOnline")
                            Text("YourHarness opens").tag("appLaunch")
                        }
                        if systemEvent == "appLaunched" || systemEvent == "appQuit" {
                            TextField("App name or bundle ID (optional)", text: $systemApp)
                                .textInputAutocapitalization(.never)
                                .autocorrectionDisabled()
                        }
                        if systemEvent == "powerChanged" {
                            Picker("Power", selection: $systemPower) {
                                Text("Any change").tag("")
                                Text("Plugged in (AC)").tag("ac")
                                Text("On battery").tag("battery")
                            }
                        }
                    } else if triggerKind == "file_watch" {
                        TextField("Folder path (e.g. ~/Downloads or blank for workspace)", text: $watchPath)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                        TextField("Glob filter (optional, e.g. *.pdf)", text: $watchGlob)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                    } else if triggerKind == "repo" {
                        Picker("Repo Event", selection: $repoEvent) {
                            Text("New git commit").tag("commitMade")
                            Text("Git branch changes").tag("branchChanged")
                            Text("PR review requested").tag("prReviewRequested")
                            Text("PR checks fail").tag("prChecksFailed")
                            Text("PR merged").tag("prMerged")
                        }
                        TextField("Branch filter (optional, e.g. main)", text: $repoBranch)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                    } else {
                        Text("Runs when you tap Run Now or call `yh trigger` on your Mac.")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    }
                }

                Section("Prompt") {
                    TextEditor(text: $prompt)
                        .frame(minHeight: 110)
                        .font(.body)
                }

                Section("Threads") {
                    Picker("Each Run", selection: $threadPolicy) {
                        Text("New thread each run").tag("new")
                        Text("New thread, settle previous").tag("new_settle_previous")
                        Text("Same thread each run").tag("same")
                    }
                }

                Section("Runtime") {
                    if !store.agents.isEmpty {
                        Picker("Agent", selection: $agent) {
                            Text("Mac Default").tag("")
                            Text("Claude").tag("claude")
                            ForEach(store.agents) { a in
                                Text(a.name).tag(a.id)
                            }
                        }
                    }
                    let modelChoices = store.models(for: agent.isEmpty || agent == "claude" ? nil : agent)
                    if !modelChoices.isEmpty {
                        Picker("Model", selection: $model) {
                            Text("Default").tag("")
                            ForEach(modelChoices) { m in
                                Text(m.name).tag(m.id)
                            }
                        }
                    }
                    Picker("Effort", selection: $effort) {
                        Text("Default").tag("")
                        Text("Low").tag("low")
                        Text("Medium").tag("medium")
                        Text("High").tag("high")
                    }
                    Picker("Permissions", selection: $mode) {
                        Text("Workspace Default").tag("")
                        Text("Auto").tag("auto")
                        Text("Ask").tag("ask")
                        Text("Accept Edits").tag("acceptEdits")
                        Text("Plan").tag("plan")
                    }
                }

                Section("Guardrails & Notifications") {
                    Picker("Cooldown", selection: $cooldownSecs) {
                        Text("Off").tag(0)
                        Text("1 minute").tag(60)
                        Text("5 minutes").tag(300)
                        Text("15 minutes").tag(900)
                        Text("1 hour").tag(3600)
                    }
                    Toggle("Only When Mac Is Plugged In", isOn: $acPowerOnly)
                    Picker("If Previous Run Still Working", selection: $overlap) {
                        Text("Skip").tag("skip")
                        Text("Queue").tag("queue")
                        Text("Allow parallel").tag("allow")
                    }
                    Picker("Notifications", selection: $notify) {
                        Text("When finished or waiting").tag("always")
                        Text("Only when waiting on you").tag("needs_attention")
                        Text("Silent").tag("silent")
                    }
                }
            }
            .navigationTitle(editingID == nil ? "New Automation" : "Edit Automation")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save") { save() }
                        .disabled(!canSave || !store.canAct)
                }
            }
            .onAppear {
                if projectId.isEmpty, let first = store.projects.first?.id {
                    projectId = first
                }
            }
        }
    }

    private func save() {
        let draft = AutomationDraft(
            id: editingID,
            name: name.trimmingCharacters(in: .whitespacesAndNewlines),
            prompt: prompt.trimmingCharacters(in: .whitespacesAndNewlines),
            isGlobal: isGlobal,
            projectId: isGlobal ? nil : projectId,
            triggerKind: triggerKind,
            scheduleText: triggerKind == "schedule" ? scheduleText.trimmingCharacters(in: .whitespacesAndNewlines) : nil,
            systemEvent: triggerKind == "system" ? systemEvent : nil,
            systemApp: triggerKind == "system" && !systemApp.isEmpty ? systemApp : nil,
            systemPower: triggerKind == "system" && !systemPower.isEmpty ? systemPower : nil,
            watchPath: triggerKind == "file_watch" ? watchPath : nil,
            watchGlob: triggerKind == "file_watch" && !watchGlob.isEmpty ? watchGlob : nil,
            repoEvent: triggerKind == "repo" ? repoEvent : nil,
            repoBranch: triggerKind == "repo" && !repoBranch.isEmpty ? repoBranch : nil,
            threadPolicy: threadPolicy,
            agent: agent.isEmpty ? nil : agent,
            model: model.isEmpty ? nil : model,
            effort: effort.isEmpty ? nil : effort,
            mode: mode.isEmpty ? nil : mode,
            workingDir: nil,
            notify: notify,
            cooldownSecs: cooldownSecs,
            acPowerOnly: acPowerOnly,
            overlap: overlap,
            enabled: initialEnabled
        )
        store.saveAutomation(draft)
        dismiss()
    }
}

private extension AutomationDraft {
    init(from auto: AutomationInfo, fallbackProjectID: String?) {
        self.init(
            id: auto.id,
            name: auto.name,
            prompt: auto.prompt,
            isGlobal: auto.isGlobal,
            projectId: auto.projectId ?? fallbackProjectID,
            triggerKind: auto.triggerKind,
            scheduleText: auto.scheduleText ?? "daily 9:00",
            systemEvent: auto.systemEvent ?? "macWake",
            systemApp: auto.systemApp,
            systemPower: auto.systemPower,
            watchPath: auto.watchPath,
            watchGlob: auto.watchGlob,
            repoEvent: auto.repoEvent ?? "commitMade",
            repoBranch: auto.repoBranch,
            threadPolicy: auto.threadPolicy,
            agent: auto.agent,
            model: auto.model,
            effort: auto.effort,
            mode: auto.mode,
            workingDir: auto.workingDir,
            notify: auto.notify,
            cooldownSecs: auto.cooldownSecs ?? 0,
            acPowerOnly: auto.acPowerOnly ?? false,
            overlap: auto.overlap,
            enabled: auto.enabled
        )
    }
}

private struct AutomationTemplate: Identifiable {
    let id: String
    let name: String
    let icon: String
    let isGlobal: Bool
    let triggerKind: String
    let scheduleText: String?
    let systemEvent: String?
    let watchPath: String?
    let watchGlob: String?
    let repoEvent: String?
    let threadPolicy: String
    let prompt: String

    func makeDraft(defaultProjectID: String?) -> AutomationDraft {
        AutomationDraft(
            id: nil,
            name: name,
            prompt: prompt,
            isGlobal: isGlobal,
            projectId: isGlobal ? nil : defaultProjectID,
            triggerKind: triggerKind,
            scheduleText: scheduleText,
            systemEvent: systemEvent,
            systemApp: nil,
            systemPower: nil,
            watchPath: watchPath,
            watchGlob: watchGlob,
            repoEvent: repoEvent,
            repoBranch: nil,
            threadPolicy: threadPolicy,
            agent: nil,
            model: nil,
            effort: nil,
            mode: nil,
            workingDir: nil,
            notify: "always",
            cooldownSecs: triggerKind == "schedule" ? 0 : 300,
            acPowerOnly: false,
            overlap: "skip",
            enabled: true
        )
    }

    static let all: [AutomationTemplate] = [
        AutomationTemplate(
            id: "morning-brief",
            name: "Morning Brief",
            icon: "sun.max",
            isGlobal: true,
            triggerKind: "schedule",
            scheduleText: "weekdays 8:30",
            systemEvent: nil,
            watchPath: nil,
            watchGlob: nil,
            repoEvent: nil,
            threadPolicy: "new_settle_previous",
            prompt: "Summarize open tasks, active threads, and anything needing attention today."
        ),
        AutomationTemplate(
            id: "wake-check",
            name: "Wake Check",
            icon: "desktopcomputer",
            isGlobal: true,
            triggerKind: "system",
            scheduleText: nil,
            systemEvent: "macWake",
            watchPath: nil,
            watchGlob: nil,
            repoEvent: nil,
            threadPolicy: "new_settle_previous",
            prompt: "Mac woke up. Check for any failed CI checks or PRs waiting for review."
        ),
        AutomationTemplate(
            id: "downloads-watch",
            name: "Watch Downloads",
            icon: "folder.badge.gearshape",
            isGlobal: true,
            triggerKind: "file_watch",
            scheduleText: nil,
            systemEvent: nil,
            watchPath: "~/Downloads",
            watchGlob: "*.pdf",
            repoEvent: nil,
            threadPolicy: "new",
            prompt: "Summarize the newly downloaded PDF file and highlight any action items."
        ),
        AutomationTemplate(
            id: "pr-checks",
            name: "PR Failure Triage",
            icon: "arrow.triangle.branch",
            isGlobal: false,
            triggerKind: "repo",
            scheduleText: nil,
            systemEvent: nil,
            watchPath: nil,
            watchGlob: nil,
            repoEvent: "prChecksFailed",
            threadPolicy: "new_settle_previous",
            prompt: "Inspect the failing CI check, explain the root cause, and propose a minimal fix."
        ),
    ]
}

@MainActor
private enum AutomationFormatting {
    private static let relativeFormatter: RelativeDateTimeFormatter = {
        let f = RelativeDateTimeFormatter()
        f.unitsStyle = .abbreviated
        return f
    }()

    static func relative(_ epochSeconds: Double) -> String {
        let date = Date(timeIntervalSince1970: epochSeconds)
        if abs(date.timeIntervalSinceNow) < 45 { return "just now" }
        return relativeFormatter.localizedString(for: date, relativeTo: Date())
    }

    static func duration(_ seconds: Double) -> String {
        let s = Int(seconds.rounded())
        if s < 60 { return "\(s)s" }
        if s < 3600 { return "\(s / 60)m \(s % 60)s" }
        return "\(s / 3600)h \((s % 3600) / 60)m"
    }

    static func threadPolicyLabel(_ raw: String) -> String {
        switch raw {
        case "new_settle_previous": "New thread, settle previous"
        case "same": "Same thread each run"
        default: "New thread each run"
        }
    }

    static func notifyLabel(_ raw: String) -> String {
        switch raw {
        case "needs_attention": "Only when waiting on you"
        case "silent": "Silent"
        default: "When finished or waiting"
        }
    }

    static func overlapLabel(_ raw: String) -> String {
        switch raw {
        case "queue": "Queue behind active run"
        case "allow": "Allow parallel runs"
        default: "Skip if still running"
        }
    }

    static func modeLabel(_ raw: String) -> String {
        switch raw {
        case "acceptEdits": "Accept Edits"
        case "bypass": "Bypass"
        default: raw.capitalized
        }
    }
}
