import HarnessCore
import SwiftUI

/// Import Cookies…: pick a browser, then the sites whose logins to bring over.
struct CookieImportSheet: View {
    let browser: BrowserSession
    @Environment(\.dismiss) private var dismiss
    @State private var sources: [CookieImport.Source] = []
    @State private var source: CookieImport.Source?
    @State private var phase = Phase.choosing
    @State private var cookies: [ImportedCookie] = []
    @State private var chosen: Set<String> = []
    @State private var search = ""

    enum Phase: Equatable {
        case choosing, reading, picking, importing
        case done(Int)
        case failed(String, needsFullDiskAccess: Bool)
    }

    private var sites: [(site: String, count: Int)] {
        Dictionary(grouping: cookies, by: \.site)
            .map { ($0.key, $0.value.count) }
            .filter { search.isEmpty || $0.site.localizedCaseInsensitiveContains(search) }
            .sorted { $0.site < $1.site }
    }

    private var chosenCount: Int { cookies.count { chosen.contains($0.site) } }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Import Cookies")
                .font(.title2.weight(.semibold))
            Text("Bring logins from another browser into this project's browser, so Claude can test pages that need you signed in. Only the sites you pick are copied; nothing leaves your Mac.")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Picker("From", selection: $source) {
                Text("Choose a browser…").tag(CookieImport.Source?.none)
                ForEach(sources) { source in
                    Text(source.label).tag(CookieImport.Source?.some(source))
                }
            }
            .disabled(phase == .reading || phase == .importing)
            .onChange(of: source) { Task { await load() } }
            content
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            HStack {
                if phase == .picking {
                    Button("Select All") { chosen.formUnion(sites.map(\.site)) }
                    Button("Select None") { chosen.removeAll() }
                        .disabled(chosen.isEmpty)
                }
                Spacer()
                Button(isDone ? "Done" : "Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                if !isDone {
                    Button(chosenCount == 0 ? "Import" : "Import \(chosenCount) Cookie\(chosenCount == 1 ? "" : "s")") {
                        Task { await runImport() }
                    }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.glassProminent)
                    .disabled(phase != .picking || chosenCount == 0)
                }
            }
        }
        .padding(20)
        .frame(width: 520, height: 520)
        .task { sources = CookieImport.sources() }
    }

    private var isDone: Bool {
        if case .done = phase { true } else { false }
    }

    @ViewBuilder
    private var content: some View {
        switch phase {
        case .choosing:
            Text("Reading Chrome, Arc, Brave, Edge or Vivaldi cookies asks you once for access to the key they're encrypted with. Safari's need Full Disk Access.")
                .font(.callout)
                .foregroundStyle(.tertiary)
                .fixedSize(horizontal: false, vertical: true)
        case .reading:
            ProgressView("Reading cookies…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .importing:
            ProgressView("Importing…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .picking:
            VStack(spacing: 8) {
                TextField("Filter sites", text: $search)
                    .textFieldStyle(.roundedBorder)
                List(sites, id: \.site) { site in
                    Toggle(isOn: Binding(get: { chosen.contains(site.site) }, set: { if $0 { chosen.insert(site.site) } else { chosen.remove(site.site) } })) {
                        HStack {
                            Text(site.site)
                            Spacer()
                            Text("\(site.count)")
                                .monospacedDigit()
                                .foregroundStyle(.secondary)
                        }
                    }
                }
                .listStyle(.bordered)
                .alternatingRowBackgrounds()
            }
        case .done(let count):
            ContentUnavailableView {
                Label("Imported \(count) Cookie\(count == 1 ? "" : "s")", systemImage: "checkmark.circle")
            } description: {
                Text("Open pages reloaded with them. Clear Browsing Data in the browser's menu removes them again.")
            }
        case .failed(let message, let needsAccess):
            VStack(alignment: .leading, spacing: 10) {
                Label(message, systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if needsAccess {
                    Button("Open Privacy Settings") { CookieImport.openFullDiskAccessSettings() }
                        .buttonStyle(.glass)
                }
            }
        }
    }

    private func load() async {
        guard let source else {
            phase = .choosing
            return
        }
        phase = .reading
        chosen.removeAll()
        do {
            cookies = try await CookieImport.read(source)
            phase = cookies.isEmpty ? .failed("\(source.label) has no cookies to import.", needsFullDiskAccess: false) : .picking
        } catch let error as CookieImport.SafariAccessError {
            phase = .failed(error.description, needsFullDiskAccess: true)
        } catch {
            phase = .failed(String(describing: error), needsFullDiskAccess: false)
        }
    }

    private func runImport() async {
        let picked = cookies.filter { chosen.contains($0.site) }
        phase = .importing
        let count = await CookieImport.store(picked, in: browser.projectID)
        // Cookie values aren't kept around once they're in the store.
        cookies = []
        for tab in browser.tabs { tab.webView.reload() }
        phase = .done(count)
    }
}
