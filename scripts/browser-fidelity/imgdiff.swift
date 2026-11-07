// How far apart two images are, the first scaled to the second's size: the share of pixels
// whose color differs by more than 40/255. With --top, only the first image's top part, as
// tall as the second's aspect ratio makes it (a full-page shot's first screen).
// Usage: imgdiff <reference> <ours> [--top]
import AppKit

func pixels(_ image: CGImage, width: Int, height: Int) -> [UInt8] {
    var buffer = [UInt8](repeating: 0, count: width * height * 4)
    let context = CGContext(data: &buffer, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
                            space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    context.interpolationQuality = .high
    context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
    return buffer
}

func load(_ path: String) -> CGImage { NSImage(contentsOfFile: path)!.cgImage(forProposedRect: nil, context: nil, hints: nil)! }
let arguments = CommandLine.arguments
let reference = load(arguments[1])
var ours = load(arguments[2])
if arguments.contains("--top") {
    let height = Int((Double(ours.width) * Double(reference.height) / Double(reference.width)).rounded())
    ours = ours.cropping(to: CGRect(x: 0, y: 0, width: ours.width, height: min(height, ours.height)))!
}
let a = pixels(reference, width: ours.width, height: ours.height), b = pixels(ours, width: ours.width, height: ours.height)
var off = 0
for index in stride(from: 0, to: a.count, by: 4) where (0..<3).contains(where: { abs(Int(a[index + $0]) - Int(b[index + $0])) > 40 }) { off += 1 }
print(String(format: "%.3f", 100 * Double(off) / Double(a.count / 4)))
