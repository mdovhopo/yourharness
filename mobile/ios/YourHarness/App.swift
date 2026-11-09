import SwiftUI

@main
struct YourHarnessApp: App {
    @State private var fleet = Fleet()
    @Environment(\.scenePhase) private var phase

    init() {
        Perf.shared.start()
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(fleet)
                .preferredColorScheme(Self.appearance)
                // The Mac's QR code, scanned with the Camera app: pairs with that Mac.
                .onOpenURL { fleet.pair($0) }
        }
        // `.inactive` passes by (Control Center, the app switcher): only the background counts.
        .onChange(of: phase) { _, phase in
            if phase == .background { fleet.setAway(true) } else if phase == .active { fleet.setAway(false) }
        }
    }

    /// `YH_APPEARANCE=light|dark`, for screenshots.
    private static var appearance: ColorScheme? {
        switch ProcessInfo.processInfo.environment["YH_APPEARANCE"] {
        case "light": .light
        case "dark": .dark
        default: nil
        }
    }
}

/// The dock: threads, usage, the Mac, and search. A thread opens over it, the dock hidden.
struct RootView: View {
    @Environment(Fleet.self) private var fleet

    var body: some View {
        @Bindable var fleet = fleet
        let store = fleet.current
        // Each tab draws the Mac shown; switching Macs builds them anew for it.
        TabView(selection: $fleet.tab) {
            Tab("Threads", systemImage: "bubble.left.and.bubble.right", value: .threads) {
                ThreadsTab(store: store).environment(store).id(store.id)
            }
            .badge(fleet.waitingElsewhere)
            Tab("Usage", systemImage: "gauge.with.dots.needle.33percent", value: .usage) {
                NavigationStack { UsageView() }.environment(store).id(store.id)
            }
            Tab("Mac", systemImage: "desktopcomputer", value: .mac) {
                NavigationStack { MacView() }.environment(store)
            }
            Tab(value: .search, role: .search) {
                NavigationStack { SearchView() }.environment(store).id(store.id)
            }
        }
        // `YH_PHONE_PAIR=<yourharness://pair… link>`: paired with, as opening the link does.
        .task {
            guard let link = ProcessInfo.processInfo.environment["YH_PHONE_PAIR"].flatMap(URL.init(string:)) else { return }
            try? await Task.sleep(for: .seconds(2))
            fleet.pair(link)
        }
        // `YH_PHONE_SWITCH=<index>`: that Mac shown 6 s after launch, as its menu does.
        .task {
            guard let i = ProcessInfo.processInfo.environment["YH_PHONE_SWITCH"].flatMap(Int.init) else { return }
            try? await Task.sleep(for: .seconds(6))
            if fleet.stores.indices.contains(i) { fleet.select(fleet.stores[i]) }
        }
    }
}

/// The Threads tab for one Mac: the list, and the open thread over it (iPhone) or beside it (iPad).
private struct ThreadsTab: View {
    @Bindable var store: Store
    @Environment(\.horizontalSizeClass) private var sizeClass

    var body: some View {
        if sizeClass == .regular {
            // iPad (or a wide window): the list beside the open thread.
            NavigationSplitView {
                ThreadListView(selection: Binding(get: { store.path.last }, set: { store.path = $0.map { [$0] } ?? [] }))
            } detail: {
                if let id = store.path.last {
                    NavigationStack { TranscriptScreen(threadID: id) }.id(id)
                } else {
                    ContentUnavailableView("No Thread Open", systemImage: "bubble.left.and.bubble.right", description: Text("Choose a thread from the list."))
                }
            }
        } else {
            NavigationStack(path: $store.path) {
                ThreadListView()
                    .navigationDestination(for: String.self) { id in
                        TranscriptScreen(threadID: id)
                            .toolbar(.hidden, for: .tabBar)
                    }
            }
        }
    }
}

enum AppTab: String, Hashable {
    case threads, usage, mac, search
}
