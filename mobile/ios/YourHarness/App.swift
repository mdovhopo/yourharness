import SwiftUI

@main
struct YourHarnessApp: App {
    @State private var store = Store()

    init() {
        Perf.shared.start()
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(store)
                .preferredColorScheme(Self.appearance)
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
    @Environment(Store.self) private var store

    var body: some View {
        @Bindable var store = store
        TabView(selection: $store.tab) {
            Tab("Threads", systemImage: "bubble.left.and.bubble.right", value: .threads) {
                NavigationStack(path: $store.path) {
                    ThreadListView()
                        .navigationDestination(for: String.self) { id in
                            TranscriptScreen(threadID: id)
                                .toolbar(.hidden, for: .tabBar)
                        }
                }
            }
            Tab("Usage", systemImage: "gauge.with.dots.needle.33percent", value: .usage) {
                NavigationStack { UsageView() }
            }
            Tab("Mac", systemImage: "desktopcomputer", value: .mac) {
                NavigationStack { MacView() }
            }
            Tab(value: .search, role: .search) {
                NavigationStack { SearchView() }
            }
        }
    }
}

enum AppTab: String, Hashable {
    case threads, usage, mac, search
}
