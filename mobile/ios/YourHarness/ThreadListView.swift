import SwiftUI

struct ThreadListView: View {
    @Environment(Store.self) private var store
    @State private var showsServer = false
    @State private var openShelves: Set<String> = []

    var body: some View {
        List {
            if store.state != .connected {
                ConnectionRow(state: store.state, address: store.serverAddress)
            }
            ForEach(store.sections) { section in
                Section(section.project.name) {
                    ForEach(section.live) { thread in
                        NavigationLink(value: thread.id) { ThreadRow(thread: thread) }
                    }
                    if !section.settled.isEmpty {
                        DisclosureGroup(isExpanded: shelfBinding(section.id)) {
                            ForEach(section.settled) { thread in
                                NavigationLink(value: thread.id) { ThreadRow(thread: thread) }
                            }
                        } label: {
                            Label("Settled · \(section.settled.count)", systemImage: "tray")
                                .foregroundStyle(.secondary)
                        }
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .overlay {
            if store.state == .connected, store.sections.isEmpty {
                ContentUnavailableView("No Threads", systemImage: "bubble.left.and.text.bubble.right", description: Text("Threads you start on your Mac show up here."))
            }
        }
        .navigationTitle("Threads")
        .navigationSubtitle(store.state == .connected ? store.serverName ?? "" : "")
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button("Server", systemImage: "desktopcomputer") { showsServer = true }
            }
        }
        .refreshable { store.reconnect() }
        .sheet(isPresented: $showsServer) { ServerSheet() }
    }

    private func shelfBinding(_ id: String) -> Binding<Bool> {
        Binding(get: { openShelves.contains(id) }, set: { open in
            if open { openShelves.insert(id) } else { openShelves.remove(id) }
        })
    }
}

struct ThreadRow: View {
    let thread: ThreadInfo

    var body: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 4) {
                    if thread.pinned {
                        Image(systemName: "pin.fill").font(.caption2).foregroundStyle(.orange)
                            .accessibilityLabel("Pinned")
                    }
                    Text(thread.title).lineLimit(1)
                }
                Text(Self.subtitle(thread))
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
            if thread.active {
                Image(systemName: "circle.fill")
                    .font(.system(size: 8))
                    .foregroundStyle(.green)
                    .symbolEffect(.pulse, options: .repeating)
                    .accessibilityLabel("Working")
            }
        }
        .padding(.vertical, 2)
    }

    private static let relative: RelativeDateTimeFormatter = {
        let f = RelativeDateTimeFormatter()
        f.unitsStyle = .abbreviated
        return f
    }()

    static func subtitle(_ t: ThreadInfo) -> String {
        let date = Date(timeIntervalSince1970: t.updatedAt / 1000)
        let when = Date().timeIntervalSince(date) < 60 ? "now" : relative.localizedString(for: date, relativeTo: Date())
        guard let model = t.model else { return when }
        return "\(when) · \(model.replacingOccurrences(of: "claude-", with: ""))"
    }
}

struct ConnectionRow: View {
    let state: Connection.State
    let address: String

    var body: some View {
        HStack(spacing: 12) {
            if case .waiting = state {
                Image(systemName: "wifi.exclamationmark").foregroundStyle(.orange)
            } else {
                ProgressView()
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.headline)
                Text(address).font(.footnote.monospaced()).foregroundStyle(.secondary).lineLimit(1)
            }
        }
        .padding(.vertical, 4)
    }

    private var title: String {
        switch state {
        case .connecting: "Connecting…"
        case .connected: "Connected"
        case .waiting(let reason, let retryIn): "\(reason) · retrying in \(retryIn) s"
        }
    }
}

struct ServerSheet: View {
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var address = ""

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField(Store.defaultAddress, text: $address)
                        .keyboardType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .font(.body.monospaced())
                } header: {
                    Text("Mac address")
                } footer: {
                    Text("Run yh-devserver on your Mac. The Simulator reaches it at 127.0.0.1.")
                }
                Section {
                    LabeledContent("Status", value: status)
                    if let name = store.serverName, store.state == .connected {
                        LabeledContent("Mac", value: name)
                    }
                }
            }
            .navigationTitle("Server")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", role: .cancel) { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Connect", role: .confirm) {
                        store.connect(to: address.trimmingCharacters(in: .whitespaces))
                        dismiss()
                    }
                    .disabled(URL(string: address)?.scheme.map { ["ws", "wss"].contains($0) } != true)
                }
            }
            .onAppear { address = store.serverAddress }
        }
        .presentationDetents([.medium])
    }

    private var status: String {
        switch store.state {
        case .connecting: "Connecting"
        case .connected: "Connected"
        case .waiting(let reason, _): reason
        }
    }
}
