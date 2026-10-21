# Changelog

What changed in each release of YourHarness. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org): while it's 0.x, a minor version can change anything, and patch versions are fixes and small additions.

Add notes under **Unreleased** as features land; `scripts/release.sh <version>` files them under the new version.

## [Unreleased]

The first release: a native macOS home for Claude Code that drives the official `claude` CLI you're already logged into. It never touches your credentials.

### Threads and projects
- Projects are local folders, each with any number of threads, kept across launches. A Home project lets you start without picking a folder.
- Threads can be pinned, settled (shelved with their history after a few idle days, or by hand) or deleted. The thread menu copies the Claude session ID and the thread ID.
- Titles are generated from the first message, with a provider and model setting, and can be regenerated or renamed inline.
- Resume any Claude Code session started elsewhere from its ID (⇧⌘O), with its full history.
- Idle `claude` processes are stopped after 10 minutes and resumed transparently.

### Conversations
- Streaming replies with rich markdown: headings, lists, task lists, tables, code and quotes. Replies are revealed at a steady pace, and half-typed syntax never flashes.
- Thinking, tool calls and inline diffs for edits.
- `#12` and `owner/repo#12` link to the right pull request or issue. Links show the pointing hand.
- Native Mermaid flowcharts and sequence diagrams, drawn in Swift, with zoom, copy and PNG export, plus a large diagram window.
- Images in messages: paste, drop or attach them. They're scaled to the API's limits.
- Messages sent while Claude works steer the turn (↩) or interrupt it (⌘↩), as in Claude Code. A queued message can still be sent now, edited or removed.
- Stop a turn (⌘.) without ending the session.

### Working with Claude Code
- Native cards for permission prompts (⌘↩ to allow, ⌘⌫ to deny), for questions, and for plans you approve into a permission mode.
- Slash commands and skills from a "/" menu, with descriptions and argument hints. `/compact`, `/clear`, `/context`, `/usage`, `/rename`, `/model` and `/effort` all have proper UI.
- `/btw` asks on the side: a read-only copy of the conversation answers in the right sidebar, without interrupting or changing the thread.
- Live sub-agents: each one gets a card showing what it's doing, its steps and its report. The right sidebar (⌥⌘0) follows a sub-agent step by step and can stop it.
- Model, reasoning-effort and permission pickers (Auto by default), plus a context-window ring. Changes apply mid-session.
- 5-hour and weekly usage limits, with an on-demand refresh.

### Workspaces
- An overview for projects that are git repos or folders of repos: each repo's branch, ahead/behind and uncommitted changes, repos you haven't cloned yet, and open pull requests from GitHub and GitLab, including reviews waiting on you and stacks.

### The app
- A single Liquid Glass design across the window, with System, Light and Dark appearances.
- Settings for agents, account and plan, executable paths, defaults for new threads, titles and diagrams.
- Low memory and a smooth transcript, even at 10,000 entries. Scrolling holds its place while history loads and replies stream.
