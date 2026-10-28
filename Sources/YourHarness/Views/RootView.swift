import HarnessCore
import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings
    @Environment(\.undoManager) private var undoManager
    /// The window is first laid out at its default size, before it grows to the saved one.
    /// Opening the right sidebar in that pass can leave no room for the left one, which
    /// AppKit then collapses while its toggle still says it's shown; so it opens once the
    /// window is on screen.
    @State private var isOnScreen = false
    @State private var inspectorRoom = InspectorRoom()

    var body: some View {
        NavigationSplitView {
            SidebarView()
                .takesWindowHeight()
                .navigationSplitViewColumnWidth(min: 220, ideal: 270, max: 380)
        } detail: {
            ZStack {
                // Switching is instant, as in Mail or Messages: no crossfade to wait through.
                if let project = app.overviewProject {
                    ProjectPage(project: project)
                        .id(project.id)
                        // Narrower than it would like, it's cut at the column's edge rather
                        // than drawn over the sidebar.
                        .clipped()
                } else if let thread = app.selectedThread {
                    // Not keyed on the thread: it keeps its composer and toolbar, and only its
                    // transcript is made anew.
                    ThreadView(thread: thread)
                } else {
                    EmptyDetailView()
                }
            }
            // As narrow as the window lets it be, down to a floor of its own: a page's content
            // would set the window's smallest width otherwise (Memory's made it 1,300 pt, past
            // the edge of a narrow screen).
            .frame(minWidth: 280, maxWidth: .infinity)
            .takesWindowHeight()
            // One surface under every view, so switching swaps content, not the canvas.
            .canvasBackground()
            .transaction(value: app.selectedThreadID) { $0.animation = nil }
            .transaction(value: app.overviewProjectID) { $0.animation = nil }
            .transaction(value: app.projectTab) { $0.animation = nil }
            // Everything in the right sidebar belongs to a thread: on a project's overview it has
            // nothing to show, so it steps aside, and comes back as it was with a thread.
            .inspector(isPresented: Binding(get: { showsInspector },
                                            set: { if app.inspectorHasThread { app.setShowsInspector($0) } })) {
                ThreadInspector(thread: app.inspectorHasThread ? app.selectedThread : nil)
                    .takesWindowHeight()
                    .inspectorColumnWidth(min: 300, ideal: 380, max: 960)
            }
        }
        .overlay {
            if app.spotlight.isOpen {
                ZStack(alignment: .top) {
                    // A click outside closes it, as with Spotlight.
                    Color.black.opacity(0.08)
                        .ignoresSafeArea()
                        .onTapGesture { app.spotlight.close() }
                    SpotlightView(spotlight: app.spotlight)
                        .padding(.top, 70)
                }
                .transition(.opacity)
            }
        }
        .animation(.easeOut(duration: 0.12), value: app.spotlight.isOpen)
        // The sidebar's usage box, opened up over the window.
        .overlay {
            if app.tokenUsage.isOpen {
                UsageOverlay()
                    .transition(.opacity.combined(with: .scale(scale: 0.98)))
            }
        }
        .animation(.smooth(duration: 0.2), value: app.tokenUsage.isOpen)
        .sheet(isPresented: Bindable(app).isResumingSession) {
            ResumeSessionSheet()
        }
        .sheet(item: Bindable(app.stateTransfer).pending) { pending in
            StateImportSheet(pending: pending)
        }
        .background(WindowReader { inspectorRoom.window = $0 })
        .onChange(of: showsInspector) { _, shown in inspectorRoom.shownChanged(shown) }
        .onChange(of: app.showsInspector) { _, shown in if !shown { inspectorRoom.closed() } }
        .onChange(of: undoManager, initial: true) { app.undoManager = undoManager }
        .onAppear {
            isOnScreen = true
            DispatchQueue.main.async {
                for window in NSApp.windows where window.isVisible { window.fitOnScreen() }
            }
            // Development: open the resume sheet filled in, for checking it without clicking.
            if ProcessInfo.processInfo.environment["YH_RESUME_SESSION"] != nil { app.isResumingSession = true }
            // Development: open ⌘K with a query, for checking it without typing.
            if let query = ProcessInfo.processInfo.environment["YH_SPOTLIGHT"] {
                DispatchQueue.main.asyncAfter(deadline: .now() + 1) { app.spotlight.open(app, query: query) }
            }
            // Development: open the usage view (`YH_USAGE_RANGE`: week, month, quarter,
            // halfYear or year), for checking it without clicking.
            if let range = ProcessInfo.processInfo.environment["YH_USAGE_RANGE"].flatMap(UsageModel.Range.init(rawValue:)) {
                app.tokenUsage.range = range
            }
            if ProcessInfo.processInfo.environment["YH_OPEN_USAGE"] != nil { app.tokenUsage.open() }
            // Development: open Settings, for checking it without a shortcut.
            if ProcessInfo.processInfo.environment["YH_OPEN_SETTINGS"] != nil { openSettings() }
            // Development: open the selected thread's browser in its own window.
            if ProcessInfo.processInfo.environment["YH_OPEN_BROWSER_WINDOW"] != nil, let thread = app.selectedThread {
                openWindow(id: "browser", value: thread.id)
            }
            // Development: open a memory specialist's window (importer or reconciler) for the
            // selected project, for checking it without clicking.
            if let role = ProcessInfo.processInfo.environment["YH_MEMORY_AGENT"].flatMap(MemoryRole.init(rawValue:)) {
                openWindow(id: "memory-agent", value: MemoryAgentKey(projectID: app.memoryProject?.id ?? app.selectedProject.id, role: role))
            }
            // Development: open a Mermaid file in the diagram window, for checking it without clicking.
            if let path = ProcessInfo.processInfo.environment["YH_OPEN_DIAGRAM"],
               let source = try? String(contentsOfFile: path, encoding: .utf8) {
                openWindow(id: "diagram", value: source)
            }
        }
    }

    private var showsInspector: Bool {
        isOnScreen && app.showsInspector && app.inspectorHasThread
    }
}

extension View {
    /// Takes the height the window gives it, whatever its content would like. Otherwise the
    /// content's smallest height becomes the window's: text that may not wrap shorter, measured
    /// at the narrowest width, comes out a word a line and thousands of points tall, and the
    /// window grew past the screen and stayed there (the Browser tab in 0.0.3, Memory in 0.0.7).
    func takesWindowHeight() -> some View {
        frame(minHeight: 0, maxHeight: .infinity)
    }
}

extension NSWindow {
    /// A window taller or wider than its screen comes back fitted to it (0.0.2, 0.0.3 and
    /// 0.0.7 could grow one past the bottom, and saved it so).
    func fitOnScreen() {
        guard !styleMask.contains(.fullScreen), let visible = (screen ?? NSScreen.main)?.visibleFrame else { return }
        guard frame.height > visible.height + 1 || frame.width > visible.width + 1 else { return }
        let size = NSSize(width: min(frame.width, visible.width), height: min(frame.height, visible.height))
        let origin = NSPoint(x: min(max(frame.minX, visible.minX), visible.maxX - size.width),
                             y: min(max(frame.minY, visible.minY), visible.maxY - size.height))
        setFrame(NSRect(origin: origin, size: size), display: true)
    }
}

/// Opening the right sidebar on a window too narrow for it, AppKit widens the window. Closing
/// it gives that width back, unless you've resized the window since; and the window never ends
/// up wider than its screen. Stepping aside for a project's page keeps the width, so going
/// between pages doesn't make the window jump.
@MainActor
private final class InspectorRoom {
    weak var window: NSWindow? {
        didSet {
            guard window !== oldValue else { return }
            resizing = window.map { window in
                NotificationCenter.default.addObserver(forName: NSWindow.willStartLiveResizeNotification, object: window, queue: .main) { [weak self] _ in
                    MainActor.assumeIsolated { self?.before = nil }
                }
            }
        }
    }
    private var resizing: NSObjectProtocol?
    /// The window's frame before the sidebar opened.
    private var before: NSRect?

    /// The sidebar showing or not, for any reason.
    func shownChanged(_ shown: Bool) {
        guard shown, let window, !window.styleMask.contains(.fullScreen) else { return }
        if before == nil { before = window.frame }
        // Once AppKit has made room.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { window.fitOnScreen() }
    }

    /// You closed the sidebar.
    func closed() {
        guard let before else { return }
        self.before = nil
        // Once the column has gone: until then the window can't be that narrow.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { [weak window] in
            guard let window, !window.styleMask.contains(.fullScreen), window.frame.width > before.width + 1 else { return }
            // Its left edge stays where it is: you may have moved it since.
            var frame = window.frame
            frame.size.width = before.width
            window.setFrame(frame, display: true, animate: true)
        }
    }
}

/// Hands over the window a view is in, once it's in one.
private struct WindowReader: NSViewRepresentable {
    let found: (NSWindow) -> Void

    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        DispatchQueue.main.async { if let window = view.window { found(window) } }
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {}
}

private struct EmptyDetailView: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        if app.projects.isEmpty {
            ContentUnavailableView {
                Label("Add Your First Project", systemImage: "folder.badge.plus")
            } description: {
                Text("Pick a folder to work in. Claude runs there, using your own Claude Code login.")
            } actions: {
                Button("Add Project…") { app.presentAddProject() }
                    .buttonStyle(.glassProminent)
                    .controlSize(.large)
            }
        } else {
            ContentUnavailableView {
                Label("No Thread Selected", systemImage: "bubble.left.and.text.bubble.right")
            } description: {
                Text("Choose a thread in the sidebar, or start a new one with ⌘N.")
            } actions: {
                Button("New Thread") { app.newThreadInCurrentProject() }
                    .buttonStyle(.glass)
            }
        }
    }
}
