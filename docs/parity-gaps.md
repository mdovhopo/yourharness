# Parity gaps (audit of 2026-11-07)

What the Swift app (legacy/YourHarness) does that the rewrite didn't yet, found by reading both.
Swift paths are under legacy/YourHarness/, Rust under crates/yh-app/src/. Tick items as they land.

## A. Safety and correctness
- [ ] 1. Worktree removal asks first ("Remove and Discard" when work would be lost); error alert on failure. Views/WorkspaceView.swift:1019-1035 · ui/project.rs
- [ ] 2. Merging a stack asks first ("Squash and Merge"). Views/StackViews.swift:56 · ui/project.rs
- [ ] 3. ⌘↩ / ⌘⌫ on question and plan cards: Answer (only once all answered) / approve in the recommended mode; Skip / Keep Planning. Views/AgentRequestCards.swift · ui/root.rs
- [ ] 4. Several permission requests at once (parallel sub-agents), each answered. Views/ThreadView.swift:40 · state/thread.rs
- [ ] 5. Long plans scroll inside the card's cap; question card capped too. AgentRequestCards.swift:167,206-225 · ui/composer.rs

## B. Transcript
- [ ] 6. Text selection in replies, tool results, thinking, notices.
- [ ] 7. Tool steps: icon, verb, key argument ("Read …/a/b.rs", "Run command"), readable browser/memory tool verbs, "Load browser tools". Views/ToolCallView.swift
- [ ] 8. A path in a Read/Edit/Write step opens in the Files tab at the line.
- [ ] 9. A running foreground command shows its last output line under its step.
- [ ] 10. Failed tool: error icon/red border; spinner while running; pretty JSON; Bash as `$ cmd`.
- [ ] 11. Browser steps: screenshot thumbnail, Show Browser button.
- [ ] 12. Floating jump buttons (top/previous/next/end).
- [ ] 13. Plan checklist inline at the newest task-tool call; one line for older ones. Views/PlanViews.swift
- [ ] 14. `/command` in your messages in bold mono; `@file` as links.
- [ ] 15. Right-click on media and attachments: Open Large, Preview/QuickTime, Show in Finder, Copy Path.
- [ ] 16. Sub-agent card: Follow in Sidebar, stats line, agent-type chip.
- [ ] 17. "Loading conversation…" while a thread opens slowly.
- [ ] 18. Diagrams: Open Large (zoom, fit), Copy Image, Save as PNG.
- [ ] Sent non-image files and folders in your message bubble (composer's `file_chip`).
- [ ] Failed background-job notes in error colour.

## C. Cards
- [ ] 19. Permission card: "Allow Run command?", tool icon, key argument, description; narrow layout.
- [ ] 20. Question card: Answer disabled until all answered; option descriptions inline; Swift's skip message.
- [ ] 21. Plan card: "Plan ready for review", recommended default (Approve · Auto) with others in a menu; Auto disabled when the model can't.

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
- [ ] 40. Drop files anywhere on the thread, with overlay (check after composer merge).
- [ ] 41. Model picker details (check after composer merge).

## G. Right sidebar
- [ ] 42. Sub-agent Stop button.
- [ ] 43. File viewer: syntax highlighting, image preview, Open in Default App for binaries, missing-file message, long-line notice.
- [ ] 44. Files toolbar/menus: Show/Hide Folder, Wrap Lines, More menu; tab menu; preview tabs.
- [ ] 45. PR rows: context menu, Merged and Closed group.
- [ ] 46. Split into a Stack button.

## H. Project page
- [ ] 47. Context menus on PRs, repos, worktrees.
- [ ] 48. New Worktree sheet: base branch, location.
- [ ] 49. Worktree/repo badges (detached, landed, nothing committed, missing folder, ahead/behind, not on disk, not found).
- [ ] 50. Stack actions: Merge Up to #N, Make GitHub Stack, Install gh stack, Add Branch on Top…, Sync, New Stack from Branches, disabled-Merge hints.
- [ ] 51. Header: New Thread, Show in Finder.
- [ ] Threads per worktree.

## I. Memory
- [ ] 52. Importer/Reconciler setup screens, model/effort per specialist, New Session, live Changes pane.
- [ ] 53. Task detail: Add a step.
- [ ] 54. Task rows: threads working on each.

## Browser
- [ ] Pick element, pop-out window, idle close.
