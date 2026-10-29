import AppKit
import HarnessCore
import ImageIO
import SwiftUI
import UniformTypeIdentifiers

/// Turns pasted, dropped or chosen files into attachments, off the main thread: images
/// Claude sees inline, anything else (a PDF, a log, a folder) by its path.
enum AttachmentIntake {
    static let limit = 20

    /// Files (copied in Finder) or raw image data (a screenshot). Nil when the pasteboard
    /// holds text, so a normal paste goes ahead.
    @MainActor
    static func sources(on pasteboard: NSPasteboard) -> [Source]? {
        let urls = pasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
        if !urls.isEmpty { return urls.map(Source.file) }
        guard !pasteboard.canReadItem(withDataConformingToTypes: [UTType.plainText.identifier]) else { return nil }
        for type in [NSPasteboard.PasteboardType.png, .tiff, NSPasteboard.PasteboardType(UTType.jpeg.identifier),
                     NSPasteboard.PasteboardType(UTType.heic.identifier)] {
            if let data = pasteboard.data(forType: type) { return [.data(data)] }
        }
        return nil
    }

    enum Source: Sendable {
        case file(URL)
        case data(Data)
    }

    static func isImageFile(_ url: URL) -> Bool {
        UTType(filenameExtension: url.pathExtension)?.conforms(to: .image) == true
    }

    /// Images ready to send inline, and the rest. An image ImageIO can't read (a PSD, an
    /// SVG) goes as a file.
    static func prepare(_ sources: [Source]) async -> (images: [ImageAttachment], files: [FileAttachment]) {
        await Task.detached(priority: .userInitiated) {
            var images: [ImageAttachment] = [], files: [FileAttachment] = []
            for source in sources.prefix(limit) {
                switch source {
                case .file(let url):
                    if isImageFile(url), let image = ImageAttachment.prepare(contentsOf: url) {
                        images.append(image)
                    } else if let file = FileAttachment(contentsOf: url) {
                        files.append(file)
                    }
                case .data(let data):
                    if let image = ImageAttachment.prepare(data) { images.append(image) }
                }
            }
            return (images, files)
        }.value
    }

    /// Files, folders and images dragged onto a thread.
    @MainActor
    static func sources(from providers: [NSItemProvider]) async -> [Source] {
        var sources: [Source] = []
        for provider in providers {
            if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier),
               let url = await loadURL(provider) {
                sources.append(.file(url))
            } else if provider.hasItemConformingToTypeIdentifier(UTType.image.identifier),
                      let data = await loadData(provider) {
                sources.append(.data(data))
            }
        }
        return sources
    }

    @MainActor
    private static func loadURL(_ provider: NSItemProvider) async -> URL? {
        await withCheckedContinuation { continuation in
            _ = provider.loadObject(ofClass: URL.self) { url, _ in continuation.resume(returning: url) }
        }
    }

    @MainActor
    private static func loadData(_ provider: NSItemProvider) async -> Data? {
        await withCheckedContinuation { continuation in
            _ = provider.loadDataRepresentation(for: .image) { data, _ in continuation.resume(returning: data) }
        }
    }

    /// The paperclip: pick files or folders.
    @MainActor
    static func choose() -> [Source] {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = true
        panel.prompt = "Attach"
        guard panel.runModal() == .OK else { return [] }
        return panel.urls.map(Source.file)
    }
}

/// Catches ⌘V while the composer is focused and the pasteboard holds files or images; text
/// pastes go through untouched. A text field can't take them itself.
@MainActor
final class ImagePasteMonitor {
    var isActive = false
    private var monitor: Any?

    func start(onPaste: @escaping @MainActor ([AttachmentIntake.Source]) -> Void) {
        guard monitor == nil else { return }
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command,
                  event.charactersIgnoringModifiers == "v" else { return event }
            let handled = MainActor.assumeIsolated { () -> Bool in
                guard let self, self.isActive, let sources = AttachmentIntake.sources(on: .general) else { return false }
                onPaste(sources)
                return true
            }
            return handled ? nil : event
        }
    }

    func stop() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
    }
}

/// Small, downsampled previews; the full image is never decoded for display.
enum Thumbnails {
    nonisolated(unsafe) private static let cache: NSCache<NSString, NSImage> = {
        let cache = NSCache<NSString, NSImage>()
        cache.countLimit = 200
        return cache
    }()

    static func cached(_ key: String) -> NSImage? { cache.object(forKey: key as NSString) }

    static func load(key: String, maxPixels: Int, source: @escaping @Sendable () -> CGImageSource?) async -> NSImage? {
        if let image = cached(key) { return image }
        let image = await Task.detached(priority: .utility) { () -> NSImage? in
            guard let source = source() else { return nil }
            let options: [CFString: Any] = [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceThumbnailMaxPixelSize: maxPixels,
            ]
            guard let cgImage = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else { return nil }
            return NSImage(cgImage: cgImage, size: .zero)
        }.value
        if let image { cache.setObject(image, forKey: key as NSString) }
        return image
    }
}

struct AttachmentThumbnail: View {
    enum Content {
        case data(ImageAttachment)
        case file(URL)
    }

    let content: Content
    let maxSize: CGSize
    @State private var image: NSImage?

    private var key: String {
        switch content {
        case .data(let attachment): "draft-\(attachment.id)"
        case .file(let url): url.lastPathComponent
        }
    }

    /// Always the full height, before and after the image loads: a row that changes height
    /// as its thumbnail arrives knocks the transcript's bottom anchoring off, and together
    /// with the composer shrinking on send it once left the whole transcript blank.
    /// Only the width follows the image; very wide or tall ones are cropped.
    private func width(for image: NSImage?) -> CGFloat {
        let height = maxSize.height
        guard let size = image?.size, size.width > 0, size.height > 0 else { return min(maxSize.width, height * 4 / 3) }
        return min(maxSize.width, max(height / 2, height * size.width / size.height))
    }

    var body: some View {
        let shown = image ?? Thumbnails.cached(key)
        Group {
            if let shown {
                Image(nsImage: shown)
                    .resizable()
                    .scaledToFill()
            } else {
                Rectangle().fill(.quaternary)
            }
        }
        .frame(width: width(for: shown), height: maxSize.height)
        .clipShape(.rect(cornerRadius: 8))
        .contentShape(.rect(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.separator))
        .task(id: key) {
            let pixels = Int(max(maxSize.width, maxSize.height) * 2)
            switch content {
            case .data(let attachment):
                let data = attachment.data
                image = await Thumbnails.load(key: key, maxPixels: pixels) { CGImageSourceCreateWithData(data as CFData, nil) }
            case .file(let url):
                image = await Thumbnails.load(key: key, maxPixels: pixels) { CGImageSourceCreateWithURL(url as CFURL, nil) }
            }
        }
    }
}

/// Images and files waiting in the composer, each with a remove button.
struct DraftAttachments: View {
    @Bindable var thread: ThreadModel

    var body: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 8) {
                ForEach(thread.draftFiles) { file in
                    FileChip(url: file.url, isDirectory: file.isDirectory, size: file.size)
                        .frame(height: 64)
                        .overlay(alignment: .topTrailing) {
                            Button("Remove", systemImage: "xmark.circle.fill") {
                                withAnimation(.snappy) { thread.draftFiles.removeAll { $0.id == file.id } }
                            }
                            .labelStyle(.iconOnly)
                            .buttonStyle(.plain)
                            .symbolRenderingMode(.palette)
                            .foregroundStyle(.white, .black.opacity(0.6))
                            .padding(3)
                        }
                        .help(file.url.path)
                        .transition(.scale(scale: 0.8).combined(with: .opacity))
                }
                ForEach(thread.draftImages) { image in
                    AttachmentThumbnail(content: .data(image), maxSize: CGSize(width: 96, height: 64))
                        .overlay(alignment: .topTrailing) {
                            Button("Remove", systemImage: "xmark.circle.fill") {
                                withAnimation(.snappy) { thread.draftImages.removeAll { $0.id == image.id } }
                            }
                            .labelStyle(.iconOnly)
                            .buttonStyle(.plain)
                            .symbolRenderingMode(.palette)
                            .foregroundStyle(.white, .black.opacity(0.6))
                            .padding(3)
                        }
                        .help("\(image.width) × \(image.height)")
                        .transition(.scale(scale: 0.8).combined(with: .opacity))
                }
            }
            .padding(.vertical, 2)
        }
        .scrollIndicators(.never)
    }
}

extension ThreadModel {
    /// Adds files and images to the next message, up to the limit. One already there isn't
    /// added twice.
    func attach(_ sources: [AttachmentIntake.Source]) {
        guard !sources.isEmpty else { return }
        Task {
            let (images, files) = await AttachmentIntake.prepare(sources)
            var room = max(0, AttachmentIntake.limit - draftImages.count - draftFiles.count)
            let newFiles = files.filter { file in !draftFiles.contains { $0.url == file.url } }.prefix(room)
            room -= newFiles.count
            withAnimation(.snappy) {
                draftFiles += newFiles
                draftImages += images.prefix(room)
            }
        }
    }
}

/// A file or folder sent with a message: its icon, name and size. No preview: Claude reads
/// it, and a click opens it in its own app.
struct FileChip: View {
    let url: URL
    let isDirectory: Bool
    var size: Int64?
    @State private var icon: NSImage?

    var body: some View {
        HStack(spacing: 8) {
            Group {
                if let icon {
                    Image(nsImage: icon).resizable()
                } else {
                    Image(systemName: isDirectory ? "folder" : "doc").resizable().scaledToFit().foregroundStyle(.secondary)
                }
            }
            .frame(width: 28, height: 28)
            VStack(alignment: .leading, spacing: 1) {
                Text(url.lastPathComponent)
                    .font(.callout.weight(.medium))
                    .lineLimit(1)
                    .truncationMode(.middle)
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
        .padding(.leading, 8)
        .padding(.trailing, 12)
        .frame(maxWidth: 240, maxHeight: .infinity, alignment: .leading)
        .background(.quaternary.opacity(0.6), in: .rect(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.separator))
        .contentShape(.rect(cornerRadius: 8))
        .task(id: url) {
            let path = url.path
            // Reading an icon touches the disk: off the main thread. A file just sent may
            // still be being copied.
            for _ in 0..<10 {
                icon = await Task.detached(priority: .utility) {
                    FileManager.default.fileExists(atPath: path) ? NSWorkspace.shared.icon(forFile: path) : nil
                }.value
                if icon != nil { return }
                try? await Task.sleep(for: .milliseconds(300))
            }
        }
    }

    private var detail: String {
        if isDirectory { return "Folder" }
        let kind = UTType(filenameExtension: url.pathExtension)?.localizedDescription ?? url.pathExtension.uppercased()
        guard let size else { return kind }
        return kind + " · " + ByteCountFormatter.string(fromByteCount: size, countStyle: .file)
    }
}
