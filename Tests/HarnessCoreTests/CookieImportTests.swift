import CommonCrypto
import Foundation
import SQLite3
import Testing
@testable import HarnessCore

struct CookieImportTests {
    // MARK: Safari

    /// One page of records, as Safari writes them.
    private static func binaryCookies(_ cookies: [(domain: String, name: String, path: String, value: String, flags: UInt32, expiry: Double)]) -> Data {
        func le32(_ v: UInt32) -> [UInt8] { withUnsafeBytes(of: v.littleEndian, Array.init) }
        func be32(_ v: UInt32) -> [UInt8] { withUnsafeBytes(of: v.bigEndian, Array.init) }
        func le64(_ v: Double) -> [UInt8] { withUnsafeBytes(of: v.bitPattern.littleEndian, Array.init) }
        let records: [[UInt8]] = cookies.map { cookie in
            var strings: [UInt8] = []
            var offsets: [UInt32] = []
            for part in [cookie.domain, cookie.name, cookie.path, cookie.value] {
                offsets.append(UInt32(56 + strings.count))
                strings += Array(part.utf8) + [0]
            }
            let size = UInt32(56 + strings.count)
            var record = le32(size) + le32(1) + le32(cookie.flags) + le32(0)
            record += offsets.flatMap(le32) + [UInt8](repeating: 0, count: 8)
            record += le64(cookie.expiry) + le64(0)
            return record + strings
        }
        let headerSize = 8 + 4 * records.count + 4
        var page = [0, 0, 1, 0] + le32(UInt32(records.count))
        var at = headerSize
        for record in records {
            page += le32(UInt32(at))
            at += record.count
        }
        page += le32(0)
        for record in records { page += record }
        return Data(Array("cook".utf8) + be32(1) + be32(UInt32(page.count)) + page)
    }

    @Test func readsSafariCookies() throws {
        let data = Self.binaryCookies([
            (".example.com", "session", "/", "abc123", 5, 800_000_000),
            ("app.test", "theme", "/settings", "dark", 0, 0),
        ])
        let cookies = try SafariCookies.parse(data)
        #expect(cookies.count == 2)
        #expect(cookies[0] == ImportedCookie(domain: ".example.com", name: "session", value: "abc123", path: "/",
                                             expires: Date(timeIntervalSince1970: 800_000_000 + 978_307_200), isSecure: true, isHTTPOnly: true))
        #expect(cookies[0].site == "example.com")
        #expect(cookies[1].expires == nil)
        #expect(cookies[1].path == "/settings")
        #expect(throws: CookieImportError.self) { try SafariCookies.parse(Data("nope".utf8)) }
        #expect(throws: CookieImportError.self) { try SafariCookies.parse(data.prefix(20)) }
    }

    // MARK: Chrome

    private static func chromeDatabase(version: Int, password: String) throws -> URL {
        let url = FileManager.default.temporaryDirectory.appending(path: "yh-chrome-\(UUID().uuidString).db")
        var db: OpaquePointer?
        try #require(sqlite3_open(url.path, &db) == SQLITE_OK)
        defer { sqlite3_close(db) }
        let key = ChromiumCookies.derivedKey(password)
        let exec = { (sql: String) in _ = sqlite3_exec(db, sql, nil, nil, nil) }
        exec("CREATE TABLE meta (key TEXT, value TEXT)")
        exec("INSERT INTO meta VALUES ('version', '\(version)')")
        exec("CREATE TABLE cookies (host_key TEXT, name TEXT, value TEXT, encrypted_value BLOB, path TEXT, expires_utc INTEGER, is_secure INTEGER, is_httponly INTEGER, samesite INTEGER)")
        func insert(_ host: String, _ name: String, plain: String, encrypted: Data?, expires: Int64, samesite: Int32) {
            var statement: OpaquePointer?
            sqlite3_prepare_v2(db, "INSERT INTO cookies VALUES (?, ?, ?, ?, '/', ?, 1, 0, ?)", -1, &statement, nil)
            let transient = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
            sqlite3_bind_text(statement, 1, host, -1, transient)
            sqlite3_bind_text(statement, 2, name, -1, transient)
            sqlite3_bind_text(statement, 3, plain, -1, transient)
            if let encrypted { _ = encrypted.withUnsafeBytes { sqlite3_bind_blob(statement, 4, $0.baseAddress, Int32(encrypted.count), transient) } }
            sqlite3_bind_int64(statement, 5, expires)
            sqlite3_bind_int(statement, 6, samesite)
            sqlite3_step(statement)
            sqlite3_finalize(statement)
        }
        var value = Data("s3cret".utf8)
        if version >= 24 {
            var hash = [UInt8](repeating: 0, count: Int(CC_SHA256_DIGEST_LENGTH))
            let host = Array(".github.com".utf8)
            CC_SHA256(host, CC_LONG(host.count), &hash)
            value = Data(hash) + value
        }
        // 2030-01-01 in microseconds since 1601.
        insert(".github.com", "user_session", plain: "", encrypted: ChromiumCookies.encrypt(value, key: key), expires: 13_537_929_600_000_000, samesite: 1)
        insert("localhost", "plain", plain: "visible", encrypted: nil, expires: 0, samesite: -1)
        return url
    }

    @Test(arguments: [20, 24]) func readsChromeCookies(version: Int) throws {
        let database = try Self.chromeDatabase(version: version, password: "hunter2")
        defer { try? FileManager.default.removeItem(at: database) }
        let cookies = try ChromiumCookies.read(database, password: "hunter2")
        let session = try #require(cookies.first { $0.name == "user_session" })
        #expect(session.value == "s3cret")
        #expect(session.isSecure)
        #expect(session.sameSite == "lax")
        #expect(session.expires == ISO8601DateFormatter().date(from: "2030-01-01T00:00:00Z"))
        let plain = try #require(cookies.first { $0.name == "plain" })
        #expect(plain.value == "visible")
        #expect(plain.expires == nil)
        #expect(plain.sameSite == nil)
    }

    @Test func wrongKeySkipsWhatItCantDecrypt() throws {
        let database = try Self.chromeDatabase(version: 24, password: "hunter2")
        defer { try? FileManager.default.removeItem(at: database) }
        let cookies = try ChromiumCookies.read(database, password: "wrong")
        #expect(!cookies.contains { $0.name == "user_session" && $0.value == "s3cret" })
    }
}
