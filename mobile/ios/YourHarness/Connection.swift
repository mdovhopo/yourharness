// The WebSocket to the Mac. Frames are decoded (and rows prepared) off the main thread, then
// handed over in batches: everything that arrived since the last turn of the main run loop
// at once, so a burst of changes costs one update.
//
// A Mac can be reached at several addresses (its Wi‑Fi one, its Tailscale one): each connect
// tries them all at once and keeps the first that answers, so the phone finds its Mac at home
// and away alike.

import Foundation

@MainActor
final class Connection {
    enum State: Equatable {
        case connecting
        case connected
        case waiting(reason: String, retryIn: Int)
    }

    var onState: (State) -> Void = { _ in }
    var onMessages: ([ToApp]) -> Void = { _ in }

    private(set) var url: URL
    /// Other hosts the Mac answers at, tried alongside `url`'s.
    var hosts: [String] = []
    /// The host the open connection went to.
    private(set) var host: String?
    private var task: URLSessionWebSocketTask?
    /// Tries still going, before one answers; the rest are dropped when one does.
    private var trying: [URLSessionWebSocketTask] = []
    private var lastError: Error?
    /// Bumped on every new socket, so a closed one's late callbacks are dropped.
    private var generation = 0
    private var failures = 0
    private var pending: [ToApp] = []
    private var flushScheduled = false

    init(url: URL) {
        self.url = url
    }

    func connect(to url: URL? = nil) {
        if let url { self.url = url }
        dropAll()
        generation += 1
        open()
    }

    /// `url` at each host it may be reached at, its own first.
    var candidates: [URL] {
        var seen: Set<String> = []
        return ([url.host() ?? ""] + hosts).compactMap { h in
            guard !h.isEmpty, seen.insert(h).inserted else { return nil }
            var c = URLComponents(url: url, resolvingAgainstBaseURL: false)
            c?.host = h
            return c?.url
        }
    }

    private func dropAll() {
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
        trying.forEach { $0.cancel(with: .goingAway, reason: nil) }
        trying = []
    }

    /// Where it connects next time (a pairing code swapped for the token it earned).
    func retarget(_ url: URL) {
        self.url = url
    }

    /// Closed for good (the Mac removed from the list): late callbacks and retries dropped.
    func close() {
        generation += 1
        dropAll()
    }

    func send(_ message: FromApp) {
        task?.send(.string(message.json)) { _ in }
    }

    private func open() {
        let generation = generation
        onState(.connecting)
        lastError = nil
        for url in candidates {
            let task = URLSession.shared.webSocketTask(with: url)
            task.maximumMessageSize = 64 << 20
            trying.append(task)
            task.resume()
            Task.detached { [weak self] in
                do {
                    while true {
                        let frame = try await task.receive()
                        let data: Data
                        switch frame {
                        case .string(let s): data = Data(s.utf8)
                        case .data(let d): data = d
                        @unknown default: continue
                        }
                        let message = ToApp.decode(data)
                        await self?.received(message, task, generation)
                    }
                } catch {
                    await self?.failed(error, task, generation)
                }
            }
        }
    }

    private func received(_ message: ToApp, _ from: URLSessionWebSocketTask, _ generation: Int) {
        guard generation == self.generation else { return }
        // The first to answer wins; the others are let go.
        if task == nil, let i = trying.firstIndex(where: { $0 === from }) {
            trying.remove(at: i)
            trying.forEach { $0.cancel(with: .goingAway, reason: nil) }
            trying = []
            task = from
            host = from.originalRequest?.url?.host()
        }
        guard task === from else { return }
        if case .hello = message {
            failures = 0
            onState(.connected)
        }
        pending.append(message)
        guard !flushScheduled else { return }
        flushScheduled = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            flushScheduled = false
            let batch = pending
            pending = []
            onMessages(batch)
        }
    }

    private func failed(_ error: Error, _ from: URLSessionWebSocketTask, _ generation: Int) {
        guard generation == self.generation else { return }
        Perf.shared.log("CONNECT \(from.originalRequest?.url?.host() ?? "?") failed: \((error as NSError).domain) \((error as NSError).code) \(error.localizedDescription)")
        if let i = trying.firstIndex(where: { $0 === from }) {
            // One of several tries: wait for the rest. A refusal says more than a host not there.
            trying.remove(at: i)
            if lastError == nil || Connection.describe(error).contains("refused") { lastError = error }
            guard trying.isEmpty else { return }
        } else if task !== from {
            return
        }
        let error = task === from ? error : (lastError ?? error)
        task = nil
        host = nil
        failures += 1
        let delay = min(8, 1 << min(failures - 1, 3))
        onState(.waiting(reason: Connection.describe(error), retryIn: delay))
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(delay))
            guard let self, generation == self.generation else { return }
            open()
        }
    }

    private static func describe(_ error: Error) -> String {
        let e = error as NSError
        if e.domain == NSURLErrorDomain {
            switch e.code {
            case NSURLErrorCannotConnectToHost, NSURLErrorNetworkConnectionLost: return "Can't reach the Mac"
            case NSURLErrorTimedOut: return "The Mac didn't answer"
            case NSURLErrorBadServerResponse: return "The Mac refused the connection"
            default: break
            }
        }
        if e.domain == NSPOSIXErrorDomain, e.code == 57 { return "Disconnected" }
        return e.localizedDescription
    }
}
