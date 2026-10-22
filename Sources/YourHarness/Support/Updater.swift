import AppKit
import CryptoKit
import Foundation
import HarnessCore

/// Keeps the app current from GitHub Releases: looks for a newer version at launch and every
/// few hours, downloads its disk image when you ask, checks it, and puts the new app next to
/// this one. Restart quits, swaps the two, and opens the new one.
///
/// Development: `YH_UPDATE_FEED=<release.json>` reads the latest release from a file shaped
/// like GitHub's `releases/latest` (its disk image can be a `file://` URL), for testing the
/// whole flow on a Dev copy. Dev copies don't look on GitHub without it: a release would
/// replace them with the main app.
@Observable @MainActor
final class Updater {
    enum Phase: Equatable {
        case idle
        case checking
        case upToDate
        case available(AppRelease)
        case downloading(AppRelease, progress: Double)
        case preparing(AppRelease)
        /// The new app is in place next to this one; Restart swaps them.
        case ready(SemanticVersion)
        case failed(String, AppRelease?)
    }

    private(set) var phase = Phase.idle
    private(set) var lastChecked: Date?
    /// The newest release seen, kept for its notes while it downloads and after.
    private(set) var latest: AppRelease?
    /// Set by a check you asked for, so "up to date" is said once and then goes away.
    private(set) var announcesUpToDate = false

    /// This copy's version; nil for a bare executable (`swift run`).
    let current: SemanticVersion? = (Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String).flatMap(SemanticVersion.init)
    let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String

    private static let gitHubLatest = URL(string: "https://api.github.com/repos/mdovhopo/yourharness/releases/latest")!
    private static let checkInterval: Duration = .seconds(6 * 60 * 60)
    private let feed = ProcessInfo.processInfo.environment["YH_UPDATE_FEED"].flatMap { $0.isEmpty ? nil : $0 }
    private let bundleURL = Bundle.main.bundleURL
    private var schedule: Task<Void, Never>?
    private var work: Task<Void, Never>?
    private var download: URLSessionDownloadTask?

    /// "0.0.2", or what this build is when it isn't a release.
    var versionLabel: String {
        guard let current else { return "Development build" }
        return isDevCopy ? "\(current) Dev" : current.description
    }

    private var isDevCopy: Bool { Bundle.main.bundleIdentifier?.hasSuffix(".dev") ?? true }

    /// Whether this copy looks for updates at all.
    var isEnabled: Bool { current != nil && bundleURL.pathExtension == "app" && (feed != nil || !isDevCopy) }

    var whyDisabled: String {
        if current == nil || bundleURL.pathExtension != "app" { return "Development builds don't update." }
        return "Dev copies don't update from GitHub."
    }

    /// The new app, waiting next to this one: `/Applications/.YourHarness.app.update`.
    private var stagedURL: URL {
        bundleURL.deletingLastPathComponent().appendingPathComponent(".\(bundleURL.lastPathComponent).update", isDirectory: true)
    }

    func start() {
        guard isEnabled, schedule == nil else { return }
        restoreStaged()
        schedule = Task { [weak self] in
            try? await Task.sleep(for: .seconds(3))
            while !Task.isCancelled {
                await self?.check()
                try? await Task.sleep(for: Self.checkInterval)
            }
        }
    }

    /// Looks now. Asked for (the menu, a button), it also says when there's nothing new.
    func checkNow() {
        guard isEnabled else { return }
        announcesUpToDate = true
        Task { await check() }
    }

    /// Clears a failed check you asked for.
    func dismissFailure() {
        if case .failed(_, nil) = phase { phase = latest.flatMap { release in current.map { release.version > $0 } == true ? .available(release) : nil } ?? .idle }
    }

    private func check() async {
        switch phase {
        case .checking, .downloading, .preparing, .ready: return
        default: break
        }
        let previous = phase
        phase = .checking
        do {
            let release = try await fetchLatest()
            lastChecked = .now
            latest = release
            if let current, release.version > current {
                phase = .available(release)
            } else {
                phase = .upToDate
                if announcesUpToDate {
                    try? await Task.sleep(for: .seconds(4))
                    announcesUpToDate = false
                }
            }
        } catch {
            lastChecked = .now
            // A background check that fails keeps what it knew; one you asked for says why.
            if announcesUpToDate {
                phase = .failed(Self.describe(error), nil)
                announcesUpToDate = false
            } else {
                phase = previous
            }
        }
    }

    private func fetchLatest() async throws -> AppRelease {
        if let feed {
            let url = feed.contains("://") ? URL(string: feed)! : URL(fileURLWithPath: feed)
            return try AppRelease.parse(gitHubLatest: try Data(contentsOf: url))
        }
        var request = URLRequest(url: Self.gitHubLatest, timeoutInterval: 20)
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        request.setValue("YourHarness/\(current?.description ?? "dev")", forHTTPHeaderField: "User-Agent")
        let (data, response) = try await URLSession.shared.data(for: request)
        if let http = response as? HTTPURLResponse, http.statusCode != 200 {
            throw UpdateError(http.statusCode == 403 || http.statusCode == 429
                ? "GitHub is limiting requests right now. Try again in a while."
                : "GitHub answered \(http.statusCode).")
        }
        return try AppRelease.parse(gitHubLatest: data)
    }

    // MARK: - Download

    func downloadUpdate() {
        let release: AppRelease
        switch phase {
        case .available(let r): release = r
        case .failed(_, let r?): release = r
        default: return
        }
        guard FileManager.default.isWritableFile(atPath: bundleURL.deletingLastPathComponent().path) else {
            phase = .failed("YourHarness can't write to \(bundleURL.deletingLastPathComponent().path). Download \(release.version) from GitHub instead.", release)
            return
        }
        phase = .downloading(release, progress: 0)
        work = Task {
            do {
                let folder = FileManager.default.temporaryDirectory.appendingPathComponent("YourHarness-update-\(UUID().uuidString)", isDirectory: true)
                try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
                defer { try? FileManager.default.removeItem(at: folder) }
                let image = folder.appendingPathComponent("YourHarness.dmg")
                try await fetch(release.diskImage, to: image) { fraction in
                    guard case .downloading = self.phase else { return }
                    self.phase = .downloading(release, progress: fraction)
                }
                try Task.checkCancellation()
                phase = .preparing(release)
                try await verifyChecksum(of: image, for: release)
                try await stage(image, release: release, scratch: folder)
                phase = .ready(release.version)
            } catch is CancellationError {
                phase = .available(release)
            } catch let error as URLError where error.code == .cancelled {
                phase = .available(release)
            } catch {
                try? FileManager.default.removeItem(at: stagedURL)
                phase = .failed(Self.describe(error), release)
            }
            work = nil
        }
    }

    func cancelDownload() {
        download?.cancel()
        work?.cancel()
    }

    private func fetch(_ url: URL, to destination: URL, progress: @escaping @MainActor (Double) -> Void) async throws {
        let finished = Finished()
        let task = URLSession.shared.downloadTask(with: url) { location, response, error in
            if let error { return finished.resume(.failure(error)) }
            if let http = response as? HTTPURLResponse, http.statusCode != 200 {
                return finished.resume(.failure(UpdateError("The download failed: the server answered \(http.statusCode).")))
            }
            guard let location else { return finished.resume(.failure(UpdateError("The download failed."))) }
            // The file is gone once this returns, so it moves now.
            do {
                try FileManager.default.moveItem(at: location, to: destination)
                finished.resume(.success(()))
            } catch {
                finished.resume(.failure(error))
            }
        }
        download = task
        defer { download = nil }
        task.resume()
        let poll = Task { @MainActor in
            while !Task.isCancelled {
                progress(task.progress.fractionCompleted)
                try? await Task.sleep(for: .milliseconds(100))
            }
        }
        defer { poll.cancel() }
        try await withTaskCancellationHandler {
            try await finished.value()
        } onCancel: {
            task.cancel()
        }
        progress(1)
    }

    private func verifyChecksum(of image: URL, for release: AppRelease) async throws {
        guard let checksum = release.checksum else { return }
        let (data, _) = try await URLSession.shared.data(from: checksum)
        guard let expected = AppRelease.digest(inChecksumFile: String(decoding: data, as: UTF8.self)) else {
            throw UpdateError("The release's checksum file couldn't be read.")
        }
        let actual = try await Task.detached {
            SHA256.hash(data: try Data(contentsOf: image, options: .mappedIfSafe)).map { String(format: "%02x", $0) }.joined()
        }.value
        guard actual == expected else { throw UpdateError("The download doesn't match the release's checksum, so it wasn't installed.") }
    }

    /// Opens the disk image, checks the app in it is a newer YourHarness, and copies it next to this one.
    private func stage(_ image: URL, release: AppRelease, scratch: URL) async throws {
        let mount = scratch.appendingPathComponent("mount", isDirectory: true)
        try FileManager.default.createDirectory(at: mount, withIntermediateDirectories: true)
        let attach = await CommandRunner.run(URL(fileURLWithPath: "/usr/bin/hdiutil"),
                                             arguments: ["attach", "-nobrowse", "-readonly", "-noautoopen", "-noverify", "-mountpoint", mount.path, image.path],
                                             timeout: 60)
        guard attach.succeeded else { throw UpdateError("The disk image wouldn't open: \(attach.stderr.trimmingCharacters(in: .whitespacesAndNewlines))") }
        defer {
            Task.detached {
                _ = await CommandRunner.run(URL(fileURLWithPath: "/usr/bin/hdiutil"), arguments: ["detach", mount.path, "-force"], timeout: 30)
            }
        }
        let apps = (try? FileManager.default.contentsOfDirectory(at: mount, includingPropertiesForKeys: nil)) ?? []
        guard let app = apps.first(where: { $0.pathExtension == "app" && Self.info(of: $0)?.identifier == Bundle.main.bundleIdentifier }) else {
            throw UpdateError("The disk image has no YourHarness in it.")
        }
        guard let version = Self.info(of: app)?.version, version == release.version else {
            throw UpdateError("The app in the disk image isn't version \(release.version).")
        }
        try? FileManager.default.removeItem(at: stagedURL)
        let copy = await CommandRunner.run(URL(fileURLWithPath: "/usr/bin/ditto"), arguments: [app.path, stagedURL.path], timeout: 120)
        guard copy.succeeded else { throw UpdateError("The new version couldn't be copied: \(copy.stderr.trimmingCharacters(in: .whitespacesAndNewlines))") }
        let verify = await CommandRunner.run(URL(fileURLWithPath: "/usr/bin/codesign"), arguments: ["--verify", "--deep", "--strict", stagedURL.path], timeout: 60)
        guard verify.succeeded else { throw UpdateError("The new version's signature doesn't check out, so it wasn't installed.") }
    }

    /// A new version downloaded before the app last quit is still ready.
    private func restoreStaged() {
        guard FileManager.default.fileExists(atPath: stagedURL.path) else { return }
        if let info = Self.info(of: stagedURL), info.identifier == Bundle.main.bundleIdentifier,
           let current, info.version > current {
            phase = .ready(info.version)
        } else {
            try? FileManager.default.removeItem(at: stagedURL)
        }
    }

    private static func info(of app: URL) -> (identifier: String, version: SemanticVersion)? {
        guard let data = try? Data(contentsOf: app.appendingPathComponent("Contents/Info.plist")),
              let plist = try? PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any],
              let identifier = plist["CFBundleIdentifier"] as? String,
              let version = (plist["CFBundleShortVersionString"] as? String).flatMap(SemanticVersion.init)
        else { return nil }
        return (identifier, version)
    }

    // MARK: - Restart

    /// Quits; once this process is gone, a small script swaps in the new app and opens it.
    /// The old copy comes back if the swap fails.
    func restart() {
        guard case .ready = phase else { return }
        let app = bundleURL.path
        let script = """
        pid=$1 app=$2 staged=$3 old=$4; shift 4
        i=0
        while kill -0 "$pid" 2>/dev/null; do
          sleep 0.1; i=$((i+1)); [ $i -gt 600 ] && exit 1
        done
        if mv "$app" "$old"; then
          if mv "$staged" "$app"; then rm -rf "$old"; else mv "$old" "$app"; fi
        fi
        xattr -dr com.apple.quarantine "$app" 2>/dev/null
        open "$@" "$app"
        """
        // A development run comes back the way it was started: same scratch state, in the background.
        let environment = ProcessInfo.processInfo.environment
        var open: [String] = []
        if environment["YH_DATA_DIR"] != nil { open.append("-g") }
        for (key, value) in environment.sorted(by: { $0.key < $1.key }) where key.hasPrefix("YH_") {
            open += ["--env", "\(key)=\(value)"]
        }
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = ["-c", script, "sh", String(getpid()), app, stagedURL.path,
                             bundleURL.deletingLastPathComponent().appendingPathComponent(".\(bundleURL.lastPathComponent).old").path] + open
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
        } catch {
            phase = .failed("Couldn't restart: \(error.localizedDescription)", nil)
            return
        }
        NSApp.terminate(nil)
    }

    private static func describe(_ error: Error) -> String {
        if let error = error as? UpdateError { return error.message }
        if let error = error as? AppRelease.ParseError { return error.description }
        if let error = error as? URLError {
            switch error.code {
            case .notConnectedToInternet, .networkConnectionLost: return "You're offline."
            case .timedOut: return "GitHub didn't answer in time."
            default: return error.localizedDescription
            }
        }
        return error.localizedDescription
    }
}

private struct UpdateError: Error {
    let message: String
    init(_ message: String) { self.message = message }
}

/// The download's completion handler, awaited.
private final class Finished: @unchecked Sendable {
    private let lock = NSLock()
    private var result: Result<Void, Error>?
    private var continuation: CheckedContinuation<Void, Error>?

    func resume(_ result: Result<Void, Error>) {
        lock.lock()
        defer { lock.unlock() }
        guard self.result == nil else { return }
        self.result = result
        continuation?.resume(with: result)
        continuation = nil
    }

    func value() async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            lock.lock()
            defer { lock.unlock() }
            if let result { continuation.resume(with: result) } else { self.continuation = continuation }
        }
    }
}
