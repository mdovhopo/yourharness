import CommonCrypto
import Foundation
import Security
import SQLite3

/// Chrome and the browsers built on it (Brave, Edge, Arc, Vivaldi): a SQLite `Cookies`
/// database per profile, values encrypted with a key kept in the login keychain.
public struct ChromiumBrowser: Sendable, Identifiable, Equatable {
    public var id: String { name }
    public let name: String
    /// Under `~/Library/Application Support`.
    public let folder: String
    /// The keychain item holding the cookie key: "Chrome Safe Storage", account "Chrome".
    public let keychainService: String
    public let keychainAccount: String

    public static let all = [
        ChromiumBrowser(name: "Chrome", folder: "Google/Chrome", keychainService: "Chrome Safe Storage", keychainAccount: "Chrome"),
        ChromiumBrowser(name: "Arc", folder: "Arc/User Data", keychainService: "Arc Safe Storage", keychainAccount: "Arc"),
        ChromiumBrowser(name: "Brave", folder: "BraveSoftware/Brave-Browser", keychainService: "Brave Safe Storage", keychainAccount: "Brave"),
        ChromiumBrowser(name: "Edge", folder: "Microsoft Edge", keychainService: "Microsoft Edge Safe Storage", keychainAccount: "Microsoft Edge"),
        ChromiumBrowser(name: "Vivaldi", folder: "Vivaldi", keychainService: "Vivaldi Safe Storage", keychainAccount: "Vivaldi"),
        ChromiumBrowser(name: "Chromium", folder: "Chromium", keychainService: "Chromium Safe Storage", keychainAccount: "Chromium"),
    ]

    public struct Profile: Sendable, Identifiable, Equatable {
        public var id: String { cookies.path }
        /// As the browser shows it: "Person 1", "Work".
        public let name: String
        public let cookies: URL
    }

    public var root: URL {
        FileManager.default.homeDirectoryForCurrentUser.appending(path: "Library/Application Support").appending(path: folder)
    }

    /// Profiles with a cookie database, named as in the browser's profile menu.
    public func profiles() -> [Profile] {
        let fm = FileManager.default
        guard let folders = try? fm.contentsOfDirectory(atPath: root.path) else { return [] }
        let names = profileNames()
        return folders.sorted { $0 == "Default" || ($1 != "Default" && $0.localizedStandardCompare($1) == .orderedAscending) }
            .compactMap { folder -> Profile? in
                guard folder == "Default" || folder.hasPrefix("Profile ") else { return nil }
                let base = root.appending(path: folder)
                let cookies = [base.appending(path: "Cookies"), base.appending(path: "Network/Cookies")]
                    .first { fm.fileExists(atPath: $0.path) }
                return cookies.map { Profile(name: names[folder] ?? folder, cookies: $0) }
            }
    }

    private func profileNames() -> [String: String] {
        guard let data = try? Data(contentsOf: root.appending(path: "Local State")),
              let state = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let cache = (state["profile"] as? [String: Any])?["info_cache"] as? [String: [String: Any]]
        else { return [:] }
        return cache.compactMapValues { $0["name"] as? String }
    }

    /// The cookie key's password from the keychain. macOS asks you first whether the app
    /// may use it; refusing throws.
    public func keychainPassword() throws(CookieImportError) -> String {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: keychainService,
            kSecAttrAccount as String: keychainAccount,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        guard status == errSecSuccess, let data = result as? Data, let password = String(data: data, encoding: .utf8) else {
            throw .noKey(status == errSecUserCanceled || status == errSecAuthFailed
                ? "Access to \(name)'s cookie key was refused, so its cookies can't be read."
                : "\(name)'s cookie key isn't in your keychain (\(status)).")
        }
        return password
    }
}

public enum ChromiumCookies {
    /// Reads a profile's cookies. The database is copied first: the browser keeps it locked.
    public static func read(_ database: URL, password: String) throws(CookieImportError) -> [ImportedCookie] {
        let fm = FileManager.default
        let copy = fm.temporaryDirectory.appending(path: "yh-cookies-\(UUID().uuidString)")
        defer {
            for suffix in ["", "-wal", "-shm", "-journal"] { try? fm.removeItem(atPath: copy.path + suffix) }
        }
        do {
            try fm.copyItem(at: database, to: copy)
        } catch {
            throw .unreadable("Couldn't read the cookie database: \(error.localizedDescription)")
        }
        for suffix in ["-wal", "-journal"] where fm.fileExists(atPath: database.path + suffix) {
            try? fm.copyItem(atPath: database.path + suffix, toPath: copy.path + suffix)
        }

        var db: OpaquePointer?
        guard sqlite3_open_v2(copy.path, &db, SQLITE_OPEN_READONLY, nil) == SQLITE_OK, let db else {
            throw .unreadable("Couldn't open the cookie database.")
        }
        defer { sqlite3_close(db) }
        let version = Int(scalar(db, "SELECT value FROM meta WHERE key = 'version'") ?? "") ?? 0
        let key = derivedKey(password)

        var statement: OpaquePointer?
        let sql = "SELECT host_key, name, value, encrypted_value, path, expires_utc, is_secure, is_httponly, samesite FROM cookies"
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK, let statement else {
            throw .malformed("The cookie database isn't in a format YourHarness knows.")
        }
        defer { sqlite3_finalize(statement) }
        var cookies: [ImportedCookie] = []
        while sqlite3_step(statement) == SQLITE_ROW {
            let host = text(statement, 0)
            var value = text(statement, 2)
            if value.isEmpty, let encrypted = blob(statement, 3), !encrypted.isEmpty {
                // Since database version 24 the plaintext starts with a hash of the domain.
                guard var plain = decrypt(encrypted, key: key) else { continue }
                if version >= 24, plain.count >= 32 { plain = plain.dropFirst(32) }
                value = String(decoding: plain, as: UTF8.self)
            }
            let expiresUTC = sqlite3_column_int64(statement, 5)
            cookies.append(ImportedCookie(
                domain: host, name: text(statement, 1), value: value, path: text(statement, 4),
                expires: expiresUTC == 0 ? nil : chromeDate(expiresUTC),
                isSecure: sqlite3_column_int(statement, 6) != 0, isHTTPOnly: sqlite3_column_int(statement, 7) != 0,
                sameSite: ["-1": nil, "0": "none", "1": "lax", "2": "strict"][String(sqlite3_column_int(statement, 8))] ?? nil))
        }
        return cookies
    }

    /// Microseconds since 1601, Windows' epoch.
    static func chromeDate(_ microseconds: Int64) -> Date {
        Date(timeIntervalSince1970: Double(microseconds) / 1_000_000 - 11_644_473_600)
    }

    static func derivedKey(_ password: String) -> [UInt8] {
        var key = [UInt8](repeating: 0, count: kCCKeySizeAES128)
        let salt = Array("saltysalt".utf8)
        _ = password.withCString { pointer in
            CCKeyDerivationPBKDF(CCPBKDFAlgorithm(kCCPBKDF2), pointer, strlen(pointer), salt, salt.count,
                                 CCPseudoRandomAlgorithm(kCCPRFHmacAlgSHA1), 1003, &key, key.count)
        }
        return key
    }

    /// "v10" + AES-128-CBC with an IV of sixteen spaces.
    static func decrypt(_ data: Data, key: [UInt8]) -> Data? {
        guard data.count > 3, data.prefix(3) == Data("v10".utf8) else { return nil }
        return crypt(CCOperation(kCCDecrypt), Data(data.dropFirst(3)), key: key)
    }

    /// For tests: what Chrome writes.
    static func encrypt(_ plain: Data, key: [UInt8]) -> Data? {
        crypt(CCOperation(kCCEncrypt), plain, key: key).map { Data("v10".utf8) + $0 }
    }

    private static func crypt(_ operation: CCOperation, _ input: Data, key: [UInt8]) -> Data? {
        let iv = [UInt8](repeating: 0x20, count: kCCBlockSizeAES128)
        var output = [UInt8](repeating: 0, count: input.count + kCCBlockSizeAES128)
        var moved = 0
        let status = input.withUnsafeBytes { bytes in
            CCCrypt(operation, CCAlgorithm(kCCAlgorithmAES), CCOptions(kCCOptionPKCS7Padding), key, key.count, iv,
                    bytes.baseAddress, input.count, &output, output.count, &moved)
        }
        return status == kCCSuccess ? Data(output.prefix(moved)) : nil
    }

    private static func scalar(_ db: OpaquePointer, _ sql: String) -> String? {
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK, let statement else { return nil }
        defer { sqlite3_finalize(statement) }
        return sqlite3_step(statement) == SQLITE_ROW ? text(statement, 0) : nil
    }

    private static func text(_ statement: OpaquePointer, _ column: Int32) -> String {
        sqlite3_column_text(statement, column).map { String(cString: $0) } ?? ""
    }

    private static func blob(_ statement: OpaquePointer, _ column: Int32) -> Data? {
        guard let bytes = sqlite3_column_blob(statement, column) else { return nil }
        return Data(bytes: bytes, count: Int(sqlite3_column_bytes(statement, column)))
    }
}
