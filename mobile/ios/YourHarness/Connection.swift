// The WebSocket to the Mac. Frames are decoded (and rows prepared) off the main thread, then
// handed over in batches: everything that arrived since the last turn of the main run loop
// at once, so a burst of changes costs one update.

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
    private var task: URLSessionWebSocketTask?
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
        task?.cancel(with: .goingAway, reason: nil)
        generation += 1
        open()
    }

    func send(_ message: FromApp) {
        task?.send(.string(message.json)) { _ in }
    }

    private func open() {
        let generation = generation
        let task = URLSession.shared.webSocketTask(with: url)
        task.maximumMessageSize = 64 << 20
        self.task = task
        onState(.connecting)
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
                    await self?.received(message, generation)
                }
            } catch {
                await self?.failed(error, generation)
            }
        }
    }

    private func received(_ message: ToApp, _ generation: Int) {
        guard generation == self.generation else { return }
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

    private func failed(_ error: Error, _ generation: Int) {
        guard generation == self.generation else { return }
        task = nil
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
