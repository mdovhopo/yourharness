import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

/// An image ready to send with a message: in a format the API accepts, no larger than it
/// takes, and small enough that the model reads it without downscaling it again.
public struct ImageAttachment: Sendable, Equatable, Identifiable {
    public let id: UUID
    public var data: Data
    /// `image/png`, `image/jpeg`, `image/gif` or `image/webp`.
    public var mediaType: String
    public var width: Int
    public var height: Int

    /// Longer edges are scaled down to this; the API would shrink them anyway.
    public static let maxDimension = 2000
    /// The API takes up to 5 MB of base64, about 3.75 MB of image.
    public static let maxBytes = 3_700_000

    static let acceptedTypes: [UTType: String] = [.png: "image/png", .jpeg: "image/jpeg", .gif: "image/gif", .webP: "image/webp"]

    public var fileExtension: String {
        switch mediaType {
        case "image/jpeg": "jpg"
        case "image/gif": "gif"
        case "image/webp": "webp"
        default: "png"
        }
    }

    public init(id: UUID = UUID(), data: Data, mediaType: String, width: Int, height: Int) {
        self.id = id
        self.data = data
        self.mediaType = mediaType
        self.width = width
        self.height = height
    }

    public static func prepare(contentsOf url: URL) -> ImageAttachment? {
        guard let data = try? Data(contentsOf: url, options: .mappedIfSafe) else { return nil }
        return prepare(data)
    }

    /// Anything ImageIO reads (PNG, JPEG, HEIC, TIFF from a copied screenshot, …). Images
    /// that already fit are sent untouched; the rest are scaled and re-encoded as PNG, or
    /// JPEG when PNG would still be too big.
    public static func prepare(_ data: Data) -> ImageAttachment? {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil),
              CGImageSourceGetCount(source) > 0,
              let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
              let width = properties[kCGImagePropertyPixelWidth] as? Int,
              let height = properties[kCGImagePropertyPixelHeight] as? Int,
              width > 0, height > 0 else { return nil }

        let orientation = properties[kCGImagePropertyOrientation] as? Int ?? 1
        if let type = CGImageSourceGetType(source).flatMap({ UTType($0 as String) }),
           let mediaType = acceptedTypes[type],
           max(width, height) <= maxDimension, data.count <= maxBytes, orientation == 1 {
            return ImageAttachment(data: data, mediaType: mediaType, width: width, height: height)
        }

        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: min(max(width, height), maxDimension),
        ]
        guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else { return nil }
        if let png = encode(image, as: .png), png.count <= maxBytes {
            return ImageAttachment(data: png, mediaType: "image/png", width: image.width, height: image.height)
        }
        for quality in [0.85, 0.7, 0.5] {
            if let jpeg = encode(image, as: .jpeg, quality: quality), jpeg.count <= maxBytes {
                return ImageAttachment(data: jpeg, mediaType: "image/jpeg", width: image.width, height: image.height)
            }
        }
        return nil
    }

    private static func encode(_ image: CGImage, as type: UTType, quality: Double? = nil) -> Data? {
        let output = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(output, type.identifier as CFString, 1, nil) else { return nil }
        let options = quality.map { [kCGImageDestinationLossyCompressionQuality: $0] as CFDictionary }
        CGImageDestinationAddImage(destination, image, options)
        guard CGImageDestinationFinalize(destination) else { return nil }
        return output as Data
    }

    /// The content block for a user message.
    var messageBlock: [String: Any] {
        ["type": "image", "source": ["type": "base64", "media_type": mediaType, "data": data.base64EncodedString()]]
    }
}
