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

struct RootView: View {
    @Environment(Store.self) private var store

    var body: some View {
        @Bindable var store = store
        NavigationStack(path: $store.path) {
            ThreadListView()
                .navigationDestination(for: String.self) { id in
                    TranscriptScreen(threadID: id)
                }
        }
    }
}
