import Foundation
import Testing
@testable import HarnessCore

/// The fixture is a real capture (sanitized): a custom command, a reply, `/rename`,
/// `/compact` and `/clear`, in one session.
struct SlashCommandParsingTests {
    private func events(fromFixture name: String) throws -> [AgentEvent] {
        let url = try #require(Bundle.module.url(forResource: name, withExtension: "jsonl", subdirectory: "Fixtures"))
        var parser = ClaudeStreamParser()
        return try String(contentsOf: url, encoding: .utf8)
            .split(separator: "\n")
            .flatMap { parser.parse(line: Data($0.utf8)) }
    }

    @Test func customCommandsRunAsTurns() throws {
        let events = try events(fromFixture: "slash-commands")
        let replies = events.compactMap { event -> String? in
            guard case .assistantMessage(_, let blocks, nil) = event else { return nil }
            let text = blocks.compactMap { if case .text(let text) = $0 { text } else { nil } }.joined()
            return text.isEmpty ? nil : text
        }
        #expect(replies.first == "Hello, Max!")
    }

    @Test func builtInCommandsPrintOutput() throws {
        let events = try events(fromFixture: "slash-commands")
        let outputs = events.compactMap { event -> String? in
            if case .commandOutput(let text) = event { return text }
            return nil
        }
        #expect(outputs.contains("Session renamed to: Greeting test"))
        #expect(events.contains(.titleChanged("Greeting test")))
    }

    @Test func compactingReportsTokensAndDropsTheReplay() throws {
        let events = try events(fromFixture: "slash-commands")
        let compaction = try #require(events.lazy.compactMap { event -> Compaction? in
            if case .compacted(let compaction) = event { return compaction }
            return nil
        }.first)
        #expect(compaction.trigger == "manual")
        #expect((compaction.tokensBefore ?? 0) > (compaction.tokensAfter ?? .max))
        #expect(events.contains(.status("compacting")))

        // The kept messages the CLI replays after the boundary are already on screen.
        let boundary = try #require(events.firstIndex { if case .compacted = $0 { true } else { false } })
        let nextResult = try #require(events[boundary...].firstIndex { if case .turnCompleted = $0 { true } else { false } })
        let replayed = events[boundary..<nextResult].filter {
            switch $0 {
            case .assistantMessage, .commandOutput: true
            default: false
            }
        }
        #expect(replayed.isEmpty)
    }

    @Test func clearStartsANewSession() throws {
        let events = try events(fromFixture: "slash-commands")
        let reset = try #require(events.firstIndex(of: .conversationReset))
        let sessions = events.compactMap { event -> String? in
            if case .sessionStarted(let info) = event { return info.sessionID }
            return nil
        }
        let after = events[reset...].compactMap { event -> String? in
            if case .sessionStarted(let info) = event { return info.sessionID }
            return nil
        }
        #expect(after.first != nil && after.first != sessions.first)
    }

    @Test func parsesCommandText() {
        #expect(SlashCommand.parse("/compact")! == ("compact", ""))
        #expect(SlashCommand.parse("/btw  what did we decide? ")! == ("btw", "what did we decide?"))
        #expect(SlashCommand.parse("/model\nhaiku")! == ("model", "haiku"))
        #expect(SlashCommand.parse("/") == nil)
        #expect(SlashCommand.parse("/usr/bin/env") == nil, "a path isn't a command")
        #expect(SlashCommand.parse("hello /there") == nil)
    }

    @Test func readsTheCommandCatalog() throws {
        var parser = ClaudeStreamParser()
        let line = #"{"type":"control_response","response":{"subtype":"success","request_id":"yh-1","response":{"models":[{"value":"haiku","displayName":"Haiku"}],"commands":[{"name":"compact","description":"Free up context","argumentHint":"<instructions>","builtin":true},{"name":"hello","description":"Say hello (project)","argumentHint":"<name>"},{"name":"clear","description":"New session","aliases":["reset","new"],"builtin":true}]}}}"#
        guard case .catalog(let catalog) = try #require(parser.parse(line: Data(line.utf8)).first) else {
            Issue.record("expected a catalog")
            return
        }
        #expect(catalog.commands.map(\.name) == ["compact", "hello", "clear"])
        #expect(catalog.commands[0].isBuiltin && !catalog.commands[1].isBuiltin)
        #expect(catalog.commands[1].argumentHint == "<name>")
        #expect(catalog.commands[2].aliases == ["reset", "new"])
    }
}

struct SideChatLaunchTests {
    @Test func aForkedReadOnlySessionLeavesNothingBehind() {
        var options = ClaudeLaunchOptions(executable: URL(fileURLWithPath: "/bin/claude"), workingDirectory: URL(fileURLWithPath: "/tmp"),
                                          environment: [:], resumeSessionID: "abc")
        options.forkSession = true
        options.tools = ["Read", "Glob", "Grep"]
        options.withoutMCP = true
        options.persistsSession = false
        let args = options.arguments
        #expect(args.contains("--fork-session"))
        #expect(args.firstIndex(of: "--resume").map { args[$0 + 1] } == "abc")
        #expect(args.firstIndex(of: "--tools").map { args[$0 + 1] } == "Read,Glob,Grep")
        #expect(args.contains("--strict-mcp-config"))
        #expect(args.contains("--no-session-persistence"))
        #expect(!ClaudeLaunchOptions(executable: URL(fileURLWithPath: "/bin/claude"), workingDirectory: URL(fileURLWithPath: "/tmp"),
                                     environment: [:]).arguments.contains("--fork-session"))
    }
}

struct ChainedCommandTests {
    @Test func findsTheCommandBeingTypedAnywhere() {
        #expect(SlashCommand.trailingCommand(in: "/rev")?.query == "rev")
        #expect(SlashCommand.trailingCommand(in: "fix the bug then /rev")?.query == "rev")
        #expect(SlashCommand.trailingCommand(in: "then /")?.query == "")
        #expect(SlashCommand.trailingCommand(in: "then /review ") == nil)
        #expect(SlashCommand.trailingCommand(in: "see src/main") == nil)
        #expect(SlashCommand.trailingCommand(in: "open /usr/bin") == nil)
        let text = "a /re"
        #expect(SlashCommand.trailingCommand(in: text).map { text[$0.start...] } == "/re")
    }

    @Test func listsSkillsChainedInAMessage() {
        let known: Set<String> = ["review", "commit", "deploy:staging", "plan"]
        #expect(SlashCommand.chainedCommands(in: "/plan the work, then /review and /commit.", known: known) == ["review", "commit"])
        #expect(SlashCommand.chainedCommands(in: "do it then /deploy:staging", known: known) == ["deploy:staging"])
        #expect(SlashCommand.chainedCommands(in: "run /review twice: /review", known: known) == ["review"])
        #expect(SlashCommand.chainedCommands(in: "look at /review/notes.md and src/commit", known: known).isEmpty)
        #expect(SlashCommand.chainedCommands(in: "unknown /nope here", known: known).isEmpty)
        #expect(SlashCommand.chainedCommands(in: "/review only", known: known).isEmpty)
        #expect(SlashCommand.chainedCommands(in: "then (/review)", known: known).isEmpty)
    }

    @Test func notesTheChain() {
        #expect(SlashCommand.chainNote(for: []) == nil)
        #expect(SlashCommand.chainNote(for: ["review"]) == "(Run the /review skill where it comes up above, with the Skill tool.)")
        #expect(SlashCommand.chainNote(for: ["review", "commit", "deploy"])
            == "(Run the /review, /commit and /deploy skills where they come up above, in that order, with the Skill tool.)")
    }
}
