import Foundation

/// `0.0.2`, `v1.4`, `2.0.0-beta.1`: compared number by number, a pre-release before its release.
public struct SemanticVersion: Sendable, Hashable, Comparable, CustomStringConvertible {
    public var numbers: [Int]
    public var prerelease: String?

    public init?(_ text: String) {
        var text = text.trimmingCharacters(in: .whitespaces)
        if text.hasPrefix("v") || text.hasPrefix("V") { text.removeFirst() }
        let parts = text.split(separator: "-", maxSplits: 1)
        guard let core = parts.first else { return nil }
        let numbers = core.split(separator: ".", omittingEmptySubsequences: false).map { Int($0) }
        guard !numbers.isEmpty, numbers.allSatisfy({ $0 != nil }) else { return nil }
        self.numbers = numbers.compactMap { $0 }
        prerelease = parts.count > 1 ? String(parts[1]) : nil
    }

    public var description: String {
        numbers.map(String.init).joined(separator: ".") + (prerelease.map { "-\($0)" } ?? "")
    }

    public static func < (a: Self, b: Self) -> Bool {
        for index in 0..<max(a.numbers.count, b.numbers.count) {
            let x = index < a.numbers.count ? a.numbers[index] : 0
            let y = index < b.numbers.count ? b.numbers[index] : 0
            if x != y { return x < y }
        }
        switch (a.prerelease, b.prerelease) {
        case let (x?, y?): return x.compare(y, options: .numeric) == .orderedAscending
        case (_?, nil): return true
        default: return false
        }
    }

    public static func == (a: Self, b: Self) -> Bool { !(a < b) && !(b < a) }

    public func hash(into hasher: inout Hasher) {
        var numbers = numbers
        while numbers.last == 0 { numbers.removeLast() }
        hasher.combine(numbers)
        hasher.combine(prerelease)
    }
}

/// A published release of the app: its disk image, the image's checksum, and its notes.
public struct AppRelease: Sendable, Equatable {
    public var version: SemanticVersion
    public var title: String
    /// The changelog section, Markdown.
    public var notes: String
    public var page: URL?
    public var publishedAt: Date?
    public var diskImage: URL
    public var diskImageSize: Int?
    /// A `shasum -a 256` line for the disk image.
    public var checksum: URL?

    public init(version: SemanticVersion, title: String, notes: String, page: URL?, publishedAt: Date?,
                diskImage: URL, diskImageSize: Int?, checksum: URL?) {
        self.version = version
        self.title = title
        self.notes = notes
        self.page = page
        self.publishedAt = publishedAt
        self.diskImage = diskImage
        self.diskImageSize = diskImageSize
        self.checksum = checksum
    }

    public enum ParseError: Error, Equatable, CustomStringConvertible {
        case notARelease
        case badVersion(String)
        case noDiskImage(String)

        public var description: String {
            switch self {
            case .notARelease: "The release feed didn't describe a release."
            case .badVersion(let tag): "The latest release, \(tag), doesn't have a version number."
            case .noDiskImage(let tag): "The latest release, \(tag), has no disk image to download."
            }
        }
    }

    /// GitHub's `GET /repos/{owner}/{repo}/releases/latest`. Drafts never show up there,
    /// and pre-releases are skipped.
    public static func parse(gitHubLatest data: Data) throws -> AppRelease {
        struct Asset: Decodable {
            var name: String
            var browser_download_url: URL
            var size: Int?
        }
        struct Release: Decodable {
            var tag_name: String
            var name: String?
            var body: String?
            var html_url: URL?
            var published_at: String?
            var prerelease: Bool?
            var assets: [Asset]
        }
        guard let release = try? JSONDecoder().decode(Release.self, from: data) else { throw ParseError.notARelease }
        guard let version = SemanticVersion(release.tag_name) else { throw ParseError.badVersion(release.tag_name) }
        guard let image = release.assets.first(where: { $0.name.lowercased().hasSuffix(".dmg") }) else {
            throw ParseError.noDiskImage(release.tag_name)
        }
        let checksum = release.assets.first { $0.name == image.name + ".sha256" }
        return AppRelease(
            version: version,
            title: release.name?.isEmpty == false ? release.name! : "YourHarness \(version)",
            notes: release.body ?? "",
            page: release.html_url,
            publishedAt: release.published_at.flatMap { try? Date($0, strategy: .iso8601) },
            diskImage: image.browser_download_url,
            diskImageSize: image.size,
            checksum: checksum?.browser_download_url
        )
    }

    /// The hex digest from a `shasum` line (`<digest>  <file name>`).
    public static func digest(inChecksumFile text: String) -> String? {
        guard let word = text.split(whereSeparator: \.isWhitespace).first else { return nil }
        let digest = word.lowercased()
        return digest.count == 64 && digest.allSatisfy(\.isHexDigit) ? digest : nil
    }

    /// The release notes without the install instructions the release page adds below them.
    public var whatsNew: String {
        guard let range = notes.range(of: #"\n#{2,3} Install"#, options: [.regularExpression, .caseInsensitive]) else { return notes }
        return String(notes[..<range.lowerBound]).trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
