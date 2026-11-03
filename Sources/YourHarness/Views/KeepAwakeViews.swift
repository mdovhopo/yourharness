import SwiftUI

/// The cup in the sidebar's footer: filled while the Mac is kept awake, faint otherwise,
/// gone when it's off and nothing was asked for. Click for what's going on and quick timers.
/// Reads only `KeepAwake`, which changes when work starts or stops, never on a tick.
struct KeepAwakeButton: View {
    @Environment(AppModel.self) private var app
    @State private var showsPopover = false

    var body: some View {
        let keepAwake = app.keepAwake
        if keepAwake.settings.enabled || keepAwake.reason != nil {
            Button {
                showsPopover.toggle()
            } label: {
                Image(systemName: keepAwake.reason == nil ? "cup.and.saucer" : "cup.and.heat.waves.fill")
                    .foregroundStyle(keepAwake.reason == nil ? AnyShapeStyle(.tertiary) : AnyShapeStyle(Color.accentColor))
                    .contentTransition(.symbolEffect(.replace))
            }
            .buttonStyle(.borderless)
            .help(KeepAwakeText.status(keepAwake))
            .popover(isPresented: $showsPopover, arrowEdge: .top) {
                KeepAwakePopover()
            }
        }
    }
}

@MainActor
enum KeepAwakeText {
    static func status(_ keepAwake: KeepAwake) -> String {
        let what = keepAwake.settings.keepsDisplayOn ? "Mac and display" : "Mac"
        switch keepAwake.reason {
        case .working: return "Keeping your \(what) awake while Claude works"
        case .afterWork(let until): return "Keeping your \(what) awake until \(until.formatted(date: .omitted, time: .shortened)), after Claude's last turn"
        case .always: return "Keeping your \(what) awake while YourHarness is open"
        case .manual(let until?): return "Keeping your \(what) awake until \(until.formatted(date: .omitted, time: .shortened))"
        case .manual(nil): return "Keeping your \(what) awake until you stop it"
        case nil:
            return keepAwake.settings.mode == .whileWorking
                ? "Your Mac can sleep. It stays awake while Claude works."
                : "Your Mac can sleep"
        }
    }
}

private struct KeepAwakePopover: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        let keepAwake = app.keepAwake
        VStack(alignment: .leading, spacing: 12) {
            Label {
                Text(KeepAwakeText.status(keepAwake))
                    .fixedSize(horizontal: false, vertical: true)
            } icon: {
                Image(systemName: keepAwake.reason == nil ? "moon.zzz" : "cup.and.heat.waves.fill")
                    .foregroundStyle(keepAwake.reason == nil ? AnyShapeStyle(.secondary) : AnyShapeStyle(Color.accentColor))
            }
            .font(.callout)

            Divider()

            Text("Keep awake for")
                .font(.caption)
                .foregroundStyle(.secondary)
            HStack(spacing: 6) {
                Button("1 hour") { keepAwake.keepAwake(for: 3_600) }
                Button("2 hours") { keepAwake.keepAwake(for: 7_200) }
                Button("4 hours") { keepAwake.keepAwake(for: 14_400) }
                Button("Until I stop") { keepAwake.keepAwake(for: nil) }
            }
            .controlSize(.small)

            if keepAwake.hasManual || isAfterWork(keepAwake.reason) {
                Button("Let the Mac Sleep", systemImage: "moon") { keepAwake.stopManual() }
                    .controlSize(.small)
            }

            Divider()

            Toggle("Keep the display on too", isOn: binding(\.keepsDisplayOn))
                .toggleStyle(.checkbox)
            HStack {
                Picker("Automatically", selection: binding(\.mode)) {
                    ForEach(KeepAwakeSettings.Mode.allCases) { Text($0.label).tag($0) }
                }
                .fixedSize()
                .disabled(!keepAwake.settings.enabled)
                Spacer()
                SettingsLink { Text("Settings…") }
                    .buttonStyle(.link)
            }
        }
        .padding(14)
        .frame(width: 320)
    }

    private func isAfterWork(_ reason: KeepAwake.Reason?) -> Bool {
        if case .afterWork = reason { true } else { false }
    }

    private func binding<Value>(_ keyPath: WritableKeyPath<KeepAwakeSettings, Value>) -> Binding<Value> {
        Binding(get: { app.keepAwake.settings[keyPath: keyPath] }, set: { value in
            var settings = app.keepAwake.settings
            settings[keyPath: keyPath] = value
            app.setKeepAwake(settings)
        })
    }
}

/// Settings → General.
struct KeepAwakeSection: View {
    @Environment(AppModel.self) private var app

    private func binding<Value>(_ keyPath: WritableKeyPath<KeepAwakeSettings, Value>) -> Binding<Value> {
        Binding(get: { app.keepAwake.settings[keyPath: keyPath] }, set: { value in
            var settings = app.keepAwake.settings
            settings[keyPath: keyPath] = value
            app.setKeepAwake(settings)
        })
    }

    private static let afterWorkChoices = [0, 5, 15, 30, 60]

    var body: some View {
        let settings = app.keepAwake.settings
        Section {
            Toggle(isOn: binding(\.enabled)) {
                Label("Keep the Mac awake", systemImage: "cup.and.heat.waves")
            }
            if settings.enabled {
                Picker("When", selection: binding(\.mode)) {
                    ForEach(KeepAwakeSettings.Mode.allCases) { Text($0.label).tag($0) }
                }
                if settings.mode == .whileWorking {
                    Picker("After the last turn", selection: binding(\.afterWorkMinutes)) {
                        ForEach(Self.afterWorkChoices, id: \.self) { minutes in
                            Text(minutes == 0 ? "Let it sleep right away" : minutes == 60 ? "Stay awake 1 hour" : "Stay awake \(minutes) minutes").tag(minutes)
                        }
                    }
                }
                Toggle("Keep the display on too", isOn: binding(\.keepsDisplayOn))
            }
        } header: {
            Text("Sleep")
        } footer: {
            Text("Stops your Mac from going to sleep on its own, as caffeinate does, so long turns keep going when you step away. The cup in the sidebar shows when it's on and keeps it awake for an hour or more on request. Closing the lid still sleeps a MacBook on battery.")
                .foregroundStyle(.secondary)
        }
    }
}
