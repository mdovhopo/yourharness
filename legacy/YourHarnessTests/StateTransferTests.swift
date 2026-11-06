import Foundation
import HarnessCore
import Testing
@testable import YourHarness

/// What Export State takes out of the app and Import State puts back.
struct StateTransferTests {
    @Test func aBrowserCookieComesBackAsItWas() throws {
        // Within the 400 days a cookie may last; whole seconds, as cookies keep them.
        let expires = Date(timeIntervalSince1970: (Date.now.timeIntervalSince1970 + 86_400).rounded())
        let cookie = try #require(HTTPCookie(properties: [
            .domain: ".example.com", .path: "/app", .name: "session", .value: "abc", .expires: expires,
            .secure: "TRUE", HTTPCookiePropertyKey("HttpOnly"): "TRUE", .sameSitePolicy: HTTPCookieStringPolicy.sameSiteStrict,
        ]))
        let exported = ImportedCookie(cookie)
        #expect(exported == ImportedCookie(domain: ".example.com", name: "session", value: "abc", path: "/app", expires: expires,
                                           isSecure: true, isHTTPOnly: true, sameSite: "strict"))

        let restored = try #require(CookieImport.httpCookie(exported))
        #expect(restored.domain == cookie.domain && restored.path == cookie.path && restored.value == cookie.value)
        #expect(restored.expiresDate == expires && restored.isSecure && restored.isHTTPOnly)
        #expect(restored.sameSitePolicy == .sameSiteStrict)
    }

    @Test func saysWhatAStateHolds() {
        var contents = StateArchive.Contents()
        contents.projects = ["Home", "Notes"]
        contents.threads = 1
        contents.memories = 12
        contents.bytes = 4_100_000
        let size = ByteCountFormatter.string(fromByteCount: 4_100_000, countStyle: .file)
        #expect(StateTransfer.summary(contents) == "2 projects, 1 thread, 12 memories, 0 images · \(size)")
    }
}
