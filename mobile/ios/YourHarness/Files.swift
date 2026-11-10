// The thread's folder on the Mac, read only (protocol 8): what changed since the last commit,
// each change's diff, the whole tree a folder at a time, Go to File, and a file's lines coloured
// as the Mac colours them, a page of 1000 at a time as they scroll into view. A file open is
// looked at again every few seconds, so an edit Claude makes shows. Links to files in replies
// and a tool's Open File land on the same viewer.

import SwiftUI

/// A file to open: the thread whose folder it's in, its path (relative, or absolute inside the
/// folder), and the lines to show.
struct FileTarget: Hashable, Identifiable {
    let thread: String
    let path: String
    var line: Int?
    var end: Int?
    var id: String { "\(thread)\n\(path)\n\(line ?? 0)" }
}

/// A folder of the thread's folder to open (`""` is the folder itself).
private struct FolderTarget: Hashable, Identifiable {
    let path: String
    var id: String { path }
}

/// A file link in a reply: `yh-file://open?path=…&line=…&end=…`.
enum FileLink {
    struct Target: Hashable {
        let path: String
        let line: Int?
        let end: Int?
    }

    static func url(_ ref: FileRef) -> URL? {
        var c = URLComponents()
        c.scheme = "yh-file"
        c.host = "open"
        c.queryItems = [URLQueryItem(name: "path", value: ref.path)] + [("line", ref.line), ("end", ref.end)].compactMap { name, n in n.map { URLQueryItem(name: name, value: String($0)) } }
        return c.url
    }

    static func parse(_ url: URL) -> Target? {
        guard url.scheme == "yh-file", let c = URLComponents(url: url, resolvingAgainstBaseURL: false),
              let path = c.queryItems?.first(where: { $0.name == "path" })?.value else { return nil }
        let number = { (name: String) in c.queryItems?.first(where: { $0.name == name })?.value.flatMap { Int($0) } }
        return Target(path: path, line: number("line"), end: number("end"))
    }
}

@MainActor private enum Env {
    static let values = ProcessInfo.processInfo.environment
    static var openedChange = false
    static var openedFolder = false
    static var found = false
}

// MARK: - The thread's folder

struct FilesScreen: View {
    let threadID: String
    @Environment(Store.self) private var store
    @State private var tab = Env.values["YH_PHONE_OPEN_FILES"] == "all" ? Tab.all : Tab.changed
    @State private var query = Env.values["YH_PHONE_FILES_QUERY"] ?? ""
    @State private var openFile: FileTarget?
    @State private var openFolder: FolderTarget?
    @State private var openChange: PullFile?

    enum Tab: Hashable { case changed, all }

    private var changes: Store.ChangesPage? { store.changes[threadID] }
    private var trimmed: String { query.trimmingCharacters(in: .whitespaces) }

    var body: some View {
        Group {
            if !trimmed.isEmpty {
                found
            } else if tab == .changed {
                changed
            } else {
                FolderList(threadID: threadID, path: "", openFile: $openFile, openFolder: $openFolder)
            }
        }
        .navigationTitle("Files")
        .navigationSubtitle(changes?.branch.map { "On \($0)" } ?? store.threads[threadID].flatMap(store.projectName) ?? "")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar(.hidden, for: .tabBar)
        .safeAreaInset(edge: .top, spacing: 0) {
            if trimmed.isEmpty {
                Picker("Show", selection: $tab) {
                    Text(changes?.files.map { $0.isEmpty ? "Changed" : "Changed · \($0.count)" } ?? "Changed").tag(Tab.changed)
                    Text("All Files").tag(Tab.all)
                }
                .pickerStyle(.segmented)
                .padding(.horizontal)
                .padding(.vertical, 8)
            }
        }
        .searchable(text: $query, placement: .navigationBarDrawer(displayMode: .always), prompt: "Go to File")
        .autocorrectionDisabled()
        .textInputAutocapitalization(.never)
        .task(id: trimmed) {
            guard !trimmed.isEmpty else { return }
            try? await Task.sleep(for: .milliseconds(150))
            store.findFiles(trimmed, in: threadID)
        }
        // Read when shown, and again when a turn ends (Claude may have changed more).
        .task(id: "\(store.state == .connected)|\(store.threads[threadID]?.isRunning == true)") {
            if store.state == .connected, store.threads[threadID]?.isRunning != true || changes == nil { store.askChanges(in: threadID) }
        }
        .navigationDestination(item: $openFile) { FileScreen(target: $0) }
        .navigationDestination(item: $openFolder) { FolderScreen(threadID: threadID, path: $0.path) }
        .navigationDestination(item: $openChange) { file in
            ChangeDiffScreen(threadID: threadID, files: changes?.files ?? [file], start: file)
        }
        // `YH_PHONE_OPEN_FILES=<folder>`: that folder opened; `YH_PHONE_CHANGE=<path or index>`: that change's diff.
        .task(id: changes?.files?.count) {
            if let wanted = Env.values["YH_PHONE_OPEN_FILES"], !["all", "changed"].contains(wanted), !Env.openedFolder {
                Env.openedFolder = true
                try? await Task.sleep(for: .seconds(1))
                openFolder = FolderTarget(path: wanted)
            }
            guard let all = changes?.files, !all.isEmpty, let wanted = Env.values["YH_PHONE_CHANGE"], !Env.openedChange else { return }
            Env.openedChange = true
            try? await Task.sleep(for: .seconds(1))
            openChange = all.first { $0.path == wanted } ?? Int(wanted).flatMap { all.indices.contains($0) ? all[$0] : nil }
        }
    }

    @ViewBuilder private var changed: some View {
        List {
            if let files = changes?.files, !files.isEmpty {
                Section {
                    ForEach(files) { file in
                        Button { openChange = file } label: { FileRow(file: file) }
                            .buttonStyle(.plain)
                            .contextMenu {
                                if file.status != "removed" {
                                    Button("Open File", systemImage: "doc.text") { openFile = FileTarget(thread: threadID, path: file.path) }
                                }
                                Button("Copy Path", systemImage: "doc.on.doc") { UIPasteboard.general.string = file.path }
                            }
                    }
                } footer: {
                    Text("Since the last commit, staged or not, new files included.")
                }
            }
        }
        .listStyle(.insetGrouped)
        .overlay {
            if let error = changes?.error, changes?.files?.isEmpty != false {
                ContentUnavailableView("Can't Show Changes", systemImage: "arrow.triangle.branch", description: Text(error))
            } else if changes?.files == nil {
                ProgressView("Looking for changes…")
            } else if changes?.files?.isEmpty == true {
                ContentUnavailableView("No Changes", systemImage: "checkmark.circle", description: Text("Nothing changed since the last commit."))
            }
        }
        .refreshable {
            store.askChanges(in: threadID)
            try? await Task.sleep(for: .milliseconds(600))
        }
    }

    /// Go to File: the folder's files matching what's typed, as `@` finds them.
    @ViewBuilder private var found: some View {
        let matches = store.fileMatches.flatMap { $0.thread == threadID && $0.query == trimmed ? $0.files : nil }
        List {
            ForEach(matches ?? [], id: \.self) { path in
                let folder = path.hasSuffix("/")
                Button {
                    if folder { openFolder = FolderTarget(path: String(path.dropLast())) } else { openFile = FileTarget(thread: threadID, path: path) }
                } label: {
                    PathRow(path: folder ? String(path.dropLast()) : path, folder: folder)
                }
                .buttonStyle(.plain)
            }
        }
        .listStyle(.plain)
        .overlay {
            if matches == nil {
                ProgressView()
            } else if matches?.isEmpty == true {
                ContentUnavailableView.search(text: trimmed)
            }
        }
        .onChange(of: matches) { _, m in
            guard let m, !Env.found else { return }
            Env.found = true
            Perf.shared.log("GOTO \(trimmed): \(m.prefix(5).joined(separator: ", "))")
        }
    }
}

/// A path found: its name, and the folder it's in under it.
private struct PathRow: View {
    let path: String
    let folder: Bool

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: folder ? "folder.fill" : FileIcon.symbol(path)).foregroundStyle(folder ? Color.accentColor : .secondary).frame(width: 22)
            VStack(alignment: .leading, spacing: 1) {
                Text((path as NSString).lastPathComponent).font(.subheadline.weight(.medium)).lineLimit(1)
                let parent = (path as NSString).deletingLastPathComponent
                if !parent.isEmpty { Text(parent).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.head) }
            }
            Spacer(minLength: 4)
            Image(systemName: "chevron.right").font(.caption.weight(.semibold)).foregroundStyle(.tertiary)
        }
        .contentShape(.rect)
    }
}

enum FileIcon {
    static func symbol(_ name: String) -> String {
        switch (name as NSString).pathExtension.lowercased() {
        case "png", "jpg", "jpeg", "gif", "webp", "heic", "bmp", "tif", "tiff", "ico", "svg": "photo"
        case "md", "markdown", "txt", "rtf": "doc.richtext"
        case "json", "yml", "yaml", "toml", "plist", "xml", "lock": "doc.badge.gearshape"
        case "": "doc"
        default: "chevron.left.forwardslash.chevron.right"
        }
    }

    static func size(_ bytes: Int) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
    }
}

/// A folder pushed from the tree or Go to File.
private struct FolderScreen: View {
    let threadID: String
    let path: String
    @State private var openFile: FileTarget?
    @State private var openFolder: FolderTarget?

    var body: some View {
        FolderList(threadID: threadID, path: path, openFile: $openFile, openFolder: $openFolder)
            .navigationTitle((path as NSString).lastPathComponent)
            .navigationSubtitle((path as NSString).deletingLastPathComponent)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar(.hidden, for: .tabBar)
            .navigationDestination(item: $openFile) { FileScreen(target: $0) }
            .navigationDestination(item: $openFolder) { FolderScreen(threadID: threadID, path: $0.path) }
    }
}

/// A folder's files and folders, as on disk; what git ignores dimmed.
private struct FolderList: View {
    let threadID: String
    let path: String
    @Binding var openFile: FileTarget?
    @Binding var openFolder: FolderTarget?
    @Environment(Store.self) private var store

    private var page: Store.DirPage? { store.dirs[Store.fileKey(threadID, path)] }

    var body: some View {
        List {
            ForEach(page?.entries ?? []) { entry in
                let full = path.isEmpty ? entry.name : "\(path)/\(entry.name)"
                Button {
                    if entry.folder { openFolder = FolderTarget(path: full) } else { openFile = FileTarget(thread: threadID, path: full) }
                } label: {
                    HStack(spacing: 10) {
                        Image(systemName: entry.folder ? "folder.fill" : FileIcon.symbol(entry.name))
                            .foregroundStyle(entry.folder ? Color.accentColor : .secondary)
                            .frame(width: 22)
                        Text(entry.name).font(.subheadline).lineLimit(1).truncationMode(.middle)
                        Spacer(minLength: 4)
                        if let size = entry.size { Text(FileIcon.size(size)).font(.caption.monospacedDigit()).foregroundStyle(.secondary) }
                        Image(systemName: "chevron.right").font(.caption.weight(.semibold)).foregroundStyle(.tertiary)
                    }
                    .contentShape(.rect)
                    .opacity(entry.ignored == true ? 0.45 : 1)
                }
                .buttonStyle(.plain)
                .accessibilityHint(entry.ignored == true ? "Ignored by git" : "")
                .contextMenu {
                    Button("Copy Path", systemImage: "doc.on.doc") { UIPasteboard.general.string = full }
                }
            }
        }
        .listStyle(.plain)
        .overlay {
            if let error = page?.error, page?.entries == nil {
                ContentUnavailableView("Can't Show This Folder", systemImage: "folder.badge.questionmark", description: Text(error))
            } else if page?.entries == nil {
                ProgressView()
            } else if page?.entries?.isEmpty == true {
                ContentUnavailableView("Empty Folder", systemImage: "folder")
            }
        }
        .refreshable {
            store.askDir(path, in: threadID)
            try? await Task.sleep(for: .milliseconds(400))
        }
        .task(id: store.state == .connected) { store.askDir(path, in: threadID) }
    }
}

// MARK: - A change's diff

private struct ChangeDiffScreen: View {
    let threadID: String
    let files: [PullFile]
    @Environment(Store.self) private var store
    @AppStorage("diff.wrap") private var wrap = true
    @State private var index: Int
    @State private var openFile: FileTarget?

    init(threadID: String, files: [PullFile], start: PullFile) {
        self.threadID = threadID
        self.files = files
        _index = State(initialValue: files.firstIndex(of: start) ?? 0)
    }

    private var file: PullFile { files[min(index, files.count - 1)] }
    private var key: String { Store.fileKey(threadID, file.path) }
    private var page: Store.PatchPage? { store.changePatches[key] }

    var body: some View {
        content
            .navigationTitle((file.path as NSString).lastPathComponent)
            .navigationSubtitle([(file.path as NSString).deletingLastPathComponent, file.additions.map { "+\($0) −\(file.deletions ?? 0)" } ?? ""].filter { !$0.isEmpty }.joined(separator: " · "))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar(.hidden, for: .tabBar)
            .toolbar {
                ToolbarItemGroup(placement: .topBarTrailing) {
                    Button(wrap ? "Don't Wrap Lines" : "Wrap Lines", systemImage: wrap ? "arrow.left.and.right.text.vertical" : "text.word.spacing") { wrap.toggle() }
                    Menu("More", systemImage: "ellipsis") {
                        if file.status != "removed" {
                            Button("Open File", systemImage: "doc.text") { openFile = FileTarget(thread: threadID, path: file.path) }
                        }
                        Button("Copy Path", systemImage: "doc.on.doc") { UIPasteboard.general.string = file.path }
                    }
                }
                if files.count > 1 {
                    ToolbarItemGroup(placement: .bottomBar) {
                        Button("Previous File", systemImage: "chevron.up") { index -= 1 }.disabled(index == 0)
                        Spacer()
                        Text("\(index + 1) of \(files.count)").font(.footnote).foregroundStyle(.secondary).monospacedDigit()
                        Spacer()
                        Button("Next File", systemImage: "chevron.down") { index += 1 }.disabled(index == files.count - 1)
                    }
                }
            }
            .navigationDestination(item: $openFile) { FileScreen(target: $0) }
            .task(id: "\(key)|\(page == nil)") {
                if page == nil, file.hasPatch != false { store.askChangePatch(file.path, in: threadID) }
            }
    }

    @ViewBuilder private var content: some View {
        if file.hasPatch == false {
            ContentUnavailableView("No Diff to Show", systemImage: "doc", description: Text("It's not text, or too big to diff."))
        } else if let lines = page?.lines {
            if lines.isEmpty {
                ContentUnavailableView("No Changes to Show", systemImage: "doc", description: Text("Nothing in this file's diff."))
            } else {
                PullDiffLines(lines: lines, wrap: wrap).id(key)
            }
        } else if let error = page?.error {
            ContentUnavailableView("Can't Show This Diff", systemImage: "doc.text.magnifyingglass", description: Text(error))
        } else {
            ProgressView("Reading the diff…").frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

// MARK: - A file

struct FileScreen: View {
    let target: FileTarget
    @Environment(Store.self) private var store
    @AppStorage("file.wrap") private var wrap = false
    @State private var find = Env.values["YH_PHONE_FILE_FIND"] ?? ""
    @State private var current = 0
    /// The last line that came into view, to read again around it when the file changes.
    @State private var seen = 0
    @State private var jumped = false

    private var key: String { Store.fileKey(target.thread, target.path) }
    private var doc: Store.FileDoc? { store.fileDocs[key] }
    private var name: String { (target.path as NSString).lastPathComponent }

    var body: some View {
        content
            .navigationTitle(name)
            .navigationSubtitle(subtitle)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar(.hidden, for: .tabBar)
            .toolbar {
                ToolbarItemGroup(placement: .topBarTrailing) {
                    if doc?.kind == "text" {
                        Button(wrap ? "Don't Wrap Lines" : "Wrap Lines", systemImage: wrap ? "arrow.left.and.right.text.vertical" : "text.word.spacing") { wrap.toggle() }
                    }
                    Menu("More", systemImage: "ellipsis") {
                        Button("Copy Path", systemImage: "doc.on.doc") { UIPasteboard.general.string = target.path }
                        if doc?.kind == "text", doc?.lines.count == doc?.total {
                            Button("Copy Contents", systemImage: "doc.on.clipboard") {
                                UIPasteboard.general.string = (0..<(doc?.total ?? 0)).compactMap { doc?.lines[$0]?.text }.joined(separator: "\n")
                            }
                        }
                    }
                }
                if !trimmedFind.isEmpty {
                    ToolbarItemGroup(placement: .bottomBar) {
                        Button("Previous Match", systemImage: "chevron.up") { step(-1) }.disabled(matches.isEmpty)
                        Spacer()
                        Text(matches.isEmpty ? (allRead ? "No matches" : "Reading…") : "\(current + 1) of \(matches.count)").font(.footnote).foregroundStyle(.secondary).monospacedDigit()
                        Spacer()
                        Button("Next Match", systemImage: "chevron.down") { step(1) }.disabled(matches.isEmpty)
                    }
                }
            }
            .searchable(text: $find, placement: .navigationBarDrawer(displayMode: .always), prompt: "Find in File")
            .autocorrectionDisabled()
            .textInputAutocapitalization(.never)
            .onSubmit(of: .search) { step(1) }
            // The page with the line to show first; the rest as they scroll into view.
            .task(id: store.state == .connected) {
                if doc?.kind == nil || doc?.error != nil { store.askFileLines(target.path, around: max(0, (target.line ?? 1) - 1), in: target.thread) }
            }
            // Looked at again every few seconds while it shows: an edit on the Mac comes through.
            .task {
                while !Task.isCancelled {
                    try? await Task.sleep(for: .seconds(3))
                    store.checkFile(target.path, in: target.thread)
                }
            }
            .onChange(of: doc?.revision) { _, _ in
                store.askFileLines(target.path, around: seen, in: target.thread)
                store.askFileLines(target.path, around: seen + 60, in: target.thread)
            }
            // Find reads the whole file (up to 100,000 lines) so every match is found.
            .onChange(of: trimmedFind) { _, words in
                current = 0
                guard !words.isEmpty, let total = doc?.total, total <= 100_000 else { return }
                for first in stride(from: 0, to: total, by: Store.filePage) { store.askFileLines(target.path, around: first, in: target.thread) }
            }
            .onChange(of: matches.count) { old, new in
                if old == 0, new > 0 { Perf.shared.log("FIND \(trimmedFind): \(new) matches") }
            }
            .task(id: doc?.total) {
                if let w = Env.values["YH_PHONE_FILE_WRAP"] { wrap = w == "1" }
                if !trimmedFind.isEmpty, let total = doc?.total, total <= 100_000 {
                    for first in stride(from: 0, to: total, by: Store.filePage) { store.askFileLines(target.path, around: first, in: target.thread) }
                }
            }
            .onDisappear { store.dropFile(target.path, in: target.thread) }
    }

    private var subtitle: String {
        var parts: [String] = []
        let folder = (target.path as NSString).deletingLastPathComponent
        if !folder.isEmpty { parts.append(folder) }
        if let doc, doc.kind == "text" { parts.append("\(doc.total) line\(doc.total == 1 ? "" : "s")") }
        if let doc, doc.kind != nil { parts.append(FileIcon.size(doc.size)) }
        return parts.joined(separator: " · ")
    }

    private var trimmedFind: String { find.trimmingCharacters(in: .whitespaces) }
    private var allRead: Bool { (doc?.lines.count ?? 0) >= (doc?.total ?? 0) }

    /// Every line with the words on it, in order (as many as are read so far).
    private var matches: [Int] {
        let words = trimmedFind
        guard !words.isEmpty, let doc else { return [] }
        return doc.lines.keys.sorted().filter { doc.lines[$0]!.text.range(of: words, options: .caseInsensitive) != nil }
    }

    private func step(_ by: Int) {
        guard !matches.isEmpty else { return }
        current = (current + by + matches.count) % matches.count
    }

    @ViewBuilder private var content: some View {
        if let doc, let error = doc.error, doc.kind == nil {
            ContentUnavailableView("Can't Open This File", systemImage: "doc.questionmark", description: Text(error))
        } else if doc?.kind == "image" {
            if let image = doc?.image {
                ScrollView([.vertical, .horizontal]) {
                    Image(uiImage: image).resizable().scaledToFit().frame(maxWidth: .infinity).padding()
                }
            } else {
                ContentUnavailableView(name, systemImage: "photo", description: Text(doc?.note ?? "This picture couldn't be read."))
            }
        } else if doc?.kind == "binary" {
            ContentUnavailableView(name, systemImage: "doc", description: Text("A binary file, \(FileIcon.size(doc?.size ?? 0)), not shown here."))
        } else if let doc, doc.kind == "text" {
            if doc.total == 0 {
                ContentUnavailableView("Empty File", systemImage: "doc")
            } else {
                FileLines(doc: doc, wrap: wrap, highlight: target.line.map { $0...max($0, target.end ?? $0) },
                          find: trimmedFind, currentMatch: matches.isEmpty ? nil : matches[min(current, matches.count - 1)],
                          jumpTo: jumped || !trimmedFind.isEmpty ? nil : target.line.map { $0 - 1 },
                          onJumped: { jumped = true },
                          onShow: { line in
                              seen = line
                              store.askFileLines(target.path, around: line, in: target.thread)
                          })
                    .safeAreaInset(edge: .top, spacing: 0) {
                        if let note = doc.note {
                            Label(note, systemImage: "info.circle").font(.caption).foregroundStyle(.secondary)
                                .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal).padding(.vertical, 6)
                                .background(.bar)
                        }
                    }
            }
        } else {
            ProgressView("Reading the file…").frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

/// A file's lines, drawn lazily: the line's number in a gutter, then the code. Lines not read yet
/// show empty and ask for their page as they come into view.
private struct FileLines: View {
    let doc: Store.FileDoc
    let wrap: Bool
    /// The lines a link pointed at (1-based).
    let highlight: ClosedRange<Int>?
    let find: String
    /// The line Find is on (0-based), scrolled to.
    let currentMatch: Int?
    /// The line to scroll to once it's read (0-based).
    let jumpTo: Int?
    let onJumped: () -> Void
    let onShow: (Int) -> Void

    private static let size: CGFloat = 12
    private static let font = Font.system(size: size, design: .monospaced)
    private static let charWidth = ("M" as NSString).size(withAttributes: [.font: UIFont.monospacedSystemFont(ofSize: size, weight: .regular)]).width
    /// A line scrolled to sits mid-screen, the view kept at its left edge.
    private static let middle = UnitPoint(x: 0, y: 0.5)

    var body: some View {
        let gutter = CGFloat(max(3, String(doc.total).count)) * Self.charWidth + 12
        GeometryReader { geo in
            let longest = CGFloat(doc.lines.values.lazy.map(\.text.count).max() ?? 0)
            let codeWidth = wrap ? nil : max(geo.size.width - gutter - 8, longest * Self.charWidth + 16)
            ScrollViewReader { proxy in
                ScrollView(wrap ? .vertical : [.vertical, .horizontal]) {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(0..<doc.total, id: \.self) { i in
                            row(i, gutter: gutter, codeWidth: codeWidth)
                                .id(i)
                                .onAppear { onShow(i) }
                        }
                    }
                    .padding(.vertical, 8)
                    // Unwrapped, it starts at the top left, not centred in the scroll view.
                    .frame(minWidth: geo.size.width, minHeight: geo.size.height, alignment: .topLeading)
                }
                .defaultScrollAnchor(.topLeading)
                .scrollIndicators(.automatic)
                .task(id: jumpTo.map { doc.lines[$0] != nil }) {
                    guard let line = jumpTo, doc.lines[line] != nil else { return }
                    try? await Task.sleep(for: .milliseconds(50))
                    proxy.scrollTo(line, anchor: Self.middle)
                    onJumped()
                }
                .onChange(of: currentMatch) { _, line in
                    if let line { withAnimation { proxy.scrollTo(line, anchor: Self.middle) } }
                }
            }
        }
    }

    @ViewBuilder private func row(_ i: Int, gutter: CGFloat, codeWidth: CGFloat?) -> some View {
        let line = doc.lines[i]
        let lit = highlight?.contains(i + 1) == true
        HStack(alignment: .firstTextBaseline, spacing: 0) {
            Text(verbatim: String(i + 1))
                .foregroundStyle(lit ? AnyShapeStyle(.tint) : AnyShapeStyle(.tertiary))
                .frame(width: gutter - 8, alignment: .trailing)
                .padding(.trailing, 8)
            Text(line.map { marked($0) } ?? AttributedString(" "))
                .fixedSize(horizontal: !wrap, vertical: true)
                .frame(width: codeWidth, alignment: .leading)
                .frame(maxWidth: wrap ? .infinity : nil, alignment: .leading)
        }
        .font(Self.font)
        .padding(.vertical, 1)
        .background(i == currentMatch ? Color.yellow.opacity(0.28) : lit ? Color.accentColor.opacity(0.12) : .clear)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Line \(i + 1): \(line?.text ?? "")")
    }

    /// The line with Find's words marked.
    private func marked(_ line: FileLine) -> AttributedString {
        guard !find.isEmpty, line.text.range(of: find, options: .caseInsensitive) != nil else { return line.styled }
        var out = line.styled
        var from = line.text.startIndex
        while let r = line.text.range(of: find, options: .caseInsensitive, range: from..<line.text.endIndex) {
            if let ar = Range<AttributedString.Index>(NSRange(r, in: line.text), in: out) { out[ar].backgroundColor = Color.yellow.opacity(0.55) }
            from = r.upperBound
        }
        return out
    }
}
