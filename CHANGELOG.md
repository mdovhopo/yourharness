# Changelog

What changed in each release of YourHarness. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org): while it's 0.x, a minor version can change anything, and patch versions are fixes and small additions.

Add notes under **Unreleased** as features land; `scripts/release.sh <version>` files them under the new version.

## [Unreleased]

### Added
- A PRs tab in the right sidebar, opened by default for a thread that has PRs: every pull request the thread is linked to, open ones first, with stacks drawn as a tree (a lane per fork, so a PR stacked on the same branch as another branches off visibly), then merged and closed ones. Each shows its state (open, draft, auto-merge, in the merge queue, merged, closed), checks, review, and what blocks merging (conflicts, behind base). It stays current: while it's showing, a few seconds after Claude runs `gh pr` or `git push`, and when a turn ends, so asking Claude to merge shows the PR merging. A PR found through a branch stays with the thread after it merges.
- A browser built in for Claude. Claude opens the app it's building in a Browser tab in the right sidebar, reads the page, clicks, types, presses keys, scrolls and takes screenshots, and every step shows in the thread. Each result tells it about new console errors and failed requests. You watch it happen, see the console and network under the page, can take control at any time, and can pick an element to put in your message with a picture of it. Tabs reopen where they were after a restart, each project keeps its own cookies, and the side chat can look at the thread's browser. Dev servers the thread started are offered in the empty browser, links to localhost in the thread open there (⌘-click for your default browser), the browser can open in its own window for a bigger view, and pages nobody has used for 30 minutes close to save memory, reopening when needed. Import Cookies… brings logins over from Safari, Chrome, Arc, Brave, Edge or Vivaldi, for the sites you pick. You can turn it off in Settings.
- Worktrees: the repos view lists each repo's worktrees, wherever they are on disk, with their branch (or a warning when one isn't on a branch), uncommitted changes, what's to push, their open PR, and the threads working in them. Worktrees whose work has landed (merged, or their remote branch deleted) are marked as done with, and missing ones as missing. New Worktree… creates one and can start a thread in it; Remove and Forget Missing clean up. A thread's toolbar shows the repos and worktrees it works in, however many, and next to them the pull requests it's linked to: open ones from the branches it works on, and any it mentions (merged and closed ones too, looked up through `gh`), and a thread can be moved to run in any of them. Repos are sorted by how many threads have worked in them. Removing a worktree is one click: one whose work has landed, that has no work yet, or whose folder is gone is removed right away; one with work in progress asks first, saying whether anything would be lost (uncommitted changes, commits on no branch) or kept on its branch. Worktrees start folded, each repo showing how many it has (with a dot when one needs a look); open them one repo at a time or all at once, and the choice is remembered. Big workspaces get a filter for repos, worktrees, branches and threads. Each repo and worktree shows its threads as one pill (how many are active and settled, or the title of a single active one) that lists them on click; a worktree only settled threads used looks quiet, since it's done with.
- Turns survive quitting and crashes. Your message is saved the moment you send it, the rest of the turn every couple of seconds, and quitting stops Claude and says so. After a crash, what Claude did meanwhile is recovered from Claude Code's session history.
- Work done on a thread's session outside YourHarness (`claude --resume` in a terminal) shows up when you open the thread.
- Unsent drafts are kept across launches.
- Plan checklist: for work with several steps, Claude lists them and ticks them off. The thread shows the plan where it changes, and a strip above the composer shows the current step while Claude works. You can turn it off in Settings.
- `@` mentions in the composer: type `@` and part of a name to pick a file or folder from the project, and Claude gets the file with your message.
- Find in a thread (⌘F): matches are highlighted in messages, code and tool calls, and ⌘G or ⇧⌘G steps between them, starting from the newest.
- Notifications: when a turn finishes while YourHarness is in the background, or Claude needs your approval, an answer or a plan review in a thread you aren't looking at. Clicking one opens the thread, and the Dock icon counts threads waiting for you. You can turn them off in Settings.

### Changed
- The usage limits in the sidebar show how long until each one resets ("2h 14m", "3d 4h"), next to the percentage.
- The right sidebar's tabs and actions moved into the window toolbar, so the panel is all content.
- The side chat can now run commands and look things up on the web. It asks you first for anything that could change something, and it still can't edit files.
- Permission cards stack their buttons under the request when space is narrow, as in the side chat.

## [0.0.1] - 2026-10-21

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
