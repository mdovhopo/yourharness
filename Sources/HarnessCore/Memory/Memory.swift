import Foundation

extension HarnessDatabase {
    /// One thing worth remembering, in one workspace: a fact about you, how you like to work,
    /// something about the project, or where to look something up.
    public struct Memory: Sendable, Equatable, Identifiable, Hashable {
        public enum Status: String, Sendable, CaseIterable, Codable {
            /// In use: given to agents.
            case active
            /// Suggested, waiting for you to review.
            case proposed
            /// Kept for the record, not used.
            case archived
        }

        public var id: String
        /// `global`, or `project:<id>`.
        public var scope: String
        /// `user`, `feedback`, `project` or `reference` (see `MemoryKind`).
        public var kind: String
        public var title: String
        /// One line: what the index shows agents, so they can tell whether to read the rest.
        public var description: String
        public var body: String
        public var status: Status
        /// Always given to agents in full, not only listed.
        public var pinned: Bool
        /// Who first wrote it: `agent`, `user`, `import` or `reconciler`.
        public var origin: String?
        public var createdAt: Date
        public var updatedAt: Date
        /// How many times an agent read it.
        public var recallCount: Int
        /// How many times it came up in an agent's search.
        public var surfacedCount: Int
        public var lastRecalledAt: Date?
        /// Set when it was merged into another memory.
        public var mergedInto: String?
        /// Where it was learned.
        public var sources: [MemorySource]
        /// Memories it points at.
        public var links: [MemoryLink]
        /// For a task (kind `task`): where it stands.
        public var taskState: TaskState?
        /// For a task: its steps, in order.
        public var steps: [TaskStep]

        public init(id: String = UUID().uuidString, scope: String = "global", kind: String = "project", title: String,
                    description: String = "", body: String, status: Status = .active, pinned: Bool = false,
                    origin: String? = nil, createdAt: Date = .now, updatedAt: Date = .now, recallCount: Int = 0,
                    surfacedCount: Int = 0, lastRecalledAt: Date? = nil, mergedInto: String? = nil,
                    sources: [MemorySource] = [], links: [MemoryLink] = [], taskState: TaskState? = nil, steps: [TaskStep] = []) {
            self.id = id
            self.scope = scope
            self.kind = kind
            self.title = title
            self.description = description
            self.body = body
            self.status = status
            self.pinned = pinned
            self.origin = origin
            self.createdAt = createdAt
            self.updatedAt = updatedAt
            self.recallCount = recallCount
            self.surfacedCount = surfacedCount
            self.lastRecalledAt = lastRecalledAt
            self.mergedInto = mergedInto
            self.sources = sources
            self.links = links
            self.taskState = taskState
            self.steps = steps
        }

        public var isTask: Bool { kind == MemoryKind.task.rawValue }
        public var stepsDone: Int { steps.count(where: \.done) }
        /// The first step not done yet.
        public var nextStep: String? { steps.first { !$0.done }?.text }
        /// Done steps as a share of all, or by state when it has none.
        public var progress: Double {
            guard !steps.isEmpty else { return taskState == .done ? 1 : 0 }
            return Double(stepsDone) / Double(steps.count)
        }

        /// The first eight characters of the ID: what agents and the index use. Tools take
        /// any unique prefix.
        public var shortID: String { String(id.prefix(8)).lowercased() }

        /// UTF-8 bytes of what it says.
        public var size: Int { title.utf8.count + description.utf8.count + body.utf8.count }
    }

    /// A transcript row a memory came from.
    public struct MemorySource: Sendable, Equatable, Hashable, Codable {
        public var threadID: String
        public var entryID: String
        public var quote: String?

        public init(threadID: String, entryID: String, quote: String? = nil) {
            self.threadID = threadID
            self.entryID = entryID
            self.quote = quote
        }
    }

    /// Where one of your asks stands.
    public enum TaskState: String, Sendable, Codable, CaseIterable {
        case todo, active, blocked, done

        public var label: String {
            switch self {
            case .todo: "To Do"
            case .active: "In Progress"
            case .blocked: "Blocked"
            case .done: "Done"
            }
        }
    }

    public struct TaskStep: Sendable, Equatable, Hashable, Codable {
        public var text: String
        public var done: Bool

        public init(_ text: String, done: Bool = false) {
            self.text = text
            self.done = done
        }
    }

    /// One memory pointing at another, with how they relate ("refines", "see also").
    public struct MemoryLink: Sendable, Equatable, Hashable, Codable {
        public var to: String
        public var relation: String?

        public init(to: String, relation: String? = nil) {
            self.to = to
            self.relation = relation
        }
    }

    /// Who did something to a memory, for its history and the activity log.
    public struct MemoryActor: Sendable, Equatable {
        /// `agent`, `user`, `importer`, `reconciler` or `app`.
        public var kind: String
        /// The thread it happened in.
        public var threadID: String?

        public init(_ kind: String, threadID: String? = nil) {
            self.kind = kind
            self.threadID = threadID
        }

        public static let user = MemoryActor("user")
        public static let app = MemoryActor("app")
    }

    /// A memory as it was at one save.
    public struct MemoryVersion: Sendable, Equatable, Identifiable {
        public var memoryID: String
        public var version: Int
        public var kind: String
        public var title: String
        public var description: String
        public var body: String
        public var status: Memory.Status
        public var savedAt: Date
        public var actor: String?
        public var threadID: String?
        public var taskState: TaskState?
        public var steps: [TaskStep] = []

        public var id: Int { version }
    }

    /// Something that happened in a workspace's memory: what the activity log lists and the
    /// analytics count.
    public struct MemoryEvent: Sendable, Equatable, Identifiable {
        public enum Kind: String, Sendable, CaseIterable {
            case created, updated, recalled, searched, injected, archived, restored, deleted, linked, unlinked, merged,
                 pinned, unpinned, imported, exported, progressed
        }

        public var id: Int
        public var scope: String
        public var memoryID: String?
        /// The memory's title then, so the log reads well after it's gone.
        public var title: String?
        public var kind: Kind
        public var actor: String?
        public var threadID: String?
        public var at: Date
        /// For a search: what was asked.
        public var query: String?
        /// For a search, how many it found; for a session, how many memories it was given.
        public var count: Int?
        public var detail: String?
    }

    /// A memory that matched a search.
    public struct MemoryHit: Sendable, Equatable {
        public var memory: Memory
        /// The match in context, with `[` `]` around the matched words.
        public var snippet: String
    }
}

/// The kinds of memory, after Claude Code's own: agents know them, and an import from it
/// keeps them.
public enum MemoryKind: String, CaseIterable, Sendable {
    case user, feedback, project, reference
    /// One of your asks and how far it's got: kept across sessions, so any thread can pick it up.
    case task

    public var label: String {
        switch self {
        case .user: "About You"
        case .feedback: "Feedback"
        case .project: "Project"
        case .reference: "Reference"
        case .task: "Task"
        }
    }

    public var detail: String {
        switch self {
        case .user: "Who you are: your role, expertise and preferences."
        case .feedback: "How you want agents to work: corrections and approaches you confirmed."
        case .project: "Ongoing work, goals and constraints the code doesn't record."
        case .reference: "Where to look things up: dashboards, docs, tickets."
        case .task: "Something you asked for, its steps and how far it's got, across sessions."
        }
    }

    /// An unknown kind (from a newer version, or an import) reads as a project memory.
    public static func of(_ raw: String) -> MemoryKind { MemoryKind(rawValue: raw) ?? .project }
}

extension HarnessDatabase {
    /// The scope string for a project's memories.
    public static func memoryScope(project: String) -> String { "project:\(project)" }
}
