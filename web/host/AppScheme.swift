import Foundation
import UniformTypeIdentifiers
import WebKit

/// Serves the page (`yhw://app/…`) from the built UI, and data (`yhw://app/api/…`) answered
/// off the main thread: a transcript page can be megabytes, and a fetch streams it straight
/// into the page's JSON parser without going through the message bridge.
final class AppScheme: NSObject, WKURLSchemeHandler, @unchecked Sendable {
    static let scheme = "yhw"
    static let shared = AppScheme()

    /// `YHW_UI_DIR`, else the app's Resources/ui, else web/ui/dist next to the build.
    let root: URL = {
        if let dir = env["YHW_UI_DIR"] { return URL(fileURLWithPath: dir, isDirectory: true) }
        if let bundled = Bundle.main.resourceURL?.appendingPathComponent("ui"),
           FileManager.default.fileExists(atPath: bundled.appendingPathComponent("index.html").path) { return bundled }
        let exe = URL(fileURLWithPath: CommandLine.arguments[0]).resolvingSymlinksInPath()
        var dir = exe.deletingLastPathComponent()
        for _ in 0..<6 {
            let candidate = dir.appendingPathComponent("web/ui/dist")
            if FileManager.default.fileExists(atPath: candidate.path) { return candidate }
            dir = dir.deletingLastPathComponent()
        }
        return URL(fileURLWithPath: FileManager.default.currentDirectoryPath).appendingPathComponent("web/ui/dist")
    }()

    var api: ((String, [String: String]) -> Data?)?
    private let queue = DispatchQueue(label: "yhw.scheme", qos: .userInitiated, attributes: .concurrent)
    private let stopped = NSHashTable<AnyObject>.weakObjects()
    private let lock = NSLock()

    func webView(_ webView: WKWebView, start task: any WKURLSchemeTask) {
        guard let url = task.request.url else { return }
        let path = url.path
        let api = self.api, root = self.root
        let taskBox = TaskBox(task)
        queue.async { [self] in
            let data: Data?, mime: String
            if path.hasPrefix("/api/") {
                var query: [String: String] = [:]
                for item in URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? [] { query[item.name] = item.value ?? "" }
                data = api?(String(path.dropFirst(5)), query)
                mime = "application/json"
            } else {
                let file = root.appendingPathComponent(path == "/" ? "index.html" : String(path.dropFirst()))
                data = try? Data(contentsOf: file)
                mime = UTType(filenameExtension: file.pathExtension)?.preferredMIMEType ?? "application/octet-stream"
            }
            DispatchQueue.main.async { [self] in
                let task = taskBox.task
                lock.lock(); let gone = stopped.contains(task as AnyObject); lock.unlock()
                if gone { return }
                guard let data else {
                    task.didFailWithError(URLError(.fileDoesNotExist))
                    return
                }
                let response = HTTPURLResponse(url: url, statusCode: 200, httpVersion: "HTTP/1.1",
                                               headerFields: ["Content-Type": mime, "Content-Length": "\(data.count)", "Cache-Control": "no-store"])!
                task.didReceive(response)
                task.didReceive(data)
                task.didFinish()
            }
        }
    }

    func webView(_ webView: WKWebView, stop task: any WKURLSchemeTask) {
        lock.lock(); stopped.add(task as AnyObject); lock.unlock()
    }
}

private final class TaskBox: @unchecked Sendable {
    let task: any WKURLSchemeTask
    init(_ task: any WKURLSchemeTask) { self.task = task }
}
