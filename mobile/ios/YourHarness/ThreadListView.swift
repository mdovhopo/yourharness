import SwiftUI

struct ThreadListView: View {
    @Environment(Store.self) private var store
    @State private var showsServer = false
    @State private var openShelves: Set<String> = []
    /// The thread being renamed, and the title being typed.
    @State private var renaming: ThreadInfo?
    @State private var title = ""
    @State private var deleting: ThreadInfo?
    private static let env = ProcessInfo.processInfo.environment

    var body: some View {
        List {
            if store.state != .connected {
                ConnectionRow(state: store.state, address: store.serverAddress)
            }
            ForEach(store.sections) { section in
                Section {
                    ForEach(section.live) { thread in
                        NavigationLink(value: thread.id) { ThreadRow(thread: thread) }
                            .modifier(ThreadActions(thread: thread, renaming: $renaming, title: $title, deleting: $deleting))
                    }
                    if !section.settled.isEmpty {
                        DisclosureGroup(isExpanded: shelfBinding(section.id)) {
                            ForEach(section.settled) { thread in
                                NavigationLink(value: thread.id) { ThreadRow(thread: thread) }
                                    .modifier(ThreadActions(thread: thread, renaming: $renaming, title: $title, deleting: $deleting))
                            }
                        } label: {
                            Label("Settled · \(section.settled.count)", systemImage: "tray")
                                .foregroundStyle(.secondary)
                        }
                    }
                } header: {
                    HStack {
                        Text(section.project.name)
                        Spacer()
                        if store.canControl {
                            Button("New Thread", systemImage: "square.and.pencil") { store.newThread(in: section.project.id) }
                                .labelStyle(.iconOnly)
                                .font(.body)
                                .accessibilityLabel("New thread in \(section.project.name)")
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
        .alert("Rename Thread", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField("Title", text: $title)
            Button("Cancel", role: .cancel) { renaming = nil }
            Button("Rename") {
                if let t = renaming, !title.trimmingCharacters(in: .whitespaces).isEmpty { store.rename(t.id, to: title) }
                renaming = nil
            }
        }
        .confirmationDialog("Delete “\(deleting?.title ?? "")”?", isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }), titleVisibility: .visible) {
            Button("Delete", role: .destructive) {
                if let t = deleting { store.delete(t.id) }
                deleting = nil
            }
        } message: {
            Text("It goes from your Mac too. Until YourHarness quits, ⌘Z on the Mac brings it back.")
        }
        // `YH_PHONE_NEW_THREAD=<first message>`: a new thread in `YH_PHONE_PROJECT` (a project's
        // name) or the first project, as its header's button does.
        .task(id: store.canControl && !store.projects.isEmpty) {
            guard store.canControl, let text = Self.env["YH_PHONE_NEW_THREAD"], !madeThread else { return }
            let wanted = Self.env["YH_PHONE_PROJECT"]
            guard let project = store.projects.first(where: { wanted == nil || $0.name == wanted }) else { return }
            madeThread = true
            try? await Task.sleep(for: .seconds(1))
            Perf.shared.log("PHONE new thread in \(project.name): \(text)")
            store.newThread(in: project.id, text: text)
        }
    }

    @State private var madeThread = false

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
            if thread.isWaiting {
                Image(systemName: "hand.raised.fill")
                    .font(.footnote)
                    .foregroundStyle(.orange)
                    .accessibilityLabel("Needs you")
            } else if thread.isRunning {
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
                    Text("Start YourHarness on your Mac with YH_SERVER, and paste the address it prints, token and all. The Simulator reaches it at 127.0.0.1.")
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

/// What a thread's row offers, as the Mac's sidebar does: pin and settle by swiping, rename and
/// delete from its menu.
private struct ThreadActions: ViewModifier {
    let thread: ThreadInfo
    @Binding var renaming: ThreadInfo?
    @Binding var title: String
    @Binding var deleting: ThreadInfo?
    @Environment(Store.self) private var store

    func body(content: Content) -> some View {
        if store.canControl {
            content
                .swipeActions(edge: .leading) {
                    Button(thread.pinned ? "Unpin" : "Pin", systemImage: thread.pinned ? "pin.slash" : "pin") { store.pin(thread.id, !thread.pinned) }
                        .tint(.orange)
                }
                .swipeActions(edge: .trailing) {
                    Button(thread.settled ? "Bring Back" : "Settle", systemImage: thread.settled ? "tray.and.arrow.up" : "checkmark.circle") { store.settle(thread.id, !thread.settled) }
                        .tint(.indigo)
                }
                .contextMenu {
                    Button("Rename…", systemImage: "pencil") {
                        title = thread.title
                        renaming = thread
                    }
                    Button(thread.pinned ? "Unpin" : "Pin", systemImage: thread.pinned ? "pin.slash" : "pin") { store.pin(thread.id, !thread.pinned) }
                    Button(thread.settled ? "Bring Back" : "Settle", systemImage: thread.settled ? "tray.and.arrow.up" : "checkmark.circle") { store.settle(thread.id, !thread.settled) }
                    Divider()
                    Button("Delete…", systemImage: "trash", role: .destructive) { deleting = thread }
                }
        } else {
            content
        }
    }
}
