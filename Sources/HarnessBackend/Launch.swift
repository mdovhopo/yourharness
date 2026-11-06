import Foundation
import HarnessCore

/// What a thread's `claude` is started with beyond its folder and model: the app's own tools
/// (served from here, over the CLI's control channel, nothing else can call them) and what the
/// agent is told about them, as the frozen app did.
extension Backend {
    func prepareLaunch(_ options: inout ClaudeLaunchOptions, thread th: String, projectID: String) async -> [any MCPToolServer] {
        var servers: [any MCPToolServer] = []
        var prompt: [String] = []
        // Claude Code's task tools: what the plan checklist shows. They load on demand.
        if library.setting("planChecklist") as? Bool ?? true { options.environment["CLAUDE_CODE_ENABLE_TODO_TOOLS"] = "1" }
        // Images you send are kept here: reading them doesn't ask.
        options.addDirectories.append(dataDirectory.appendingPathComponent("attachments").path)

        if library.setting("memoryEnabled") as? Bool ?? true {
            // One memory, the app's: Claude Code's own files would compete with it.
            if library.setting("claudeCodeMemoryOff") as? Bool ?? true { options.environment["CLAUDE_CODE_DISABLE_AUTO_MEMORY"] = "1" }
            let scope = HarnessDatabase.memoryScope(project: projectID)
            let actor = HarnessDatabase.MemoryActor("agent", threadID: th)
            let name = library.project(projectID)?["name"] as? String ?? "this project"
            let database = library.db
            // Memory tools touch only the app's database: they don't ask.
            options.allowedTools.append("mcp__memory")
            servers.append(MemoryToolServer(database: database, scope: scope, access: .agent, actor: actor) { [weak self] scope in
                Task { @MainActor in self?.sink(["t": "memoryChanged", "scope": scope]) }
            })
            // The index of what's remembered, built off the main thread; each launch is counted.
            let text = await Task.detached(priority: .userInitiated) {
                let built = MemoryPrompt.build(memories: database.memories(scope: scope, status: .active), workspace: name)
                database.noteMemoryInjection(scope: scope, count: built.listed, bytes: built.bytes, actor: actor)
                return built.text
            }.value
            prompt.append(text)
        }
        if let extra = launchExtras?(th) {
            servers += extra.servers
            options.allowedTools += extra.allowedTools
            prompt += extra.prompt
        }
        if !prompt.isEmpty {
            options.appendSystemPrompt = ([options.appendSystemPrompt].compactMap(\.self) + prompt).joined(separator: "\n\n")
        }
        return servers
    }
}

/// More tools for a thread's launch, from parts of the backend that serve their own.
public struct LaunchExtras {
    public var servers: [any MCPToolServer] = []
    public var allowedTools: [String] = []
    public var prompt: [String] = []
    public init(servers: [any MCPToolServer] = [], allowedTools: [String] = [], prompt: [String] = []) {
        self.servers = servers
        self.allowedTools = allowedTools
        self.prompt = prompt
    }
}
