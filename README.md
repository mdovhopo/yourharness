# YourHarness

A fast, native macOS home for AI coding agents. Bring your own subscription: YourHarness drives the official CLI you're already logged into (Claude Code first, others later) and builds a better day-to-day experience on top. It doesn't touch your credentials.

**Goals:** low memory and instant UI, good thread management, visible usage limits, fully local projects, and memory as a first-class feature.

## Status

An early spike. What works today:

- Projects (local folders) and multiple threads per project, persisted across launches. Threads can be pinned, settled (moved to a per-project Settled shelf with their session stopped and history kept, automatically after 3 idle days by default, as in T3 Code), or deleted; the thread menu also copies the Claude session ID (for `claude --resume`) and the thread ID
- Resume a Claude Code session started elsewhere (File → Resume Claude Code Session…, ⇧⌘O): paste its ID or a `claude --resume` command, preview it, and it opens as a thread with its history (the live branch, across rewinds and compactions) that continues with `--resume` in the session's folder. Sessions are read from `~/.claude/projects` (or `$CLAUDE_CONFIG_DIR`), never written
- Generated thread titles, following T3 Code: a thread is named after its first message right away, then retitled in the background by a one-shot request with no tools, in an empty folder (Claude Code on Haiku by default; provider and model in Settings → General → Thread Titles, or off). Linked GitHub PRs and issues are read for context. Regenerate a title from the whole conversation, or rename inline, from the thread menu
- Streaming Claude Code sessions: markdown (headings, nested and task lists, tables, code, quotes), thinking, tool calls with inline Edit diffs. Replies are revealed at a steady, word-by-word pace instead of in network bursts, and half-typed syntax never flashes. `#12`, `repo#12` and `owner/repo#12` link to the right PR or issue using the project's repos and open PRs
- Native permission prompts (Allow ⌘↩ / Deny ⌘⌫) via the CLI's `can_use_tool` control protocol
- Questions and plans get their own cards: when Claude asks (`AskUserQuestion`), pick options (single or multi-select) or write your own answer, or Skip; when it presents a plan (`ExitPlanMode`), read it as markdown and approve it into Auto, Accept Edits or Ask (that becomes the project's mode), or keep planning with feedback
- Sub-agents: when Claude hands work to a sub-agent (the Agent tool), its call becomes a live card with what it's doing right now, its latest steps, tool count and time, then its report. The right sidebar (⌥⌘0, or the card's sidebar button) follows one step by step and can stop it. Background sub-agents keep the thread marked as working after the turn ends, and their history is saved with the thread
- Images in messages: paste screenshots (⌘V), drop image files on a thread, or use the paperclip. They're scaled to the API's limits (2000 px, under 5 MB), shown in the message, and kept in the data folder
- Stop (⌘.) mid-turn without killing the session
- Composer with model, reasoning-effort and permission pickers (Auto by default: the CLI's classifier runs what looks safe and asks about the rest), plus a context-window ring. Models and effort levels come live from the CLI's `initialize` catalog. Model and permission changes apply instantly mid-session
- A Home project, so you can start a thread without picking a folder
- Settings (⌘,): agent discovery (Claude Code, plus Codex, Cursor, Gemini and OpenCode detection), account and plan from `claude auth status`, custom executable paths, and defaults for new threads
- 5-hour and weekly usage limits from the CLI's `rate_limit_event`, with an on-demand refresh that sends one tiny Haiku request (no tools, MCP, settings or saved session)
- Idle `claude` processes are reaped after 10 minutes and transparently `--resume`d
- Native diagrams: ```` ```mermaid ```` flowcharts and sequence diagrams are parsed, laid out and drawn in Swift (no web view), with source, copy, PNG export and zoom. Agents are told they can use them via `--append-system-prompt` (Settings → General → Diagrams)
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
    Markdown/             block parser (streaming-aware), PR/issue reference linking
    Diagrams/             Mermaid flowchart + sequence parsers, layered graph layout, sequence layout
  YourHarness/            SwiftUI app
    Model/                AppModel, ThreadModel (event reducer), Store (JSON persistence)
    Views/                sidebar, thread, composer, tool calls, markdown
```

The Claude adapter runs `claude -p --input-format stream-json --output-format stream-json --include-partial-messages --permission-prompt-tool stdio`. A single process serves the whole thread: user messages, interrupts and permission decisions are written as JSON lines to stdin.

## Known gaps / next up

See [ROADMAP.md](ROADMAP.md) for the bigger picture. Smaller gaps in what exists today:

- Text selection across transcript blocks (SwiftUI limitation; an AppKit/TextKit 2 transcript would fix it if the stress test shows we need it anyway)
- Syntax highlighting in code blocks
- Sub-agents in resumed Claude Code sessions show their report but not their steps (those live in separate session files)
- Git worktree per thread
- More adapters (Codex app-server, ACP)
