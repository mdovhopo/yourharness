// The messages the Mac and the phone trade (docs/mobile.md, "The wire"), mirroring
// crates/yh-proto (protocol 5: live state, actions, the whole app's controls, sub-agents, pictures,
// diagrams, search, files, commands, limits, side chats). JSON text frames tagged with `type`, keys in snake_case.
// Unknown messages and fields are ignored.

import Foundation
import UIKit

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
    /// Protocol 4: a sub-agent's call, its run summarized.
    let subagent: SubagentInfo?
}

extension Row {
    /// The same row whatever its `rev`: a server started again numbers its copies anew.
    func sameContent(as o: Row) -> Bool {
        seq == o.seq && id == o.id && kind == o.kind && text == o.text && toolName == o.toolName && toolInput == o.toolInput
            && toolResult == o.toolResult && toolResultLength == o.toolResultLength && toolIsError == o.toolIsError
            && noticeStyle == o.noticeStyle && date == o.date && turnEndedAt == o.turnEndedAt && attachments == o.attachments
            && changedFiles == o.changedFiles && subagent == o.subagent
    }
}

/// A slash command the composer offers (protocol 5).
struct SlashCommand: Decodable, Sendable, Hashable, Identifiable {
    let name: String
    let hint: String?
    let description: String?
    var id: String { name }
}

/// A usage window: how much is used (0 to 1), when it starts over (protocol 5).
struct LimitWindow: Decodable, Sendable, Hashable, Identifiable {
    let name: String
    let used: Double
    let resetsAt: Double?
    var id: String { name }
}

/// Where a search found its words (protocol 5).
struct SearchHit: Decodable, Sendable, Hashable, Identifiable {
    let thread: String
    let seq: Int
    let snippet: String
    var id: String { "\(thread)#\(seq)" }
}

/// A sub-agent's run as its card shows it (protocol 4).
struct SubagentInfo: Decodable, Sendable, Hashable {
    let description: String
    let status: String
    let agentType: String?
    let toolUses: Int?
    let tokens: Int?
    let durationMs: Int?
    let activity: String?
    let background: Bool?
}

struct SubagentStep: Decodable, Sendable, Hashable {
    let tool: String?
    let text: String
    let result: String?
    let isError: Bool?
}

struct SubagentRun: Decodable, Sendable {
    let info: SubagentInfo
    let prompt: String?
    let steps: [SubagentStep]
    let report: String?
}

/// A colour in a drawing: a role resolved in the phone's own light or dark, or the author's own.
struct Ink: Decodable, Sendable {
    let role: String?
    let rgba: [Double]?
    let alpha: Double
}

struct DrawnPath: Decodable, Sendable {
    let d: String
    let fill: Ink?
    let stroke: Ink?
    let width: Double?
    let dashed: Bool?
}

struct DrawnLabel: Decodable, Sendable {
    let lines: [String]
    let x: Double
    let y: Double
    let width: Double?
    let size: Double
    let lineHeight: Double
    let weight: Int
    let color: Ink
}

/// A Mermaid diagram as the Mac draws it (protocol 4).
struct Drawing: Decodable, Sendable {
    let width: Double
    let height: Double
    let kind: String
    let paths: [DrawnPath]
    let labels: [DrawnLabel]
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
    /// Protocol 3: the models and permission modes to choose from; 5: each project's slash commands.
    case options(models: [ModelOption], modes: [String], commands: [String: [SlashCommand]])
    /// Protocol 5: usage limits; what a search found; a thread's files for `@`.
    case limits([LimitWindow])
    case searchResults(query: String, hits: [SearchHit])
    case files(thread: String, query: String, files: [String])
    /// Protocol 3: a row's whole tool input and result.
    case toolResult(thread: String, seq: Int, input: String?, result: String?)
    /// Protocol 4: a sub-agent's whole run; a row's picture (decoded, or why not); a diagram drawn.
    case subagent(thread: String, seq: Int, run: SubagentRun)
    case media(thread: String, seq: Int, index: Int, image: UIImage?, error: String?)
    case diagram(key: String, drawing: Drawing?, error: String?)
    case error(String)
    case unknown

    private struct Tag: Decodable { let type: String }
    private struct Hello: Decodable { let `protocol`: Int?; let server: String; let data: String }
    private struct LiveMessage: Decodable { let thread: String; let live: Live }
    private struct Done: Decodable { let id: String; let error: String?; let thread: String? }
    private struct Options: Decodable { let models: [ModelOption]; let modes: [String]; let commands: [String: [SlashCommand]]? }
    private struct Limits: Decodable { let windows: [LimitWindow] }
    private struct Found: Decodable { let query: String; let hits: [SearchHit] }
    private struct Files: Decodable { let thread: String; let query: String; let files: [String] }
    private struct Whole: Decodable { let thread: String; let seq: Int; let input: String?; let result: String? }
    private struct Sub: Decodable { let thread: String; let seq: Int; let run: SubagentRun }
    private struct Picture: Decodable { let thread: String; let seq: Int; let index: Int; let data: String?; let error: String? }
    private struct Diagram: Decodable { let key: String; let drawing: Drawing?; let error: String? }
    private struct Threads: Decodable { let projects: [Project]; let threads: [ThreadInfo] }
    private struct Upserted: Decodable { let thread: ThreadInfo }
    private struct Removed: Decodable { let id: String }
    private struct Snapshot: Decodable { let thread: String; let firstSeq: Int; let count: Int; let rows: [Row] }
    private struct Changed: Decodable { let thread: String; let count: Int; let rows: [Row] }
    private struct Page: Decodable { let thread: String; let rows: [Row] }
    private struct Failure: Decodable { let message: String }

    /// A picture scaled down to at most 1600 points on its longer side, decoded now.
    static func fitted(_ image: UIImage) -> UIImage {
        let longest = max(image.size.width, image.size.height)
        guard longest > 1600 else { return image.preparingForDisplay() ?? image }
        let scale = 1600 / longest
        return image.preparingThumbnail(of: CGSize(width: image.size.width * scale, height: image.size.height * scale)) ?? image
    }

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
                return .options(models: m.models, modes: m.modes, commands: m.commands ?? [:])
            case "limits": return .limits(try d.decode(Limits.self, from: data).windows)
            case "search_results":
                let m = try d.decode(Found.self, from: data)
                return .searchResults(query: m.query, hits: m.hits)
            case "files":
                let m = try d.decode(Files.self, from: data)
                return .files(thread: m.thread, query: m.query, files: m.files)
            case "tool_result":
                let m = try d.decode(Whole.self, from: data)
                return .toolResult(thread: m.thread, seq: m.seq, input: m.input, result: m.result)
            case "subagent":
                let m = try d.decode(Sub.self, from: data)
                return .subagent(thread: m.thread, seq: m.seq, run: m.run)
            case "media":
                let m = try d.decode(Picture.self, from: data)
                // Decoded here, off the main thread, at most 1600 points on its longer side.
                let image = m.data.flatMap { Data(base64Encoded: $0) }.flatMap(UIImage.init(data:)).map(ToApp.fitted)
                return .media(thread: m.thread, seq: m.seq, index: m.index, image: image, error: image == nil ? (m.error ?? "The picture couldn't be read.") : nil)
            case "diagram":
                let m = try d.decode(Diagram.self, from: data)
                return .diagram(key: m.key, drawing: m.drawing, error: m.error)
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
    case subscribeThread(String, limit: Int, known: Int? = nil)
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
    // Protocol 4.
    case subagent(thread: String, seq: Int)
    case media(thread: String, seq: Int, index: Int)
    case diagram(key: String, source: String)
    // Protocol 5.
    case search(query: String)
    case files(thread: String, query: String)
    case askAside(id: String, thread: String, text: String)

    var actionID: String? {
        switch self {
        case .send(let id, _, _, _, _), .stop(let id, _), .respond(let id, _, _, _), .withdraw(let id, _, _), .sendNow(let id, _, _),
             .answer(let id, _, _, _), .skipQuestion(let id, _, _), .approvePlan(let id, _, _, _, _), .keepPlanning(let id, _, _, _),
             .answerQuestion(let id, _, _, _), .newThread(let id, _, _), .setModel(let id, _, _), .setEffort(let id, _, _),
             .setMode(let id, _, _), .rename(let id, _, _), .pin(let id, _, _), .settle(let id, _, _), .delete(let id, _), .askAside(let id, _, _): id
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
        case .askAside(_, _, let text): "ask aside \(text.prefix(40))"
        default: "\(self)"
        }
    }

    var json: String {
        let object: [String: Any] = switch self {
        case .subscribeApp: ["type": "subscribe_app"]
        case .subscribeThread(let t, let limit, let known): ["type": "subscribe_thread", "thread": t, "limit": limit, "known": known.map { $0 as Any } ?? NSNull()]
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
        case .subagent(let t, let seq): ["type": "subagent", "thread": t, "seq": seq]
        case .media(let t, let seq, let index): ["type": "media", "thread": t, "seq": seq, "index": index]
        case .diagram(let key, let source): ["type": "diagram", "key": key, "source": source]
        case .search(let query): ["type": "search", "query": query]
        case .files(let t, let query): ["type": "files", "thread": t, "query": query]
        case .askAside(let id, let t, let text): ["type": "ask_aside", "id": id, "thread": t, "text": text]
        }
        let data = try! JSONSerialization.data(withJSONObject: object)
        return String(decoding: data, as: UTF8.self)
    }
}
