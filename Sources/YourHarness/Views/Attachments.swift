import AppKit
import HarnessCore
import ImageIO
import SwiftUI
import UniformTypeIdentifiers

/// Turns pasted, dropped or chosen images into attachments, off the main thread.
enum ImageIntake {
    static let limit = 20

    /// Image files (copied in Finder) or raw image data (a screenshot). Nil when the
    /// pasteboard holds text, so a normal paste goes ahead.
    @MainActor
    static func sources(on pasteboard: NSPasteboard) -> [Source]? {
        let urls = (pasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? [])
            .filter(isImageFile)
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

    static func prepare(_ sources: [Source]) async -> [ImageAttachment] {
        await Task.detached(priority: .userInitiated) {
            sources.prefix(limit).compactMap { source in
                switch source {
                case .file(let url): ImageAttachment.prepare(contentsOf: url)
                case .data(let data): ImageAttachment.prepare(data)
                }
            }
        }.value
    }

    /// Files and images dragged onto a thread.
    @MainActor
    static func sources(from providers: [NSItemProvider]) async -> [Source] {
        var sources: [Source] = []
        for provider in providers {
            if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier),
               let url = await loadURL(provider), isImageFile(url) {
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

    /// The paperclip: pick image files.
    @MainActor
    static func choose() -> [Source] {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.image]
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = false
        panel.prompt = "Attach"
        guard panel.runModal() == .OK else { return [] }
        return panel.urls.map(Source.file)
    }
}

/// Catches ⌘V while the composer is focused and the pasteboard holds images; text pastes
/// go through untouched. A text field can't take images itself.
@MainActor
final class ImagePasteMonitor {
    var isActive = false
    private var monitor: Any?

    func start(onPaste: @escaping @MainActor ([ImageIntake.Source]) -> Void) {
        guard monitor == nil else { return }
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command,
                  event.charactersIgnoringModifiers == "v" else { return event }
            let handled = MainActor.assumeIsolated { () -> Bool in
                guard let self, self.isActive, let sources = ImageIntake.sources(on: .general) else { return false }
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

    var body: some View {
        Group {
            if let image = image ?? Thumbnails.cached(key) {
                Image(nsImage: image)
                    .resizable()
                    .aspectRatio(contentMode: .fit)
            } else {
                RoundedRectangle(cornerRadius: 8).fill(.quaternary)
                    .aspectRatio(4 / 3, contentMode: .fit)
            }
        }
        .frame(maxWidth: maxSize.width, maxHeight: maxSize.height)
        .clipShape(.rect(cornerRadius: 8))
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

/// Images waiting in the composer, each with a remove button.
struct DraftAttachments: View {
    @Bindable var thread: ThreadModel

    var body: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 8) {
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
    /// Adds images to the next message, up to the limit.
    func attach(_ sources: [ImageIntake.Source]) {
        guard !sources.isEmpty else { return }
        Task {
            let images = await ImageIntake.prepare(sources)
            let room = max(0, ImageIntake.limit - draftImages.count)
            withAnimation(.snappy) { draftImages += images.prefix(room) }
        }
    }
}
