// The app icon, drawn: a Y of two threads (teal and violet) joining into one white stem, the
// app's agents gathered into one. Stable on graphite, Next on night blue. Writes PNGs:
//
//   icon <out dir>
//
// `<variant>-ios.png`: 1024 px, full bleed and opaque (iOS masks it, and App Store Connect
// refuses alpha); `<variant>-mac-<size>.png`: macOS's grid (an 824/1024 squircle with its
// shadow), at every size an .iconset holds. scripts/icons.sh turns them into the apps' icons.

import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

func rgb(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
    CGColor(red: CGFloat((hex >> 16) & 255) / 255, green: CGFloat((hex >> 8) & 255) / 255, blue: CGFloat(hex & 255) / 255, alpha: a)
}

/// Apple's continuous-corner tile, near enough: a superellipse.
func squircle(_ r: CGRect) -> CGPath {
    let p = CGMutablePath(), n = 5.0, steps = 720
    for i in 0...steps {
        let t = Double(i) / Double(steps) * 2 * .pi, c = cos(t), s = sin(t)
        let pt = CGPoint(x: r.midX + pow(abs(c), 2 / n) * (c < 0 ? -1 : 1) * r.width / 2, y: r.midY + pow(abs(s), 2 / n) * (s < 0 ? -1 : 1) * r.height / 2)
        i == 0 ? p.move(to: pt) : p.addLine(to: pt)
    }
    p.closeSubpath()
    return p
}

enum Variant: String, CaseIterable { case stable, next }

func background(_ ctx: CGContext, _ s: CGFloat, _ v: Variant) {
    let (colors, from, to): ([CGColor], CGPoint, CGPoint) = switch v {
    case .stable: ([rgb(0x1C2026), rgb(0x2E3540)], CGPoint(x: 0, y: s), CGPoint(x: 0, y: 0))
    case .next: ([rgb(0x101B3D), rgb(0x2A2370)], CGPoint(x: 0, y: s), CGPoint(x: s, y: 0))
    }
    let g = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB), colors: colors as CFArray, locations: nil)!
    ctx.drawLinearGradient(g, start: from, end: to, options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
}

func strand(_ ctx: CGContext, _ color: CGColor, _ w: CGFloat, _ pts: [CGPoint]) {
    ctx.setStrokeColor(color); ctx.setLineWidth(w); ctx.setLineCap(.round); ctx.setLineJoin(.round)
    ctx.move(to: pts[0])
    if pts.count == 4 { ctx.addCurve(to: pts[3], control1: pts[1], control2: pts[2]) } else { pts.dropFirst().forEach { ctx.addLine(to: $0) } }
    ctx.strokePath()
}

/// The mark on a tile `s` points square (origin bottom left).
func mark(_ ctx: CGContext, _ s: CGFloat) {
    let w = s * 0.085, cx = 0.5, top = 0.76, joinY = 0.47, foot = 0.24, spread = 0.23
    let join = CGPoint(x: s * cx, y: s * joinY)
    strand(ctx, rgb(0xFFFFFF), w, [join, CGPoint(x: s * cx, y: s * foot)])
    for (side, color) in [(-1.0, rgb(0x5EEAD4)), (1.0, rgb(0xA78BFA))] {
        let x = s * (cx + side * spread)
        strand(ctx, color, w, [CGPoint(x: x, y: s * top), CGPoint(x: x, y: s * (joinY + 0.10)), CGPoint(x: s * cx, y: s * (joinY + 0.12)), join])
    }
}

func render(_ v: Variant, mac: Bool, size: Int) -> CGImage {
    let alpha = mac ? CGImageAlphaInfo.premultipliedLast : .noneSkipLast
    let ctx = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: alpha.rawValue)!
    let S = CGFloat(size)
    if mac {
        let tile = S * 824 / 1024, inset = (S - tile) / 2
        let rect = CGRect(x: inset, y: inset + S * 0.008, width: tile, height: tile)
        ctx.saveGState()
        ctx.setShadow(offset: CGSize(width: 0, height: -S * 0.012), blur: S * 0.03, color: rgb(0x000000, 0.35))
        ctx.addPath(squircle(rect)); ctx.setFillColor(rgb(0x000000)); ctx.fillPath()
        ctx.restoreGState()
        ctx.addPath(squircle(rect)); ctx.clip()
        ctx.translateBy(x: rect.minX, y: rect.minY)
        background(ctx, tile, v); mark(ctx, tile)
    } else {
        background(ctx, S, v); mark(ctx, S)
    }
    return ctx.makeImage()!
}

func save(_ img: CGImage, _ path: String) {
    let d = CGImageDestinationCreateWithURL(URL(fileURLWithPath: path) as CFURL, UTType.png.identifier as CFString, 1, nil)!
    CGImageDestinationAddImage(d, img, nil)
    guard CGImageDestinationFinalize(d) else { fatalError("couldn't write \(path)") }
}

let out = CommandLine.arguments.dropFirst().first ?? "."
for v in Variant.allCases {
    save(render(v, mac: false, size: 1024), "\(out)/\(v.rawValue)-ios.png")
    for size in [16, 32, 64, 128, 256, 512, 1024] {
        save(render(v, mac: true, size: size), "\(out)/\(v.rawValue)-mac-\(size).png")
    }
}
