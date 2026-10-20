import Foundation

/// A question the agent asks you with its `AskUserQuestion` tool. It arrives as a
/// permission request; you answer by allowing it with your answers added to its input.
public struct AgentQuestion: Sendable, Equatable, Identifiable {
    public struct Option: Sendable, Equatable, Identifiable {
        public var id: String { label }
        public var label: String
        public var description: String?
    }

    public var id: String { question }
    public var question: String
    /// A short tag, like "Auth method".
    public var header: String?
    public var multiSelect: Bool
    public var options: [Option]

    public static let toolName = "AskUserQuestion"

    public static func parse(inputJSON: String) -> [AgentQuestion]? {
        guard let input = try? JSONSerialization.jsonObject(with: Data(inputJSON.utf8)) as? [String: Any],
              let questions = input["questions"] as? [[String: Any]] else { return nil }
        let parsed = questions.compactMap { object -> AgentQuestion? in
            guard let question = object["question"] as? String else { return nil }
            let options = (object["options"] as? [[String: Any]] ?? []).compactMap { option -> Option? in
                guard let label = option["label"] as? String else { return nil }
                return Option(label: label, description: option["description"] as? String)
            }
            return AgentQuestion(question: question, header: object["header"] as? String,
                                 multiSelect: object["multiSelect"] as? Bool ?? false, options: options)
        }
        return parsed.isEmpty ? nil : parsed
    }

    /// The tool input with `answers` added, keyed by question text. Several choices are
    /// joined with ", ", as the terminal does.
    public static func answeredInput(inputJSON: String, answers: [String: [String]]) -> String {
        var input = (try? JSONSerialization.jsonObject(with: Data(inputJSON.utf8)) as? [String: Any]) ?? [:]
        input["answers"] = answers.mapValues { $0.joined(separator: ", ") }
        return ClaudeStreamParser.jsonString(input)
    }
}

/// How a permission request is settled.
public enum PermissionDecision: Sendable, Equatable {
    /// Run the tool, optionally with changed input (answers to questions), and optionally
    /// switch permission mode for the rest of the session (approving a plan into Auto).
    case allow(updatedInputJSON: String? = nil, mode: String? = nil)
    /// Don't run it; the message goes back to the agent (feedback on a plan).
    case deny(message: String)
}

/// The agent's plan, waiting for approval through its `ExitPlanMode` tool.
public enum AgentPlan {
    public static let toolName = "ExitPlanMode"

    public static func text(inputJSON: String) -> String? {
        guard let input = try? JSONSerialization.jsonObject(with: Data(inputJSON.utf8)) as? [String: Any] else { return nil }
        return (input["plan"] as? String).flatMap { $0.isEmpty ? nil : $0 }
    }
}
