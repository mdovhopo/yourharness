import SwiftUI

struct ThreadListView: View {
    @Environment(Store.self) private var store
    @State private var openShelves: Set<String> = []
    /// The thread being renamed, and the title being typed.
    @State private var renaming: ThreadInfo?
    @State private var title = ""
    @State private var deleting: ThreadInfo?
    private static let env = ProcessInfo.processInfo.environment

    var body: some View {
        Group {
            if store.state != .connected, !store.everConnected, store.sections.isEmpty, case .waiting = store.state {
                Unreachable()
            } else {
                list
            }
        }
        .navigationTitle("Threads")
        .navigationSubtitle(store.state == .connected ? store.serverName ?? "" : "")
    }

    private var list: some View {
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
        .refreshable { store.reconnect() }
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

/// The Mac can't be reached and never was this time: what to do about it.
private struct Unreachable: View {
    @Environment(Store.self) private var store

    var body: some View {
        ContentUnavailableView {
            Label("Can't Reach Your Mac", systemImage: "desktopcomputer.trianglebadge.exclamationmark")
        } description: {
            Text("Open YourHarness on your Mac with the phone server turned on. The phone keeps trying \(retry).")
            Text(store.serverAddress).font(.footnote.monospaced())
        } actions: {
            Button("Try Again") { store.reconnect() }
                .buttonStyle(.borderedProminent)
            Button("Mac Settings") { store.tab = .mac }
        }
    }

    private var retry: String {
        if case .waiting(_, let s) = store.state { return "every few seconds (next in \(s) s)" }
        return "every few seconds"
    }
}

/// The dock's search: threads whose titles match, then rows where the words were said, from
/// the Mac's index. A result opens its thread in Threads.
struct SearchView: View {
    @Environment(Store.self) private var store
    @State private var query = ProcessInfo.processInfo.environment["YH_PHONE_SEARCH"] ?? ""

    var body: some View {
        results
            .navigationTitle("Search")
            .searchable(text: $query, prompt: "Titles and conversations")
            .task(id: query) {
                // Asked once typing pauses.
                try? await Task.sleep(for: .milliseconds(300))
                guard !Task.isCancelled else { return }
                store.search(query)
                if !query.isEmpty { Perf.shared.log("SEARCH \(query)") }
            }
            .onChange(of: store.searchHits) { _, hits in
                if !hits.isEmpty { Perf.shared.log("SEARCH found \(hits.count): \(hits[0].snippet.prefix(60))") }
            }
    }

    private var results: some View {
        let words = query.trimmingCharacters(in: .whitespaces)
        let titled = store.sections.flatMap { $0.live + $0.settled }.filter { $0.title.localizedCaseInsensitiveContains(words) }
        let hits = store.searchQuery == words ? store.searchHits : []
        return List {
            if !titled.isEmpty {
                Section("Threads") {
                    ForEach(titled) { t in
                        Button { store.show(t.id) } label: { ThreadRow(thread: t).contentShape(.rect) }
                            .buttonStyle(.plain)
                    }
                }
            }
            if !hits.isEmpty {
                Section("In Conversations") {
                    ForEach(hits) { hit in
                        Button { store.show(hit.thread) } label: {
                            VStack(alignment: .leading, spacing: 3) {
                                Text(store.threads[hit.thread]?.title ?? "Thread").font(.subheadline.weight(.semibold)).lineLimit(1)
                                Text(Self.marked(hit.snippet)).font(.subheadline).foregroundStyle(.secondary).lineLimit(3)
                            }
                            .padding(.vertical, 2)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .contentShape(.rect)
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .overlay {
            if words.isEmpty {
                ContentUnavailableView("Search Threads", systemImage: "magnifyingglass", description: Text("Thread titles, and what was said in them, on your Mac."))
            } else if titled.isEmpty, hits.isEmpty {
                if store.canSearch, store.searchQuery == words, words.count >= 2 {
                    ContentUnavailableView.search(text: words)
                } else if !store.canSearch, store.state == .connected {
                    ContentUnavailableView("Titles Only", systemImage: "magnifyingglass", description: Text("Searching conversations needs a newer YourHarness on the Mac."))
                }
            }
        }
    }

    /// The words found, in `[brackets]` from the Mac, in bold.
    static func marked(_ snippet: String) -> AttributedString {
        var out = AttributedString()
        var rest = Substring(snippet)
        while let open = rest.firstIndex(of: "["), let close = rest[open...].firstIndex(of: "]") {
            out += AttributedString(String(rest[..<open]))
            var found = AttributedString(String(rest[rest.index(after: open)..<close]))
            found.inlinePresentationIntent = .stronglyEmphasized
            found.foregroundColor = .primary
            out += found
            rest = rest[rest.index(after: close)...]
        }
        out += AttributedString(String(rest))
        return out
    }
}

/// A usage window as the Mac's footer shows it: how much is used, when it starts over.
struct LimitRow: View {
    let window: LimitWindow

    /// Its reset time passed since the Mac last heard: what it says is from before.
    private var stale: Bool { window.resetsAt.map { $0 < Date().timeIntervalSince1970 } ?? false }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(Self.name(window.name))
                Spacer()
                Text(window.used, format: .percent.precision(.fractionLength(0)))
                    .monospacedDigit()
                    .foregroundStyle(!stale && window.used >= 0.9 ? .red : .secondary)
            }
            ProgressView(value: min(max(window.used, 0), 1))
                .tint(stale ? .gray : window.used >= 0.9 ? .red : window.used >= 0.7 ? .orange : .accentColor)
            if let at = window.resetsAt {
                let date = Date(timeIntervalSince1970: at)
                Group {
                    if stale {
                        Text("Started over \(date, format: .relative(presentation: .named)); not heard from since")
                    } else {
                        Text("Resets \(date, format: .relative(presentation: .named))")
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 2)
        .opacity(stale ? 0.7 : 1)
        .accessibilityElement(children: .combine)
    }

    static func name(_ raw: String) -> String {
        switch raw {
        case "five_hour": "5-hour limit"
        case "seven_day": "Weekly limit"
        case "seven_day_opus": "Weekly · Opus"
        case "seven_day_sonnet": "Weekly · Sonnet"
        default: raw.replacingOccurrences(of: "_", with: " ").capitalized
        }
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

/// The dock's Mac page: where the phone connects, and how that's going.
struct MacView: View {
    @Environment(Store.self) private var store
    @State private var address = ""

    var body: some View {
        Form {
            Section {
                LabeledContent("Status", value: status)
                if let name = store.serverName, store.state == .connected {
                    LabeledContent("Mac", value: name)
                    LabeledContent("Protocol", value: "\(store.serverProtocol)")
                }
                if store.state != .connected {
                    Button("Try Again") { store.reconnect() }
                }
            }
            Section {
                TextField(Store.defaultAddress, text: $address)
                    .keyboardType(.URL)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .font(.body.monospaced())
                Button("Connect") { store.connect(to: address.trimmingCharacters(in: .whitespaces)) }
                    .disabled(address == store.serverAddress || URL(string: address)?.scheme.map { ["ws", "wss"].contains($0) } != true)
            } header: {
                Text("Mac address")
            } footer: {
                Text("Start YourHarness on your Mac with YH_SERVER, and paste the address it prints, token and all. The Simulator reaches it at 127.0.0.1.")
            }
        }
        .navigationTitle("Mac")
        .onAppear { address = store.serverAddress }
    }

    private var status: String {
        switch store.state {
        case .connecting: "Connecting"
        case .connected: "Connected"
        case .waiting(let reason, _): reason
        }
    }
}

/// The dock's Usage page: Claude's limits as the Mac last heard them.
struct UsageView: View {
    @Environment(Store.self) private var store

    var body: some View {
        List {
            if !store.limits.isEmpty {
                Section {
                    ForEach(store.limits) { LimitRow(window: $0) }
                } footer: {
                    Text("As your Mac last heard from Claude. It hears again with each turn.")
                }
            }
        }
        .listStyle(.insetGrouped)
        .overlay {
            if store.limits.isEmpty {
                ContentUnavailableView("No Usage Yet", systemImage: "gauge.with.dots.needle.33percent", description: Text(store.state == .connected ? "Your Mac hasn't heard Claude's limits yet. They come with its next turn." : "Connect to your Mac to see Claude's limits."))
            }
        }
        .navigationTitle("Usage")
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
