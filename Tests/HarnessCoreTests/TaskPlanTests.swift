import Foundation
import Testing
@testable import HarnessCore

struct TaskPlanTests {
    /// The real capture (sanitized): three tasks created, each taken through in progress
    /// to completed, after a ToolSearch that loads the task tools.
    private func calls() throws -> [(id: String, name: String, input: String, result: String?)] {
        let url = try #require(Bundle.module.url(forResource: "task-plan", withExtension: "jsonl", subdirectory: "Fixtures"))
        var parser = ClaudeStreamParser()
        var calls: [(id: String, name: String, input: String, result: String?)] = []
        for line in try String(contentsOf: url, encoding: .utf8).split(separator: "\n") {
            for event in parser.parse(line: Data(line.utf8)) {
                switch event {
                case .assistantMessage(_, let content, nil):
                    for case .toolUse(let id, let name, let input) in content { calls.append((id, name, input, nil)) }
                case .toolResults(let results, nil):
                    for result in results {
                        if let index = calls.firstIndex(where: { $0.id == result.toolUseID }) { calls[index].result = result.content }
                    }
                default:
                    break
                }
            }
        }
        return calls
    }

    @Test func followsTheCapturedSession() throws {
        let calls = try calls()
        #expect(calls.filter { TaskPlan.isPlanTool(name: $0.name, inputJSON: $0.input) }.count == 10, "the ToolSearch, 3 creates and 6 updates")
        #expect(!calls.contains { $0.name == "Bash" && TaskPlan.isPlanTool(name: $0.name, inputJSON: $0.input) })

        var plan = TaskPlan()
        var seenWorking = false
        for call in calls {
            plan.apply(toolName: call.name, inputJSON: call.input, result: call.result, toolUseID: call.id)
            if plan.current?.id == "2" {
                seenWorking = true
                #expect(plan.completedCount == 1)
                #expect(plan.current?.activeForm == "Counting lines in notes2.txt")
            }
        }
        #expect(seenWorking)
        #expect(plan.items.map(\.id) == ["1", "2", "3"])
        #expect(plan.items.map(\.subject) == ["Count lines in notes1.txt", "Count lines in notes2.txt", "Add up the line counts"])
        #expect(plan.isFinished)
    }

    @Test func aTaskStandsInForItsIDUntilTheResult() {
        var plan = TaskPlan()
        plan.apply(toolName: "TaskCreate", inputJSON: #"{"subject":"Ship it"}"#, result: nil, toolUseID: "toolu_1")
        #expect(plan.items.map(\.id) == ["toolu_1"])
        var settled = TaskPlan()
        settled.apply(toolName: "TaskCreate", inputJSON: #"{"subject":"Ship it"}"#, result: "Task #7 created successfully: Ship it", toolUseID: "toolu_1")
        settled.apply(toolName: "TaskUpdate", inputJSON: #"{"taskId":7,"status":"in_progress"}"#, result: nil, toolUseID: "toolu_2")
        #expect(settled.current?.id == "7")
        settled.apply(toolName: "TaskUpdate", inputJSON: #"{"taskId":"7","status":"deleted"}"#, result: nil, toolUseID: "toolu_3")
        #expect(settled.items.isEmpty)
    }

    @Test func todoWriteReplacesTheList() {
        var plan = TaskPlan()
        plan.apply(toolName: "TodoWrite", inputJSON: #"{"todos":[{"content":"A","status":"completed","activeForm":"Doing A"},{"content":"B","status":"in_progress","activeForm":"Doing B"}]}"#, result: nil, toolUseID: "t1")
        #expect(plan.items.map(\.subject) == ["A", "B"])
        #expect(plan.current?.activeForm == "Doing B")
        plan.apply(toolName: "TodoWrite", inputJSON: #"{"todos":[{"content":"C","status":"pending"}]}"#, result: nil, toolUseID: "t2")
        #expect(plan.items.map(\.subject) == ["C"])
        #expect(!plan.isFinished)
    }

    @Test func unknownShapesAreIgnored() {
        var plan = TaskPlan()
        plan.apply(toolName: "TaskUpdate", inputJSON: #"{"taskId":"9","status":"teleported"}"#, result: nil, toolUseID: "t1")
        plan.apply(toolName: "TaskCreate", inputJSON: "{not json", result: nil, toolUseID: "t2")
        #expect(plan.items.isEmpty)
        #expect(!TaskPlan.isPlanTool(name: "ToolSearch", inputJSON: #"{"query":"select:Read,TaskCreate"}"#))
        #expect(!TaskPlan.isPlanTool(name: "ToolSearch", inputJSON: #"{"query":"task tools"}"#))
    }
}
