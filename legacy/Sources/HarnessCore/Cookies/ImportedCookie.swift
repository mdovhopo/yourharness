import Foundation

/// A cookie read from another browser's store, to bring a login into the app's browser.
public struct ImportedCookie: Sendable, Equatable, Codable {
    public var domain: String
    public var name: String
    public var value: String
    public var path: String
    /// Nil: a session cookie.
    public var expires: Date?
    public var isSecure: Bool
    public var isHTTPOnly: Bool
    /// "lax", "strict" or "none"; nil when the browser didn't say.
    public var sameSite: String?

    public init(domain: String, name: String, value: String, path: String = "/", expires: Date? = nil,
                isSecure: Bool = false, isHTTPOnly: Bool = false, sameSite: String? = nil) {
        self.domain = domain
        self.name = name
        self.value = value
        self.path = path
        self.expires = expires
        self.isSecure = isSecure
        self.isHTTPOnly = isHTTPOnly
        self.sameSite = sameSite
    }

    /// The site it belongs to, for picking what to import: `.github.com` → `github.com`.
    public var site: String {
        domain.hasPrefix(".") ? String(domain.dropFirst()) : domain
    }

    public var isExpired: Bool {
        expires.map { $0 < .now } ?? false
    }
}

public enum CookieImportError: Error, Equatable, CustomStringConvertible {
    case unreadable(String)
    case malformed(String)
    case noKey(String)

    public var description: String {
        switch self {
        case .unreadable(let message), .malformed(let message), .noKey(let message): message
        }
    }
}
