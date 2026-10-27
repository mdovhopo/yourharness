# YourHarness

A fast, native macOS home for AI coding agents. Bring your own subscription: YourHarness drives the official CLI you're already logged into (Claude Code first, others later) and builds a better day-to-day experience on top. It doesn't touch your credentials.

**Goals:** low memory and instant UI, good thread management, visible usage limits, fully local projects, and memory as a first-class feature.

## Status

An early spike. What works today:

- Projects (local folders) and multiple threads per project, persisted across launches. Threads can be pinned, settled (moved to a per-project Settled shelf with their session stopped and history kept, automatically after 3 idle days by default, as in T3 Code), or deleted; the thread menu also copies the Claude session ID (for `claude --resume`) and the thread ID
- Resume a Claude Code session started elsewhere (File → Resume Claude Code Session…, ⇧⌘O): paste its ID or a `claude --resume` command, preview it, and it opens as a thread with its history (the live branch, across rewinds and compactions) that continues with `--resume` in the session's folder. Sessions are read from `~/.claude/projects` (or `$CLAUDE_CONFIG_DIR`), never written
- Generated thread titles, following T3 Code: a thread is named after its first message right away, then retitled in the background by a one-shot request with no tools, in an empty folder (Claude Code on Haiku by default; provider and model in Settings → General → Thread Titles, or off). Linked GitHub PRs and issues are read for context. Regenerate a title from the whole conversation, or rename inline, from the thread menu. When the subject changes, the thread's agent calls `mcp__threads__update_title` (with a hint; always loaded, never deferred behind tool search) and the same title model rewrites the title from the conversation; a title you chose (rename, /rename, or one asked of Claude) stays. Settings → Thread Titles turns that off, and its Title Prompt replaces the built-in title rules with yours (the answer format, previous title and conversation are still added)
- Streaming Claude Code sessions: markdown (headings, nested and task lists, tables, code, quotes), thinking, tool calls with inline Edit diffs. Replies are revealed at a steady, word-by-word pace instead of in network bursts, and half-typed syntax never flashes. `#12`, `repo#12` and `owner/repo#12` link to the right PR or issue using the project's repos and open PRs
- Native permission prompts (Allow ⌘↩ / Deny ⌘⌫) via the CLI's `can_use_tool` control protocol
- Questions and plans get their own cards: when Claude asks (`AskUserQuestion`), pick options (single or multi-select) or write your own answer, or Skip; when it presents a plan (`ExitPlanMode`), read it as markdown and approve it into Auto, Accept Edits or Ask (that becomes the project's mode), or keep planning with feedback
- Slash commands and skills: type "/" in the composer for a menu of everything Claude Code offers in that project (built-ins, your commands and skills, with descriptions and argument hints). `/compact` and `/clear` show what happened (tokens before and after, a fresh start), outputs like `/context` and `/usage` render as cards, `/rename` renames the thread, and `/model` and `/effort` go through the composer's own pickers
- `@` mentions: type `@` in the composer for the project's files and folders (what git sees, nothing ignored, plus repos cloned inside a workspace; a folder walk elsewhere), ranked by name, then path, then letters in order. The mention goes in as `@path` (quoted with spaces), which Claude Code expands into the file. Listed in the background and cached for 30 seconds. Not offered for a Home project that is your whole home folder
- `/btw <question>` asks on the side: the right sidebar's Side Chat answers from a copy of the thread (a forked session with no edit tools, no MCP, nothing saved) that reads freely and asks before running commands or fetching from the web, whatever the project's mode, so the thread isn't interrupted and nothing is added to it. Follow-ups continue there. The sidebar's tabs (Side Chat, Sub-agents) and New Side Chat sit in its part of the window toolbar
- Sub-agents: when Claude hands work to a sub-agent (the Agent tool), its call becomes a live card with what it's doing right now, its latest steps, tool count and time, then its report. The right sidebar (⌥⌘0, or the card's sidebar button) follows one step by step and can stop it. Background sub-agents keep the thread marked as working after the turn ends, and their history is saved with the thread
- Images in messages: paste screenshots (⌘V), drop image files on a thread, or use the paperclip. They're scaled to the API's limits (2000 px, under 5 MB), shown in the message, and kept in the data folder
- Messages while Claude works, as in Claude Code: ↩ steers (the message waits above the composer until Claude finishes its current step, then joins the turn, marked Steered), ⌘↩ interrupts and sends right away (marked as such). A queued message can still be sent now, edited or removed (the CLI's message lifecycle and `cancel_async_message`)
- Stop (⌘.) mid-turn without killing the session; anything still queued comes back to the composer
- Composer with model, reasoning-effort and permission pickers (Auto by default: the CLI's classifier runs what looks safe and asks about the rest), plus a context-window ring. Models and effort levels come live from the CLI's `initialize` catalog. Model and permission changes apply instantly mid-session
- Find in a thread (⌘F, then ⌘G/⇧⌘G or ↩/⇧↩, Esc to close): a glass find bar over the transcript. It searches messages, notices, command output and tool inputs off the main thread, steps message by message from the newest, and highlights every match (the current message's more strongly)
- Plan checklist: Claude Code's task tools (`TaskCreate`/`TaskUpdate`, or the older `TodoWrite`) are turned on for current models (`CLAUDE_CODE_ENABLE_TODO_TOOLS`, Settings → General → Plans) and replayed into a checklist. Each run of task calls shows as one plan row (the newest as the full list), and a strip above the composer shows the current step and progress while Claude works
- A browser for agents ([docs/browser.md](docs/browser.md)): each thread has WebKit tabs in the right sidebar's Browser tab, which Claude drives through `mcp__browser__*` tools (open, navigate, snapshot, screenshot, click, type, press, hover, select, scroll, drag, wait_for, console, network, evaluate, resize, dialog, upload, close). The tools are an MCP server inside the app, reached over the CLI's control channel (no port, nothing else can call them) and pre-approved. Snapshots are an outline with refs, kept under 18 KB; clicks and keys are real `NSEvent`s that never take your keyboard focus; console and network are captured in the page and shown under it. Take Control pauses the agent's actions, the element picker puts what you point at in the composer, and pages keep running off screen while the agent works. Tabs are saved with the thread, cookies per project. Dev servers come from what the thread's commands print and from `lsof` on the processes under its `claude`; localhost links in the thread open in its browser (⌘-click: default browser); Open in Window gives the browser its own window; pages unused and unseen for 30 minutes are closed and reopen on next use. Import Cookies… (the browser's More menu) copies the sites you pick from Safari (`Cookies.binarycookies`, needs Full Disk Access) or a Chromium profile (Chrome, Arc, Brave, Edge, Vivaldi: the `Cookies` database, decrypted with the key from the browser's "Safe Storage" keychain item, which macOS asks you about) into the project's cookie store. Settings → General → Browser
- Notifications for threads you aren't looking at: a turn finished while the app is in the background, or Claude waiting for your approval, answer or plan review (any time). Clicking one opens its thread; the Dock icon counts threads waiting for you. Permission is asked the first time there's something to say. Settings → General → Notifications
- A Home project, so you can start a thread without picking a folder
- Settings (⌘,): appearance (System, Light or Dark; System by default), agent discovery (Claude Code, plus Codex, Cursor, Gemini and OpenCode detection), account and plan from `claude auth status`, custom executable paths, and defaults for new threads
- 5-hour and weekly usage limits from the CLI's `rate_limit_event`, with an on-demand refresh that sends one tiny Haiku request (no tools, MCP, settings or saved session)
- Idle `claude` processes are reaped after 10 minutes and transparently `--resume`d
- Everything is kept in one SQLite database in Application Support (`harness.sqlite`, see [docs/storage.md](docs/storage.md)): saves write only what changed, big tool output is stored once and compressed, and every transcript is indexed for full-text search in the background. The JSON files of 0.0.6 and earlier move in on first launch and are left in place
- Nothing is lost to a quit or crash: your message is saved when sent and the turn every couple of seconds while it runs (written off the main thread). Quitting stops each `claude` process and marks the turn as stopped. A thread still marked mid-turn at launch was cut off by a crash, and opening it fills in what the CLI recorded meanwhile from the session file (lined up by tool call IDs and your messages). The same fills in work done in `claude --resume` elsewhere. Drafts are saved too
- Native diagrams: ```` ```mermaid ```` flowcharts and sequence diagrams are parsed, laid out and drawn in Swift (no web view), with source, copy, PNG export and zoom. Agents are told they can use them via `--append-system-prompt` (Settings → General → Diagrams)
- Worktrees, tracked rather than assigned: the repos view lists each repo's linked worktrees (`git worktree list`, so wherever they live; ones nested in the project fold under their repo) with branch or "Not on a branch", changes, sync, open PR, locked and missing states, and "done with" when the remote branch was deleted or the branch's own commits are in the default branch (a branch still on the default branch's first-parent line has no work yet). Threads are linked to every checkout they touched, from the folders in their tool calls (file paths, search paths, paths in commands, `EnterWorktree`), saved with the thread and worked out once for older ones. A thread's toolbar lists its checkouts; New Worktree…, New Thread Here, Run This Thread Here, Remove and Forget Missing Worktrees act on them
- Workspaces: any project that is a git repo, a folder of repos, or a "shell" repo holding gitignored clones gets an overview (click the project name). It shows each repo's branch, ahead/behind and uncommitted changes, repos the workspace references but you haven't cloned (from its compose file), and open pull requests from GitHub (`gh`) and GitLab (`glab`): what needs your review, yours, everyone else's, and stacks. Read-only; see [docs/workspaces.md](docs/workspaces.md)
- Memory ([docs/memory.md](docs/memory.md)), per project: agents save what's worth knowing next time (who you are, how you like to work, decisions, where things live) through `mcp__memory__*` tools the app serves, and every thread there starts with an index of it (a line a memory, about 2,000 tokens at most; pinned ones in full) and searches and reads the rest. Claude Code's own memory files are turned off so there's one memory. The Memory view (⇧⌘M) lists, searches and edits memories with their links, history (restore any version) and where they were learned; Insights says in plain words how memory is used (cost per session, what's read, never read, duplicated, searched for and missing) with charts; Activity logs every save, read, search and session; Graph draws the links. Export as JSON or Markdown (Claude Code's format), import either back, or Claude Code's own memory, directly; the Importer (an agent in its own window) turns anything else into memories, and the Reconciler merges duplicates, fixes stale or vague memories and links related ones, asking first (each on the model you pick). Your multi-step asks are kept as tasks (steps, what's next, blocked and why), so every thread starts knowing where they stand; a thread's toolbar shows its tasks. A project's name opens its page: Pull Requests | Memory
- Threads for agents: `mcp__threads__*` tools the app serves to every thread (pre-approved; side chats don't get them) list, read and search the user's threads, rename, settle and pin them, and list and import Claude Code sessions (`~/.claude/projects`) as threads that continue them, without taking you there. Nothing can be deleted. A thread in Home reaches every project; a thread anywhere else only its own project's threads, and only sessions that ran in its folder. Its instructions keep changes to what you ask for, and have it list and ask before changing more than two threads.
- Export and import the whole state (File → Export State… / Import State…, or Settings → General → Data): one `.yhstate` file (a zip) with a snapshot of the database (projects, threads, transcripts, memories and their history, settings), images, macOS's defaults for the app (window sizes and places), browser cookies only if you tick the box, and a versioned `manifest.json` saying what made it (app version, build, commit), on what Mac and macOS, with which CLIs. For reproducing a bug with someone's exact state, or moving to another Mac. Import shows what's in the file next to what you have, warns that it replaces everything and can't be undone, saves a copy of the current state first (unless you untick it), and restarts to swap it in; git worktrees and project folders are never touched. Agents have no tool for it. See [docs/storage.md](docs/storage.md#export-and-import).
- Search everything (⌘K): a Spotlight-style panel over threads by title, what was said in them (the full-text index), memories and tasks in every project, projects and actions; a conversation hit opens the thread with Find on the matched word
- Live app memory readout; Debug menu with a 10,000-entry stress transcript

## Install

Download the latest DMG from [Releases](https://github.com/mdovhopo/yourharness/releases/latest), open it and drag YourHarness to Applications. The app isn't notarized yet, so the first time you open it macOS refuses: go to System Settings → Privacy & Security and click **Open Anyway**, or run `xattr -dr com.apple.quarantine /Applications/YourHarness.app` once. What's in each version: [CHANGELOG.md](CHANGELOG.md).

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

`scripts/install.sh` builds and installs the app in /Applications as your main copy (your threads and settings carry over).

## Releasing

Versions follow [semver](https://semver.org) and live in `VERSION`; the build number counts commits. Add notes under **Unreleased** in CHANGELOG.md as features land, then:

```sh
scripts/release.sh 0.0.2     # files the notes under 0.0.2, tests, tags v0.0.2, pushes, installs in /Applications
```

The pushed tag runs `.github/workflows/release.yml`, which tests, builds `YourHarness-<version>.dmg` (`scripts/dmg.sh`) and publishes a GitHub release with that version's notes. Run the workflow by hand for a DMG of any commit.

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
    MCP/                  in-app MCP tool servers (JSON-RPC), served over the CLI's control channel
    Storage/              the SQLite database: state, transcripts, blobs, change feed, full-text index
    Memory/               memories in the database, their analytics, the prompt index, the memory tools, export/import
  YourHarness/            SwiftUI app
    Model/                AppModel, ThreadModel (event reducer), Store (state and transcripts to the database)
    Browser/              the agent's browser: WebKit tabs, injected scripts, the browser tools
    Views/                sidebar, thread, composer, tool calls, markdown
```

The Claude adapter runs `claude -p --input-format stream-json --output-format stream-json --include-partial-messages --permission-prompt-tool stdio`. A single process serves the whole thread: user messages, interrupts and permission decisions are written as JSON lines to stdin.

## Known gaps / next up

See [ROADMAP.md](ROADMAP.md) for the bigger picture. Smaller gaps in what exists today:

- Text selection across transcript blocks (SwiftUI limitation; an AppKit/TextKit 2 transcript would fix it if the stress test shows we need it anyway)
- Sub-agents in resumed Claude Code sessions show their report but not their steps (those live in separate session files)
- Git worktree per thread
- More adapters (Codex app-server, ACP)
