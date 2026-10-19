# YourHarness

A fast, native macOS home for AI coding agents. Bring your own subscription: YourHarness drives the official CLI you're already logged into (Claude Code first, others later) and builds a better day-to-day experience on top. It doesn't touch your credentials.

**Goals:** low memory and instant UI, good thread management, visible usage limits, fully local projects, and memory as a first-class feature.

## Status

An early spike. What works today:

- Projects (local folders) and multiple threads per project, persisted across launches
- Streaming Claude Code sessions: markdown, code blocks, thinking, tool calls with inline Edit diffs
- Native permission prompts (Allow ⌘↩ / Deny ⌘⌫) via the CLI's `can_use_tool` control protocol
- Stop (⌘.) mid-turn without killing the session
- Composer with model, reasoning-effort and permission pickers, plus a context-window ring. Models and effort levels come live from the CLI's `initialize` catalog. Model and permission changes apply instantly mid-session
- A Home project, so you can start a thread without picking a folder
- Settings (⌘,): agent discovery (Claude Code, plus Codex, Cursor, Gemini and OpenCode detection), account and plan from `claude auth status`, custom executable paths, and defaults for new threads
- 5-hour and weekly usage limits from the CLI's `rate_limit_event`
- Idle `claude` processes are reaped after 10 minutes and transparently `--resume`d
- Workspaces: any project that is a git repo, a folder of repos, or a "shell" repo holding gitignored clones gets an overview (click the project name). It shows each repo's branch, ahead/behind and uncommitted changes, repos the workspace references but you haven't cloned (from its compose file), and open pull requests from GitHub (`gh`) and GitLab (`glab`): what needs your review, yours, everyone else's, and stacks. Read-only; see [docs/workspaces.md](docs/workspaces.md)
- Live app memory readout; Debug menu with a 10,000-entry stress transcript

## Requirements

- macOS 26+ on Apple Silicon
- Xcode 26+ (Swift 6.2+)
- [Claude Code](https://docs.claude.com/en/docs/claude-code) installed and logged in (`claude` on your `PATH`)

## Build & run

```sh
scripts/bundle.sh            # builds build/YourHarness.app (release, arm64)
open build/YourHarness.app

swift test                   # parser tests against real captured CLI output
YH_INTEGRATION=1 swift test --filter Integration   # drives the real claude CLI (Haiku)
```

If `swift build` fails with "Could not initialize build system", `xcode-select` is pointing at the Command Line Tools. Either run `sudo xcode-select -s /Applications/Xcode.app/Contents/Developer` or prefix commands with `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer`. `bundle.sh` does the latter automatically.

## Architecture

```
Sources/
  HarnessCore/            agent-agnostic core, no UI imports
    AgentEvent.swift      normalized event model every adapter emits
    Claude/
      ClaudeStreamParser  stream-json lines → AgentEvent (pure, fixture-tested)
      ClaudeSession       one long-lived `claude -p` process over stdin/stdout
    ShellEnvironment      login-shell env capture (GUI apps don't get your PATH)
    Git/                  read-only git status and remote parsing
    Workspace/            finds repos in a project folder (nested clones, compose build contexts)
    Forge/                open PRs/MRs via `gh`/`glab` GraphQL, stack detection
  YourHarness/            SwiftUI app
    Model/                AppModel, ThreadModel (event reducer), Store (JSON persistence)
    Views/                sidebar, thread, composer, tool calls, markdown
```

The Claude adapter runs `claude -p --input-format stream-json --output-format stream-json --include-partial-messages --permission-prompt-tool stdio`. A single process serves the whole thread: user messages, interrupts and permission decisions are written as JSON lines to stdin.

## Known gaps / next up

See [ROADMAP.md](ROADMAP.md) for the bigger picture. Smaller gaps in what exists today:

- Text selection across transcript blocks (SwiftUI limitation; an AppKit/TextKit 2 transcript would fix it if the stress test shows we need it anyway)
- Syntax highlighting in code blocks
- Subagent (Task) activity is hidden; it should nest under its tool call
- Git worktree per thread
- More adapters (Codex app-server, ACP)
