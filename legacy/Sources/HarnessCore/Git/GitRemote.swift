import Foundation

public enum Forge: String, Sendable, Codable, Hashable {
    case github, gitlab
}

/// Where a repo's `origin` lives. Credentials embedded in the URL are dropped on parse and
/// never stored or shown.
public struct GitRemote: Sendable, Codable, Hashable {
    public var host: String
    /// `owner/name`, or `group/subgroup/name` on GitLab.
    public var path: String
    public var forge: Forge?

    public var name: String { path.split(separator: "/").last.map(String.init) ?? path }
    public var owner: String { path.split(separator: "/").dropLast().joined(separator: "/") }
    public var webURL: URL? { URL(string: "https://\(host)/\(path)") }

    public init(host: String, path: String, forge: Forge?) {
        self.host = host
        self.path = path
        self.forge = forge
    }

    /// Accepts `git@host:owner/repo.git`, `ssh://git@host:22/owner/repo`, and
    /// `https://user:token@host/owner/repo.git`.
    public init?(url raw: String) {
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        var host: String
        var path: String
        if text.contains("://") {
            guard let components = URLComponents(string: text), let parsedHost = components.host else { return nil }
            host = parsedHost
            path = components.path
        } else if let colon = text.firstIndex(of: ":"), !text[..<colon].contains("/") {
            // scp-like syntax: [user@]host:path
            let authority = text[..<colon]
            host = String(authority.split(separator: "@").last ?? authority)
            path = String(text[text.index(after: colon)...])
        } else {
            return nil
        }
        path = path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        if path.hasSuffix(".git") { path.removeLast(4) }
        guard !host.isEmpty, path.contains("/") else { return nil }
        host = host.lowercased()

        self.host = host
        self.path = path
        if host.contains("github") {
            forge = .github
        } else if host.contains("gitlab") {
            forge = .gitlab
        } else {
            forge = nil
        }
    }
}
