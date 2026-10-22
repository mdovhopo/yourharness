# Changelog

What changed in each release of YourHarness. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org): while it's 0.x, a minor version can change anything, and patch versions are fixes and small additions.

Add notes under **Unreleased** as features land; `scripts/release.sh <version>` files them under the new version.

## [Unreleased]

### Added
- **Threads settle when their work lands.** Once every PR a thread is linked to has merged, and nothing new has happened in it since, it moves to the Settled shelf, the way idle threads do. One PR still open, or closed without merging, keeps it where it is. PRs are looked up in the background, so this happens for threads you aren't looking at too.
- **Undo for thread actions.** Settling, unpinning or deleting a thread shows Undo for a few seconds above the limits box, and ⌘Z (Edit → Undo) takes it back too. Deleting no longer asks first: the thread's files are only removed when YourHarness quits.
- **Claude's work folds away when a turn ends.** While Claude works you see every step; once it's done, the thinking, tool calls, sub-agents and what it said along the way fold into one line between your message and its answer: "Worked for 2m 14s · 9 tool calls · edited 3 files". Click it to see everything. Notices and messages you sent mid-turn stay in view.
- **How long a turn takes.** The working indicator counts up while Claude works, and each finished turn says how long it took.
- **Times on your messages,** and your messages stand out more: tinted and outlined, with a little more room above each turn.
- **Claude can show you images and videos.** When a screenshot, a screen recording, a simulator capture or a chart would say more than words, Claude puts it in the conversation: images open in Preview, videos play right there. The files are copied into the thread, so they stay after the originals are gone. It doesn't ask first, since it only shows you files.
- **Syntax highlighting in code blocks,** in light and dark, for Swift, TypeScript, JavaScript, Python, Go, Rust, Java, Kotlin, C, Ruby, shell, JSON, YAML, TOML, SQL, HTML, CSS, diffs and more.
- **Jump between your messages.** ⌘[ and ⌘] (Thread menu) or the small arrows at the bottom right of the transcript go to your previous and next message; past the last, to the end.
### Changed
- **YourHarness is signed with its own certificate** instead of ad hoc. macOS will ask once more for the folders and keychain items it used (it's a new identity), and from then on remembers your answers across updates. Updates are only installed when they're signed the same way.

### Fixed
- Removing a worktree is instant, even offline: it no longer rescans the project and asks GitHub for PRs before it's done.

## [0.0.3] - 2026-10-22

### Highlights
YourHarness now updates itself. From this version on, new releases show up in the app with their notes, download with a click, and install with a restart. No more downloading disk images by hand.

### Added
- **Updates in the app.** The version shows at the bottom of the limits box. When a new release is out, the box offers it with its notes (What's New), downloads it when you click Update, and checks it against the release's checksum. Restart then quits and opens the new version, after a confirmation that says which threads it would stop. YourHarness looks at launch and every six hours, and **Check for Updates…** is in the app menu. ([`dcb68b4`](https://github.com/mdovhopo/yourharness/commit/dcb68b4))
- **Release notes in the app.** What's New renders the notes from GitHub for every release since yours, each with a link to its page; after an update, the limits box says what you updated to and shows what's new. ([`9941d67`](https://github.com/mdovhopo/yourharness/commit/9941d67))
- **A Releases tab in Settings**, with the version, updates, and every release's notes, marked Installed and New. ([`bc19dd3`](https://github.com/mdovhopo/yourharness/commit/bc19dd3))

### Fixed
- Settings no longer shows a stray, unnamed tab: it was the Agents tab's refresh button, now **Detect Again** at the end of that tab. ([`bc19dd3`](https://github.com/mdovhopo/yourharness/commit/bc19dd3))
- The left sidebar no longer goes missing at launch when the right sidebar was left open. Its button also said it was showing, so the first click did nothing. ([`8e2eac7`](https://github.com/mdovhopo/yourharness/commit/8e2eac7))

## [0.0.2] - 2026-10-22

### Highlights
YourHarness now keeps track of the work around a conversation, not just the conversation: the pull requests a thread is linked to, live in a new PRs tab; the repos and worktrees it works in; and a real browser Claude drives to test what it builds. Turns also survive quitting and crashes.

### Added
- **Pull requests, first class.** A PRs tab leads the right sidebar and opens by default for a thread that has PRs: every PR the thread works on or mentions, open ones first, then merged and closed ones. ([`de29ec4`](https://github.com/mdovhopo/yourharness/commit/de29ec4))
  - Stacks are drawn as a tree: a line of PRs in one lane, each fork branching off visibly, every dot showing the PR's state and layer. ([`52722d6`](https://github.com/mdovhopo/yourharness/commit/52722d6), [`fee8a29`](https://github.com/mdovhopo/yourharness/commit/fee8a29))
  - Compact two-line rows: title with review, checks and age, then repo, number and branches; a chip only for states worth saying (draft, auto-merge, in the merge queue, merged, closed, conflicts, behind base). ([`8b646d1`](https://github.com/mdovhopo/yourharness/commit/8b646d1))
  - Kept current while you watch, a few seconds after Claude runs `gh pr` or `git push`, and when a turn ends, so asking Claude to merge shows the PR merging. A PR found through a branch stays with the thread after it merges. ([`de29ec4`](https://github.com/mdovhopo/yourharness/commit/de29ec4))
  - A PR button in the thread's toolbar, counting them and opening the tab. ([`05754cf`](https://github.com/mdovhopo/yourharness/commit/05754cf))
  - The repos view draws stacks as a joined-up staircase. ([`c3999d4`](https://github.com/mdovhopo/yourharness/commit/c3999d4))
- **A browser built in for Claude.** Claude opens the app it's building in a Browser tab, reads the page, clicks, types, scrolls and takes screenshots, and every step shows in the thread, with new console errors and failed requests reported back to it. You watch, see the console and network under the page, can take control at any time, and can pick an element to put in your message. Each project keeps its own cookies, and the side chat can look at the thread's browser. You can turn it off in Settings. ([`cb77cd9`](https://github.com/mdovhopo/yourharness/commit/cb77cd9), [`9326d5b`](https://github.com/mdovhopo/yourharness/commit/9326d5b))
  - Dev servers the thread started are offered in the empty browser, localhost links in the thread open there (⌘-click for your default browser), it can pop out into its own window, and idle pages close after 30 minutes to save memory. ([`2cf453d`](https://github.com/mdovhopo/yourharness/commit/2cf453d))
  - Import Cookies… brings logins over from Safari, Chrome, Arc, Brave, Edge or Vivaldi, for the sites you pick. ([`30dadcc`](https://github.com/mdovhopo/yourharness/commit/30dadcc))
- **Worktrees, tracked rather than assigned.** A thread works in as many repos and worktrees as it needs, and YourHarness keeps track. ([`f80a907`](https://github.com/mdovhopo/yourharness/commit/f80a907), [`e4a8ffa`](https://github.com/mdovhopo/yourharness/commit/e4a8ffa))
  - The repos view lists each repo's worktrees wherever they are on disk: branch (or a warning when one isn't on a branch), changes, what's to push, open PR, and which threads work in each. Worktrees whose work has landed are marked done with; missing ones are marked missing. ([`f80a907`](https://github.com/mdovhopo/yourharness/commit/f80a907))
  - A thread's toolbar lists the repos and worktrees it works in, and a thread can be moved to run in any of them. New Worktree… creates one and can start a thread in it. ([`f80a907`](https://github.com/mdovhopo/yourharness/commit/f80a907))
  - Repos are sorted by how much threads use them. Worktrees start folded, with a count and a dot when one needs a look; Expand All / Collapse All, and a filter for big workspaces. ([`9003412`](https://github.com/mdovhopo/yourharness/commit/9003412), [`00ed1f0`](https://github.com/mdovhopo/yourharness/commit/00ed1f0))
  - Threads show as one pill per repo or worktree, active and settled apart, so a worktree only settled threads used reads as done with. ([`cc3ccb0`](https://github.com/mdovhopo/yourharness/commit/cc3ccb0))
  - Removing a worktree is one click; it only asks when there's work in it, and says whether anything would be lost. ([`7845708`](https://github.com/mdovhopo/yourharness/commit/7845708))
- **Turns survive quitting and crashes.** Your message is saved the moment you send it and the rest of the turn every couple of seconds; quitting stops Claude and says so; after a crash, what Claude did meanwhile is recovered from Claude Code's session history. Work done on a thread's session outside YourHarness (`claude --resume`) shows up when you open the thread, and unsent drafts are kept. ([`87705f2`](https://github.com/mdovhopo/yourharness/commit/87705f2))
- **Notifications** when a turn finishes while YourHarness is in the background, or Claude needs your approval, an answer or a plan review. Clicking one opens the thread, and the Dock icon counts threads waiting for you. ([`cd9dd0d`](https://github.com/mdovhopo/yourharness/commit/cd9dd0d))
- **Plan checklist:** for work with several steps, Claude lists them and ticks them off, in the thread and in a strip above the composer. ([`8553bd9`](https://github.com/mdovhopo/yourharness/commit/8553bd9))
- **Find in a thread (⌘F),** with matches highlighted in messages, code and tool calls, and ⌘G / ⇧⌘G to step through them. ([`6202921`](https://github.com/mdovhopo/yourharness/commit/6202921))
- **`@` mentions in the composer:** type `@` and part of a name to give Claude a file or folder from the project. ([`79ce174`](https://github.com/mdovhopo/yourharness/commit/79ce174))

### Changed
- The usage limits in the sidebar show how long until each one resets ("2h 14m", "3d 4h"). ([`b3b38d1`](https://github.com/mdovhopo/yourharness/commit/b3b38d1))
- The right sidebar's tabs and actions moved into the window toolbar, so the panel is all content. ([`31865dd`](https://github.com/mdovhopo/yourharness/commit/31865dd))
- The side chat can run commands and look things up on the web, asking you first for anything that could change something; it still can't edit files. ([`31865dd`](https://github.com/mdovhopo/yourharness/commit/31865dd))
- Permission cards stack their buttons under the request when space is narrow. ([`31865dd`](https://github.com/mdovhopo/yourharness/commit/31865dd))

### Fixed
- A project could show no pull requests until refreshed by hand, when it was scanned before `git` was found at launch or two refreshes overlapped. ([`de29ec4`](https://github.com/mdovhopo/yourharness/commit/de29ec4))
- Thread links to repos no longer mistake branch names in commands (`-b feature/login`) for folders. ([`e4a8ffa`](https://github.com/mdovhopo/yourharness/commit/e4a8ffa))

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
