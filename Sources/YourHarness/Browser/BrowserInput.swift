import AppKit
import WebKit

/// A key as the agent names it (`Enter`, `Shift+Tab`, `Meta+A`, `a`), as AppKit sends it.
struct BrowserKey {
    let keyCode: UInt16
    let characters: String
    let charactersIgnoringModifiers: String
    let modifiers: NSEvent.ModifierFlags

    init?(_ spec: String) {
        var parts = spec.split(separator: "+", omittingEmptySubsequences: false).map(String.init)
        // "Meta++" is Meta and "+".
        if spec.hasSuffix("++") { parts = Array(parts.dropLast(2)) + ["+"] }
        guard let keyName = parts.popLast(), !keyName.isEmpty else { return nil }
        var modifiers: NSEvent.ModifierFlags = []
        for part in parts {
            switch part.lowercased() {
            case "meta", "cmd", "command", "⌘", "controlormeta": modifiers.insert(.command)
            case "control", "ctrl", "⌃": modifiers.insert(.control)
            case "alt", "option", "opt", "⌥": modifiers.insert(.option)
            case "shift", "⇧": modifiers.insert(.shift)
            default: return nil
            }
        }
        let named: [String: (UInt16, Int)] = [
            "enter": (36, 0x0D), "return": (36, 0x0D), "tab": (48, 0x09), "space": (49, 0x20), " ": (49, 0x20),
            "backspace": (51, 0x7F), "escape": (53, 0x1B), "esc": (53, 0x1B), "delete": (117, NSDeleteFunctionKey),
            "arrowup": (126, NSUpArrowFunctionKey), "arrowdown": (125, NSDownArrowFunctionKey),
            "arrowleft": (123, NSLeftArrowFunctionKey), "arrowright": (124, NSRightArrowFunctionKey),
            "up": (126, NSUpArrowFunctionKey), "down": (125, NSDownArrowFunctionKey),
            "left": (123, NSLeftArrowFunctionKey), "right": (124, NSRightArrowFunctionKey),
            "home": (115, NSHomeFunctionKey), "end": (119, NSEndFunctionKey),
            "pageup": (116, NSPageUpFunctionKey), "pagedown": (121, NSPageDownFunctionKey),
        ]
        if let (code, scalar) = named[keyName.lowercased()] {
            let character = String(UnicodeScalar(UInt32(scalar)).map(Character.init) ?? " ")
            keyCode = code
            characters = character
            charactersIgnoringModifiers = character
            self.modifiers = modifiers.union(code >= 115 ? [.function] : [])
            return
        }
        guard keyName.count == 1, let code = Self.codes[keyName.lowercased()] else { return nil }
        keyCode = code
        let shifted = modifiers.contains(.shift) ? keyName.uppercased() : keyName
        charactersIgnoringModifiers = keyName.lowercased()
        // With ⌘ or ⌃, AppKit's characters are the plain key; otherwise what it types.
        characters = modifiers.contains(.control) ? String(UnicodeScalar(UInt8(max(1, (keyName.lowercased().unicodeScalars.first?.value ?? 97) - 96)))) : shifted
        self.modifiers = modifiers
    }

    /// ⌘A, ⌘C, ⌘X or ⌘V: "a", "c", "x", "v".
    var editingShortcut: String? {
        guard modifiers.intersection([.command, .control, .option, .shift]) == .command else { return nil }
        return ["a", "c", "x", "v"].contains(charactersIgnoringModifiers) ? charactersIgnoringModifiers : nil
    }

    private static let codes: [String: UInt16] = [
        "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12, "w": 13,
        "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23, "=": 24, "9": 25,
        "7": 26, "-": 27, "8": 28, "0": 29, "]": 30, "o": 31, "u": 32, "[": 33, "i": 34, "p": 35, "l": 37, "j": 38,
        "'": 39, "k": 40, ";": 41, "\\": 42, ",": 43, "/": 44, "n": 45, "m": 46, ".": 47, "`": 50, "+": 24,
    ]
}

@MainActor
enum BrowserScreenshot {
    /// A JPEG of part of the page (CSS pixels, within the viewport), or all of what's in
    /// view, at most `maxWidth` pixels wide: a full desktop page is readable at 1280.
    static func capture(_ tab: BrowserTab, cssRect: CGRect? = nil, maxWidth: Double = 1280) async throws -> Data {
        let web = tab.webView
        let zoom = web.pageZoom
        let viewSize = CGSize(width: web.bounds.width / zoom, height: web.bounds.height / zoom)
        let area = (cssRect ?? CGRect(origin: .zero, size: viewSize)).intersection(CGRect(origin: .zero, size: viewSize))
        guard area.width >= 1, area.height >= 1 else { throw BrowserError("That's outside the visible page; scroll it into view first.") }
        let configuration = WKSnapshotConfiguration()
        let y = web.isFlipped ? area.minY * zoom : web.bounds.height - area.maxY * zoom
        configuration.rect = CGRect(x: area.minX * zoom, y: y, width: area.width * zoom, height: area.height * zoom)
        let width = min(area.width, maxWidth)
        configuration.snapshotWidth = NSNumber(value: width)
        configuration.afterScreenUpdates = true
        let image = try await web.takeSnapshot(configuration: configuration)
        guard let data = jpeg(image, width: Int(width.rounded()), height: Int((area.height * width / area.width).rounded())) else {
            throw BrowserError("Couldn't capture the page.")
        }
        return data
    }

    /// Drawn at exactly this many pixels: the snapshot comes back at the screen's scale.
    static func jpeg(_ image: NSImage, width: Int, height: Int, quality: Double = 0.8) -> Data? {
        guard width > 0, height > 0,
              let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: width, pixelsHigh: height, bitsPerSample: 8,
                                         samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                                         bytesPerRow: 0, bitsPerPixel: 0)
        else { return nil }
        rep.size = NSSize(width: width, height: height)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        NSColor.white.setFill()
        NSRect(x: 0, y: 0, width: width, height: height).fill()
        image.draw(in: NSRect(x: 0, y: 0, width: width, height: height))
        NSGraphicsContext.restoreGraphicsState()
        return rep.representation(using: .jpeg, properties: [.compressionFactor: quality])
    }
}
