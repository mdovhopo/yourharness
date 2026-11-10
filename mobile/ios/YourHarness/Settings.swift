// The phone's own settings, from the gear in Threads: how the list shows your Macs, how code
// wraps. What belongs to a Mac stays on the Mac tab.

import SwiftUI

struct SettingsSheet: View {
    @Environment(Fleet.self) private var fleet
    @Environment(\.dismiss) private var dismiss
    @AppStorage("diff.wrap") private var wrapDiffs = true
    @AppStorage("file.wrap") private var wrapFiles = false

    var body: some View {
        @Bindable var fleet = fleet
        NavigationStack {
            Form {
                Section {
                    Toggle("Show All Macs Together", isOn: $fleet.showsAllMacs)
                        .disabled(fleet.stores.count < 2)
                } header: {
                    Text("Threads")
                } footer: {
                    Text(fleet.stores.count < 2
                        ? "With more than one Mac, list every Mac’s threads in one list."
                        : "Every Mac’s projects in one list, a project on several Macs once, each thread saying its Mac. Opening a thread shows its Mac.")
                }
                Section("Code") {
                    Toggle("Wrap Lines in Diffs", isOn: $wrapDiffs)
                    Toggle("Wrap Lines in Files", isOn: $wrapFiles)
                }
                Section {
                    Button {
                        dismiss()
                        fleet.tab = .mac
                    } label: {
                        LabeledContent {
                            Text("\(fleet.stores.count)")
                        } label: {
                            Label(fleet.stores.count > 1 ? "Your Macs" : "Your Mac", systemImage: "desktopcomputer")
                        }
                    }
                    .foregroundStyle(.primary)
                }
                Section {
                    LabeledContent("Version", value: Self.version)
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done", systemImage: "checkmark") { dismiss() }
                }
            }
        }
    }

    private static var version: String {
        let info = Bundle.main.infoDictionary
        let short = info?["CFBundleShortVersionString"] as? String ?? "?"
        let build = info?["CFBundleVersion"] as? String
        return build.map { "\(short) (\($0))" } ?? short
    }
}
