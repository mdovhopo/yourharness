import AppKit
import CoreServices
import HarnessCore
import SwiftUI
import UniformTypeIdentifiers

/// The thread's folder as an outline: a folder is read when it's opened, never the whole
/// tree, and only opened folders are read again when something in them changes.
struct FileTreeView: NSViewRepresentable {
    let files: FilesModel
    let linker: FileLinker
    let selectedPath: String?
    let reveal: FilesModel.Reveal?

    func makeCoordinator() -> FileTreeController {
        FileTreeController(files: files)
    }

    func makeNSView(context: Context) -> NSScrollView {
        context.coordinator.start()
        return context.coordinator.scrollView
    }

    func updateNSView(_ view: NSScrollView, context: Context) {
        let controller = context.coordinator
        controller.setLinker(linker)
        if let reveal, reveal.serial != controller.revealSerial {
            controller.revealSerial = reveal.serial
            controller.reveal(reveal.path)
        } else {
            controller.select(selectedPath)
        }
    }

    static func dismantleNSView(_ view: NSScrollView, coordinator: FileTreeController) {
        coordinator.stop()
    }
}

@MainActor
final class FileNode {
    /// Relative to the root; "" for the root itself.
    let path: String
    let name: String
    let isFolder: Bool
    /// Nil until the folder's been read.
    var children: [FileNode]?
    weak var parent: FileNode?

    init(path: String, name: String, isFolder: Bool, parent: FileNode?) {
        self.path = path
        self.name = name
        self.isFolder = isFolder
        self.parent = parent
    }

    func childPath(_ name: String) -> String {
        path.isEmpty ? name : path + "/" + name
    }
}

@MainActor
final class FileTreeController: NSObject, NSOutlineViewDataSource, NSOutlineViewDelegate {
    let scrollView = NSScrollView()
    private let outline = NSOutlineView()
    private let files: FilesModel
    private let rootNode: FileNode
    /// Folders read so far, by path: what a change on disk is matched against.
    private var folders: [String: FileNode] = [:]
    private var reading: [String: Task<Void, Never>] = [:]
    private var linker = FileLinker.empty
    private var watcher: FolderWatcher?
    private var selectingInCode = false
    var revealSerial = -1
    private var pendingSelection: String?

    init(files: FilesModel) {
        self.files = files
        rootNode = FileNode(path: "", name: files.root.lastPathComponent, isFolder: true, parent: nil)
        super.init()
        folders[""] = rootNode

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("name"))
        column.resizingMask = .autoresizingMask
        outline.addTableColumn(column)
        outline.outlineTableColumn = column
        outline.headerView = nil
        outline.style = .sourceList
        outline.rowSizeStyle = .small
        outline.indentationPerLevel = 12
        outline.autoresizesOutlineColumn = false
        outline.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        outline.backgroundColor = .clear
        outline.dataSource = self
        outline.delegate = self
        outline.target = self
        outline.doubleAction = #selector(doubleClicked)
        outline.setAccessibilityLabel("Files")
        let menu = NSMenu()
        menu.delegate = self
        outline.menu = menu

        scrollView.documentView = outline
        scrollView.hasVerticalScroller = true
        scrollView.autohidesScrollers = true
        scrollView.drawsBackground = false
    }

    func start() {
        Task {
            await read(rootNode)
            outline.reloadData()
            await restoreExpanded()
            if let pendingSelection { select(pendingSelection) }
        }
        let root = files.root.resolvingSymlinksInPath().path
        watcher = FolderWatcher(root: root) { [weak self] changed in self?.changed(changed, root: root) }
    }

    func stop() {
        watcher = nil
        reading.values.forEach { $0.cancel() }
    }

    func setLinker(_ linker: FileLinker) {
        guard linker.revision != self.linker.revision else { return }
        self.linker = linker
        // Only the rows that are made: the rest pick it up when they scroll in.
        outline.enumerateAvailableRowViews { _, row in
            guard let node = self.outline.item(atRow: row) as? FileNode,
                  let cell = self.outline.view(atColumn: 0, row: row, makeIfNecessary: false) as? NSTableCellView else { return }
            cell.textField?.textColor = self.isIgnored(node) ? .tertiaryLabelColor : .labelColor
        }
    }

    // MARK: Reading folders

    private nonisolated static func list(_ url: URL) -> [(name: String, isFolder: Bool)] {
        let keys: [URLResourceKey] = [.isDirectoryKey]
        guard let urls = try? FileManager.default.contentsOfDirectory(at: url, includingPropertiesForKeys: keys, options: []) else { return [] }
        var entries: [(name: String, isFolder: Bool)] = urls.compactMap { url in
            let name = url.lastPathComponent
            guard name != ".git", name != ".DS_Store" else { return nil }
            return (name, (try? url.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory == true)
        }
        // Folders first, then by name as Finder sorts them.
        entries.sort { $0.isFolder != $1.isFolder ? $0.isFolder : $0.name.localizedStandardCompare($1.name) == .orderedAscending }
        return entries
    }

    /// Reads a folder off the main thread (again, when `fresh`), keeping the nodes of what's
    /// still there, so folders open inside it stay open.
    private func read(_ node: FileNode, fresh: Bool = false) async {
        if !fresh, node.children != nil { return }
        if let reading = reading[node.path] {
            await reading.value
            if !fresh { return }
        }
        let url = files.url(for: node.path)
        let task = Task {
            let entries = await Task.detached(priority: .userInitiated) { Self.list(url) }.value
            guard !Task.isCancelled else { return }
            let existing = Dictionary((node.children ?? []).map { ($0.name, $0) }, uniquingKeysWith: { first, _ in first })
            let children = entries.map { entry in
                if let kept = existing[entry.name], kept.isFolder == entry.isFolder { return kept }
                return FileNode(path: node.childPath(entry.name), name: entry.name, isFolder: entry.isFolder, parent: node)
            }
            for gone in (node.children ?? []) where !children.contains(where: { $0 === gone }) {
                forget(gone)
            }
            for child in children where child.isFolder { folders[child.path] = child }
            node.children = children
        }
        reading[node.path] = task
        await task.value
        reading[node.path] = nil
    }

    private func forget(_ node: FileNode) {
        guard node.isFolder else { return }
        folders[node.path] = nil
        node.children?.forEach(forget)
    }

    private func restoreExpanded() async {
        // Parents first, so each opens inside an open one.
        for path in files.expanded.sorted(by: { $0.count(where: { $0 == "/" }) < $1.count(where: { $0 == "/" }) }) {
            guard let node = folders[path], let parent = node.parent, parent === rootNode || outline.isItemExpanded(parent) else { continue }
            await read(node)
            outline.reloadItem(node, reloadChildren: true)
            outline.expandItem(node)
        }
    }

    /// Opens the folders down to `path` and selects it, as a link to it does.
    func reveal(_ path: String) {
        Task {
            let components = path.split(separator: "/").map(String.init)
            var node = rootNode
            await read(node)
            for component in components {
                guard let next = node.children?.first(where: { $0.name == component }) else { break }
                // A folder linked to opens too.
                if next.isFolder, !outline.isItemExpanded(next) {
                    await read(next)
                    outline.reloadItem(next, reloadChildren: true)
                    outline.expandItem(next)
                }
                node = next
            }
            select(path, scroll: true)
        }
    }

    func select(_ path: String?, scroll: Bool = false) {
        pendingSelection = path
        guard let path else {
            if outline.selectedRow >= 0 { inCode { outline.deselectAll(nil) } }
            return
        }
        guard let node = node(at: path) else { return }
        let row = outline.row(forItem: node)
        guard row >= 0 else { return }
        if outline.selectedRow != row { inCode { outline.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false) } }
        if scroll { outline.scrollRowToVisible(row) }
    }

    private func node(at path: String) -> FileNode? {
        if let folder = folders[path] { return folder }
        let parent = (path as NSString).deletingLastPathComponent
        let name = (path as NSString).lastPathComponent
        return folders[parent]?.children?.first { $0.name == name }
    }

    private func inCode(_ change: () -> Void) {
        selectingInCode = true
        change()
        selectingInCode = false
    }

    /// Folders that changed on disk: those that have been read are read again.
    private func changed(_ paths: [String], root: String) {
        var dirty: [FileNode] = []
        for path in paths {
            var relative = path.hasSuffix("/") ? String(path.dropLast()) : path
            if relative == root { relative = "" }
            else if relative.hasPrefix(root + "/") { relative = String(relative.dropFirst(root.count + 1)) }
            else { continue }
            if relative == ".git" || relative.hasPrefix(".git/") || relative.contains("/.git/") || relative.hasSuffix("/.git") { continue }
            if let node = folders[relative], node.children != nil, !dirty.contains(where: { $0 === node }) { dirty.append(node) }
        }
        for node in dirty {
            Task {
                await read(node, fresh: true)
                let selected = files.selectedPath
                outline.reloadItem(node === rootNode ? nil : node, reloadChildren: true)
                select(selected)
            }
        }
    }

    private func isIgnored(_ node: FileNode) -> Bool {
        guard linker.isComplete, !linker.isEmpty else { return false }
        return !linker.contains(node.isFolder ? node.path + "/" : node.path)
    }

    // MARK: Data source

    func outlineView(_ outlineView: NSOutlineView, numberOfChildrenOfItem item: Any?) -> Int {
        ((item as? FileNode) ?? rootNode).children?.count ?? 0
    }

    func outlineView(_ outlineView: NSOutlineView, child index: Int, ofItem item: Any?) -> Any {
        ((item as? FileNode) ?? rootNode).children![index]
    }

    func outlineView(_ outlineView: NSOutlineView, isItemExpandable item: Any) -> Bool {
        (item as? FileNode)?.isFolder == true
    }

    func outlineView(_ outlineView: NSOutlineView, shouldExpandItem item: Any) -> Bool {
        guard let node = item as? FileNode else { return false }
        if node.children != nil { return true }
        // Read first, then open: the folder never shows empty.
        Task {
            await read(node)
            outline.reloadItem(node, reloadChildren: true)
            outline.expandItem(node)
        }
        return false
    }

    func outlineViewItemDidExpand(_ notification: Notification) {
        guard let node = notification.userInfo?["NSObject"] as? FileNode else { return }
        files.setExpanded(node.path, true)
    }

    func outlineViewItemDidCollapse(_ notification: Notification) {
        guard let node = notification.userInfo?["NSObject"] as? FileNode else { return }
        files.setExpanded(node.path, false)
    }

    // MARK: Delegate

    private static let cellID = NSUserInterfaceItemIdentifier("file")
    private var icons: [String: NSImage] = [:]

    func outlineView(_ outlineView: NSOutlineView, viewFor tableColumn: NSTableColumn?, item: Any) -> NSView? {
        guard let node = item as? FileNode else { return nil }
        let cell = outlineView.makeView(withIdentifier: Self.cellID, owner: nil) as? NSTableCellView ?? makeCell()
        cell.textField?.stringValue = node.name
        cell.textField?.textColor = isIgnored(node) ? .tertiaryLabelColor : .labelColor
        cell.imageView?.image = icon(for: node)
        return cell
    }

    private func makeCell() -> NSTableCellView {
        let cell = NSTableCellView()
        cell.identifier = Self.cellID
        let image = NSImageView()
        image.translatesAutoresizingMaskIntoConstraints = false
        image.imageScaling = .scaleProportionallyUpOrDown
        let text = NSTextField(labelWithString: "")
        text.translatesAutoresizingMaskIntoConstraints = false
        text.lineBreakMode = .byTruncatingMiddle
        text.font = .systemFont(ofSize: NSFont.smallSystemFontSize + 1)
        cell.addSubview(image)
        cell.addSubview(text)
        cell.imageView = image
        cell.textField = text
        NSLayoutConstraint.activate([
            image.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 2),
            image.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            image.widthAnchor.constraint(equalToConstant: 16),
            image.heightAnchor.constraint(equalToConstant: 16),
            text.leadingAnchor.constraint(equalTo: image.trailingAnchor, constant: 5),
            text.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -2),
            text.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
        return cell
    }

    /// Finder's icon for the kind of file, by its extension: never read from the file itself.
    private func icon(for node: FileNode) -> NSImage? {
        let key = node.isFolder ? "/" : (node.name as NSString).pathExtension.lowercased()
        if let known = icons[key] { return known }
        let type: UTType = node.isFolder ? .folder : (UTType(filenameExtension: key) ?? .data)
        let image = NSWorkspace.shared.icon(for: type)
        icons[key] = image
        return image
    }

    func outlineViewSelectionDidChange(_ notification: Notification) {
        guard !selectingInCode, let node = outline.item(atRow: outline.selectedRow) as? FileNode, !node.isFolder else { return }
        files.open(node.path)
    }

    @objc private func doubleClicked() {
        guard let node = outline.item(atRow: outline.clickedRow) as? FileNode else { return }
        if node.isFolder {
            if outline.isItemExpanded(node) { outline.collapseItem(node) } else { outline.expandItem(node) }
        } else {
            files.open(node.path, keep: true)
        }
    }
}

extension FileTreeController: NSMenuDelegate {
    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        guard let node = outline.item(atRow: outline.clickedRow) as? FileNode else { return }
        let url = files.url(for: node.path)
        if !node.isFolder {
            menu.addItem(ClosureMenuItem("Open in Tab") { [files] in files.open(node.path, keep: true) })
        }
        menu.addItem(ClosureMenuItem("Open in Default App") { NSWorkspace.shared.open(url) })
        menu.addItem(ClosureMenuItem("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting([url]) })
        menu.addItem(.separator())
        menu.addItem(ClosureMenuItem("Copy Path") { FileActions.copy(url.path) })
        menu.addItem(ClosureMenuItem("Copy Relative Path") { FileActions.copy(node.path) })
    }
}

/// A menu item that runs a closure.
final class ClosureMenuItem: NSMenuItem {
    private let run: () -> Void

    init(_ title: String, run: @escaping () -> Void) {
        self.run = run
        super.init(title: title, action: #selector(runClosure), keyEquivalent: "")
        target = self
    }

    @available(*, unavailable)
    required init(coder: NSCoder) { fatalError() }

    @objc private func runClosure() { run() }
}

enum FileActions {
    static func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }
}

/// FSEvents on a folder: which folders under it changed, a moment after they did, on the
/// main thread. Gone with its owner.
final class FolderWatcher {
    private var stream: FSEventStreamRef?
    private let handler: Handler

    private final class Handler: @unchecked Sendable {
        let onChange: @MainActor ([String]) -> Void
        init(_ onChange: @escaping @MainActor ([String]) -> Void) { self.onChange = onChange }
    }

    init(root: String, onChange: @escaping @MainActor ([String]) -> Void) {
        handler = Handler(onChange)
        var context = FSEventStreamContext(version: 0, info: Unmanaged.passUnretained(handler).toOpaque(), retain: nil, release: nil, copyDescription: nil)
        let callback: FSEventStreamCallback = { _, info, count, paths, _, _ in
            guard let info else { return }
            let handler = Unmanaged<Handler>.fromOpaque(info).takeUnretainedValue()
            let array = Unmanaged<CFArray>.fromOpaque(paths).takeUnretainedValue() as? [String] ?? []
            MainActor.assumeIsolated { handler.onChange(Array(array.prefix(count))) }
        }
        stream = FSEventStreamCreate(nil, callback, &context, [root] as CFArray, FSEventStreamEventId(kFSEventStreamEventIdSinceNow), 0.3,
                                     FSEventStreamCreateFlags(kFSEventStreamCreateFlagUseCFTypes))
        guard let stream else { return }
        FSEventStreamSetDispatchQueue(stream, .main)
        FSEventStreamStart(stream)
    }

    deinit {
        guard let stream else { return }
        FSEventStreamStop(stream)
        FSEventStreamInvalidate(stream)
        FSEventStreamRelease(stream)
    }
}
