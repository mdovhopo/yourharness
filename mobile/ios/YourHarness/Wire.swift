// The messages the Mac and the phone trade (docs/mobile.md, "The wire"), mirroring
// crates/yh-proto (protocol 5: live state, actions, the whole app's controls, sub-agents, pictures,
// diagrams, search, files, commands, limits, side chats). JSON text frames tagged with `type`, keys in snake_case.
// Unknown messages and fields are ignored.

import Foundation
import SwiftUI
import UIKit

struct Project: Codable, Sendable, Hashable, Identifiable {
    let id: String
    let name: String
    /// Protocol 3: its permission mode (`auto`, `ask`, `acceptEdits`, `plan`, `bypass`).
    let mode: String?
}

/// A model a thread can use (protocol 3's `options`).
struct ModelOption: Codable, Sendable, Hashable, Identifiable {
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

struct ThreadInfo: Codable, Sendable, Hashable, Identifiable {
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
    /// Commands and sub-agents running in the background, after its turn.
    let jobs: Int?

    var isRunning: Bool { running ?? active }
    var isWaiting: Bool { waiting ?? false }
    var backgroundJobs: Int { jobs ?? 0 }
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

/// Something Claude left running: a command, a watch (Monitor) or a sub-agent in the
/// background. Its clock ticks here; the Mac sends it again only when it changes.
struct Job: Decodable, Sendable, Hashable, Identifiable {
    let id: String
    /// `command`, `watch` or `agent`.
    let kind: String
    let description: String
    let background: Bool?
    /// Seconds since 1970.
    let startedAt: Double
    /// What a watch last saw, or what a sub-agent is doing now.
    let lastLine: String?
    /// The running sub-agent that started it: it counts with that one.
    let agent: String?
}

/// What the open thread's run is doing, beyond its rows.
struct Live: Decodable, Sendable, Equatable {
    let running: Bool
    let activity: String?
    let requests: [Request]
    let questions: [MCPQuestion]
    let queue: [Queued]
    /// What's running, sub-agents first.
    let jobs: [Job]
    let contextTokens: Int?
    let contextWindow: Int?

    enum CodingKeys: String, CodingKey { case running, activity, requests, questions, queue, jobs, contextTokens, contextWindow }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        running = try c.decodeIfPresent(Bool.self, forKey: .running) ?? false
        activity = try c.decodeIfPresent(String.self, forKey: .activity)
        requests = try c.decodeIfPresent([Request].self, forKey: .requests) ?? []
        questions = try c.decodeIfPresent([MCPQuestion].self, forKey: .questions) ?? []
        queue = try c.decodeIfPresent([Queued].self, forKey: .queue) ?? []
        jobs = try c.decodeIfPresent([Job].self, forKey: .jobs) ?? []
        contextTokens = try c.decodeIfPresent(Int.self, forKey: .contextTokens)
        contextWindow = try c.decodeIfPresent(Int.self, forKey: .contextWindow)
    }
}

struct Row: Codable, Sendable {
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
    /// Protocol 8: the thread's files a reply names, for links.
    let files: [FileRef]?
}

/// A file of the thread's folder a reply names (protocol 8): as written there, the path it is
/// (relative to the folder), the lines it points at.
struct FileRef: Codable, Sendable, Hashable {
    let text: String
    let path: String
    let line: Int?
    let end: Int?
}

/// One entry of a folder in the thread's folder (protocol 8).
struct DirEntry: Decodable, Sendable, Hashable, Identifiable {
    let name: String
    let folder: Bool
    /// Git ignores it: dimmed.
    let ignored: Bool?
    let size: Int?
    var id: String { name }
}

/// A line of a file, coloured as the Mac colours it (made as the frame is decoded).
struct FileLine: Sendable, Hashable {
    let text: String
    let styled: AttributedString
}

/// A page of a file (protocol 8): `lines` start at `from`, of `total`.
struct FilePage: Sendable {
    /// `text`, `binary` or `image`.
    let kind: String
    let size: Int
    let modified: Double
    let total: Int
    let from: Int
    let lines: [FileLine]
    let note: String?
    let image: UIImage?
    let unchanged: Bool

    fileprivate struct Raw: Decodable {
        let kind: String
        let size: Int
        let modified: Double
        let total: Int?
        let from: Int?
        let lines: [String]?
        let spans: [[[Int]]]?
        let note: String?
        let data: String?
        let unchanged: Bool?
    }

    /// Kind 0 keyword, 1 string, 2 comment, 3 number: the colours Xcode uses for them.
    static let colours: [Color] = [Color(uiColor: .systemPink), Color(uiColor: .systemRed), .secondary, Color(uiColor: .systemBlue)]

    fileprivate init(_ raw: Raw) {
        kind = raw.kind
        size = raw.size
        modified = raw.modified
        total = raw.total ?? 0
        from = raw.from ?? 0
        note = raw.note
        unchanged = raw.unchanged ?? false
        image = raw.data.flatMap { Data(base64Encoded: $0) }.flatMap(UIImage.init(data:)).map(ToApp.fitted)
        let spans = raw.spans ?? []
        lines = (raw.lines ?? []).enumerated().map { i, text in
            var styled = AttributedString(text)
            for span in i < spans.count ? spans[i] : [] where span.count == 3 && span[1] > span[0] {
                guard let range = Range<AttributedString.Index>(NSRange(location: span[0], length: span[1] - span[0]), in: styled) else { continue }
                styled[range].foregroundColor = FilePage.colours[min(max(span[2], 0), 3)]
            }
            return FileLine(text: text, styled: styled)
        }
    }
}

extension Row {
    /// The same row whatever its `rev`: a server started again numbers its copies anew.
    func sameContent(as o: Row) -> Bool {
        seq == o.seq && id == o.id && kind == o.kind && text == o.text && toolName == o.toolName && toolInput == o.toolInput
            && toolResult == o.toolResult && toolResultLength == o.toolResultLength && toolIsError == o.toolIsError
            && noticeStyle == o.noticeStyle && date == o.date && turnEndedAt == o.turnEndedAt && attachments == o.attachments
            && changedFiles == o.changedFiles && subagent == o.subagent && files == o.files
    }
}

/// One open PR as the project's page on the Mac lists it (protocol 5): in a stack (`stack`, its
/// bottom PR; `depth`, how far up) and a section (`review`, `mine`, `others`).
struct PullInfo: Decodable, Sendable, Hashable, Identifiable {
    let repo: String
    let number: Int
    let title: String
    let url: String
    let author: String?
    let draft: Bool
    let review: String?
    let checks: String?
    let mergeStatus: String?
    let autoMerge: Bool?
    let inMergeQueue: Bool?
    let updatedAt: String?
    let head: String
    let base: String
    let mine: Bool
    let section: String
    let stack: String
    let depth: Int
    var id: String { "\(repo)#\(number)" }
}

/// A project's PRs as the Mac answered.
struct PullsAnswer: Sendable {
    let pulls: [PullInfo]
    let errors: [String]
    let accounts: [String]
}

/// One PR in full, for its review page (protocol 6): the Mac reads it through `gh` or `glab`.
struct PullDetail: Decodable, Sendable, Hashable {
    let repo: String
    let number: Int
    let title: String
    let url: String
    /// `open`, `merged`, `closed`.
    let state: String
    let draft: Bool
    let author: String?
    let body: String?
    let head: String
    let base: String
    let additions: Int?
    let deletions: Int?
    let changedFiles: Int?
    let commits: Int?
    let createdAt: String?
    let updatedAt: String?
    let mergedAt: String?
    let labels: [String]?
    /// `approved`, `changesRequested`, `required` or empty.
    let review: String?
    /// `CLEAN`, `BLOCKED`, `BEHIND`, `DIRTY`, `UNSTABLE`, `DRAFT`, `UNKNOWN`.
    let mergeStatus: String?
    let autoMerge: Bool?
    /// Failing first.
    let checks: [PullCheck]?
    /// Oldest first.
    let comments: [PullComment]?
    /// Of `squash`, `merge`, `rebase`, the repo's default first.
    let mergeMethods: [String]?
}

/// A file a PR changes (protocol 6).
struct PullFile: Decodable, Sendable, Hashable, Identifiable {
    let path: String
    let previousPath: String?
    /// `added`, `removed`, `modified`, `renamed`, `copied`, `changed`.
    let status: String
    let additions: Int?
    let deletions: Int?
    /// Whether the forge gives its diff (not for huge or binary files).
    let hasPatch: Bool?
    var id: String { path }
}

/// A line of a file's diff, numbered as in the file before (`old`) and after (`new`).
struct DiffLine: Sendable, Hashable {
    enum Kind: Sendable, Hashable { case hunk, context, added, removed, note }
    let kind: Kind
    let old: Int?
    let new: Int?
    let text: String

    /// A unified diff's hunks (what GitHub's `patch` holds) as lines. Call it off the main thread.
    static func parse(_ patch: String) -> [DiffLine] {
        var lines: [DiffLine] = []
        var old = 0, new = 0
        for raw in patch.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = raw.hasSuffix("\r") ? raw.dropLast() : raw
            guard let first = line.first else {
                if !lines.isEmpty { lines.append(DiffLine(kind: .context, old: old, new: new, text: "")); old += 1; new += 1 }
                continue
            }
            switch first {
            case "@":
                // `@@ -12,7 +12,8 @@ fn context()`
                let parts = line.split(separator: " ")
                if parts.count >= 3 {
                    old = Int(parts[1].dropFirst().split(separator: ",").first ?? "") ?? old
                    new = Int(parts[2].dropFirst().split(separator: ",").first ?? "") ?? new
                }
                lines.append(DiffLine(kind: .hunk, old: nil, new: nil, text: String(line)))
            case "+":
                if line.hasPrefix("+++ ") && lines.isEmpty { continue }
                lines.append(DiffLine(kind: .added, old: nil, new: new, text: String(line.dropFirst()))); new += 1
            case "-":
                if line.hasPrefix("--- ") && lines.isEmpty { continue }
                lines.append(DiffLine(kind: .removed, old: old, new: nil, text: String(line.dropFirst()))); old += 1
            case "\\":
                lines.append(DiffLine(kind: .note, old: nil, new: nil, text: String(line.dropFirst(2))))
            default:
                lines.append(DiffLine(kind: .context, old: old, new: new, text: String(line.dropFirst()))); old += 1; new += 1
            }
        }
        // The empty line a patch ending in a newline leaves.
        if let last = lines.last, last.kind == .context, last.text.isEmpty { lines.removeLast() }
        return lines
    }
}

struct PullCheck: Decodable, Sendable, Hashable {
    let name: String
    let workflow: String?
    /// `passing`, `failing`, `pending`, `skipped`.
    let state: String
    let url: String?
}

struct PullComment: Decodable, Sendable, Hashable {
    let author: String?
    let body: String
    let at: String?
    /// `comment`, `review` or `code`.
    let kind: String
    /// `approved`, `changesRequested`, `commented`, `dismissed`.
    let verdict: String?
    let path: String?
    let line: Int?
    let url: String?
}

/// A thread's browser around its pictures (protocol 7): its tabs, and the page's size in CSS
/// pixels, which is where a tap on a picture lands.
struct BrowserState: Decodable, Sendable, Equatable {
    var tabs: [BrowserTab] = []
    var selected: Int?
    var canBack = false
    var canForward = false
    var width: Double = 0
    var height: Double = 0
    /// Taken over on the Mac: the agent's actions wait; yours still work.
    var userControl = false
    /// The focused element takes typing.
    var editing = false
    var dialog: BrowserDialog?

    var shown: BrowserTab? { tabs.first { $0.id == selected } ?? tabs.last }

    init() {}
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        tabs = try c.decodeIfPresent([BrowserTab].self, forKey: .tabs) ?? []
        selected = try c.decodeIfPresent(Int.self, forKey: .selected)
        canBack = try c.decodeIfPresent(Bool.self, forKey: .canBack) ?? false
        canForward = try c.decodeIfPresent(Bool.self, forKey: .canForward) ?? false
        width = try c.decodeIfPresent(Double.self, forKey: .width) ?? 0
        height = try c.decodeIfPresent(Double.self, forKey: .height) ?? 0
        userControl = try c.decodeIfPresent(Bool.self, forKey: .userControl) ?? false
        editing = try c.decodeIfPresent(Bool.self, forKey: .editing) ?? false
        dialog = try c.decodeIfPresent(BrowserDialog.self, forKey: .dialog)
    }
    private enum CodingKeys: String, CodingKey { case tabs, selected, canBack, canForward, width, height, userControl, editing, dialog }
}

struct BrowserTab: Decodable, Sendable, Equatable, Identifiable {
    let id: Int
    var title: String = ""
    var url: String = ""
    var loading = false

    private enum CodingKeys: String, CodingKey { case id, title, url, loading }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(Int.self, forKey: .id)
        title = try c.decodeIfPresent(String.self, forKey: .title) ?? ""
        url = try c.decodeIfPresent(String.self, forKey: .url) ?? ""
        loading = try c.decodeIfPresent(Bool.self, forKey: .loading) ?? false
    }
}

/// A thread's browser in a few words (protocol 7): pages open, the shown one, and when the agent
/// last opened one (seconds since 1970).
struct BrowserSummary: Decodable, Sendable, Equatable {
    let tabs: Int
    let url: String?
    let title: String?
    let openedAt: Double?
}

struct BrowserDialog: Decodable, Sendable, Equatable {
    /// `alert`, `confirm` or `prompt`.
    let kind: String
    let message: String
    let defaultText: String?
}

/// What the phone does in a thread's browser (protocol 7); points in the page's CSS pixels.
enum BrowserInput: Sendable, Equatable {
    case tap(x: Double, y: Double)
    case scroll(x: Double, y: Double, dx: Double, dy: Double)
    case text(String)
    case key(String)
    case navigate(String)
    case back, forward, reload
    case open(String)
    case selectTab(Int)
    case closeTab(Int)
    case size(String, width: Double? = nil, height: Double? = nil)
    case dialog(accept: Bool, text: String?)

    var object: [String: Any] {
        switch self {
        case .tap(let x, let y): ["kind": "tap", "x": x, "y": y]
        case .scroll(let x, let y, let dx, let dy): ["kind": "scroll", "x": x, "y": y, "dx": dx, "dy": dy]
        case .text(let text): ["kind": "text", "text": text]
        case .key(let key): ["kind": "key", "key": key]
        case .navigate(let url): ["kind": "navigate", "url": url]
        case .back: ["kind": "back"]
        case .forward: ["kind": "forward"]
        case .reload: ["kind": "reload"]
        case .open(let url): ["kind": "open", "url": url]
        case .selectTab(let tab): ["kind": "select_tab", "tab": tab]
        case .closeTab(let tab): ["kind": "close_tab", "tab": tab]
        case .size(let preset, let width, let height):
            if let width, let height { ["kind": "size", "preset": preset, "width": width, "height": height] } else { ["kind": "size", "preset": preset] }
        case .dialog(let accept, let text): text.map { ["kind": "dialog", "accept": accept, "text": $0] } ?? ["kind": "dialog", "accept": accept]
        }
    }
}

/// A slash command the composer offers (protocol 5).
struct SlashCommand: Codable, Sendable, Hashable, Identifiable {
    let name: String
    let hint: String?
    let description: String?
    var id: String { name }
}

/// A usage window: how much is used (0 to 1), when it starts over (protocol 5).
/// The Claude subscription a Mac's limits belong to (one per Mac for now). Sent by Macs since
/// 0.1.8; older ones leave it out.
struct Subscription: Decodable, Sendable, Hashable {
    /// "Claude Max", "Claude Pro", "API key".
    let plan: String
    let email: String?
    let org: String?
    /// Which account: Macs signed in to the same one show their limits together.
    var account: String { email ?? org ?? plan }
}

struct LimitWindow: Decodable, Sendable, Hashable, Identifiable {
    let name: String
    let used: Double
    let resetsAt: Double?
    var id: String { name }
}

/// The Mac's battery, as its menu bar shows it. Sent by Macs since 0.1.11 that have one.
struct Power: Decodable, Sendable, Hashable {
    /// 0 to 100.
    let percent: Int
    let charging: Bool
    /// On a charger, charging or not.
    let pluggedIn: Bool
}

/// Where a search found its words (protocol 5).
struct SearchHit: Decodable, Sendable, Hashable, Identifiable {
    let thread: String
    let seq: Int
    let snippet: String
    var id: String { "\(thread)#\(seq)" }
}

/// A sub-agent's run as its card shows it (protocol 4).
struct SubagentInfo: Codable, Sendable, Hashable {
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
    /// `mac` (protocol 5): the Mac's lasting ID.
    case hello(protocol: Int, server: String, data: String, mac: String?, hosts: [String])
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
    case limits([LimitWindow], Subscription?)
    /// The Mac's battery (0.1.11; Macs with one).
    case power(Power)
    case searchResults(query: String, hits: [SearchHit])
    case files(thread: String, query: String, files: [String])
    /// Protocol 3: a row's whole tool input and result.
    case toolResult(thread: String, seq: Int, input: String?, result: String?)
    /// Protocol 4: a sub-agent's whole run; a row's picture (decoded, or why not); a diagram drawn.
    case subagent(thread: String, seq: Int, run: SubagentRun)
    case media(thread: String, seq: Int, index: Int, image: UIImage?, error: String?)
    case diagram(key: String, drawing: Drawing?, error: String?)
    case error(String)
    /// Protocol 5: a project's open PRs.
    case pulls(project: String, answer: PullsAnswer)
    /// Protocol 5: paired with the Mac (the connection carried a pairing code): its token.
    case paired(device: String, token: String)
    /// Protocol 6: a PR in full, or why it couldn't be read.
    case pull(url: String, detail: PullDetail?, error: String?)
    case pullFiles(url: String, files: [PullFile], error: String?)
    /// A file's diff, already split into lines.
    case pullPatch(url: String, path: String, lines: [DiffLine]?, error: String?)
    /// Protocol 7: the watched browser's tabs; a picture of its shown tab (decoded here).
    case browser(thread: String, state: BrowserState)
    case browserFrame(thread: String, tab: Int, seq: Int, image: UIImage?)
    case browserSummary(thread: String, summary: BrowserSummary)
    /// Protocol 8: a folder of the thread's folder; a page of a file; what changed since the
    /// last commit; one change's diff, split into lines.
    case dir(thread: String, path: String, entries: [DirEntry], error: String?)
    case file(thread: String, path: String, page: FilePage?, error: String?)
    case changes(thread: String, files: [PullFile], branch: String?, error: String?)
    case changePatch(thread: String, path: String, lines: [DiffLine]?, error: String?)
    case unknown

    /// Changes what the phone keeps of this Mac between launches (`Store`'s cache).
    var changesCache: Bool {
        switch self {
        case .hello, .threads, .threadUpserted, .threadRemoved, .options: true
        default: false
        }
    }

    private struct Tag: Decodable { let type: String }
    private struct Dir: Decodable { let thread: String; let path: String; let entries: [DirEntry]?; let error: String? }
    private struct File: Decodable { let thread: String; let path: String; let page: FilePage.Raw?; let error: String? }
    private struct Changes: Decodable { let thread: String; let files: [PullFile]?; let branch: String?; let error: String? }
    private struct ChangePatch: Decodable { let thread: String; let path: String; let patch: String?; let error: String? }
    private struct Pulls: Decodable { let project: String; let pulls: [PullInfo]; let errors: [String]?; let accounts: [String]? }
    private struct Paired: Decodable { let device: String; let token: String }
    private struct Pull: Decodable { let url: String; let pull: PullDetail?; let error: String? }
    private struct PullFiles: Decodable { let url: String; let files: [PullFile]?; let error: String? }
    private struct PullPatch: Decodable { let url: String; let path: String; let patch: String?; let error: String? }
    private struct Browser: Decodable { let thread: String; let state: BrowserState }
    private struct Summary: Decodable { let thread: String; let summary: BrowserSummary }
    private struct Frame: Decodable { let thread: String; let tab: Int; let seq: Int; let data: String }
    private struct Hello: Decodable { let `protocol`: Int?; let server: String; let data: String; let mac: String?; let hosts: [String]? }
    private struct LiveMessage: Decodable { let thread: String; let live: Live }
    private struct Done: Decodable { let id: String; let error: String?; let thread: String? }
    private struct Options: Decodable { let models: [ModelOption]; let modes: [String]; let commands: [String: [SlashCommand]]? }
    private struct Limits: Decodable { let windows: [LimitWindow]; let subscription: Subscription? }
    private struct PowerMessage: Decodable { let power: Power }
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
                return .hello(protocol: m.protocol ?? 1, server: m.server, data: m.data, mac: m.mac, hosts: m.hosts ?? [])
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
            case "limits":
                let m = try d.decode(Limits.self, from: data)
                return .limits(m.windows, m.subscription)
            case "power": return .power(try d.decode(PowerMessage.self, from: data).power)
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
            case "pulls":
                let m = try d.decode(Pulls.self, from: data)
                return .pulls(project: m.project, answer: PullsAnswer(pulls: m.pulls, errors: m.errors ?? [], accounts: m.accounts ?? []))
            case "pull":
                let m = try d.decode(Pull.self, from: data)
                return .pull(url: m.url, detail: m.pull, error: m.error)
            case "pull_files":
                let m = try d.decode(PullFiles.self, from: data)
                return .pullFiles(url: m.url, files: m.files ?? [], error: m.error)
            case "pull_patch":
                let m = try d.decode(PullPatch.self, from: data)
                return .pullPatch(url: m.url, path: m.path, lines: m.patch.map(DiffLine.parse), error: m.error)
            case "browser":
                let m = try d.decode(Browser.self, from: data)
                return .browser(thread: m.thread, state: m.state)
            case "browser_summary":
                let m = try d.decode(Summary.self, from: data)
                return .browserSummary(thread: m.thread, summary: m.summary)
            case "browser_frame":
                let m = try d.decode(Frame.self, from: data)
                // Decoded here, off the main thread, ready to draw.
                let image = Data(base64Encoded: m.data).flatMap(UIImage.init(data:)).map { $0.preparingForDisplay() ?? $0 }
                return .browserFrame(thread: m.thread, tab: m.tab, seq: m.seq, image: image)
            case "dir":
                let m = try d.decode(Dir.self, from: data)
                return .dir(thread: m.thread, path: m.path, entries: m.entries ?? [], error: m.error)
            case "file":
                let m = try d.decode(File.self, from: data)
                return .file(thread: m.thread, path: m.path, page: m.page.map(FilePage.init), error: m.error)
            case "changes":
                let m = try d.decode(Changes.self, from: data)
                return .changes(thread: m.thread, files: m.files ?? [], branch: m.branch, error: m.error)
            case "change_patch":
                let m = try d.decode(ChangePatch.self, from: data)
                return .changePatch(thread: m.thread, path: m.path, lines: m.patch.map(DiffLine.parse), error: m.error)
            case "paired":
                let m = try d.decode(Paired.self, from: data)
                return .paired(device: m.device, token: m.token)
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
    case watchBrowser(thread: String, width: Int)
    case unwatchBrowser
    case browserAck(seq: Int)
    case browserInput(id: String, thread: String, input: BrowserInput)
    case subscribeApp
    case subscribeThread(String, limit: Int, known: Int? = nil)
    case unsubscribeThread
    /// Protocol 5: only this one (a side chat closed).
    case unsubscribe(String)
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
    case newThread(id: String, project: String, text: String?, model: String? = nil, effort: String? = nil, mode: String? = nil)
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
    case pulls(project: String, fresh: Bool)
    case pull(url: String)
    case pullFiles(url: String)
    case pullPatch(url: String, path: String)
    case mergePull(id: String, url: String, method: String, auto: Bool)
    // Protocol 8.
    case listDir(thread: String, path: String)
    case readFile(thread: String, path: String, from: Int, count: Int, known: Double?)
    case changes(thread: String)
    case changePatch(thread: String, path: String)

    var actionID: String? {
        switch self {
        case .send(let id, _, _, _, _), .stop(let id, _), .respond(let id, _, _, _), .withdraw(let id, _, _), .sendNow(let id, _, _),
             .answer(let id, _, _, _), .skipQuestion(let id, _, _), .approvePlan(let id, _, _, _, _), .keepPlanning(let id, _, _, _),
             .answerQuestion(let id, _, _, _), .newThread(let id, _, _, _, _, _), .setModel(let id, _, _), .setEffort(let id, _, _),
             .setMode(let id, _, _), .rename(let id, _, _), .pin(let id, _, _), .settle(let id, _, _), .delete(let id, _), .askAside(let id, _, _), .mergePull(let id, _, _, _), .browserInput(let id, _, _): id
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
        case .mergePull(_, let url, let method, let auto): "merge \(url) \(method)\(auto ? " auto" : "")"
        case .browserInput(_, _, let input): "browser \(input)"
        default: "\(self)"
        }
    }

    var json: String {
        let object: [String: Any] = switch self {
        case .subscribeApp: ["type": "subscribe_app"]
        case .subscribeThread(let t, let limit, let known): ["type": "subscribe_thread", "thread": t, "limit": limit, "known": known.map { $0 as Any } ?? NSNull()]
        case .unsubscribeThread: ["type": "unsubscribe_thread"]
        case .unsubscribe(let t): ["type": "unsubscribe_thread", "thread": t]
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
        case .newThread(let id, let project, let text, let model, let effort, let mode):
            ["type": "new_thread", "id": id, "project": project, "text": text ?? "", "model": model.map { $0 as Any } ?? NSNull(),
             "effort": effort.map { $0 as Any } ?? NSNull(), "mode": mode.map { $0 as Any } ?? NSNull()]
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
        case .pulls(let project, let fresh): ["type": "pulls", "project": project, "fresh": fresh]
        case .pull(let url): ["type": "pull", "url": url]
        case .pullFiles(let url): ["type": "pull_files", "url": url]
        case .pullPatch(let url, let path): ["type": "pull_patch", "url": url, "path": path]
        case .mergePull(let id, let url, let method, let auto): ["type": "merge_pull", "id": id, "url": url, "method": method, "auto": auto]
        case .listDir(let t, let path): ["type": "list_dir", "thread": t, "path": path]
        case .readFile(let t, let path, let from, let count, let known):
            ["type": "read_file", "thread": t, "path": path, "from": from, "count": count, "known": known.map { $0 as Any } ?? NSNull()]
        case .changes(let t): ["type": "changes", "thread": t]
        case .changePatch(let t, let path): ["type": "change_patch", "thread": t, "path": path]
        case .watchBrowser(let t, let width): ["type": "watch_browser", "thread": t, "width": width]
        case .unwatchBrowser: ["type": "unwatch_browser"]
        case .browserAck(let seq): ["type": "browser_ack", "seq": seq]
        case .browserInput(let id, let t, let input): ["type": "browser_input", "id": id, "thread": t, "input": input.object]
        }
        let data = try! JSONSerialization.data(withJSONObject: object)
        return String(decoding: data, as: UTF8.self)
    }
}
