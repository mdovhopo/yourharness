import AppKit
import SwiftUI

/// Development: `YH_NAV_PROBE=<folder>` goes through the ways between a thread, a new
/// thread, a project's pull requests and its memory at launch, through the same calls the
/// sidebar's clicks make, and after each step draws the window into `<folder>/NN-step.png`
/// and prints to stderr what's on screen and the window's frame and smallest size. A step
/// that leaves the window bigger than it was, or bigger than its screen, says FAIL; the right
/// sidebar may widen it, and closing it must give that back.
/// Off unless set.
@MainActor
enum NavigationProbe {
    static func runIfRequested(app: AppModel) {
        guard let folder = ProcessInfo.processInfo.environment["YH_NAV_PROBE"] else { return }
        try? FileManager.default.createDirectory(atPath: folder, withIntermediateDirectories: true)
        Task {
            try? await Task.sleep(for: .seconds(3))
            guard let window = NSApp.windows.first(where: { $0.isVisible && $0.frame.width > 600 }) else {
                log("no window")
                return
            }
            let threads = app.visibleThreadIDs.compactMap { id in app.threads.first { $0.id == id } }
            let talked = threads.filter { !$0.entries.isEmpty || $0.sessionID != nil }
            guard let first = talked.first, let project = app.project(id: first.projectID) else {
                log("needs a thread with a conversation")
                return
            }
            let second = talked.dropFirst().first ?? first
            let home = app.homeProject
            var steps: [(String, () -> Void)] = [
                ("thread", { app.selectedThreadID = first.id }),
                ("pull requests", { app.showProject(project) }),
                ("same thread again", { app.selectedThreadID = first.id }),
                ("memory", { app.showMemory(of: project) }),
                ("same thread again", { app.selectedThreadID = first.id }),
                ("memory", { app.showMemory(of: project) }),
                ("other thread", { app.selectedThreadID = second.id }),
                ("pull requests", { app.showProject(project) }),
                ("memory tab", { app.showProject(project, tab: .memory) }),
                ("pull requests tab", { app.showProject(project, tab: .workspace) }),
                ("new thread", { withAnimation(SidebarMetrics.selectionSpring) { _ = app.newThread(in: project) } }),
                ("memory", { app.showMemory(of: project) }),
                ("new thread again", { _ = app.newThread(in: project) }),
                ("home memory", { app.showMemory(of: home) }),
                ("thread", { app.selectedThreadID = first.id }),
                ("inspector", { app.setShowsInspector(true) }),
                ("pull requests", { app.showProject(project) }),
                ("thread", { app.selectedThreadID = first.id }),
                ("memory", { app.showMemory(of: project) }),
                ("other thread", { app.selectedThreadID = second.id }),
                ("inspector off", { app.setShowsInspector(false) }),
            ]
            // The narrowest the window goes, then everything again: content measured narrow is
            // what grew the window before.
            steps.append(("narrow", { window.setFrame(NSRect(origin: window.frame.origin, size: NSSize(width: 300, height: window.frame.height)), display: true) }))
            steps += steps.prefix(steps.count - 1).map { ($0.0 + " (narrow)", $0.1) }
            var failures = 0
            var widthBeforeInspector: CGFloat?
            for (index, (name, step)) in steps.enumerated() {
                let before = window.frame
                step()
                try? await Task.sleep(for: .milliseconds(1200))
                let after = window.frame
                let screen = (window.screen ?? NSScreen.main)?.visibleFrame ?? .zero
                let shown = app.overviewProject.map { "project \($0.name) · \(app.projectTab.rawValue)" }
                    ?? app.selectedThread.map { "thread \($0.title) · \($0.entries.count) entries" } ?? "nothing"
                if name.hasPrefix("inspector") && !name.hasPrefix("inspector off") { widthBeforeInspector = before.width }
                let widthAllowed = name.hasPrefix("inspector off") ? (widthBeforeInspector ?? before.width)
                    : name.hasPrefix("inspector") || name == "narrow" ? .infinity : before.width
                let grew = name != "narrow" && (after.height > before.height + 1 || after.width > widthAllowed + 1)
                let offScreen = after.height > screen.height + 1 || after.width > screen.width + 1
                if grew || offScreen { failures += 1 }
                log(String(format: "%@ %02d %@ → %@  frame %.0f×%.0f (was %.0f×%.0f) min %.0f×%.0f",
                           grew || offScreen ? "FAIL" : "ok  ", index, name, shown,
                           after.width, after.height, before.width, before.height, window.minSize.width, window.minSize.height))
                WindowSnapshot.write(to: "\(folder)/\(String(format: "%02d", index))-\(name.replacingOccurrences(of: " ", with: "-")).png")
            }
            log(failures == 0 ? "NAV PROBE PASS" : "NAV PROBE FAIL: \(failures) steps")
        }
    }

    private static func log(_ line: String) {
        FileHandle.standardError.write(Data(("nav " + line + "\n").utf8))
    }
}
