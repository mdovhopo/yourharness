import AppKit
import HarnessCore
import WebKit

/// Bringing logins over from your other browsers into a project's browser: you pick the
/// browser and the sites, and only those cookies are copied. Nothing is read until you
/// pick a browser, and macOS asks first (Full Disk Access for Safari, the keychain for
/// the key Chrome encrypts its cookies with).
enum CookieImport {
    enum Source: Identifiable, Hashable {
        case safari
        case chromium(ChromiumBrowser, ChromiumBrowser.Profile)

        var id: String {
            switch self {
            case .safari: "safari"
            case .chromium(_, let profile): profile.id
            }
        }

        var label: String {
            switch self {
            case .safari: "Safari"
            case .chromium(let browser, let profile): browser.profiles().count > 1 ? "\(browser.name) — \(profile.name)" : browser.name
            }
        }

        static func == (a: Source, b: Source) -> Bool { a.id == b.id }
        func hash(into hasher: inout Hasher) { hasher.combine(id) }
    }

    /// Safari always (its file can't even be looked at without asking), and every
    /// Chromium profile that has cookies.
    static func sources() -> [Source] {
        [.safari] + ChromiumBrowser.all.flatMap { browser in browser.profiles().map { Source.chromium(browser, $0) } }
    }

    /// Off the main thread: the keychain prompt blocks until you answer it.
    static func read(_ source: Source) async throws -> [ImportedCookie] {
        try await Task.detached(priority: .userInitiated) {
            let cookies: [ImportedCookie]
            switch source {
            case .safari:
                cookies = try readSafari()
            case .chromium(let browser, let profile):
                cookies = try ChromiumCookies.read(profile.cookies, password: try browser.keychainPassword())
            }
            return cookies.filter { !$0.isExpired && !$0.name.isEmpty }
        }.value
    }

    private static func readSafari() throws -> [ImportedCookie] {
        var lastError: Error?
        for file in SafariCookies.files {
            do {
                return try SafariCookies.parse(try Data(contentsOf: file))
            } catch let error as CocoaError where error.code == .fileReadNoSuchFile {
                continue
            } catch let error as CocoaError where error.code == .fileReadNoPermission {
                lastError = SafariAccessError()
            } catch {
                lastError = error
            }
        }
        throw lastError ?? CookieImportError.unreadable("Safari has no cookies saved.")
    }

    struct SafariAccessError: Error, CustomStringConvertible {
        let description = "macOS keeps Safari's cookies private. To import them, turn on Full Disk Access for YourHarness in System Settings → Privacy & Security, then try again."
    }

    static func openFullDiskAccessSettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles") {
            NSWorkspace.shared.open(url)
        }
    }

    /// Into the project's cookie store; returns how many went in.
    @MainActor
    static func store(_ cookies: [ImportedCookie], in projectID: UUID) async -> Int {
        let store = BrowserProfiles.dataStore(for: projectID).httpCookieStore
        var count = 0
        for cookie in cookies {
            guard let http = httpCookie(cookie) else { continue }
            await store.setCookie(http)
            count += 1
        }
        return count
    }

    static func httpCookie(_ cookie: ImportedCookie) -> HTTPCookie? {
        var properties: [HTTPCookiePropertyKey: Any] = [
            .domain: cookie.domain, .path: cookie.path.isEmpty ? "/" : cookie.path, .name: cookie.name, .value: cookie.value,
        ]
        if let expires = cookie.expires { properties[.expires] = expires }
        if cookie.isSecure { properties[.secure] = "TRUE" }
        if cookie.isHTTPOnly { properties[HTTPCookiePropertyKey("HttpOnly")] = "TRUE" }
        switch cookie.sameSite {
        case "lax": properties[.sameSitePolicy] = HTTPCookieStringPolicy.sameSiteLax
        case "strict": properties[.sameSitePolicy] = HTTPCookieStringPolicy.sameSiteStrict
        default: break
        }
        return HTTPCookie(properties: properties)
    }

    /// Development: `YH_IMPORT_COOKIES=<Cookies.binarycookies>` imports a Safari-format file
    /// into the selected thread's project at launch, for testing without a real browser.
    @MainActor
    static func importIfRequested(app: AppModel) {
        guard let path = ProcessInfo.processInfo.environment["YH_IMPORT_COOKIES"], !path.isEmpty,
              let thread = app.selectedThread,
              let data = FileManager.default.contents(atPath: path),
              let cookies = try? SafariCookies.parse(data)
        else { return }
        Task {
            let count = await store(cookies, in: thread.projectID)
            FileHandle.standardError.write(Data("imported \(count) cookies\n".utf8))
        }
    }
}
