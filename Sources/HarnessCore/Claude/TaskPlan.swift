import Foundation

/// The checklist Claude keeps while it works, rebuilt from its task tool calls in order:
/// `TaskCreate` and `TaskUpdate` (Claude Code's task tools), or `TodoWrite` (the older
/// tool, which replaces the whole list each time).
public struct TaskPlan: Equatable, Sendable {
    public enum Status: String, Sendable {
        case pending
        case inProgress = "in_progress"
        case completed
    }

    public struct Item: Equatable, Sendable, Identifiable {
        public var id: String
        public var subject: String
        /// "Running the tests" for "Run the tests": how it reads while it's underway.
        public var activeForm: String?
        public var status: Status
    }

    public private(set) var items: [Item] = []

    public init() {}

    /// The calls that change or read the plan; the transcript shows them as the plan.
    public static let toolNames: Set<String> = ["TaskCreate", "TaskUpdate", "TaskList", "TaskGet", "TodoWrite"]

    /// Loading the task tools (they're deferred) belongs to the plan too.
    public static func isPlanTool(name: String, inputJSON: String) -> Bool {
        if toolNames.contains(name) { return true }
        guard name == "ToolSearch", let query = object(inputJSON)?["query"] as? String, query.hasPrefix("select:") else { return false }
        let names = query.dropFirst("select:".count).split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
        return !names.isEmpty && names.allSatisfy(toolNames.contains)
    }

    public var completedCount: Int { items.count { $0.status == .completed } }
    public var isFinished: Bool { !items.isEmpty && items.allSatisfy { $0.status == .completed } }
    /// What Claude is on now.
    public var current: Item? { items.first { $0.status == .inProgress } }

    /// `result` is the call's output, which is where a new task's ID comes from.
    public mutating func apply(toolName: String, inputJSON: String, result: String?, toolUseID: String) {
        guard let input = Self.object(inputJSON) else { return }
        switch toolName {
        case "TaskCreate":
            guard let subject = input["subject"] as? String else { return }
            // Its ID arrives with the result; until then the call stands in for it.
            let id = result.flatMap(Self.createdID) ?? toolUseID
            guard !items.contains(where: { $0.id == id }) else { return }
            items.append(Item(id: id, subject: subject, activeForm: input["activeForm"] as? String, status: .pending))
        case "TaskUpdate":
            guard let id = Self.string(input["taskId"]) else { return }
            let index = items.firstIndex { $0.id == id }
            if input["status"] as? String == "deleted" {
                if let index { items.remove(at: index) }
                return
            }
            let status = (input["status"] as? String).flatMap(Status.init(rawValue:))
            if let index {
                if let status { items[index].status = status }
                if let subject = input["subject"] as? String { items[index].subject = subject }
                if let activeForm = input["activeForm"] as? String { items[index].activeForm = activeForm }
            } else if let subject = input["subject"] as? String {
                // Made before this conversation began (task lists can outlive a session).
                items.append(Item(id: id, subject: subject, activeForm: input["activeForm"] as? String, status: status ?? .pending))
            }
        case "TodoWrite":
            guard let todos = input["todos"] as? [[String: Any]] else { return }
            items = todos.enumerated().compactMap { index, todo in
                guard let content = todo["content"] as? String else { return nil }
                return Item(id: Self.string(todo["id"]) ?? "\(index)", subject: content, activeForm: todo["activeForm"] as? String,
                            status: (todo["status"] as? String).flatMap(Status.init(rawValue:)) ?? .pending)
            }
        default:
            break
        }
    }

    /// "Task #12 created successfully: …" → "12".
    private static func createdID(_ result: String) -> String? {
        guard let range = result.range(of: #"(?<=Task #)[^\s:]+"#, options: .regularExpression) else { return nil }
        return String(result[range])
    }

    private static func object(_ json: String) -> [String: Any]? {
        (try? JSONSerialization.jsonObject(with: Data(json.utf8))) as? [String: Any]
    }

    private static func string(_ value: Any?) -> String? {
        switch value {
        case let string as String: string
        case let number as NSNumber: number.stringValue
        default: nil
        }
    }
}
