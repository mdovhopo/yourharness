// The messages the Mac and the phone trade (docs/mobile.md, "The wire"), mirroring
// crates/yh-devserver/src/wire.rs. JSON text frames tagged with `type`, keys in snake_case.
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
    /// Its rows changed in the last few seconds.
    let active: Bool
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
    case hello(server: String, data: String)
    case threads(projects: [Project], threads: [ThreadInfo])
    case threadUpserted(ThreadInfo)
    case threadRemoved(String)
    case snapshot(thread: String, firstSeq: Int, count: Int, rows: [RowModel])
    case rowsChanged(thread: String, count: Int, rows: [RowModel])
    case pageRows(thread: String, rows: [RowModel])
    case error(String)
    case unknown

    private struct Tag: Decodable { let type: String }
    private struct Hello: Decodable { let server: String; let data: String }
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
                return .hello(server: m.server, data: m.data)
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

    var json: String {
        let object: [String: Any] = switch self {
        case .subscribeApp: ["type": "subscribe_app"]
        case .subscribeThread(let t, let limit): ["type": "subscribe_thread", "thread": t, "limit": limit]
        case .unsubscribeThread: ["type": "unsubscribe_thread"]
        case .page(let t, let before, let limit): ["type": "page", "thread": t, "before_seq": before, "limit": limit]
        }
        let data = try! JSONSerialization.data(withJSONObject: object)
        return String(decoding: data, as: UTF8.self)
    }
}
