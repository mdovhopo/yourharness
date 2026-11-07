# Parity gaps (audit of 2026-11-07)

What the Swift app (legacy/YourHarness) does that the rewrite didn't yet, found by reading both.
Swift paths are under legacy/YourHarness/, Rust under crates/yh-app/src/. Tick items as they land.

## A. Safety and correctness
- [x] 1. Worktree removal asks first ("Remove and Discard" when work would be lost); error alert on failure. Views/WorkspaceView.swift:1019-1035 · ui/project.rs
- [x] 2. Merging a stack asks first ("Squash and Merge"). Views/StackViews.swift:56 · ui/project.rs
- [ ] 3. ⌘↩ / ⌘⌫ on question and plan cards: Answer (only once all answered) / approve in the recommended mode; Skip / Keep Planning. Views/AgentRequestCards.swift · ui/root.rs
- [x] 4. Several permission requests at once (parallel sub-agents), each answered. Views/ThreadView.swift:40 · state/thread.rs (`permissions` is a list; the composer shows the oldest with "N more requests waiting", each answered by its request id; the Dock badge, sidebar marks, notifications (the newest request) and side chat follow. Checked with a real Haiku turn: two parallel Task sub-agents each asking for Bash, both cards queued, both answered.)
- [ ] 5. Long plans scroll inside the card's cap; question card capped too. AgentRequestCards.swift:167,206-225 · ui/composer.rs

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
- [ ] 20. Question card: Answer disabled until all answered; option descriptions inline; Swift's skip message.
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
- [x] 43. File viewer: syntax highlighting, image preview, Open in Default App for binaries, missing-file message, long-line notice. (Colouring is yh-core's light keyword/string/number/comment pass, line by line, so a comment or string spanning lines only colours its first line; the file isn't watched for changes.)
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

## Browser
- [x] Pick element, pop-out window, idle close. (Open in Window in the panel's ⋯ menu: the thread's pages move to a window of their own, the panel then says so with Show Window and Move Back Here; closing the window brings them back. Its own window counts as looked at for idle close.)
- [x] Quick Look on what waits in the composer (click; Quick Look in its menu).
- [x] The composer grows for a draft put there (a browser pick, `YH_DRAFT`, a queued message taken back), as typing does (auto-grow to 15 lines, then it scrolls).
