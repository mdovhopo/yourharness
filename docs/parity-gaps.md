# Parity gaps (audit of 2026-11-07)

What the Swift app (legacy/YourHarness) does that the rewrite didn't yet, found by reading both.
Swift paths are under legacy/YourHarness/, Rust under crates/yh-app/src/. Tick items as they land.

## A. Safety and correctness
- [x] 1. Worktree removal asks first ("Remove and Discard" when work would be lost); error alert on failure. Views/WorkspaceView.swift:1019-1035 · ui/project.rs
- [x] 2. Merging a stack asks first ("Squash and Merge"). Views/StackViews.swift:56 · ui/project.rs
- [x] 3. ⌘↩ / ⌘⌫ on question and plan cards: Answer (only once all answered) / approve in the recommended mode; Skip / Keep Planning. Views/AgentRequestCards.swift · ui/root.rs
- [x] 4. Several permission requests at once (parallel sub-agents), each answered. Views/ThreadView.swift:40 · state/thread.rs (`permissions` is a list; the composer shows the oldest with "N more requests waiting", each answered by its request id; the Dock badge, sidebar marks, notifications (the newest request) and side chat follow. Checked with a real Haiku turn: two parallel Task sub-agents each asking for Bash, both cards queued, both answered.)
- [x] 5. Long plans scroll inside the card's cap; question card capped too. AgentRequestCards.swift:167,206-225 · ui/composer.rs

## B. Transcript
- [x] 6. Text selection in replies, tool results, thinking, notices. (`ui/selectable.rs`: gpui-base's window selection, which survives the cached transcript; a drag across rows works, ⌘C copies. Not yet: your own messages with links or a /command, diagram labels; a selection whose end row leaves the list's drawn rows is dropped.)
- [x] 7. Tool steps: icon, verb, key argument ("Read …/a/b.rs", "Run command"), readable browser/memory tool verbs, "Load browser tools". Views/ToolCallView.swift (`ui/steps.rs`; icons from gpui-kit's default Lucide set)
- [x] 8. A path in a Read/Edit/Write step opens in the Files tab at the line.
- [x] 9. A running foreground command shows its last output line under its step.
- [x] 10. Failed tool: error icon/red border; spinner while running; pretty JSON; Bash as `$ cmd`. (The running mark is a still loader icon: a GPUI animation redraws its view, here the whole transcript, every frame.)
- [x] 11. Browser steps: screenshot thumbnail, Show Browser button.
- [x] 12. Floating jump buttons (top/previous/next/end). (At the end, following it, "previous" shows whenever the thread doesn't fit, as the list can't say its top row then.)
- [x] 13. Plan checklist inline at the newest task-tool call; one line for older ones. Views/PlanViews.swift
- [x] 14. `/command` in your messages in bold mono; `@file` as links. (A command at the start only, not chained ones later in the message.)
- [x] 15. Right-click on media and attachments: Open Large, Preview/QuickTime, Show in Finder, Copy Path.
- [x] 16. Sub-agent card: Follow in Sidebar, stats line, agent-type chip. (Stats from what the rewrite tracks: activity and steps; no tokens or running clock.)
- [x] 17. "Loading conversation…" while a thread opens slowly.
- [x] 18. Diagrams: Open Large (zoom, fit), Copy Image, Save as PNG. (A GPUI window; images through resvg from an SVG of the layout, labels in Helvetica Neue.)
- [x] Sent non-image files and folders in your message bubble (a chip of the transcript's own; the composer doesn't record non-image attachments yet: `YH_SEND_FILES` with a README sent none).
- [x] Failed background-job notes in error colour (saved as `notice_style` error, as are `claude` exits and failed turns).

## C. Cards
- [x] 19. Permission card: "Allow Run command?", tool icon, key argument, description; narrow layout. (`composer::permission_card`, from `ui/steps.rs`'s presentation; a command shows whole up to four lines; the buttons wrap under the request when the card is narrow, as in the side chat, which uses the same card.)
- [x] 20. Question card: Answer disabled until all answered; option descriptions inline; Swift's skip message.
- [x] 21. Plan card: "Plan ready for review", recommended default (Approve · Auto) with others in a menu; Auto disabled when the model can't. (Auto when the model's catalog entry has `supportsAutoMode`, else Accept Edits; feedback written before approving follows as your next message; Keep Planning sends the Swift app's words.)

## D. Sidebar
- [x] 22. Collapsible projects (saved; activity dot/count when collapsed; skipped by keyboard).
- [x] 23. Thread age ("5m", "3h", "2d").
- [x] 24. Hover actions on rows (Pin, Settle, …); pin glyph.
- [x] 25. New Thread and Search buttons at the top.
- [x] 26. "N PRs waiting for your review" badge on project headers.
- [x] 27. Hover Memory button on project headers; selected-project highlight; Home icon.
- [x] 28. ↑/↓ move between threads when the sidebar has focus.
- [x] 29. Home: Change Default Folder…; thread: Settle Automatically When Idle; Regenerate Title disabled while running, title dimmed while generating.
- [x] 55. Settled shelf open/closed and "Show N more" saved.
- [x] 56. Rename: Escape cancels.

## E. Usage footer
- [x] 30. Refresh limits button (tiny Haiku request, spinner, error state).
- [x] 31. Limits saved between launches (rateLimit, rateLimitUpdatedAt).
- [x] 32. Reset countdown, tooltip, pre-reset reading as 0, Opus/Sonnet windows labelled, orange 70% / red 90%, click opens Usage.
- [x] 33. App memory use in the footer.

## F. Window
- [x] 34. First-run empty state (Add Your First Project), No Thread Selected with New Thread.
- [x] 35. Thread header subtitle: project · worktree · model.
- [x] 36. Tasks chip opens a popover (state, progress, steps, Show in Memory).
- [x] 37. Notifications: click opens the thread, project subtitle, real body, sound, cleared on open.
- [x] 38. Spotlight: Recent Threads, Tasks, Memories, Projects, Actions; subtitles.
- [x] 39. Resume Claude Code Session sheet (preview, project choice, prefill from clipboard, Show Thread).
- [x] 40. Drop files anywhere on the thread, with overlay (check after composer merge). (A dashed "Drop files to attach" overlay that GPUI shows by its drag style, so nothing re-renders; checked through GPUI's own file-drop events, `YH_DROP`.)
- [x] 41. Model picker details (check after composer merge). (Model: "Claude Code" header, each model's description, Pinned Versions submenu, footnote; Effort: Auto plus each level's description; Permissions: each mode's long name and what it does, Bypass in red, Auto "Not available with …" when the model can't. No plan chip: the rewrite doesn't read the account's plan.)

## G. Right sidebar
- [x] 42. Sub-agent Stop button. (Its task ID now comes with `task_started`; checked that the probe gets it and calls `stop_task`, not yet on a sub-agent that kept running.)
- [x] 43. File viewer: syntax highlighting, image preview, Open in Default App for binaries, missing-file message, long-line notice. (Colouring is yh-core's light keyword/string/number/comment pass, line by line, each line starting in what the lines above left open, so block comments and multi-line strings colour whole for the languages that have them. The folder is watched (FSEvents, `state/file_watch.rs`): the tree reads changed folders again, an open file reloads keeping its place.)
- [x] 44. Files toolbar/menus: Show/Hide Folder, Wrap Lines, More menu; tab menu; preview tabs. (Wrap Lines draws the file in GPUI's measured `list`.)
- [x] 45. PR rows: context menu, Merged and Closed group.
- [x] 46. Split into a Stack button.

## H. Project page
- [x] 47. Context menus on PRs, repos, worktrees.
- [x] 48. New Worktree sheet: base branch, location.
- [x] 49. Worktree/repo badges (detached, landed, nothing committed, missing folder, ahead/behind, not on disk, not found).
- [x] 50. Stack actions: Merge Up to #N, Make GitHub Stack, Install gh stack, Add Branch on Top…, Sync, New Stack from Branches, disabled-Merge hints. (New Stack from Branches checked end to end in the sandbox, without submitting; merge, link, sync and add only as far as their questions and sheets, so the shared sandbox's stacks stay. A command's progress and errors show in the page header, not on each stack.)
- [x] 51. Header: New Thread, Show in Finder.
- [x] Threads per worktree. (A chip per checkout with a menu of its active and settled threads.)

## I. Memory
- [x] 52. Importer/Reconciler setup screens, model/effort per specialist, New Session, live Changes pane. (The setup is a dialog over the window, not a window of its own; the specialist's model and effort, Changes, Open Memory and New Session are in its thread's header, Changes as a menu read when opened; Set Up… shows there before it starts. Specialists opened from ⌘K still start without the setup.)
- [x] 53. Task detail: Add a step.
- [x] 54. Task rows: threads working on each.
- [x] Memory detail: Preview as Markdown (default) and Edit, Link To and Unlink (links both ways), Restore This Version, Copy ID, Merged into. Activity: Show filter, the thread each event happened in.

## Right sidebar, second pass (2026-11-08)
- [x] Files: tree read from disk a folder at a time and watched, ignored files dimmed, row menus, ⌘F in the viewer.
- [x] PRs: stacks as a card with a lane graph, unsubmitted `gh stack` layers, PRs of every written checkout's branch (from the project's scan, saved with the thread as `workedPullRequests`), re-synced 3 s and 12 s after Claude touches PRs, polled every 15 s.
- [x] Sub-agents: steps as the transcript presents them (`steps::present`, Markdown, selectable output; not the transcript's own rows, which are tied to its list), live activity, "in the background", agent type in the run list.
- [x] /mcp: Remove asks first; sign-in wait with Open Again, Copy Link, pasted callback; Ask Claude to Fix It; Add Server by URL (headers, OAuth client ID, secret, callback port), command (environment) or JSON; Import from Claude Desktop; `McpSheet::new_embedded` for Settings. Sign-in card's Open Again; elicitation switches and menus.
- [x] Full-text index catches up a few seconds after launch without a write.

## Browser
- [x] Pick element, pop-out window, idle close. (Open in Window in the panel's ⋯ menu: the thread's pages move to a window of their own, the panel then says so with Show Window and Move Back Here; closing the window brings them back. Its own window counts as looked at for idle close.)
- [x] Quick Look on what waits in the composer (click; Quick Look in its menu).
- [x] The composer grows for a draft put there (a browser pick, `YH_DRAFT`, a queued message taken back), as typing does (auto-grow to 15 lines, then it scrolls).

## Second audit (2026-11-08): all three areas, file by file

Three read-only audits went through every Swift source file against the rewrite (thread and composer; window, sidebar, pages and app; right sidebar, browser, memory and MCP), not trusting the ticks above. About 80 gaps, every one fixed and checked by an agent per area (commit messages say what and how; CHANGELOG lists them for readers):

- [x] Thread (22 + 6 from the smoke test): the login shell's environment for `claude` (yh-core shell_env.rs; `YH_PATH` skips it), Stop as a quiet "Interrupted", ⌘. like the Stop button, find marks words and opens folded work, plan strip only while working, failed resume clears the session, title refinement with linked PRs, `status` / `task_progress` / `elicitation_complete`, the working indicator at the transcript's end, code blocks scroll sideways and tool boxes cap at 320 pt, job state in step headers, "edited N files" from git, the full slash menu and `/mcp enable|disable|reconnect`, the bolt button and steering placeholder, side chat's read-only memory and browser, `gh stack` instructions, removed worktrees, effort changes, video first frames, wording, right-click Copy / Look Up, focus on switch; sub-agent runs saved, queued messages once, empty plans hidden, ToolSearch labels.
- [x] App and window (26 + 2): closing the window quits (Dock reopens), creation order in the sidebar, the neighbour after settle/delete, ⌘Z for thread actions, reusing a blank thread, a resizable right sidebar, the full menu bar, the quit question on every path, the updater's What's New / Restart question / Cancel / Dismiss / "latest" / "Updated to", auto-settle on merged PRs, removed projects keep their ID, Add Project opens a thread, settle unpins and closes the browser, the Usage page's split / day table / Refresh / Esc, state transfer's sign-ins, comparison sheet and Home, Settings as its own window with MCP inside, project page empty state / filter / expand / nudge, the Settings rows, layout icons, the idle dot, App Nap, the sidebar toggle, the window kept on screen, ⌘N's project, the CLI error text, VoiceOver labels where GPUI allows.
- [x] Browser (13 + 1): popups and `target=_blank` as tabs, `window.close`, crash reload, Take Control, tabs saved with the thread, the JS dialog card, document statuses, refs across pages, this thread's dev servers, Stop / appearance / Web Inspector / Clear confirmation / log strip, the activity overlay, picked element pictures, `select` by `value`, cookie import details; parked pages in an off-screen window (agent actions 2.5 s → 0.18 s while hidden).
- [x] Panels, memory, MCP (11 + 1): the file tree and open files follow the disk, find in file, tree menus, ignored files dimmed, stacks as a lane graph with local `gh stack` layers, PRs looked at again after Claude touches them and found through every checkout, sub-agent detail, memory Preview / links / restore / Merged into / Activity filter, /mcp's remove question, sign-in help, Fix It, Add Server's fields, Claude Desktop import, the sign-in card's Open Again and form controls, the search index catching up at launch.
- [x] Data: 0.0.6's `state.json` and `threads/` imported into `harness.sqlite` as the Swift app did (checked on `legacy-0.0.6`); what the rewrite writes reads back in 0.0.16 (claudeModels, claudeMemorySyncedAt).

Exercised live (2026-11-08), each in a test copy on scratch data:
- [x] Wheel in a capped tool box (a real `seq 1 300` Bash output, `YH_WHEEL` through the window): the box moves while it can and the transcript holds; at the box's top or bottom, and over a sideways code block, the transcript moves.
- [x] A video's first frame (an AVFoundation-made .mp4 sent with `YH_SEND_FILES`): the tile shows the frame with a play badge, light and dark.
- [x] The whole update flow with two `.next` copies (0.0.15 and a 9.9.9 DMG from `scripts/dmg.sh`, ad hoc, a `file://` feed): What's New, download, checksum, staging, "9.9.9 is ready", Restart's question (`YH_RESTART_ANSWER`), the bundle replaced where it was and opened again, "Updated to 9.9.9".
- [x] VoiceOver, as far as a tree dump goes (`YH_AX_DUMP`, read inside the app): the sidebar (threads by title and state), the header, the footer and the composer have roles and names. Fixed on the way: cached views (sidebar, footer, transcript) dropped out of the tree when reused (GPUI patch), icon-only buttons had no name (gpui-component patch: their tooltip), the pickers now say their choice. Not reached: transcript rows aren't in the tree at all, and nobody has listened with VoiceOver itself.
- [x] Settings: every tab, light and dark, every General row seen; switches (notifications, display, plans, browser ×2, diagrams) and pickers (appearance, default effort) clicked through the Settings window (`YH_CLICK_WINDOW`) change what's stored. Fixed: switch knobs were near-black in dark mode.
- [x] Project page on 8 repos and 10 worktrees: the filter shows, narrows to matches, says "Nothing matches"; Collapse All / Expand All.
- [x] MCP: a local stdio server's form (text, switch, menu) drawn in the thread, the switch and menu driven by clicks, the answer reaching the server (`accept`, `subscribe: false`, `color: blue`); the sign-in card for an `http` server needing OAuth, Not Now / Sign In, and with `allow` the sign-in page printed, never opened.
- [x] `gh stack` instructions in the prompt (`YH_RUNTIME_LOG`). Fixed: the first thread started at launch went without them (the check hadn't answered yet; now it looks at gh's extensions folder meanwhile).
- [x] Effort changed mid-turn (`/effort high` queued during a Haiku turn): the turn finishes, `claude` ends once it's over and the next message starts it with `--effort high`. An effort changed while idle now ends the idle `claude` at once, as the Swift app did.

Left open: a multi-line field (Settings' title prompt) keeps the wheel at its end instead of passing it to the page.

Not in the Swift app either, so not gaps: URL scheme, Services provider, AppleScript, Spotlight indexing, a Dock menu, drop on the Dock icon, login item, multiple main windows, an app icon.
