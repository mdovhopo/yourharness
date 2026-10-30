import Foundation
import HarnessCore

/// A thread's Files tab: the folder it shows, the files open in it and where to go in the
/// one on screen. Only paths: a file's text lives in the viewer while it's on screen, so a
/// thread with tabs open costs nothing until you look.
@MainActor @Observable
final class FilesModel {
    struct Tab: Identifiable, Equatable {
        /// Relative to the root.
        let path: String
        /// Replaced by the next file clicked, unless you keep it (double-click, or scroll in it).
        var isPreview: Bool
        var id: String { path }
        var name: String { (path as NSString).lastPathComponent }
    }

    /// Where to go once a file shows: lines, or text to find there (what an edit wrote).
    struct Reveal: Equatable {
        var path: String
        var line: Int?
        var endLine: Int?
        var text: String?
        var serial: Int
    }

    /// The thread's folder: the project, or its worktree.
    let root: URL
    private(set) var tabs: [Tab] = []
    private(set) var selectedPath: String?
    private(set) var reveal: Reveal?
    /// A folder or file to show in the tree, from a link.
    private(set) var treeReveal: Reveal?
    /// Open folders in the tree, relative, without a trailing "/".
    private(set) var expanded: Set<String> = []
    var showsTree = true {
        didSet { if showsTree != oldValue { onChange?() } }
    }
    /// Something worth saving changed.
    @ObservationIgnored var onChange: (() -> Void)?
    /// Where each file was scrolled to, while the app runs.
    @ObservationIgnored var scrollPositions: [String: CGPoint] = [:]
    @ObservationIgnored private var serial = 0
    /// Past this, the oldest tabs you aren't looking at close.
    static let tabLimit = 24

    init(root: URL, restoring saved: SavedFiles?) {
        self.root = root.standardizedFileURL
        guard let saved else { return }
        tabs = saved.tabs.map { Tab(path: $0, isPreview: $0 == saved.preview) }
        selectedPath = saved.selected.flatMap { selected in tabs.contains { $0.path == selected } ? selected : nil } ?? tabs.last?.path
        expanded = Set(saved.expanded)
        showsTree = saved.hidesTree != true
    }

    var saved: SavedFiles {
        SavedFiles(tabs: tabs.map(\.path), preview: tabs.first(where: \.isPreview)?.path, selected: selectedPath,
                   expanded: expanded.sorted(), hidesTree: showsTree ? nil : true)
    }

    var selectedTab: Tab? { tabs.first { $0.path == selectedPath } }

    func url(for path: String) -> URL {
        path.isEmpty ? root : root.appending(path: path)
    }

    /// A path inside the root, relative to it; nil for anything outside.
    func relativePath(forAbsolute path: String) -> String? {
        let standardized = (path as NSString).standardizingPath
        let base = root.path
        guard standardized.hasPrefix(base + "/") else { return nil }
        return String(standardized.dropFirst(base.count + 1))
    }

    /// Opens a file, in the preview tab unless `keep`; a folder (ending in "/") shows in the tree.
    func open(_ path: String, line: Int? = nil, endLine: Int? = nil, text: String? = nil, keep: Bool = false) {
        serial += 1
        if path.hasSuffix("/") {
            showsTree = true
            treeReveal = Reveal(path: String(path.dropLast()), serial: serial)
            return
        }
        if let index = tabs.firstIndex(where: { $0.path == path }) {
            if keep { tabs[index].isPreview = false }
        } else if let preview = tabs.firstIndex(where: \.isPreview) {
            tabs[preview] = Tab(path: path, isPreview: !keep)
        } else {
            tabs.append(Tab(path: path, isPreview: !keep))
            while tabs.count > Self.tabLimit, let oldest = tabs.firstIndex(where: { $0.path != path }) {
                scrollPositions[tabs[oldest].path] = nil
                tabs.remove(at: oldest)
            }
        }
        selectedPath = path
        reveal = Reveal(path: path, line: line, endLine: endLine, text: text, serial: serial)
        treeReveal = Reveal(path: path, serial: serial)
        onChange?()
    }

    /// Keeps a preview tab open, as a double-click does.
    func keep(_ path: String) {
        guard let index = tabs.firstIndex(where: { $0.path == path }), tabs[index].isPreview else { return }
        tabs[index].isPreview = false
        onChange?()
    }

    func select(_ path: String) {
        guard path != selectedPath, tabs.contains(where: { $0.path == path }) else { return }
        selectedPath = path
        serial += 1
        treeReveal = Reveal(path: path, serial: serial)
        onChange?()
    }

    func close(_ path: String) {
        guard let index = tabs.firstIndex(where: { $0.path == path }) else { return }
        tabs.remove(at: index)
        scrollPositions[path] = nil
        if selectedPath == path {
            selectedPath = tabs.isEmpty ? nil : tabs[min(index, tabs.count - 1)].path
        }
        onChange?()
    }

    func closeOthers(_ path: String) {
        tabs.removeAll { $0.path != path }
        scrollPositions = scrollPositions.filter { $0.key == path }
        selectedPath = tabs.first?.path
        onChange?()
    }

    func setExpanded(_ path: String, _ isExpanded: Bool) {
        let changed = isExpanded ? expanded.insert(path).inserted : expanded.remove(path) != nil
        if changed { onChange?() }
    }
}
