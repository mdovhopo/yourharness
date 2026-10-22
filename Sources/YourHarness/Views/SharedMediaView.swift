import AVFoundation
import AVKit
import ImageIO
import SwiftUI

/// Images and videos Claude showed you (`show_media`), from the thread's own copies.
struct SharedMediaView: View {
    @Environment(AppModel.self) private var app
    @Environment(ThreadModel.self) private var thread: ThreadModel?
    let entry: TranscriptEntry

    private var paths: [String] { MediaTool.paths(inputJSON: entry.toolInput) }

    private var caption: String? {
        let input = (try? JSONSerialization.jsonObject(with: Data(entry.toolInput.utf8))) as? [String: Any]
        return (input?["caption"] as? String).flatMap { $0.isEmpty ? nil : $0 }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Image(systemName: "photo.on.rectangle")
                    .foregroundStyle(.secondary)
                Text(caption ?? summary)
                    .foregroundStyle(caption == nil ? .secondary : .primary)
                if entry.isStreaming { ProgressView().controlSize(.small) }
            }
            .font(.callout)
            if entry.toolIsError, let result = entry.toolResult {
                Text(result)
                    .font(.callout)
                    .foregroundStyle(.red)
                    .textSelection(.enabled)
            } else if let thread, !entry.isStreaming {
                // Several side by side, wrapping; one alone gets more room.
                let items = paths.enumerated().map { index, path in
                    MediaItem(url: app.store.attachmentURL(MediaTool.storedName(thread: thread.id, toolUseID: entry.id, index: index, path: path)),
                              original: path, kind: MediaTool.kind(of: path) ?? .image)
                }.filter { FileManager.default.fileExists(atPath: $0.url.path) }
                let single = items.count == 1
                LazyVGrid(columns: [GridItem(.adaptive(minimum: single ? 300 : 180, maximum: single ? 640 : 320), spacing: 10, alignment: .topLeading)],
                          alignment: .leading, spacing: 10) {
                    ForEach(items, id: \.url) { item in
                        MediaTile(item: item, large: single)
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var summary: String {
        let kinds = paths.compactMap(MediaTool.kind(of:))
        let videos = kinds.count { $0 == .video }, images = kinds.count - videos
        var parts: [String] = []
        if images > 0 { parts.append(images == 1 ? "an image" : "\(images) images") }
        if videos > 0 { parts.append(videos == 1 ? "a video" : "\(videos) videos") }
        return "Claude showed " + (parts.isEmpty ? "media" : parts.joined(separator: " and "))
    }
}

private struct MediaItem {
    let url: URL
    let original: String
    let kind: MediaTool.Kind
}

private struct MediaTile: View {
    let item: MediaItem
    let large: Bool
    @State private var poster: NSImage?
    @State private var player: AVPlayer?

    var body: some View {
        ZStack {
            if let player {
                VideoPlayer(player: player)
                    .aspectRatio(aspect, contentMode: .fit)
            } else if let poster {
                Image(nsImage: poster)
                    .resizable()
                    .aspectRatio(contentMode: .fit)
                    .overlay {
                        if item.kind == .video {
                            Image(systemName: "play.circle.fill")
                                .font(.system(size: 44))
                                .symbolRenderingMode(.palette)
                                .foregroundStyle(.white, .black.opacity(0.45))
                        }
                    }
            } else {
                RoundedRectangle(cornerRadius: Theme.innerRadius)
                    .fill(.quaternary)
                    .aspectRatio(4 / 3, contentMode: .fit)
                    .overlay { ProgressView().controlSize(.small) }
            }
        }
        .frame(maxHeight: large ? 480 : 240)
        .clipShape(.rect(cornerRadius: Theme.innerRadius))
        .overlay(RoundedRectangle(cornerRadius: Theme.innerRadius).stroke(.separator))
        .contentShape(.rect)
        .onTapGesture {
            if item.kind == .video, player == nil {
                let player = AVPlayer(url: item.url)
                self.player = player
                player.play()
            } else if item.kind == .image {
                NSWorkspace.shared.open(item.url)
            }
        }
        .pointerStyle(.link)
        .help(item.kind == .video ? "Play \((item.original as NSString).lastPathComponent)" : "Open \((item.original as NSString).lastPathComponent) in Preview")
        .contextMenu {
            Button("Open", systemImage: "arrow.up.forward.app") { NSWorkspace.shared.open(item.url) }
            Button("Show in Finder", systemImage: "folder") { NSWorkspace.shared.activateFileViewerSelecting([item.url]) }
            Button("Copy Original Path", systemImage: "doc.on.doc") { Pasteboard.copy(item.original) }
        }
        .task(id: item.url) { poster = await Self.poster(for: item) }
        .onDisappear {
            player?.pause()
            player = nil
        }
    }

    private var aspect: CGFloat {
        guard let poster, poster.size.height > 0 else { return 16 / 9 }
        return poster.size.width / poster.size.height
    }

    /// A downsampled image, so a 4K screenshot doesn't sit in memory at full size; a video's
    /// first frame.
    private static func poster(for item: MediaItem) async -> NSImage? {
        let url = item.url
        switch item.kind {
        case .image:
            return await Task.detached {
                guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
                      let image = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                          kCGImageSourceCreateThumbnailFromImageAlways: true,
                          kCGImageSourceCreateThumbnailWithTransform: true,
                          kCGImageSourceThumbnailMaxPixelSize: 1400,
                      ] as CFDictionary)
                else { return nil }
                return NSImage(cgImage: image, size: NSSize(width: image.width, height: image.height))
            }.value
        case .video:
            let generator = AVAssetImageGenerator(asset: AVURLAsset(url: url))
            generator.appliesPreferredTrackTransform = true
            generator.maximumSize = CGSize(width: 1400, height: 1400)
            guard let (image, _) = try? await generator.image(at: .zero) else { return nil }
            return NSImage(cgImage: image, size: NSSize(width: image.width, height: image.height))
        }
    }
}
