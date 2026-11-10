import SwiftUI

struct ThreadListView: View {
    @Environment(Store.self) private var store
    @Environment(Fleet.self) private var fleet
    @State private var openShelves: Set<String> = []
    /// The thread being renamed, and the title being typed.
    @State private var renaming: ThreadTarget?
    @State private var title = ""
    @State private var deleting: ThreadTarget?
    /// The project whose PRs are open in a sheet, and its Mac (the one shown, if nil).
    @State private var pullsFor: Project?
    @State private var pullsOn: Store?
    /// New Thread's sheet, open on this project, and its Mac (the one shown, if nil).
    @State private var composingIn: Project?
    @State private var composingOn: Store?
    /// The phone's settings (`YH_PHONE_SETTINGS=1` at launch).
    @State private var settingsOpen = ProcessInfo.processInfo.environment["YH_PHONE_SETTINGS"] == "1"
    private static let env = ProcessInfo.processInfo.environment

    var body: some View {
        Group {
            if !fleet.mixesMacs, store.state != .connected, !store.everConnected, store.sections.isEmpty, case .waiting = store.state {
                Unreachable()
            } else {
                list
            }
        }
        .navigationTitle(fleet.stores.count > 1 && !fleet.mixesMacs ? store.displayName : "Threads")
        .navigationSubtitle(subtitle)
        // Inline with several Macs shown one at a time: the title shows its menu's chevron.
        .navigationBarTitleDisplayMode(fleet.stores.count > 1 && !fleet.mixesMacs ? .inline : .automatic)
        .toolbarTitleMenu {
            // Several Macs: the title is the Mac shown, and switches to another.
            if fleet.stores.count > 1, !fleet.mixesMacs { MacMenu() }
        }
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button("Settings", systemImage: "gearshape") { settingsOpen = true }
            }
            // Reconnecting with the list on screen: a spinner up here, nothing in the list moves.
            if let mac = reconnecting {
                ToolbarItem(placement: .topBarTrailing) { ConnectionIndicator().environment(mac) }
            }
            if fleet.mixesMacs {
                // Every Mac listed: New Thread asks which Mac first.
                let macs = fleet.stores.filter(\.offersControl)
                if !macs.isEmpty {
                    ToolbarItem(placement: .topBarTrailing) {
                        Menu("New Thread", systemImage: "square.and.pencil") {
                            Section("New Thread On") {
                                ForEach(macs) { mac in
                                    Button(mac.displayName, systemImage: "desktopcomputer") { compose(on: mac, in: mac.newThreadProject) }
                                        .disabled(mac.projects.isEmpty)
                                }
                            }
                        }
                    }
                }
            } else if store.offersControl {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("New Thread", systemImage: "square.and.pencil") { compose(on: nil, in: store.newThreadProject) }
                        .disabled(store.projects.isEmpty)
                }
            }
        }
        // Here, not on the list: a Mac never reached swaps the list for `Unreachable` at each try,
        // which took the sheet down with it.
        .sheet(isPresented: $settingsOpen) { SettingsSheet() }
    }

    /// The Mac whose spinner shows: not connected, its list on screen.
    private var reconnecting: Store? {
        if fleet.mixesMacs { return fleet.stores.first { $0.state != .connected && !$0.sections.isEmpty } }
        return store.state != .connected && !store.sections.isEmpty ? store : nil
    }

    private func compose(on mac: Store?, in project: Project?) {
        composingOn = mac
        composingIn = project
    }

    private var subtitle: String {
        guard fleet.stores.count > 1 else { return store.serverName ?? "" }
        if fleet.mixesMacs {
            let waiting = fleet.stores.reduce(0) { $0 + $1.waitingCount }
            return waiting > 0 ? "\(fleet.stores.count) Macs · \(waiting) waiting on you" : "\(fleet.stores.count) Macs"
        }
        let others = fleet.waitingElsewhere
        return others > 0 ? "\(others) waiting on another Mac" : "\(fleet.stores.count) Macs"
    }

    /// The open thread, when the list stands beside it (iPad): a row is chosen, not pushed.
    var selection: Binding<String?>?

    @ViewBuilder private var list: some View {
        if let selection {
            styled(List(selection: selection) { rows })
        } else {
            styled(List { rows })
        }
    }

    @ViewBuilder private var rows: some View {
        if fleet.mixesMacs { mixedRows } else { macRows }
    }

    @ViewBuilder private var macRows: some View {
            if store.state != .connected, store.sections.isEmpty {
                ConnectionRow(state: store.state, address: store.shownAddress)
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
                        if store.offersSearch {
                            Button("Pull Requests", systemImage: "arrow.triangle.pull") { pullsOn = nil; pullsFor = section.project }
                                .labelStyle(.iconOnly)
                                .font(.body)
                                .accessibilityLabel("Pull requests in \(section.project.name)")
                        }
                    }
                }
            }
    }

    /// Every Mac's threads in one list: projects of the same name together, each row saying its Mac.
    /// A row opens its thread on its own Mac, which becomes the one shown.
    @ViewBuilder private var mixedRows: some View {
        // A Mac that hasn't sent its list this time: how it's getting on, above the rest.
        ForEach(fleet.stores.filter { $0.state != .connected && $0.sections.isEmpty }) { mac in
            ConnectionRow(state: mac.state, address: "\(mac.displayName) · \(mac.shownAddress)")
        }
        ForEach(fleet.mixedSections) { section in
            Section {
                ForEach(section.live) { mixedRow($0) }
                if !section.settled.isEmpty {
                    DisclosureGroup(isExpanded: shelfBinding("mixed:" + section.id)) {
                        ForEach(section.settled) { mixedRow($0) }
                    } label: {
                        Label("Settled · \(section.settled.count)", systemImage: "tray")
                            .foregroundStyle(.secondary)
                    }
                }
            } header: {
                HStack {
                    Text(section.name)
                    Spacer()
                    let withPulls = section.projects.filter { $0.mac.offersSearch }
                    if withPulls.count == 1, let only = withPulls.first {
                        Button("Pull Requests", systemImage: "arrow.triangle.pull") { pullsOn = only.mac; pullsFor = only.project }
                            .labelStyle(.iconOnly)
                            .font(.body)
                            .accessibilityLabel("Pull requests in \(section.name)")
                    } else if withPulls.count > 1 {
                        // The same project on several Macs: its PRs as each Mac sees them.
                        Menu("Pull Requests", systemImage: "arrow.triangle.pull") {
                            Section("Pull Requests From") {
                                ForEach(withPulls, id: \.mac.id) { p in
                                    Button(p.mac.displayName, systemImage: "desktopcomputer") { pullsOn = p.mac; pullsFor = p.project }
                                }
                            }
                        }
                        .labelStyle(.iconOnly)
                        .font(.body)
                        .accessibilityLabel("Pull requests in \(section.name)")
                    }
                }
            }
        }
    }

    private func mixedRow(_ item: Fleet.MixedThread) -> some View {
        let open = selection != nil && item.mac === fleet.current && item.mac.path.last == item.thread.id
        return Button { fleet.open(item.thread.id, on: item.mac) } label: {
            HStack(spacing: 8) {
                ThreadRow(thread: item.thread, mac: item.mac.displayName)
                // Drawn as a pushed row's chevron (iPhone); beside the open thread (iPad), none.
                if selection == nil {
                    Image(systemName: "chevron.forward")
                        .font(.footnote.weight(.semibold))
                        .foregroundStyle(.tertiary)
                }
            }
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .listRowBackground(open ? Color(.systemFill) : Color(.secondarySystemGroupedBackground))
        .modifier(ThreadActions(thread: item.thread, renaming: $renaming, title: $title, deleting: $deleting))
        .environment(item.mac)
    }

    /// The list's look, alerts and hooks, whichever way it chooses rows.
    private func styled(_ content: some View) -> some View {
        content
        .listStyle(.insetGrouped)
        .overlay {
            if fleet.mixesMacs ? fleet.stores.allSatisfy({ $0.state == .connected && $0.sections.isEmpty }) : (store.state == .connected && store.sections.isEmpty) {
                ContentUnavailableView("No Threads", systemImage: "bubble.left.and.text.bubble.right", description: Text("Threads you start on your Mac show up here."))
            }
        }
        .refreshable { if fleet.mixesMacs { fleet.stores.forEach { $0.reconnect() } } else { store.reconnect() } }
        .sheet(item: $pullsFor) { PullsSheet(project: $0).environment(pullsOn ?? store) }
        .sheet(item: $composingIn) { project in
            let mac = composingOn ?? store
            NewThreadSheet(project: project, mac: fleet.mixesMacs ? mac.displayName : nil).environment(mac)
        }
        // `YH_PHONE_PULLS=<project name>`: its PRs open, as its header's button does.
        .task(id: store.projects.count) {
            guard let name = Self.env["YH_PHONE_PULLS"], pullsFor == nil, let p = store.projects.first(where: { $0.name == name }) else { return }
            pullsFor = p
        }
        .alert("Rename Thread", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField("Title", text: $title)
            Button("Cancel", role: .cancel) { renaming = nil }
            Button("Rename") {
                if let t = renaming, !title.trimmingCharacters(in: .whitespaces).isEmpty { t.store.rename(t.thread.id, to: title) }
                renaming = nil
            }
        }
        .confirmationDialog("Delete “\(deleting?.thread.title ?? "")”?", isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }), titleVisibility: .visible) {
            Button("Delete", role: .destructive) {
                if let t = deleting { t.store.delete(t.thread.id) }
                deleting = nil
            }
        } message: {
            Text("It goes from your Mac too. Until YourHarness quits, ⌘Z on the Mac brings it back.")
        }
        // `YH_PHONE_NEW_THREAD=<first message>`: a new thread in `YH_PHONE_PROJECT` (a project's
        // name) or the first project, as New Thread's sheet sends it.
        // `YH_PHONE_COMPOSE=<draft>`: New Thread pressed, its sheet on `YH_PHONE_PROJECT` or the
        // last used project, the draft waiting in it (for screenshots).
        .task(id: store.canControl && !store.projects.isEmpty) {
            guard store.canControl, let draft = Self.env["YH_PHONE_COMPOSE"], composingIn == nil, !madeThread else { return }
            let wanted = Self.env["YH_PHONE_PROJECT"]
            guard let project = wanted.flatMap({ w in store.projects.first { $0.name == w } }) ?? store.newThreadProject else { return }
            madeThread = true
            if !draft.isEmpty { store.newThreadDrafts[project.id] = draft }
            composingIn = project
        }
        .task(id: store.canControl && !store.projects.isEmpty) {
            guard store.canControl, let text = Self.env["YH_PHONE_NEW_THREAD"], !madeThread else { return }
            let wanted = Self.env["YH_PHONE_PROJECT"]
            guard let project = store.projects.first(where: { wanted == nil || $0.name == wanted }) else { return }
            madeThread = true
            try? await Task.sleep(for: .seconds(1))
            let (model, effort) = (Self.env["YH_PHONE_NEW_MODEL"], Self.env["YH_PHONE_NEW_EFFORT"])
            Perf.shared.log("PHONE new thread in \(project.name): \(text) · \(model ?? "default") · \(effort ?? "default")")
            store.newThread(in: project.id, text: text, model: model, effort: effort)
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
    @Environment(Fleet.self) private var fleet
    @State private var scanning = false

    var body: some View {
        if let failed = store.pairingFailed {
            ContentUnavailableView {
                Label("Couldn't Pair", systemImage: "exclamationmark.triangle")
            } description: {
                Text(failed)
            } actions: {
                Button("Scan Again") { scanning = true }
                    .buttonStyle(.borderedProminent)
                if fleet.stores.count > 1 { Button("Your Macs") { fleet.tab = .mac } }
            }
            .sheet(isPresented: $scanning) { ScannerSheet() }
        } else if store.serverAddress == Store.defaultAddress {
            // A new install, paired with nothing yet.
            ContentUnavailableView {
                Label("Pair with Your Mac", systemImage: "qrcode.viewfinder")
            } description: {
                Text("On the Mac, open YourHarness → Settings → Phone, turn on Let phones connect, and scan the code it shows. Your phone and Mac need to be on the same Wi‑Fi.")
            } actions: {
                Button("Scan the Mac's Code") { scanning = true }
                    .buttonStyle(.borderedProminent)
                Button("Other Ways") { fleet.tab = .mac }
            }
            .sheet(isPresented: $scanning) { ScannerSheet() }
        } else {
            unreachable
        }
    }

    private var unreachable: some View {
        ContentUnavailableView {
            Label("Can't Reach Your Mac", systemImage: "desktopcomputer.trianglebadge.exclamationmark")
        } description: {
            Text("Open YourHarness on your Mac with the phone server turned on. Away from its Wi‑Fi, turn Tailscale on here and on the Mac. The phone keeps trying \(retry).")
            Text(store.shownAddress).font(.footnote.monospaced())
        } actions: {
            Button("Try Again") { store.reconnect() }
                .buttonStyle(.borderedProminent)
            Button(fleet.stores.count > 1 ? "Your Macs" : "Mac Settings") { fleet.tab = .mac }
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
            .navigationSubtitle(store.displayName)
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
    private var used: Double { min(max(window.used, 0), 1) }
    private var tint: Color { stale ? .gray : used >= 0.9 ? .red : used >= 0.7 ? .orange : .accentColor }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline) {
                Text(Self.name(window.name)).font(.body.weight(.medium))
                Spacer()
                Text(window.used, format: .percent.precision(.fractionLength(0)))
                    .font(.body.weight(.semibold))
                    .monospacedDigit()
                    .foregroundStyle(!stale && used >= 0.9 ? .red : .primary)
            }
            // A thick bar, as the Mac's footer draws it: easy to read at a glance.
            GeometryReader { box in
                ZStack(alignment: .leading) {
                    Capsule().fill(Color(.tertiarySystemFill))
                    Capsule().fill(tint).frame(width: max(box.size.width * used, used > 0 ? 10 : 0))
                }
            }
            .frame(height: 10)
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
        .padding(.vertical, 4)
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
    /// The Mac it's on, said first under its title (the list of every Mac).
    var mac: String? = nil

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
                Text(mac.map { "\($0) · \(Self.subtitle(thread))" } ?? Self.subtitle(thread))
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
            } else if thread.backgroundJobs > 0 {
                // The turn's over, something it started still runs (a dev server, a sub-agent).
                Image(systemName: "hourglass")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .accessibilityLabel(thread.backgroundJobs == 1 ? "1 job running" : "\(thread.backgroundJobs) jobs running")
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

/// Not connected this moment: a spinner while it tries, a warning while it waits to try again.
/// Tapping it tries now.
private struct ConnectionIndicator: View {
    @Environment(Store.self) private var store

    var body: some View {
        Button { store.reconnect() } label: {
            if case .waiting = store.state {
                Image(systemName: "wifi.exclamationmark").foregroundStyle(.orange)
            } else {
                ProgressView()
            }
        }
        .accessibilityLabel(accessibilityTitle)
    }

    private var accessibilityTitle: String {
        if case .waiting(let reason, _) = store.state { return "Can't reach your Mac: \(reason). Try again" }
        return "Connecting to your Mac"
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

/// The dock's Mac page: the Macs the phone knows, each connected; one is shown. Add one by the
/// address its YourHarness prints; swipe one away to forget it.
struct MacView: View {
    @Environment(Fleet.self) private var fleet
    @State private var address = ProcessInfo.processInfo.environment["YH_PHONE_ADD_MAC"] ?? ""
    @State private var scanning = false

    var body: some View {
        Form {
            Section {
                Button {
                    scanning = true
                } label: {
                    Label("Scan the Mac's Code", systemImage: "qrcode.viewfinder")
                }
            } footer: {
                Text("On the Mac, open YourHarness → Settings → Phone and turn on Let phones connect. Your phone and Mac need to be on the same Wi‑Fi.")
            }
            Section {
                ForEach(fleet.stores) { mac in
                    Button { fleet.select(mac) } label: { MacRow(mac: mac, shown: mac === fleet.current) }
                        .buttonStyle(.plain)
                        .swipeActions {
                            if fleet.stores.count > 1 {
                                Button("Forget", systemImage: "trash", role: .destructive) { fleet.remove(mac) }
                            }
                        }
                        .contextMenu {
                            if mac.state != .connected { Button("Try Again", systemImage: "arrow.clockwise") { mac.reconnect() } }
                            if fleet.stores.count > 1 { Button("Forget This Mac", systemImage: "trash", role: .destructive) { fleet.remove(mac) } }
                        }
                }
            } header: {
                Text(fleet.stores.count > 1 ? "Your Macs" : "Your Mac")
            } footer: {
                if fleet.stores.count > 1 { Text("All stay connected; tap one to show it. Only the one shown streams a thread.") }
            }
            Section {
                TextField("Pairing link or ws:// address", text: $address)
                    .keyboardType(.URL)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .font(.body.monospaced())
                Button("Add Mac") {
                    if fleet.add(address) { address = "" }
                }
                .disabled(URL(string: address)?.scheme.map { ["ws", "wss", "yourharness"].contains($0) } != true)
            } header: {
                Text("Add by Link")
            } footer: {
                Text("A yourharness://pair link from the Mac's code, or a ws:// address with its token.")
            }
        }
        .navigationTitle("Macs")
        .sheet(isPresented: $scanning) { ScannerSheet() }
        .task {
            // `YH_PHONE_ADD_MAC=<address>`: added as the button does.
            if ProcessInfo.processInfo.environment["YH_PHONE_ADD_MAC"] != nil, fleet.add(address) { address = "" }
        }
    }
}

/// A Mac in the list: its name, how it's connected, and what's going on there.
private struct MacRow: View {
    let mac: Store
    let shown: Bool

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "desktopcomputer")
                .font(.title3)
                .foregroundStyle(mac.state == .connected ? Color.accentColor : .secondary)
                .frame(width: 30)
            VStack(alignment: .leading, spacing: 2) {
                Text(mac.displayName).font(.body.weight(shown ? .semibold : .regular)).lineLimit(1)
                if let failed = mac.pairingFailed {
                    Text(failed).font(.subheadline).foregroundStyle(.orange).lineLimit(3)
                } else {
                    HStack(spacing: 8) {
                        Text(mac.pairing ? "Pairing…" : MacMenu.status(of: mac, waiting: false)).lineLimit(1)
                        if mac.state == .connected, let power = mac.power { BatteryLabel(power: power) }
                    }
                    .font(.subheadline).foregroundStyle(.secondary)
                }
                Text(mac.viaTailscale ? "\(mac.shownAddress) · Tailscale" : mac.shownAddress)
                    .font(.caption.monospaced()).foregroundStyle(.tertiary).lineLimit(1)
            }
            Spacer(minLength: 0)
            if mac.waitingCount > 0 {
                Label("\(mac.waitingCount)", systemImage: "hand.raised.fill").font(.footnote).foregroundStyle(.orange)
                    .accessibilityLabel("\(mac.waitingCount) waiting on you")
            }
            if shown {
                Image(systemName: "checkmark").font(.body.weight(.semibold)).foregroundStyle(Color.accentColor)
                    .accessibilityLabel("Shown")
            }
        }
        .contentShape(.rect)
        .padding(.vertical, 2)
    }
}

/// The Mac's battery in a Mac's row: its level as iOS draws its own, red when low and not charging.
private struct BatteryLabel: View {
    let power: Power

    var body: some View {
        let level = [0, 25, 50, 75, 100].min { abs($0 - power.percent) < abs($1 - power.percent) } ?? 100
        let low = power.percent <= 20 && !power.pluggedIn
        HStack(spacing: 4) {
            Image(systemName: power.charging ? "battery.100percent.bolt" : "battery.\(level)percent")
                .foregroundStyle(low ? Color.red : power.pluggedIn ? Color.green : Color.secondary)
            Text("\(power.percent)%").monospacedDigit()
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(power.charging ? "Battery \(power.percent)%, charging" : power.pluggedIn ? "Battery \(power.percent)%, on power" : "Battery \(power.percent)%")
    }
}

/// The Threads title's menu with several Macs: each, with what's going on there; the shown one ticked.
struct MacMenu: View {
    @Environment(Fleet.self) private var fleet

    var body: some View {
        ForEach(fleet.stores) { mac in
            Button {
                fleet.select(mac)
            } label: {
                Label {
                    Text(mac.displayName)
                    Text(Self.status(of: mac))
                } icon: {
                    Image(systemName: mac === fleet.current ? "checkmark" : mac.waitingCount > 0 ? "hand.raised.fill" : "desktopcomputer")
                }
            }
        }
        Divider()
        Button("Your Macs…", systemImage: "desktopcomputer.and.macbook") { fleet.tab = .mac }
    }

    /// "Connected · 2 waiting on you · 1 working", or why it isn't.
    static func status(of mac: Store, waiting: Bool = true) -> String {
        switch mac.state {
        case .connecting: return "Connecting…"
        case .waiting(let reason, _): return reason
        case .connected:
            var parts = ["Connected"]
            if waiting, mac.waitingCount > 0 { parts.append("\(mac.waitingCount) waiting on you") }
            if mac.runningCount > 0 { parts.append("\(mac.runningCount) working") }
            return parts.joined(separator: " · ")
        }
    }
}

/// The dock's Usage page: Claude's limits as the Macs last heard them, a section for each
/// subscription (one per Mac for now; Macs signed in to the same account share one), the shown
/// Mac's first.
struct UsageView: View {
    @Environment(Store.self) private var store
    @Environment(Fleet.self) private var fleet

    /// A subscription's limits and the Macs signed in to it.
    @MainActor struct SubscriptionGroup: Identifiable {
        let id: String
        let subscription: Subscription?
        var macs: [Store]
        /// Each window's freshest reading among its Macs: the latest reset, then the most used.
        var windows: [LimitWindow] {
            var best: [String: LimitWindow] = [:]
            var order: [String] = []
            for w in macs.flatMap({ $0.limits }) {
                guard let held = best[w.name] else { best[w.name] = w; order.append(w.name); continue }
                if (w.resetsAt ?? 0, w.used) > (held.resetsAt ?? 0, held.used) { best[w.name] = w }
            }
            return order.compactMap { best[$0] }
        }
    }

    private var groups: [SubscriptionGroup] {
        var out: [SubscriptionGroup] = []
        for mac in [store] + fleet.stores.filter({ $0 !== store }) {
            let key = mac.subscription.map { "sub:\($0.account)" } ?? "mac:\(mac.id)"
            if let i = out.firstIndex(where: { $0.id == key }) { out[i].macs.append(mac) }
            else { out.append(SubscriptionGroup(id: key, subscription: mac.subscription, macs: [mac])) }
        }
        return out.filter { !$0.windows.isEmpty }
    }

    var body: some View {
        let groups = groups
        List {
            ForEach(groups) { group in
                Section {
                    ForEach(group.windows) { LimitRow(window: $0) }
                } header: {
                    SubscriptionHeader(subscription: group.subscription, macs: group.macs.map { $0.displayName })
                }
            }
            if !groups.isEmpty {
                Section {} footer: {
                    Text("As your Macs last heard from Claude. They hear again with each turn.")
                }
            }
        }
        .listStyle(.insetGrouped)
        .overlay {
            if groups.isEmpty {
                ContentUnavailableView("No Usage Yet", systemImage: "gauge.with.dots.needle.33percent", description: Text(store.state == .connected ? "Your Mac hasn't heard Claude's limits yet. They come with its next turn." : "Connect to your Mac to see Claude's limits."))
            }
        }
        .navigationTitle("Usage")
    }
}

/// "Claude Max · a@b.c", and the Macs signed in to it.
private struct SubscriptionHeader: View {
    let subscription: Subscription?
    let macs: [String]

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 6) {
                Image(systemName: "person.crop.circle")
                Text(subscription?.plan ?? "Claude").fontWeight(.semibold)
                if let who = subscription?.email ?? subscription?.org {
                    Text(who).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                }
            }
            .font(.subheadline)
            .foregroundStyle(.primary)
            Text(macs.joined(separator: " · "))
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .textCase(nil)
        .accessibilityElement(children: .combine)
    }
}

/// What a thread's row offers, as the Mac's sidebar does: pin and settle by swiping, rename and
/// delete from its menu.
/// A thread an action waits on (renamed, deleted), and the Mac it's on.
struct ThreadTarget {
    let store: Store
    let thread: ThreadInfo
}

private struct ThreadActions: ViewModifier {
    let thread: ThreadInfo
    @Binding var renaming: ThreadTarget?
    @Binding var title: String
    @Binding var deleting: ThreadTarget?
    @Environment(Store.self) private var store

    func body(content: Content) -> some View {
        // What the Mac offers, not whether it's connected now: a reconnect mustn't rebuild every row.
        if store.offersControl {
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
                        renaming = ThreadTarget(store: store, thread: thread)
                    }
                    Button(thread.pinned ? "Unpin" : "Pin", systemImage: thread.pinned ? "pin.slash" : "pin") { store.pin(thread.id, !thread.pinned) }
                    Button(thread.settled ? "Bring Back" : "Settle", systemImage: thread.settled ? "tray.and.arrow.up" : "checkmark.circle") { store.settle(thread.id, !thread.settled) }
                    Divider()
                    Button("Delete…", systemImage: "trash", role: .destructive) { deleting = ThreadTarget(store: store, thread: thread) }
                }
        } else {
            content
        }
    }
}

/// A new thread's first message, and the project, model, effort and permissions it starts with.
/// Nothing is made on the Mac until it's sent; closed unsent, what's typed waits for that
/// project's next New Thread.
struct NewThreadSheet: View {
    /// The project it starts in; the sheet's picker can choose another.
    @State private var projectID: String
    /// The Mac it's made on, named in the title (the list of every Mac).
    private let mac: String?
    init(project: Project, mac: String? = nil) {
        _projectID = State(initialValue: project.id)
        self.mac = mac
    }
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var text = ""
    /// "" for the Mac's default.
    @State private var model = ""
    @State private var effort = ""
    @State private var mode = "auto"
    @FocusState private var focused: Bool

    private var project: Project? { store.projects.first { $0.id == projectID } }
    private var empty: Bool { text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    private var chosen: ModelOption? { store.models.first { $0.id == model } }
    private var efforts: [String] { chosen?.efforts ?? ["low", "medium", "high", "xhigh", "max"] }
    private var modes: [String] { store.modes.isEmpty ? ["auto", "ask", "acceptEdits", "plan", "bypass"] : store.modes }

    var body: some View {
        NavigationStack {
            Form {
                TextField("What are we working on?", text: $text, axis: .vertical)
                    .lineLimit(4...16)
                    .focused($focused)
                Section {
                    Picker(selection: $projectID) {
                        ForEach(store.projects) { p in Text(p.name).tag(p.id) }
                    } label: {
                        Label("Project", systemImage: "folder")
                    }
                    Picker(selection: $model) {
                        Text("Default").tag("")
                        ForEach(store.models.filter { !$0.id.hasPrefix("claude-") && $0.id != "default" }) { m in Text(m.name).tag(m.id) }
                        if store.models.contains(where: { $0.id.hasPrefix("claude-") }) {
                            Section("Versions") {
                                ForEach(store.models.filter { $0.id.hasPrefix("claude-") }) { m in Text(m.name).tag(m.id) }
                            }
                        }
                    } label: {
                        Label("Model", systemImage: "sparkles")
                    }
                    Picker(selection: $effort) {
                        Text("Default").tag("")
                        ForEach(efforts, id: \.self) { e in Text(ThreadSettings.effortName(e)).tag(e) }
                    } label: {
                        Label("Effort", systemImage: "gauge.with.dots.needle.33percent")
                    }
                    Picker(selection: $mode) {
                        ForEach(modes, id: \.self) { m in Text(ThreadSettings.modeName(m)).tag(m) }
                    } label: {
                        Label("Permissions", systemImage: "shield")
                    }
                } footer: {
                    Text("Permissions apply to every thread in \(project?.name ?? "the project").")
                }
                .pickerStyle(.menu)
            }
            .navigationTitle("New Thread")
            .navigationSubtitle([project?.name, mac].compactMap { $0 }.joined(separator: " · "))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close", systemImage: "xmark") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Send", systemImage: "arrow.up") {
                        store.newThread(
                            in: projectID, text: text.trimmingCharacters(in: .whitespacesAndNewlines),
                            model: model.isEmpty ? nil : model, effort: effort.isEmpty ? nil : effort,
                            mode: mode == (project?.mode ?? "auto") ? nil : mode)
                        text = ""
                        dismiss()
                    }
                    .disabled(empty || !store.canAct || project == nil)
                }
            }
        }
        .presentationDetents([.medium, .large])
        // An effort the new model doesn't offer goes back to Default.
        .onChange(of: model) { if !effort.isEmpty && !efforts.contains(effort) { effort = "" } }
        // Another project: what's typed goes with it; its permissions, and its model and effort
        // if New Thread chose them there before.
        .onChange(of: projectID) { old, new in
            store.newThreadDrafts[old] = nil
            if let c = store.newThreadChoices[new] { (model, effort) = c }
            mode = project?.mode ?? "auto"
        }
        .onAppear {
            text = store.newThreadDrafts[projectID] ?? ""
            if let c = store.newThreadChoices[projectID] { (model, effort) = c }
            mode = project?.mode ?? "auto"
            if let m = Self.env["YH_PHONE_NEW_MODEL"] { model = m }
            if let e = Self.env["YH_PHONE_NEW_EFFORT"] { effort = e }
            focused = true
        }
        .onDisappear {
            store.newThreadDrafts[projectID] = empty ? nil : text
            store.newThreadChoices[projectID] = (model, effort)
        }
    }

    private static let env = ProcessInfo.processInfo.environment
}
