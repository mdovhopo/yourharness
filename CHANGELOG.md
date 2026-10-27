# Changelog

What changed in each release of YourHarness. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org): while it's 0.x, a minor version can change anything, and patch versions are fixes and small additions.

Add notes under **Unreleased** as features land; `scripts/release.sh <version>` files them under the new version.

## [Unreleased]

### Added
- **Claude can look after your threads.** Ask a thread to find an earlier conversation, read what it worked out, rename or settle threads, pin one, or bring in a session you ran with Claude Code in a terminal, and it does it without taking you anywhere. It can't delete threads. A thread in Home reaches all your projects; a thread in a project only that project's threads, and only Claude Code sessions run in its folder. It changes threads only when you ask, and checks with you before changing more than two.
- **Titles keep up with the conversation.** When a thread turns to something else, Claude has its title rewritten, by the same title model and in the same style as the first one. A name you gave the thread yourself stays. Turn it off in Settings → General → Thread Titles, where you can now also edit the prompt titles are written by.
- **Export State and Import State.** File → Export State… puts everything YourHarness has into one file: your projects, threads, memories, settings and images, the window layout, and which app version, Mac and tools it came from. Your browser sign-ins go in only if you tick the box. Import State brings a file back. It first shows what's in the file next to what you have now and warns that everything will be replaced. It saves a copy of your current state unless you say not to, then restarts. Use it to move to another Mac, or to send someone the exact state a bug happens in.

### Fixed
- **The PRs sidebar stays still.** Coming back to the app (from Mission Control or another window) refreshes your repos, and with a stack or PRs in several repos, the cards could swap places each time: a PR's place depended on whether the latest refresh had found it through its branch, and a repo's list holds only its 20 most recently updated PRs, so a quiet one drifted in and out. PRs now keep the order they came into the thread, and a failed `git status` no longer makes a repo forget its branch until the next refresh.

## [0.0.9] - 2026-10-26

### Changed
- **Threads working while you're elsewhere cost far less.** With the window minimized, hidden, covered, or on another Space, the app used to redraw every streamed word as if you were watching. Now it catches up once a second, and at once when you come back; anything that needs you (an approval, a finished turn) still arrives right away. With 24 busy threads, the app's CPU while you're away went from about three quarters of a core to under a third.

### Fixed
- **Full screen works after you've used the browser.** Once the built-in browser had opened a page, going full screen left everything but the sidebar blank: the off-screen window where pages wait while no panel shows them was moved onto the screen and covered the app. It stays off screen now.
- **No more long freezes after hours of busy threads.** The turning Claude mark and the pulsing "Working…" were animations that never end, and every later animation of the same view could queue up behind them: with several threads working for hours, the app froze for minutes. They now follow the clock instead, and look the same.

## [0.0.8] - 2026-10-26

### Fixed
- **No more freezing or quitting in full screen.** With the window full screen or filling the screen, opening a project's Memory page froze the app for many seconds and then made it quit unexpectedly: the page asked for a window taller than the screen, and the window couldn't grow.
- **The window keeps the size you give it.** Opening a project's Memory page (with its "Two memories" note) made the window as tall as it could go, past the bottom of the screen, and it stayed that tall: a short thread then sat below the screen's edge and looked blank. No page can make the window bigger any more, and a window saved too big fits the screen again.
- **Memory opens without resizing the window.** With memories in it, a project's Memory page (from the brain in the sidebar) widened the window to about 1,300 pt, past the edge of a narrow screen. No page can make the window wider now, and Memory fits a narrow window: the tabs go under the title, the list narrows, and a task's state becomes a menu.
- An opened Thought no longer leaves a gap of empty lines under it (Claude's thinking ends in blank lines, and they were drawn).
- Opening the right sidebar on a narrow window still widens it to make room, but never past the screen, and closing the sidebar gives the width back (unless you resized the window meanwhile).

## [0.0.7] - 2026-10-25

### Added
- **Memory, per project.** Agents now remember what's worth knowing next time (who you are, how you like to work, decisions, where things live) and every new thread in the project starts with it: an index of a line a memory (about 2,000 tokens at most), with the rest a search away. Claude Code's own memory files are turned off so there's one memory (Settings → General → Memory).
- **The Memory view** (⇧⌘M, or the brain on a project in the sidebar): search, edit, pin, link and archive memories, see where each was learned and every version (restore any of them). **Insights** tells you in plain words how memory is working (what it costs each session, what agents read, what they never read, what looks duplicated, what they searched for and didn't find) with charts; **Activity** lists every save, read, search and session; **Graph** draws how memories link.
- **Tasks in memory.** When you ask for something with several steps, or that will go on later, agents keep it as a task: its steps, what's done, what's next, why it's blocked, and every thread that worked on it. Every new thread starts knowing where your open tasks stand, so "carry on with dark mode" picks up at the next step. See them in Memory → Tasks (tick steps yourself), and in a thread's toolbar.
- **Search everything with ⌘K** (or Search at the top of the sidebar): threads, what was said in them, memories and tasks across your projects, projects and actions, Spotlight-style. A hit in a conversation opens the thread with Find on that word.
- **Import and export memory.** Export as JSON or as Markdown in Claude Code's format, and import either back; import Claude Code's own memory for a project in one click. **The Importer**, an agent in its own window, turns anything else (notes, CLAUDE.md files, a ChatGPT export, pasted text) into memories, without duplicates or secrets.
- **The Reconciler**, an agent in its own window, goes through a project's memory with you: merges duplicates, fixes or archives what's out of date, tightens vague memories and links related ones. It shows its plan first, and every change can be undone. Pick the model and effort the Importer and Reconciler use in their window, or for both in Settings → General → Memory.
- **One memory, not two.** A project's Memory page tells you when Claude Code still keeps its own memory for the folder (as it does when you run `claude` in Terminal) or has memories YourHarness doesn't, and **Use One Memory** brings in what's missing and turns Claude Code's memory off for that folder (in its `.claude/settings.local.json`, kept out of git). Settings can turn it off everywhere.
- **One page per project.** Click a project's name for its page: Pull Requests and Memory, switched in the middle of the toolbar.

### Changed
- **Your threads live in a database now.** Everything the app keeps moves into one SQLite file the first time this version opens, without you doing anything: settings, projects, threads and every transcript, the old files left as they were. Saving while Claude works writes only what changed instead of the whole thread every couple of seconds, so a long thread costs no more to save than a short one, and big tool output (whole files read, long logs) is stored once, compressed, and kept out of memory beyond what a tool call shows. It's the groundwork for memory.
- **Every thread is searchable underneath**: what was said, commands and the start of their output are indexed in the background, ready for search across threads and for memory.
- **Resume a Claude Code session into the right project.** The Resume sheet has Add to: your projects, starting on the project for the session's folder, or else the workspace that folder is in, or else a new project for it. The thread keeps running in the session's own folder, so it still continues the session.
- A thread you hadn't opened since launch could lose its saved history when one of its PRs was found through its branch; Claude's own session history usually filled it back in, under "Continued outside YourHarness".

## [0.0.6] - 2026-10-24

### Changed
- **Claude's replies read and select as one text.** Paragraphs, lists, quotes and tables are one piece of text: drag across a whole reply, table included, and ⌘C copies what you see (table cells separated by tabs), with formatting and a real table when you paste into Notes or Pages. Code blocks still scroll sideways and copy on their own.
- **Smoother streaming.** Only the part of a reply that's still arriving is redrawn, so long replies no longer slow down as they grow (a third of the work on an 8 KB reply), and you can select text while the rest streams in. Text no longer clips or jumps when a tool call appears, and "Working…" no longer flashes between Claude's steps.
- **Copy a table** with the button on its header row when the pointer is over it: markdown, or a table where spreadsheets and rich text go.
- The Copy button under Claude's replies sits on the right.
- A line break inside a list item keeps its indent.
- **GitHub stacks, with their buttons.** PRs in one of GitHub's stacked pull requests show as that stack ("Stack #4 · 2 of 4 merged"), in GitHub's order, merged layers included, in a thread's PRs and the repos view. **Merge Stack** squash-merges the whole stack on GitHub, or pick **Merge Up to #n** from its menu; branches made with `gh stack` that have no PR yet show as Not submitted, and **Submit** pushes them and opens their PRs. Stacks made with `gh stack` and not on GitHub yet have a card of their own. It all runs GitHub's `gh stack` extension, which the app offers to install.
- **Make stacks from the app.** New Stack from Branches… (a repo's menu in the repos view) makes existing branches a GitHub stack, bottom first, and submits it; a stack's ⋯ menu adds a branch on top or syncs it with GitHub after merges below; Split into a Stack in a thread's PRs asks Claude to split its work into stacked PRs. Claude knows how `gh stack` works when it's installed.
- **Make GitHub Stack** on a stack made only of branches (each PR based on the one below) turns its PRs into one of GitHub's stacks, as they are, so they merge as a stack from the app.
- PRs Claude opens or merges with `gh stack` count as the thread's own work, so its stack shows whole in the thread's PRs instead of mostly under Mentioned.
- **A thread's PRs stay current** when they're merged or closed outside the app. One link in the thread that GitHub couldn't find (a deleted repo, a PR number that doesn't exist) stopped all of them updating, Refresh included.
- **The right sidebar switches tabs like a Mac window.** The toolbar no longer fades out and back in on every switch, and tabs you've opened stay as they were (scrolled, half-typed) when you come back. New Side Chat moved into the side chat itself.
- A project's overview uses the whole window: the right sidebar, which only ever said No Thread Selected there, steps aside, with its toggle, and comes back as you left it when you open a thread.
- **Top and end of a thread** in one click: arrows above and below the message arrows, and Thread → Top of Thread (⇧⌘[) and End of Thread (⇧⌘]). Going to the top shows the earlier turns on the way.

## [0.0.5] - 2026-10-23

### Changed
- **Claude's work is easy to find again.** Each finished turn's work is a row of its own ("Worked for 2m 14s · 9 tool calls"), with Show work; opened, every step shows inside it as one group, under Hide work.
- **Copy buttons** under Claude's replies and beside your messages, when the pointer is over them.
- **Full-page screenshots show the whole page.** A full-screen section (`100vh`, Tailwind's `h-screen`) stretched to the page's full height and pushed everything below it out of the picture; it stays one screen tall now, as in a browser.
- **Hover and drag work as with a real mouse.** Hovering in the built-in browser never applied `:hover` unless its window was in front, so hover menus and styles looked broken. Clicks and drags now tell the page the button is down (drags that check let go halfway), and dragging a `draggable` element does HTML drag and drop, which never dropped anything before. A drag says whether the page took the drop.
- Clicks in a scaled page land on the exact pixel; they could be a pixel off.
- Browser screenshots say the page's real size and how much the image is scaled, and a phone-sized page without `<meta name="viewport" content="width=device-width">` gets a note that a real phone lays it out 980px wide.
- Running JavaScript in the browser that uses a variable named `seen` no longer fails.
- **The built-in browser shows pages at their true size.** Text under 18px came out at 18px, because the page was shrunk to fit the panel and WebKit kept small text readable, and pages saw a pixel ratio of 1 instead of a Retina screen's 2. Now fonts, layout and `devicePixelRatio` match Safari on the same screen.
- The browser's page sizes match today's screens: Laptop (a 14-inch MacBook Pro, 1512 × 982, the new default), Desktop (1920 × 1080), Tablet (an 11-inch iPad Pro, 834 × 1210) and Phone (an iPhone 17 Pro, 402 × 874).
- The arrows for jumping between your messages are two separate round buttons, and each shows only when it has somewhere to go: no ↓ at the end of the thread, no ↑ above your first message.
- **Switching threads is instant.** Going back to one of the last few threads you were in takes a few tens of milliseconds and keeps your exact place: their transcripts stay made. Everything else got lighter too: no crossfade or sliding selection to wait through, a long thread's history loads off the main thread and only its latest 40 turns are laid out (scroll to the top for more), the composer and toolbar stay put instead of being rebuilt, and transcripts you haven't looked at in a while are let go to save memory.
- **Text in the transcript selects like Mac text.** Claude's replies are AppKit text: drag across paragraphs, lists and headings, double-click a word, ⌘C, Look Up; links show the pointing hand. Code blocks too, and they no longer slow down a long thread.

### Fixed
- A transcript with one entry the app couldn't read showed as empty; now only that entry is skipped.

## [0.0.4] - 2026-10-23

### Highlights
A calmer transcript: once a turn ends, Claude's work folds into one line ("Worked for 2m 14s · 9 tool calls"), so your messages and its answers are what you read, with times, turn lengths and ⌘[ / ⌘] to jump between your messages. A thread's PRs are now the ones it worked on, and threads settle by themselves when all of them merge. Also new: chaining skills anywhere in a message, Claude showing you images and videos, syntax highlighting, Undo, and a stable signing identity, so macOS stops asking for permissions again after each update.

### Added
- **Claude's work folds away when a turn ends.** While Claude works you see every step; once it's done, the thinking, tool calls, sub-agents and what it said along the way fold into one line between your message and its answer: "Worked for 2m 14s · 9 tool calls · edited 3 files". Click it to see everything. Notices and messages you sent mid-turn stay in view. ([`f34a078`](https://github.com/mdovhopo/yourharness/commit/f34a078), [`d8a1b0d`](https://github.com/mdovhopo/yourharness/commit/d8a1b0d))
- **How long a turn takes, and when you sent each message.** The working indicator counts up while Claude works, each finished turn says how long it took, and your messages show their time. They also stand out more: tinted and outlined, with a little more room above each turn. ([`f34a078`](https://github.com/mdovhopo/yourharness/commit/f34a078))
- **Jump between your messages.** ⌘[ and ⌘] (Thread menu) or the small arrows at the bottom right of the transcript go to your previous and next message; past the last, to the end. ([`f34a078`](https://github.com/mdovhopo/yourharness/commit/f34a078))
- **Threads remember where you were.** Switch away and back and you're at the same place; if you were at the end, you're at the end, following new replies. ([`0988ff9`](https://github.com/mdovhopo/yourharness/commit/0988ff9))
- **A new thread asks what you're working on.** Its composer waits in the middle of the window under "What are we working on in acme?" and the project's folder, and glides down to its place when you send. ([`8c5c3bc`](https://github.com/mdovhopo/yourharness/commit/8c5c3bc))
- **Slash commands anywhere in a message, to chain skills.** Type `/` partway through a message and the menu lists the skills Claude can run itself; "implement it, then /code-review and /commit" runs each where it comes up, in order. Claude Code only runs the command a message starts with, so YourHarness adds a line asking Claude to run the others with its Skill tool; the message you see stays as you wrote it. ([`66a89b3`](https://github.com/mdovhopo/yourharness/commit/66a89b3))
- **Claude can show you images and videos.** When a screenshot, a screen recording, a simulator capture or a chart would say more than words, Claude puts it in the conversation: images open in Preview, videos play right there. The files are copied into the thread, so they stay after the originals are gone. It doesn't ask first, since it only shows you files. ([`f895b50`](https://github.com/mdovhopo/yourharness/commit/f895b50))
- **Syntax highlighting in code blocks,** in light and dark, for Swift, TypeScript, JavaScript, Python, Go, Rust, Java, Kotlin, C, Ruby, shell, JSON, YAML, TOML, SQL, HTML, CSS, diffs and more. ([`a6b9d99`](https://github.com/mdovhopo/yourharness/commit/a6b9d99))
- **Threads settle when their work lands.** Once every PR a thread worked on has merged, and nothing new has happened in it since, it moves to the Settled shelf, the way idle threads do. One still open, or closed without merging, keeps it where it is. PRs are looked up in the background, so this happens for threads you aren't looking at too. ([`5773503`](https://github.com/mdovhopo/yourharness/commit/5773503), [`4366aea`](https://github.com/mdovhopo/yourharness/commit/4366aea))
- **Undo for thread actions.** Settling, unpinning or deleting a thread shows Undo for a few seconds above the limits box, and ⌘Z (Edit → Undo) takes it back too. Deleting no longer asks first: the thread's files are only removed when YourHarness quits. ([`a652ee7`](https://github.com/mdovhopo/yourharness/commit/a652ee7))

### Changed
- **A thread's PRs are the ones it worked on.** The PRs tab lists the PRs the thread (or one of its sub-agents) opened, merged, edited, reviewed or commented on, and the ones on branches it changed (edits, commits, pushes), as the stack tree; PRs that only came up (links you pasted, ones Claude looked at or cited) are listed under Mentioned, below. The toolbar counts, the tab opening by default, and settling when work merges go by the PRs it worked on. ([`4366aea`](https://github.com/mdovhopo/yourharness/commit/4366aea), [`597d425`](https://github.com/mdovhopo/yourharness/commit/597d425))
- **⇧↩ starts a new line** in the composer and the side chat, where the cursor is, as in any text area; ↩ sends, as before (⌥↩ still works too). ([`4188819`](https://github.com/mdovhopo/yourharness/commit/4188819))
- **YourHarness is signed with its own certificate** instead of ad hoc. macOS will ask once more for the folders and keychain items it used (it's a new identity), and from then on remembers your answers across updates. Updates are only installed when they're signed the same way. ([`8691d00`](https://github.com/mdovhopo/yourharness/commit/8691d00))

### Fixed
- Opening the Browser tab on a Mac where it had never been used made the window grow past the bottom of the screen, pushing the sidebars down; the window stayed that tall afterwards. It no longer grows, and a window saved too tall fits the screen again. ([`a989b6b`](https://github.com/mdovhopo/yourharness/commit/a989b6b))
- Removing a worktree is instant, even offline: it no longer rescans the project and asks GitHub for PRs before it's done. ([`daad007`](https://github.com/mdovhopo/yourharness/commit/daad007))

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
