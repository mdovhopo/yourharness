// The phone's own settings, from the gear in Threads: how the list shows your Macs, how code
// wraps. What belongs to a Mac stays on the Mac tab.

import SwiftUI

struct SettingsSheet: View {
    @Environment(Fleet.self) private var fleet
    @Environment(\.dismiss) private var dismiss
    @AppStorage("diff.wrap") private var wrapDiffs = true
    @AppStorage("file.wrap") private var wrapFiles = false
    @AppStorage("dictation.engine") private var dictationEngine = Dictation.Engine.phone
    @AppStorage("dictation.language") private var dictationLanguage = ""
    @State private var languages: [Locale] = []

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
                Section {
                    Picker("Transcribe On", selection: $dictationEngine) {
                        Text("iPhone").tag(Dictation.Engine.phone)
                        Text("Mac").tag(Dictation.Engine.mac)
                    }
                    if dictationEngine == .phone, !languages.isEmpty {
                        Picker("Language", selection: $dictationLanguage) {
                            Text("Automatic").tag("")
                            ForEach(languages, id: \.identifier) { locale in
                                Text(Locale.current.localizedString(forIdentifier: locale.identifier) ?? locale.identifier).tag(locale.identifier)
                            }
                        }
                    }
                } header: {
                    Text("Voice Input")
                } footer: {
                    Text(dictationEngine == .phone
                        ? "The iPhone’s own speech model: private, works offline, words appear as you speak. iOS downloads each language the first time."
                        : "What you say is sent to your Mac when you stop, and written out by the voice model picked there (YourHarness → Settings → Voice). Better with code words and mixed languages; needs your Mac connected.")
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
            .task { languages = await Dictation.languages() }
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
