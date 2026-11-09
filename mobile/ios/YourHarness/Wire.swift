// The messages the Mac and the phone trade (docs/mobile.md, "The wire"), mirroring
// crates/yh-proto (protocol 3: live state, actions, the whole app's controls). JSON text frames tagged with `type`, keys in snake_case.
// Unknown messages and fields are ignored.

import Foundation

struct Project: Decodable, Sendable, Hashable, Identifiable {
    let id: String
    let name: String
    /// Protocol 3: its permission mode (`auto`, `ask`, `acceptEdits`, `plan`, `bypass`).
    let mode: String?
}

/// A model a thread can use (protocol 3's `options`).
struct ModelOption: Decodable, Sendable, Hashable, Identifiable {
    let id: String
    let name: String
    let efforts: [String]
    let auto: Bool

    enum CodingKeys: String, CodingKey { case id, name, efforts, auto }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decode(String.self, forKey: .name)
        efforts = try c.decodeIfPresent([String].self, forKey: .efforts) ?? []
        auto = try c.decodeIfPresent(Bool.self, forKey: .auto) ?? false
    }
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
    /// Protocol 3: the effort chosen for it.
    let effort: String?
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

/// An MCP server's question mid-call: `form` (accepted with its defaults, or declined) or `url`
/// (a page to visit).
struct MCPQuestion: Decodable, Sendable, Hashable, Identifiable {
    let id: String
    let server: String
    let message: String
    let mode: String?
    let url: String?
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
    /// An action carrying this id was taken, or why not; `thread`: the one it made.
    case done(id: String, error: String?, thread: String?)
    /// Protocol 3: the models and permission modes to choose from.
    case options(models: [ModelOption], modes: [String])
    /// Protocol 3: a row's whole tool input and result.
    case toolResult(thread: String, seq: Int, input: String?, result: String?)
    case error(String)
    case unknown

    private struct Tag: Decodable { let type: String }
    private struct Hello: Decodable { let `protocol`: Int?; let server: String; let data: String }
    private struct LiveMessage: Decodable { let thread: String; let live: Live }
    private struct Done: Decodable { let id: String; let error: String?; let thread: String? }
    private struct Options: Decodable { let models: [ModelOption]; let modes: [String] }
    private struct Whole: Decodable { let thread: String; let seq: Int; let input: String?; let result: String? }
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
                return .done(id: m.id, error: m.error, thread: m.thread)
            case "options":
                let m = try d.decode(Options.self, from: data)
                return .options(models: m.models, modes: m.modes)
            case "tool_result":
                let m = try d.decode(Whole.self, from: data)
                return .toolResult(thread: m.thread, seq: m.seq, input: m.input, result: m.result)
            case "error": return .error(try d.decode(Failure.self, from: data).message)
            default: return .unknown
            }
        } catch {
            return .error("couldn't read a message: \(error)")
        }
    }
}

/// A picture sent with a message: JPEG or PNG bytes, base64 on the wire.
struct OutgoingImage: Sendable, Hashable {
    let mediaType: String
    let data: Data
}

enum FromApp {
    case subscribeApp
    case subscribeThread(String, limit: Int)
    case unsubscribeThread
    case page(String, beforeSeq: Int, limit: Int)
    // Protocol 2's actions; each carries an id its `done` answers.
    case send(id: String, thread: String, text: String, now: Bool, images: [OutgoingImage] = [])
    case stop(id: String, thread: String)
    case respond(id: String, thread: String, requestID: String, allow: Bool)
    case withdraw(id: String, thread: String, message: String)
    case sendNow(id: String, thread: String, message: String)
    // Protocol 3.
    case answer(id: String, thread: String, requestID: String, answers: [String: String])
    case skipQuestion(id: String, thread: String, requestID: String)
    case approvePlan(id: String, thread: String, requestID: String, mode: String, feedback: String?)
    case keepPlanning(id: String, thread: String, requestID: String, feedback: String?)
    case answerQuestion(id: String, thread: String, requestID: String, accept: Bool)
    case newThread(id: String, project: String, text: String?)
    case setModel(id: String, thread: String, model: String?)
    case setEffort(id: String, thread: String, effort: String?)
    case setMode(id: String, thread: String, mode: String)
    case rename(id: String, thread: String, title: String)
    case pin(id: String, thread: String, on: Bool)
    case settle(id: String, thread: String, on: Bool)
    case delete(id: String, thread: String)
    case toolResult(thread: String, seq: Int)

    var actionID: String? {
        switch self {
        case .send(let id, _, _, _, _), .stop(let id, _), .respond(let id, _, _, _), .withdraw(let id, _, _), .sendNow(let id, _, _),
             .answer(let id, _, _, _), .skipQuestion(let id, _, _), .approvePlan(let id, _, _, _, _), .keepPlanning(let id, _, _, _),
             .answerQuestion(let id, _, _, _), .newThread(let id, _, _), .setModel(let id, _, _), .setEffort(let id, _, _),
             .setMode(let id, _, _), .rename(let id, _, _), .pin(let id, _, _), .settle(let id, _, _), .delete(let id, _): id
        default: nil
        }
    }

    /// For logs.
    var name: String {
        switch self {
        case .send(_, _, let text, let now, let images): (now ? "send now \(text.prefix(40))" : "send \(text.prefix(40))") + (images.isEmpty ? "" : " +\(images.count) pictures")
        case .stop: "stop"
        case .respond(_, _, _, let allow): allow ? "allow" : "deny"
        case .withdraw: "withdraw"
        case .sendNow: "send queued now"
        case .answer(_, _, _, let answers): "answer \(answers.values.sorted().joined(separator: "; "))"
        case .skipQuestion: "skip question"
        case .approvePlan(_, _, _, let mode, _): "approve plan \(mode)"
        case .keepPlanning: "keep planning"
        case .answerQuestion(_, _, _, let accept): accept ? "accept question" : "decline question"
        case .newThread: "new thread"
        case .setModel(_, _, let m): "model \(m ?? "default")"
        case .setEffort(_, _, let e): "effort \(e ?? "default")"
        case .setMode(_, _, let m): "mode \(m)"
        case .rename(_, _, let t): "rename \(t)"
        case .pin(_, _, let on): on ? "pin" : "unpin"
        case .settle(_, _, let on): on ? "settle" : "unsettle"
        case .delete: "delete"
        default: "\(self)"
        }
    }

    var json: String {
        let object: [String: Any] = switch self {
        case .subscribeApp: ["type": "subscribe_app"]
        case .subscribeThread(let t, let limit): ["type": "subscribe_thread", "thread": t, "limit": limit]
        case .unsubscribeThread: ["type": "unsubscribe_thread"]
        case .page(let t, let before, let limit): ["type": "page", "thread": t, "before_seq": before, "limit": limit]
        case .send(let id, let t, let text, let now, let images):
            ["type": "send", "id": id, "thread": t, "text": text, "now": now,
             "images": images.map { ["media_type": $0.mediaType, "data": $0.data.base64EncodedString()] }]
        case .stop(let id, let t): ["type": "stop", "id": id, "thread": t]
        case .respond(let id, let t, let request, let allow): ["type": "respond", "id": id, "thread": t, "request_id": request, "allow": allow]
        case .withdraw(let id, let t, let message): ["type": "withdraw", "id": id, "thread": t, "message": message]
        case .sendNow(let id, let t, let message): ["type": "send_now", "id": id, "thread": t, "message": message]
        case .answer(let id, let t, let request, let answers): ["type": "answer", "id": id, "thread": t, "request_id": request, "answers": answers]
        case .skipQuestion(let id, let t, let request): ["type": "skip_question", "id": id, "thread": t, "request_id": request]
        case .approvePlan(let id, let t, let request, let mode, let feedback):
            ["type": "approve_plan", "id": id, "thread": t, "request_id": request, "mode": mode, "feedback": feedback ?? ""]
        case .keepPlanning(let id, let t, let request, let feedback): ["type": "keep_planning", "id": id, "thread": t, "request_id": request, "feedback": feedback ?? ""]
        case .answerQuestion(let id, let t, let request, let accept): ["type": "answer_question", "id": id, "thread": t, "request_id": request, "accept": accept]
        case .newThread(let id, let project, let text): ["type": "new_thread", "id": id, "project": project, "text": text ?? ""]
        case .setModel(let id, let t, let model): ["type": "set_model", "id": id, "thread": t, "model": model.map { $0 as Any } ?? NSNull()]
        case .setEffort(let id, let t, let effort): ["type": "set_effort", "id": id, "thread": t, "effort": effort.map { $0 as Any } ?? NSNull()]
        case .setMode(let id, let t, let mode): ["type": "set_mode", "id": id, "thread": t, "mode": mode]
        case .rename(let id, let t, let title): ["type": "rename", "id": id, "thread": t, "title": title]
        case .pin(let id, let t, let on): ["type": "pin", "id": id, "thread": t, "on": on]
        case .settle(let id, let t, let on): ["type": "settle", "id": id, "thread": t, "on": on]
        case .delete(let id, let t): ["type": "delete", "id": id, "thread": t]
        case .toolResult(let t, let seq): ["type": "tool_result", "thread": t, "seq": seq]
        }
        let data = try! JSONSerialization.data(withJSONObject: object)
        return String(decoding: data, as: UTF8.self)
    }
}
