import Foundation

/// Which CLI writes thread titles, and with what model.
public struct TitleGenerationSettings: Sendable, Equatable, Codable {
    public enum Provider: String, Sendable, Codable, CaseIterable, Identifiable {
        case claude, codex

        public var id: String { rawValue }

        public var cli: AgentCLI {
            switch self {
            case .claude: .claude
            case .codex: .codex
            }
        }

        /// Cheap and fast; a title is a few words. Codex's matches T3 Code's choice.
        public var defaultModel: String {
            switch self {
            case .claude: "haiku"
            case .codex: "gpt-6-luna"
            }
        }
    }

    /// Nil keeps the first words of your first message as the title.
    public var provider: Provider?
    /// Nil uses the provider's default.
    public var model: String?

    public init(provider: Provider? = .claude, model: String? = nil) {
        self.provider = provider
        self.model = model
    }

    public var effectiveModel: String? { model ?? provider?.defaultModel }
}

public struct GeneratedTitle: Sendable, Equatable {
    public var title: String
    /// The first message didn't say what the thread is about ("fix this"); worth another
    /// try once the conversation has more to go on.
    public var needsRefinement: Bool
}

public struct TitleGenerationError: Error, Equatable, CustomStringConvertible {
    public var description: String
}

/// Runs a one-shot, tool-less request through the official CLI to name a thread. Each run
/// starts in an empty temporary folder, so the model sees only the prompt.
public enum TitleGenerator {
    public static let timeout: TimeInterval = 180

    public static func generate(
        prompt: String,
        provider: TitleGenerationSettings.Provider,
        model: String,
        executable: URL,
        environment: [String: String]
    ) async -> Result<GeneratedTitle, TitleGenerationError> {
        let folder = FileManager.default.temporaryDirectory
            .appendingPathComponent("yourharness-title-\(UUID().uuidString)", isDirectory: true)
        do {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        } catch {
            return .failure(TitleGenerationError(description: "Couldn't create a working folder: \(error.localizedDescription)"))
        }
        defer { try? FileManager.default.removeItem(at: folder) }

        switch provider {
        case .claude:
            return await runClaude(prompt: prompt, model: model, executable: executable, environment: environment, folder: folder)
        case .codex:
            return await runCodex(prompt: prompt, model: model, executable: executable, environment: environment, folder: folder)
        }
    }

    // MARK: - Claude Code

    /// No tools, MCP servers, hooks, slash commands or saved session: just the prompt in,
    /// schema-shaped JSON out.
    static func claudeArguments(model: String) -> [String] {
        [
            "-p",
            "--output-format", "json",
            "--json-schema", ThreadTitlePrompt.outputSchema,
            "--model", model,
            "--settings", #"{"disableAllHooks":true}"#,
            "--tools", "",
            "--disable-slash-commands",
            "--strict-mcp-config",
            "--permission-mode", "dontAsk",
            "--no-session-persistence",
        ]
    }

    private static func runClaude(
        prompt: String, model: String, executable: URL, environment: [String: String], folder: URL
    ) async -> Result<GeneratedTitle, TitleGenerationError> {
        let output = await CommandRunner.run(
            executable, arguments: claudeArguments(model: model), environment: environment,
            timeout: timeout, outputLimit: 1024 * 1024, input: prompt, currentDirectory: folder
        )
        if output.timedOut { return .failure(TitleGenerationError(description: "Claude didn't answer in time.")) }
        if output.launchFailed { return .failure(TitleGenerationError(description: output.stderr)) }
        return parseClaude(stdout: output.stdout, stderr: output.stderr, status: output.status)
    }

    /// `--output-format json` prints one result object (older CLIs: an array of messages
    /// ending in one). A failure says why in that result.
    static func parseClaude(stdout: String, stderr: String, status: Int32) -> Result<GeneratedTitle, TitleGenerationError> {
        let json = try? JSONSerialization.jsonObject(with: Data(stdout.utf8))
        let messages = (json as? [[String: Any]]) ?? [json as? [String: Any]].compactMap { $0 }
        let result = messages.last { $0["type"] as? String == "result" } ?? messages.last

        if let result, result["is_error"] as? Bool == true || status != 0 {
            let detail = (result["result"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
            return .failure(TitleGenerationError(description: detail.flatMap { $0.isEmpty ? nil : $0 } ?? "Claude exited with code \(status)."))
        }
        guard status == 0 else {
            let detail = CLIDiscovery.firstLine(stderr) ?? CLIDiscovery.firstLine(stdout)
            return .failure(TitleGenerationError(description: detail ?? "Claude exited with code \(status)."))
        }
        if let structured = result?["structured_output"] as? [String: Any], let title = decode(structured) {
            return .success(title)
        }
        // Some versions only put the JSON in the text result.
        if let text = result?["result"] as? String,
           let object = try? JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any],
           let title = decode(object) {
            return .success(title)
        }
        return .failure(TitleGenerationError(description: "Claude returned an unexpected answer."))
    }

    // MARK: - Codex

    static func codexArguments(model: String, schemaPath: String, outputPath: String, mcpServersOff: String?) -> [String] {
        var arguments = [
            "exec",
            // Plugins and apps bring MCP servers of their own; the user's are turned off by name.
            "--config", "features.plugins=false",
            "--config", "features.apps=false",
        ]
        if let mcpServersOff { arguments += ["--config", mcpServersOff] }
        arguments += [
            "--ephemeral",
            "--skip-git-repo-check",
            "-s", "read-only",
            "--model", model,
            "--config", #"model_reasoning_effort="low""#,
            "--output-schema", schemaPath,
            "--output-last-message", outputPath,
            "-",
        ]
        return arguments
    }

    /// `mcp_servers={"name"={enabled=false},...}` for every server the user has on.
    static func codexMCPServersOff(listJSON: String) -> String? {
        guard let servers = try? JSONSerialization.jsonObject(with: Data(listJSON.utf8)) as? [[String: Any]] else { return nil }
        let names = servers.filter { $0["enabled"] as? Bool ?? true }.compactMap { $0["name"] as? String }
        guard !names.isEmpty else { return nil }
        let entries = names.map { name -> String in
            let quoted = (try? JSONSerialization.data(withJSONObject: [name]))
                .flatMap { String(data: $0, encoding: .utf8) }
                .map { String($0.dropFirst().dropLast()) } ?? "\"\(name)\""
            return "\(quoted)={enabled=false}"
        }
        return "mcp_servers={\(entries.joined(separator: ","))}"
    }

    private static func runCodex(
        prompt: String, model: String, executable: URL, environment: [String: String], folder: URL
    ) async -> Result<GeneratedTitle, TitleGenerationError> {
        let schemaURL = folder.appendingPathComponent("schema.json")
        let outputURL = folder.appendingPathComponent("output.json")
        do {
            try Data(ThreadTitlePrompt.outputSchema.utf8).write(to: schemaURL)
        } catch {
            return .failure(TitleGenerationError(description: "Couldn't write the output schema."))
        }
        let servers = await CommandRunner.run(
            executable, arguments: ["mcp", "list", "--json"], environment: environment,
            timeout: 10, currentDirectory: folder
        )
        let arguments = codexArguments(
            model: model, schemaPath: schemaURL.path, outputPath: outputURL.path,
            mcpServersOff: servers.succeeded ? codexMCPServersOff(listJSON: servers.stdout) : nil
        )
        let output = await CommandRunner.run(
            executable, arguments: arguments, environment: environment,
            timeout: timeout, input: prompt, currentDirectory: folder
        )
        if output.timedOut { return .failure(TitleGenerationError(description: "Codex didn't answer in time.")) }
        guard output.succeeded else {
            let detail = CLIDiscovery.firstLine(output.stderr) ?? CLIDiscovery.firstLine(output.stdout)
            return .failure(TitleGenerationError(description: detail ?? "Codex exited with code \(output.status)."))
        }
        guard let data = try? Data(contentsOf: outputURL),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let title = decode(object) else {
            return .failure(TitleGenerationError(description: "Codex returned an unexpected answer."))
        }
        return .success(title)
    }

    // MARK: -

    private static func decode(_ object: [String: Any]) -> GeneratedTitle? {
        guard let title = object["title"] as? String else { return nil }
        return GeneratedTitle(
            title: ThreadTitlePrompt.sanitize(title),
            needsRefinement: object["needsRefinement"] as? Bool ?? false
        )
    }
}
