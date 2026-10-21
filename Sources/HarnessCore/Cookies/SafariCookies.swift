import Foundation

/// Safari's `Cookies.binarycookies`: a "cook" header, then pages of cookie records.
/// Page sizes are big-endian; everything inside a page is little-endian.
public enum SafariCookies {
    /// Where Safari keeps them. Reading it needs Full Disk Access.
    public static var files: [URL] {
        let home = FileManager.default.homeDirectoryForCurrentUser
        return [
            home.appending(path: "Library/Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies"),
            home.appending(path: "Library/Cookies/Cookies.binarycookies"),
        ]
    }

    /// Seconds from 1970 to 2001, where Safari's dates count from.
    private static let referenceOffset: TimeInterval = 978_307_200

    public static func parse(_ data: Data) throws(CookieImportError) -> [ImportedCookie] {
        let bytes = [UInt8](data)
        guard bytes.count >= 8, bytes[0..<4] == [0x63, 0x6F, 0x6F, 0x6B] else { throw .malformed("This isn't a Safari cookie file.") }
        let pageCount = Int(bigEndian(bytes, 4))
        var sizes: [Int] = []
        var offset = 8
        for _ in 0..<pageCount {
            guard offset + 4 <= bytes.count else { throw .malformed("Safari's cookie file is cut short.") }
            sizes.append(Int(bigEndian(bytes, offset)))
            offset += 4
        }
        var cookies: [ImportedCookie] = []
        for size in sizes {
            guard offset + size <= bytes.count, size >= 8 else { throw .malformed("Safari's cookie file is cut short.") }
            cookies += page(Array(bytes[offset..<offset + size]))
            offset += size
        }
        return cookies
    }

    private static func page(_ page: [UInt8]) -> [ImportedCookie] {
        let count = Int(littleEndian(page, 4))
        var cookies: [ImportedCookie] = []
        for index in 0..<count {
            let at = 8 + index * 4
            guard at + 4 <= page.count else { break }
            let start = Int(littleEndian(page, at))
            guard start + 56 <= page.count else { continue }
            let size = Int(littleEndian(page, start))
            guard size >= 56, start + size <= page.count else { continue }
            let record = Array(page[start..<start + size])
            let flags = littleEndian(record, 8)
            func string(_ field: Int) -> String {
                let position = Int(littleEndian(record, field))
                guard position < record.count else { return "" }
                let end = record[position...].firstIndex(of: 0) ?? record.count
                return String(decoding: record[position..<end], as: UTF8.self)
            }
            let expiry = double(record, 40)
            cookies.append(ImportedCookie(
                domain: string(16), name: string(20), value: string(28), path: string(24),
                expires: expiry > 0 ? Date(timeIntervalSince1970: expiry + referenceOffset) : nil,
                isSecure: flags & 1 != 0, isHTTPOnly: flags & 4 != 0))
        }
        return cookies
    }

    private static func bigEndian(_ bytes: [UInt8], _ at: Int) -> UInt32 {
        bytes[at..<at + 4].reduce(0) { $0 << 8 | UInt32($1) }
    }

    private static func littleEndian(_ bytes: [UInt8], _ at: Int) -> UInt32 {
        guard at + 4 <= bytes.count else { return 0 }
        return bytes[at..<at + 4].reversed().reduce(0) { $0 << 8 | UInt32($1) }
    }

    private static func double(_ bytes: [UInt8], _ at: Int) -> Double {
        guard at + 8 <= bytes.count else { return 0 }
        let bits = bytes[at..<at + 8].reversed().reduce(UInt64(0)) { $0 << 8 | UInt64($1) }
        return Double(bitPattern: bits)
    }
}
