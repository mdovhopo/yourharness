# Changelog

What changed in each release of YourHarness. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org): while it's 0.x, a minor version can change anything, and patch versions are fixes and small additions.

Add notes under **Unreleased** as features land; `scripts/release.sh <version>` files them under the new version.

## [Unreleased]

### Fixed

- Phone: tapping the microphone no longer closes the app once you've allowed it to listen.

## [0.1.10] - 2026-11-10

### Added

- Phone: see and use the thread's browser from your Mac. The globe in a thread's toolbar shows the page Claude is testing as it changes, and you can use it too: tap to click, drag one finger to scroll, pinch with two to zoom, type into fields with the phone's keyboard, change the address, go back and forward, and switch tabs. Tapping a `localhost` link in a reply opens it in the Mac's browser and shows it on the phone, so the app Claude is building works from your phone or iPad, at home or over Tailscale. More → This Phone lays the page out at your phone's size. When Claude opens a page in the thread you're reading, an iPad shows the browser beside the conversation by itself, and an iPhone offers it in a card above the message field (nothing streams until you tap Show); the globe has a dot while pages are open. Pictures go out only when the page changes, and only as fast as your connection takes them. Needs the Mac on this version.

## [0.1.9] - 2026-11-10

### Added

- Phone: tapping a pull request's link in a thread opens a review page right in the app instead of the browser: its status (reviews, checks, conflicts), checks, description and conversation, with Squash and Merge (or Merge When Ready) and a button to open it on GitHub. PRs in a project's PR list open it too. Needs the Mac on this version.
- Phone: a PR's review page lists the files it changes; tap one for its diff, with comments shown under the lines they're on, lines wrapped or scrolling sideways, and arrows to step through the files.
- Updates can install by themselves: turn on "Install updates automatically" in Settings → Releases (off by default). New releases then download on their own, and once nothing has run for two minutes (no thread working or waiting for you, no background jobs, no files waiting to be sent) a card in the window's top right counts down two minutes to the restart, with Later (15 minutes) and Restart Now. Typing or anything starting calls it off. When the window is out of sight a notification warns you, with its own Later button; with the screen locked it restarts straight away. Your threads and drafts open again where they were.

### Fixed

- Updating from several versions behind goes straight to the latest: an update downloaded earlier and not yet restarted into is replaced by a newer release when one comes out, instead of installing the old one first.

## [0.1.8] - 2026-11-10

### Changed

- Usage limits show which subscription they belong to (Claude Max; the account in its tooltip) and have thicker, easier to read bars, on the Mac and on the phone. The phone's Usage tab shows each connected Mac's subscription; signing in to another account drops the last one's figures instead of showing them as yours.
- Phone: one New Thread button at the top of the thread list instead of one per project; its sheet picks the project, starting on the last one you used.

## [0.1.7] - 2026-11-10

### Fixed
- New Thread on the phone lets you pick the model, reasoning effort and permissions before the first message goes, and remembers your pick for that project.

## [0.1.6] - 2026-11-10

### Fixed
- Links in Claude's replies on the phone open in Safari when you tap them, web addresses written out without a link too.
- The phone opens on the thread list it last had, even before it reaches the Mac, and a reconnect no longer moves anything: a spinner shows in the corner, and in a thread the send button spins until it's back.
- The phone keeps the last messages of each thread you opened, so a thread shows them at once, even before the phone reaches the Mac; then only what's new arrives.

## [0.1.5] - 2026-11-10

### Added
- The phone shows what Claude left running in a thread (background commands, watches, sub-agents) above the composer, with how long each has run, and an hourglass in the thread list while they run after the turn. A thread stops streaming to the phone while the app is in the background.
- YourHarness has an icon: a Y of two threads joining into one, your agents gathered in one place. Development builds of YourHarness Next show it on night blue.

### Changed
- New Thread opens at once and creates nothing: the thread is saved, and joins the sidebar, when you send its first message. Leave it empty and it's gone; leave it with something typed and New Thread in that project brings your draft back (until you quit).
- On the phone too: New Thread opens a sheet to write the first message, and the thread is made on the Mac when you send it. Close the sheet and nothing is made; what you typed waits for that project's next New Thread.

## [0.1.4] - 2026-11-10

### Added
- Settings → Phone: let your iPhone or iPad on the same Wi‑Fi follow and run your threads. Pair a phone by scanning the QR code shown there; each phone gets a key of its own, and Forget cuts one off. Traffic isn't encrypted on the network yet, so use it on networks you trust. The phone app itself isn't released yet. It listens on port 47878, or the next free one when another app has it.
- Phones over Tailscale: with Tailscale on the Mac and the phone, the phone reaches your Mac from anywhere, not just on its Wi‑Fi. It tries the Wi‑Fi and Tailscale addresses at once and keeps whichever answers. Only devices logged in to Tailscale as you get in, and the way is encrypted. Settings → Phone shows Tailscale's state, and Over Tailscale Only keeps phones off the Wi‑Fi altogether.

## [0.1.3] - 2026-11-09

### Changed
- Claude's replies are turned into a thread's rows on a thread of their own, away from the window, so the window has less to do while turns stream.

### Added
- The groundwork for a companion iPhone and iPad app: an experimental server, off unless YourHarness is started with `YH_SERVER`, that only listens on this Mac for now. The app itself isn't released yet.

## [0.1.2] - 2026-11-09

### Fixed
- A thread shows as working again when you message it after YourHarness reopened mid-turn with background jobs left running. Claude Code answered the jobs first, which looked like the end of your turn.
- Clicking a picture in a thread no longer quits YourHarness while Quick Look is still loading it.

## [0.1.1] - 2026-11-09

### Fixed
- Files: a file's lines scroll sideways when they're wider than the panel, and their text can be selected and copied.
- Mermaid diagrams whose quoted labels run over more than one line are drawn again instead of shown as code.

## [0.1.0] - 2026-11-09

**YourHarness is now a Rust app.** This release replaces the Swift and SwiftUI app with a complete rewrite in Rust, drawn with GPUI (the GPU-accelerated framework the Zed editor uses) in a single native process. The transcript stays smooth and memory stays low with many long threads streaming at once, and switching threads takes a few hundredths of a second. It's the same app to macOS: the same name, bundle ID and signature, so it updates from 0.0.16 like any release and keeps the permissions you granted. Everything you had carries over: projects, threads, memory, settings, usage history and browser sign-ins are read from where 0.0.16 kept them, and the database stays in 0.0.x's format, so going back to 0.0.16 still works. The Swift app's code is gone from the repository.

### Changed
- A new look throughout: quieter sidebar rows with a soft selection, icons instead of text symbols, SF Mono for code, one type scale, and Settings, Memory, Usage and the project page laid out like System Settings, with segmented controls and grouped rows.
- The composer is one calm card with its controls as quiet icons along the bottom and a round Send; questions, plans and permission requests sit above it in matching cards. The right sidebar's tabs are a row of icons, each panel has a header like the thread's, and the Files tree, pull requests, sub-agents and browser have proper icons and tabs.
- Menus look like the rest of the app: the model, effort and permission pickers, the More and right-click menus, the browser's page menu and Settings' pickers sit on the same grey as other popovers instead of near-black, in the app's text sizes with compact rows, a thin edge and a soft shadow, the row under the pointer softly tinted, section names and notes small and quiet.
- Tool calls in the transcript read as what they did ("Read …/README.md", "Run command"), each a compact line that opens into a card; failed ones are red. A finished turn's work folds into one quiet line.

### Added
- Memory's Tasks tab is a board: To Do, In Progress, Blocked and Done side by side, each scrolling on its own, every card showing how far along the task is, its next step or note and when it last changed. Done shows the recent ones with Show All for the rest; in a narrow window the columns stack.
- Pointing at a memory on Memory's Graph lights it, its links and its neighbours and dims the rest, names them, and shows the memory in a card; the pointer becomes a hand, and a click opens it.
- Memory's Insights leave tasks out: they record work and are rarely read by design, so the memory count, how much gets read, never-read, most read, by-kind and the chart are about memories alone, with the task count noted beside them.
- History from 0.0.6 and earlier, kept as `state.json` and `threads/`, moves into the database the first time, in the background, as 0.0.16 would have moved it; a damaged file is skipped and noted, never a crash. Going back to 0.0.16 afterwards reads everything this version wrote.
- Select text anywhere in a reply, a tool's output or a thinking block, and copy part of it.
- Code blocks have a header with the language and a Copy button; tables copy as tab-separated text; diagrams open large, and copy or save as PNG. Copy buttons say "Copied" for a moment after a click, and a diagram's actions are icons with tooltips.
- Narrow windows work down to the smallest size: the sidebars give way before the thread does (the left one folds away when there's no room beside the right one), and Memory, Usage and the project page switch to icons and one column at a time by their own width. A memory's text box grows with what you write.
- Several permission requests at once (from parallel sub-agents) each get their own answer, one after another.
- ⌘↩ answers a question once every question has an answer, or approves a plan in the recommended mode; ⌘⌫ skips or keeps planning.
- The browser: back, forward and reload, an address field, closable tabs, console and network panes, Pick an element for your message, local dev servers offered when nothing's open, pages left alone for 30 minutes close (and reopen when the browser is next used), and the whole browser can open in its own window.
- Links that open a new window (`target=_blank`, `window.open`, sign-in popups) open as a new tab in the thread's browser, up to eight, and close again when the page closes itself; a page whose process crashes loads again by itself.
- Take Control in the browser's toolbar stops Claude acting there while you use it (it can still look), with a banner and Give Back; Claude is told, and its old element references stop working.
- A thread's browser tabs are saved with it and reopen the next time you or Claude use the browser, even after a quit; pages closed for being idle come back the same way.
- When a page shows an alert, confirm or prompt while you browse, a card asks you ("The page says…", OK and Cancel); Claude can still answer it with its own tool.
- Claude hears when a page answers 404 or 500, when a page fails to load, and when a link was a download the browser skipped; element references keep counting up across pages, so an old one never names an element on the next page.
- The browser's empty state offers the dev servers this thread started (what's listening under its `claude`, and the local addresses its commands printed), not every server on the Mac.
- The browser's toolbar has Stop while a page loads, and its menu Page Size, Page Appearance (Same as the App, Light, Dark), Show Web Inspector and Close Tab; Clear Browsing Data asks first. Under the page, the console and network sit in a strip you can fold, filter, show errors only, clear, and open a row of to read all of it.
- While Claude clicks, types, scrolls or drags, the page shows where, ringed, with what it's doing; screenshots never include it.
- An element you pick in the browser comes into your message with a picture of it.
- Import Sign-ins has Select All and Select None, offers Open Privacy Settings when Safari's cookies need Full Disk Access, and reloads open pages afterwards.
- The thread's header shows the repos and worktrees it works in, with their state, and can run the thread in another one.
- Settings → Agents shows where `claude` is (and lets you choose another), the account it's signed in with, `gh` and `glab`.
- The sidebar's left column hides with ⌃⌘S and resizes by dragging; the window's size and place and the open thread come back at launch.
- Drop files anywhere on a thread to attach them.
- The Files tab keeps up with the disk: files made, deleted or renamed show in the tree as they happen, and an open file reloads when it's saved, staying where you were. Files git ignores show dimmed rather than hidden. ⌘F in an open file finds in it; right-click a file in the tree to open it in a tab, in its app, in Finder, or copy its path. Comments and strings over several lines are coloured all the way down.
- The PRs tab draws stacks as a card with their tree, shows a `gh stack` stack's branches that aren't submitted yet, finds the PRs of every branch the thread worked on (each repo and worktree, GitHub and GitLab), and checks again a few seconds after Claude merges, opens or pushes, and every 15 seconds while it shows.
- Sub-agents Claude starts in the background no longer look like a finished thread: the strip above the composer lists each ("Agent: …") with what it's doing now, how long it's run and Stop, and a click follows it in the Sub-agents tab; the thread says "2 sub-agents working…", its sidebar row shows it working ("2 sub-agents running"), the Sub-agents tab carries a count, Quit asks first, and YourHarness keeps their `claude` running until they report.
- Following a sub-agent reads like the thread: its steps as tool calls with their output, what it's doing right now, whether it runs in the background, and its report as formatted text you can select. The list of runs shows each one's agent type.
- A memory shows its text formatted, with Edit to change it; Link To and Unlink tie memories together both ways; History can restore an older version; More copies its ID; a merged memory links to where it went. Memory's Activity can show only changes, reads and searches, or sessions, and links to the thread each thing happened in.
- `/mcp` asks before removing a server; while you sign in it can open the page again, copy its link, or take the address the browser ended on; a server that won't start has Ask Claude to Fix It. Add Server takes a URL (with headers and OAuth client details if its docs ask), a command with its environment, or JSON, and can import Claude Desktop's servers. Claude's sign-in card can open the page again, and a server's questions show yes/no as a switch and choices as a menu.
- Search across threads finds old conversations a few seconds after launch, without waiting for something new to be written first.
- Closing the window quits, as in 0.0.16, and every way of quitting (⌘Q, the close button, Dock → Quit, logging out, Restart to update) asks first while background jobs run.
- The full menu bar: About YourHarness, Services, Hide, Close Window (⌘W), Enter Full Screen (⌃⌘F), a Window menu and Help; "Hide Usage" and "Hide Right Sidebar" say what they'll do, and items that can't do anything now are greyed out.
- ⌘Z and Edit → Undo take back the last thread action (delete, pin, unpin, settle, un-settle), even with the composer focused when you haven't typed since.
- Threads stay in the order they were made, pinned first, so rows don't jump as threads work. Settling or deleting the open thread opens the one next to it.
- New Thread reuses an untouched blank thread in the project instead of making another; Add Project opens a new thread in the folder (an already-added one too), and its panel says what the folder is for. A project you removed and add again gets its memory back.
- Settling a thread also unpins it and closes its browser; threads whose pull requests have all merged settle on their own.
- The right sidebar resizes by dragging its edge (300–960 pt), and opening it on a narrow window widens the window within the screen, giving the width back when it closes. A window saved bigger than its screen comes back fitted to it.
- Updates: What's New with the release's notes from GitHub, a Restart question that says which threads it stops, Cancel while downloading, Dismiss on a failed check, "X is the latest" after a check you asked for, "Updated to X" after an update, and the version in the sidebar opens a card with the build and the release notes. Settings → Releases lists every release, Installed and New tagged.
- Settings is a window of its own (⌘, brings it forward) and its MCP tab lists your servers everywhere in place. Agents shows Git, who `gh` and `glab` are signed in as with a Get link when one is missing, how to sign `claude` in (`claude auth login`, Copy, Open Terminal) and the account's usage. Memory has Turn Off Everywhere… and the Importer and Reconciler's model and effort; Notifications has Open System Settings when they're off; Home has Reset; Thread Titles lists your models and says why the last title failed.
- The project page: a "No Git Repositories" state, a filter for big workspaces, Expand All / Collapse All for worktrees, and a nudge when Claude Code keeps a memory of its own there. Workspaces of repos show a stack icon, here and in the sidebar.
- Usage: Stacked or By Model, a per-day table paged Newer/Older, Refresh, Esc to close, input/output/cache read/cache write in the totals, and when the prices were checked.
- Export State asks whether to include the browser's sign-ins and says when it's done, with Show in Finder; Import State compares what you have now with what's in the file and saves a copy of yours first.
- Idle threads show a small grey dot; VoiceOver reads sidebar rows (title and what they're doing), project headers and the thread's title.
- While Claude works, macOS doesn't nap the app in the background.
- Claude and the commands it runs get your login shell's environment (PATH and all), as a fresh Terminal window has it, even when the app is opened from the Dock or from inside a Claude Code session.
- Find in thread marks the words it found (the current match orange, the rest yellow), opens folded work to show a match, ignores case and accents, and ⌘G goes round from the end.
- While Claude works, the end of the thread says what it's doing ("Compacting the conversation…") and for how long; the plan's strip above the composer shows only while it's being worked on; a lightning button stops Claude and sends what you wrote right away, and the composer says how steering works.
- A command in the background shows how long it's run, or how it ended ("Exited 1 · 3s"), in its step, and opening it shows what it's printing.
- "Edited N files" counts what changed in the turn by any means, a shell command or a script included; with another thread in the same folder, only the files the turn named.
- The / menu lists every match with where it comes from (YourHarness, MCP, project, a plugin), finds MCP prompts by the name it shows, and knows a project's commands before its first message. `/mcp enable`, `disable` and `reconnect` work in place.
- A thread's first title, when its subject wasn't clear yet, is written again once Claude has answered; linked GitHub PRs and issues are looked up so the title says what they're about.
- The side chat can look at the thread's browser and search the memory, read-only; it stays until you close it and never notifies.
- Videos show their first frame; built-in commands' output is headed "Command output"; the PR button counts the PRs the thread worked on; "Show work" and "Hide work" on a finished turn; ⌘-click opens a local address in your default browser; right-click on selected text offers Copy and Look Up.
- Code blocks scroll sideways rather than wrap, and long tool input and output scroll inside their box, the wheel moving on to the thread at their ends.

### Fixed
- The Sub-agents tab lists every sub-agent the thread ever ran again, not only those in the part of a long thread that's loaded: after a restart it no longer drops to the last few, and picking an old one shows its steps and report.
- When your shell is slow to start, `gh`, `glab` and other Homebrew tools are still found: the usual tool folders are added to PATH. Settings → Agents says how the environment was read.
- Long task steps wrap inside the thread's tasks popover, which now scrolls instead of running off the screen.
- Pictures attached to a message, and pictures Claude shows, are thumbnails of a set size again, instead of drawing at full size over the message and the rows below.
- Pull requests load again in the right sidebar when the app is opened from the Dock or Finder: looking them up now finds `gh` and `glab` on your shell's PATH.
- The conversation and a sub-agent's detail have a scroll bar again, following your Mac's Show scroll bars setting: while scrolling, or always.
- A web page in the browser no longer takes your typing. Pages that focus a field on their own (YouTube, sign-in forms) used to pull the keyboard out of the composer, so Space, Shift+Space and other keys went to the page. Now the page gets the keyboard only after you press Take Control, and Give Back returns it to the app.
- Opened work no longer has an empty "Thought" line between steps where Claude Code keeps the thinking to itself, and a refused tool call reads without its `<tool_use_error>` tags.
- `/compact` no longer shows the output of `/context` and other built-in commands a second time.
- Reading back while Claude works, "Working…" no longer sits on top of the conversation's text: it floats in a pill of its own, as the jump buttons do.
- Claude's actions in the browser no longer take two seconds longer each while the browser isn't showing (you're reading the thread, or another one is open): its pages wait off screen where they keep drawing.
- An imported state's Home runs in this Mac's folder (a development run's own `home`), not the exporter's, and an export names the app and build that made it.
- Without `claude`, a thread says "Couldn't find a working `claude` CLI. Check Settings → Agents."
- A message sent while a thread was still loading no longer shows twice.
- A finished turn no longer sends its notification twice.
- Stop (the button or ⌘.) puts queued messages back in the composer first, and a message sent while Claude works waits above the composer instead of also showing in the thread; it joins the thread where Claude reads it, marked Steered or Interrupted.
- A session that can't be resumed says so and the next message starts a new one; queued messages come back to the composer if Claude exits.
- Changing the effort while Claude works no longer ends the turn: it applies once Claude is idle.
- A thread whose worktree was removed runs in its project's folder again.
- A sub-agent's steps and report are kept across relaunches; several MCP questions at once each wait their turn, and a page-mode one closes when you finish on the page.
- The first thread you start right after launch is told how `gh stack` makes stacked PRs too, not only the ones after it.
- VoiceOver reads the sidebar, the thread's header and the composer: the thread list and the footer's buttons were missing from what it saw, icon-only buttons now say what they do, and the Model, effort and Permissions pickers say their current choice.
- Switches have a white knob in dark mode, as macOS's do.
- What's New and Release Notes open in the middle of the window, like the app's other dialogs, instead of beside the button you clicked.
- The sidebar's update card shows What's New as a link lined up under the version, instead of plain text set off to the right.
- ⌘K's matches in conversations read as plain text, without the reply's `**` and backtick marks.
- VoiceOver reaches the rest of the sidebar: Show more, the Settled shelf, Add Project, and each project's collapse, Memory and New Thread buttons.
- Memory's Graph keeps a few memories together in the middle of the page instead of flinging them to its far corners.
- A Claude Code session's title (Resume Session, ⌘K) is cut on a word with an ellipsis instead of mid-word.
- In Settings → General, scrolling over the title prompt scrolls the page once the prompt reaches its top or bottom, instead of stopping there.
- VoiceOver reads the conversation: your messages, Claude's replies, notices, sub-agents, a turn's "Worked for …" line and each step by what it did and on what, with whether it's open, running or failed.
- The Pull Requests tab shows a GitHub stack whole when the thread worked on one of its PRs (it said "3 PRs" over one, "into" the PR below rather than the trunk), shows your branch's unsubmitted `gh stack` as soon as the project is read instead of 15 seconds later, and long titles end in an ellipsis.
- `/mcp` and Settings → MCP no longer list the app's own servers (browser, memory, threads, harness) under This session, and `/mcp disable all` no longer turns off Claude's browser and memory with yours.
- Claude's browser `navigate` with `url: "back"` (or forward, reload) goes back instead of searching the web for the word, and a page that fails to load is named in the error, not the page before it.

## [0.0.16] - 2026-11-04

### Added
- `/mcp` opens your MCP servers instead of sending you to the terminal: what each is doing, its tools, an on/off switch per project, Retry for one that won't start (or ask Claude to fix it), and sign-in for servers on the web, in your own browser so your company's sign-in works. `/mcp enable`, `disable` and `reconnect` with a server's name do it straight away.
- Add MCP servers from `/mcp` without the terminal: paste a server's URL and sign in through your browser, or give a command or paste a README's JSON, for every project or just this one; or bring over Claude Desktop's. Remove one from its page. Claude Code in the terminal sees the same servers, and a running thread picks up a new one right away.
- Claude asks you to sign in to an MCP server it needs, right in the thread, instead of sending you to the terminal: Sign In opens the server's page in your browser, and Claude carries on in the same turn once it's connected. Not Now tells Claude no.
- When an MCP server needs you (to sign in, or because it won't start), a small label by the composer says so; click it for `/mcp`.
- When an MCP server's tool needs something from you mid-task (a few details in a form, or a page to visit, such as confirming in your browser), it asks above the composer instead of the turn hanging.
- Prompts from your MCP servers show in the `/` menu as `/server:prompt`, with an MCP label, as soon as the server connects, and run when you pick or type them.
- Settings → MCP lists the MCP servers you have for every project, and claude.ai connectors, with sign-in, adding and removing.
- Your Mac stays awake while Claude works, so long turns keep going when you step away. A small cup in the sidebar shows when it's on and keeps the Mac awake for an hour, a few hours or until you stop it. Settings → General → Sleep turns it off, keeps it awake whenever the app is open, gives a few minutes' grace after the last turn, or keeps the display on too.
- Click a picture or video in a conversation, yours or one Claude showed you, to see it large in Quick Look, with full screen and the arrow keys going through the others in the message. Videos you attach show their first frame, and a video Claude shows has a button to open it large while it plays. Pictures and files waiting in the composer open in Quick Look on a click too. Preview and QuickTime Player are still in the right-click menu.

### Fixed
- The right sidebar's toolbar button stays when the sidebar is closed, so there's a visible way to open it again.

## [0.0.15] - 2026-11-01

### Changed
- Threads in the sidebar stay in the order they were made, newest first, instead of moving as they become active. Pinned threads still go on top.
- The transcript is a new native list. Going back to a thread you've been in takes a few hundredths of a second and finds it exactly as you left it; a thread you haven't opened yet answers the click at once, with its rows right after. Scrolling stays smooth whatever else is going on.
- A turn's opened work shows every step as rows of their own in the same box, however many hundreds there are.

### Fixed
- The freeze that ended in a crash. Going to the top of a thread (and likely other jumps) while a turn streamed could stop the window for good, or until macOS ended the app.
- Scrolling with the pointer over a code block moves the conversation; sideways still scrolls the block.
- Regenerating the title of a long thread no longer freezes the window while its history is read.
- The window stays responsive when a turn prints very long output, and another thread's agent reading a long thread no longer holds it up.
- The prompt of a new thread no longer fades over the conversation when you leave it.
- macOS doesn't put the app to sleep behind your other windows while Claude works, so requests for permission and notifications come on time.
- Memory comes back down after long turns: a thread that's let go of no longer keeps its session's tool output.

## [0.0.14] - 2026-11-01

### Added
- A loading indicator while a long thread's history reads in.

### Fixed
- A very long turn folds its older work into the Worked row as it goes, where you can still open it, so the conversation doesn't keep growing while Claude works.
- Opening a turn's work with hundreds of tool calls no longer freezes for seconds, and many long threads loading at once share the window's time instead of stalling it.
- The window no longer waits behind other threads' histories being read, which could freeze it for a second or more with many threads going.
- Opening or sending to a very long thread no longer stalls the window while its history loads, and a thread no longer goes blank for a moment when a long turn ends.
- Fixed freezes while Claude works in long threads with the pointer resting over the conversation, some lasting minutes. Copy buttons under messages now keep their space, so hovering doesn't move anything.
- A background command's output no longer draws over its header and your attachments when you open it above the composer.
- Many threads working at once stay much smoother: threads out of sight no longer redraw, and long threads no longer go through all their rows on every update.
- Long threads give memory back after a turn ends.
- A long thread no longer freezes the app when you send to it, and saving while Claude works stays quick however long the thread gets.

## [0.0.13] - 2026-10-30

### Added
- **Files tab.** The right sidebar has a Files tab: the thread's folder (its worktree, if it has one) as a tree, and the files you open from it in tabs, read-only, with syntax colors and line numbers. Paths in the chat are links: in Claude's replies (`src/app.ts:12`, `app.ts:12-20`, `README.md#L5`, code spans too), in your own messages (`@mentions`), and the file in a Read, Edit or Write step. Clicking one opens the file at that line, marked, or for an edit, where the edit went. ⌘-click opens it in its own app. A link only appears for a file that's really in the folder; a name two files share isn't linked rather than linked to the wrong one, and nothing outside the folder is linked. Links you click reuse one preview tab (in italics) until you keep it with a double-click, so reading through a reply doesn't pile up tabs. A file Claude changes while you read it reloads where you were, and the tree follows files coming and going. Go to File finds any file by name (↑↓ and Return work). In a workspace, the files of every repo in it link and open too, even when its own repo ignores them. Files git ignores are dimmed. Big files open as fast as small ones (only what's on screen is laid out; files over 8 MB show their start, lines over 5,000 characters are cut, so a minified file doesn't freeze the app), pictures show as pictures, and the tabs, the folders you opened and the tree's state are kept with the thread. The sidebar's tabs are icons now, to fit five. See [docs/files.md](docs/files.md).

## [0.0.12] - 2026-10-29

### Added
- **Attach any file, not just images.** Drop, paste or pick (the paperclip) PDFs, logs, CSVs, spreadsheets, Word files, code, whole folders: anything Claude Code can work with. Images still go to Claude inline; other files are kept with the thread and Claude is told where they are, then opens them with its Read tool (text, PDFs, notebooks) or a command for other formats, as it does with a path dragged into Claude Code in the terminal. Reading them never asks for approval. Folders, and files over 200 MB, are read where they are. In the message they show as a chip with the file's icon, name and kind; click to open it, or Show in Finder.

## [0.0.11] - 2026-10-29

### Added
- **See what Claude is waiting on.** When Claude leaves something running in the background (a build, a dev server, a loop sampling a metric, a watch on a log), a strip above the composer says "Claude is waiting on" with each job, how long it's been going and the last line it printed. Open a job to see the end of its output as it comes, or stop it. The thread shows an hourglass in the sidebar until Claude is back, and you get one "finished" notification when it's really done, not one each time it checks in. When a job ends, the thread says how ("failed with exit code 1 after 35s"). A long command Claude waits on in the turn shows its running time and latest output line instead of only a spinner.
- **Background jobs keep running.** A thread with a job running is no longer put to sleep after 10 idle minutes, settled or unloaded, which stopped the job. Quitting with jobs running asks first, and the thread notes which jobs quitting stopped.

### Fixed
- **Threads with many PR links are fast again.** Each PR link a thread mentioned was looked up again (its pattern rebuilt from scratch) every time the toolbar or the right sidebar redrew, and every few minutes for every thread at once. A thread with a hundred links took a second to switch to and made the whole app hitch. Switching to one now takes about a third of that, and the check every few minutes no longer makes the app hitch.
- **A thread no longer stays blank after a turn in a big window.** 0.0.10 fixed this for a normal-sized window; with the window filling the screen, a turn's work folding away could still leave nothing but the composer until you sent a message. A thread found that way now rebuilds its view within about a second, at its end.
- **Sending a message in a long thread keeps you with the reply.** Going to your new message could land a little short while the rows above were still being measured, and that was taken for you scrolling up: the reply then streamed in below the screen.

## [0.0.10] - 2026-10-29

### Added
- **See what you've spent.** Click the usage box at the bottom of the sidebar (or View → Usage, ⇧⌘U) for a big usage view: your 5-hour and weekly limits with when they reset, then the tokens you used over the past week, month, 90 days, 180 days or year (input, output, cache reads and writes), what they would have cost at Anthropic's API prices, the share each model took, a chart per day in tokens or dollars (stacked, or a row per model), and a day-by-day list. The numbers come from Claude Code's own history on this Mac, read in the background, so sessions you ran in a terminal count too, plus this app's side chats and memory agents. History stays in the view after Claude Code deletes old sessions. The range you pick is remembered. Esc closes it.
- **Claude can look after your threads.** Ask a thread to find an earlier conversation, read what it worked out, rename or settle threads, pin one, or bring in a session you ran with Claude Code in a terminal, and it does it without taking you anywhere. It can't delete threads. A thread in Home reaches all your projects; a thread in a project only that project's threads, and only Claude Code sessions run in its folder. It changes threads only when you ask, and checks with you before changing more than two.
- **Titles keep up with the conversation.** When a thread turns to something else, Claude has its title rewritten, by the same title model and in the same style as the first one. A name you gave the thread yourself stays. Turn it off in Settings → General → Thread Titles, where you can now also edit the prompt titles are written by.
- **Export State and Import State.** File → Export State… puts everything YourHarness has into one file: your projects, threads, memories, settings and images, the window layout, and which app version, Mac and tools it came from. Your browser sign-ins go in only if you tick the box. Usage history goes too, side chats' included. Import State brings a file back. It first shows what's in the file next to what you have now and warns that everything will be replaced. It saves a copy of your current state unless you say not to, then restarts. Use it to move to another Mac, or to send someone the exact state a bug happens in.

### Fixed
- **A thread no longer goes blank when a turn ends.** When Claude finished and its work folded away, a thread could show nothing but the composer, and stay that way when you went to another thread and back, until you sent a message. Rows that faded in and out while Claude worked confused the list underneath, so they now just appear. If a thread is ever found showing nothing anyway, it goes to its end on its own, and notes what it saw in the data folder's `diagnostics` (for Debug → Capture Transcript State).
- **The conversation keeps up with Claude.** During a long turn the transcript could stop following on its own: when a long answer streamed past the bottom, a big block of output arrived at once, or the view was still scrolling to the previous row when the next one came. It then stayed put for the rest of the turn, and when the turn ended its work could fold away and leave you above the answer. It now stays at the end until you scroll up yourself, and picks up again when you scroll back down or jump to the end.
- **A working thread no longer keeps the Mac busy.** The turning Claude mark and the pulsing "Working…" redrew the whole window on every frame, about half a CPU core for as long as any thread worked, whichever thread you were looking at. They now animate the way macOS's own spinners do, for next to nothing (about 1% while a thread works, from about 30%).
- **"Edited N files" counts every file a turn changed.** It counted only Claude's edit tools, so files changed by shell commands, scripts or commits were missing (a turn that rewrote half the app said "edited 1 file"). It now asks git what changed in the folder between the start of the turn and its end, without touching your index or files. If another thread was working in the same folder at the time, only files this turn's own commands name count.
- **Switching to a thread full of diagrams is quicker.** Diagrams are drawn off the main thread, so switching into a thread that ends in a few big ones costs about a quarter less: no longer more than the same amount of text.
- **Jumping between messages works over tall replies.** With one reply taller than the screen in view (a big diagram, a long answer), Previous Message was turned off and ⌘[ did nothing, as if there were no message above.
- **A long draft no longer shows half a line at the top.** Once a draft passed 12 lines and scrolled, the bottoms of the line above the first one showed over it.
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
