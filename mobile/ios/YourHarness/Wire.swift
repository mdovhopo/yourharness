// The messages the Mac and the phone trade (docs/mobile.md, "The wire"), mirroring
// crates/yh-proto (protocol 2: live state and actions; yh-devserver speaks 1, read-only). JSON text frames tagged with `type`, keys in snake_case.
// Unknown messages and fields are ignored.

import Foundation

struct Project: Decodable, Sendable, Hashable, Identifiable {
    let id: String
    let name: String
}

struct ThreadInfo: Decodable, Sendable, Hashable, Identifiable {
    let id: String
    let projectId: String
    let title: String
    /// Milliseconds since 1970.
    let createdAt: Double
    let updatedAt: Double
    let pinned: Bool
    let settled: Bool
    let model: String?
    /// Working now, or its rows changed in the last few seconds.
    let active: Bool
    /// Protocol 2: a turn is under way; something waits on you.
    let running: Bool?
    let waiting: Bool?

    var isRunning: Bool { running ?? active }
    var isWaiting: Bool { waiting ?? false }
}

/// A request waiting on you: a tool to allow, Claude's question (`AskUserQuestion`) or a plan
/// (`ExitPlanMode`).
struct Request: Decodable, Sendable, Hashable, Identifiable {
    let id: String
    let tool: String
    let description: String
    /// JSON, trimmed to 4000 characters on the wire.
    let input: String
}

/// An MCP server's question mid-call: answered on the Mac.
struct MCPQuestion: Decodable, Sendable, Hashable, Identifiable {
    let id: String
    let server: String
    let message: String
}

struct Queued: Decodable, Sendable, Hashable, Identifiable {
    let id: String
    let text: String
}

/// What the open thread's run is doing, beyond its rows.
struct Live: Decodable, Sendable, Equatable {
    let running: Bool
    let activity: String?
    let requests: [Request]
    let questions: [MCPQuestion]
    let queue: [Queued]
    let contextTokens: Int?
    let contextWindow: Int?

    enum CodingKeys: String, CodingKey { case running, activity, requests, questions, queue, contextTokens, contextWindow }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        running = try c.decodeIfPresent(Bool.self, forKey: .running) ?? false
        activity = try c.decodeIfPresent(String.self, forKey: .activity)
        requests = try c.decodeIfPresent([Request].self, forKey: .requests) ?? []
        questions = try c.decodeIfPresent([MCPQuestion].self, forKey: .questions) ?? []
        queue = try c.decodeIfPresent([Queued].self, forKey: .queue) ?? []
        contextTokens = try c.decodeIfPresent(Int.self, forKey: .contextTokens)
        contextWindow = try c.decodeIfPresent(Int.self, forKey: .contextWindow)
    }
}

struct Row: Decodable, Sendable {
    let seq: Int
    /// Of two copies of a row, the higher `rev` is newer, whichever message brought it.
    let rev: Int
    let id: String
    let kind: String
    let text: String
    let toolName: String?
    let toolInput: String?
    let toolResult: String?
    let toolResultLength: Int?
    let toolIsError: Bool?
    let noticeStyle: String?
    /// Seconds since 1970.
    let date: Double?
    let turnEndedAt: Double?
    let attachments: [String]?
    let changedFiles: [String]?
}

enum ToApp: Sendable {
    case hello(protocol: Int, server: String, data: String)
    case threads(projects: [Project], threads: [ThreadInfo])
    case threadUpserted(ThreadInfo)
    case threadRemoved(String)
    case snapshot(thread: String, firstSeq: Int, count: Int, rows: [RowModel])
    case rowsChanged(thread: String, count: Int, rows: [RowModel])
    case pageRows(thread: String, rows: [RowModel])
    case live(thread: String, Live)
    /// An action carrying this id was taken, or why not.
    case done(id: String, error: String?)
    case error(String)
    case unknown

    private struct Tag: Decodable { let type: String }
    private struct Hello: Decodable { let `protocol`: Int?; let server: String; let data: String }
    private struct LiveMessage: Decodable { let thread: String; let live: Live }
    private struct Done: Decodable { let id: String; let error: String? }
    private struct Threads: Decodable { let projects: [Project]; let threads: [ThreadInfo] }
    private struct Upserted: Decodable { let thread: ThreadInfo }
    private struct Removed: Decodable { let id: String }
    private struct Snapshot: Decodable { let thread: String; let firstSeq: Int; let count: Int; let rows: [Row] }
    private struct Changed: Decodable { let thread: String; let count: Int; let rows: [Row] }
    private struct Page: Decodable { let thread: String; let rows: [Row] }
    private struct Failure: Decodable { let message: String }

    /// Decodes a frame and prepares its rows for drawing (markdown parsed): call it off the main thread.
    static func decode(_ data: Data) -> ToApp {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        do {
            switch try d.decode(Tag.self, from: data).type {
            case "hello":
                let m = try d.decode(Hello.self, from: data)
                return .hello(protocol: m.protocol ?? 1, server: m.server, data: m.data)
            case "threads":
                let m = try d.decode(Threads.self, from: data)
                return .threads(projects: m.projects, threads: m.threads)
            case "thread_upserted": return .threadUpserted(try d.decode(Upserted.self, from: data).thread)
            case "thread_removed": return .threadRemoved(try d.decode(Removed.self, from: data).id)
            case "snapshot":
                let m = try d.decode(Snapshot.self, from: data)
                return .snapshot(thread: m.thread, firstSeq: m.firstSeq, count: m.count, rows: m.rows.map(RowModel.init))
            case "rows_changed":
                let m = try d.decode(Changed.self, from: data)
                return .rowsChanged(thread: m.thread, count: m.count, rows: m.rows.map(RowModel.init))
            case "page_rows":
                let m = try d.decode(Page.self, from: data)
                return .pageRows(thread: m.thread, rows: m.rows.map(RowModel.init))
            case "live":
                let m = try d.decode(LiveMessage.self, from: data)
                return .live(thread: m.thread, m.live)
            case "done":
                let m = try d.decode(Done.self, from: data)
                return .done(id: m.id, error: m.error)
            case "error": return .error(try d.decode(Failure.self, from: data).message)
            default: return .unknown
            }
        } catch {
            return .error("couldn't read a message: \(error)")
        }
    }
}

enum FromApp {
    case subscribeApp
    case subscribeThread(String, limit: Int)
    case unsubscribeThread
    case page(String, beforeSeq: Int, limit: Int)
    // Protocol 2's actions; each carries an id its `done` answers.
    case send(id: String, thread: String, text: String, now: Bool)
    case stop(id: String, thread: String)
    case respond(id: String, thread: String, requestID: String, allow: Bool)
    case withdraw(id: String, thread: String, message: String)
    case sendNow(id: String, thread: String, message: String)

    var actionID: String? {
        switch self {
        case .send(let id, _, _, _), .stop(let id, _), .respond(let id, _, _, _), .withdraw(let id, _, _), .sendNow(let id, _, _): id
        default: nil
        }
    }

    /// For logs.
    var name: String {
        switch self {
        case .send(_, _, let text, let now): now ? "send now \(text.prefix(40))" : "send \(text.prefix(40))"
        case .stop: "stop"
        case .respond(_, _, _, let allow): allow ? "allow" : "deny"
        case .withdraw: "withdraw"
        case .sendNow: "send queued now"
        default: "\(self)"
        }
    }

    var json: String {
        let object: [String: Any] = switch self {
        case .subscribeApp: ["type": "subscribe_app"]
        case .subscribeThread(let t, let limit): ["type": "subscribe_thread", "thread": t, "limit": limit]
        case .unsubscribeThread: ["type": "unsubscribe_thread"]
        case .page(let t, let before, let limit): ["type": "page", "thread": t, "before_seq": before, "limit": limit]
        case .send(let id, let t, let text, let now): ["type": "send", "id": id, "thread": t, "text": text, "now": now]
        case .stop(let id, let t): ["type": "stop", "id": id, "thread": t]
        case .respond(let id, let t, let request, let allow): ["type": "respond", "id": id, "thread": t, "request_id": request, "allow": allow]
        case .withdraw(let id, let t, let message): ["type": "withdraw", "id": id, "thread": t, "message": message]
        case .sendNow(let id, let t, let message): ["type": "send_now", "id": id, "thread": t, "message": message]
        }
        let data = try! JSONSerialization.data(withJSONObject: object)
        return String(decoding: data, as: UTF8.self)
    }
}
