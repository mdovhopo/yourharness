# YourHarness

Native macOS (26+, Apple Silicon only) harness over AI coding CLIs. Being rewritten in **Rust on
GPUI**, one process. Read docs/architecture.md first: layers, what the app must keep, the plan
and what each chunk covers. The SwiftUI app this replaces is frozen in `legacy/YourHarness`
(tag `swift-app-frozen`), and its Swift core and the superseded web rewrite are in
`legacy/Sources`, `legacy/web`. Read them for what a feature does, never port them one for one.
`legacy/CLAUDE.md` has the old app's notes.

## Layout

- `crates/yh-core`: storage (`db.rs`, `harness.sqlite` in the Swift app's format; `library.rs`,
  project and thread records), `claude` processes and stream-json (`claude.rs`), Claude Code
  history (`history.rs`), crash recovery (`recovery.rs`), markdown, and later memory, git, forge,
  MCP servers. No GPUI. `cargo test -p yh-core` reads the `extreme` preset when it's there.
- `crates/yh-app`: the app.
  - `state/`: what it knows, no drawing. `workspace.rs` owns projects, threads, selection,
    each thread's `claude` and every request (send, stop, allow, pin, rename…), and emits
    `WorkspaceEvent`s; `thread.rs` holds a thread's rows.
  - `ui/`: `root.rs` (window, transcript deck), `sidebar.rs`, `transcript.rs`, `rows.rs`,
    `composer.rs` (with the permission, question and plan cards).
  - `platform.rs` (AppKit), `probe.rs` (the perf run), `theme.rs`.
- `patches/`, `scripts/vendor.sh`: GPUI crates we patch, unpacked into `vendor/` (not
  committed). `gpui-pre-macos`: `GPUI_KEEP_DRAWING=1` draws a window parked off screen;
  `gpui-pre`: cached views draw anew while VoiceOver listens (a reused one dropped out of its
  tree); `gpui-component`: an icon-only button is named by its tooltip.
- UI building blocks come from **gpui-kit** (GPUI 0.3.11 + gpui-component, Apache-2.0):
  inputs, selects, popovers, dialogs, tables, charts, menus. Zed's own crates (ui, editor,
  markdown) are reference code only: GPL, and tied to Zed.

## Build and run

```sh
export PATH="$HOME/.cargo/bin:$PATH" DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
scripts/vendor.sh && cargo build --release && cargo test
scripts/bundle.sh                 # → build/YourHarness Next.app
APP=test-harness-root/next-test/app/"YourHarness Next.app" FEATURES=snapshots scripts/bundle.sh
```

The bundle ID is `com.mdovhopo.yourharness.next` until the switch, so it never shares
permissions or data with the user's app (`/Applications/YourHarness.app`, the frozen 0.0.16).
**Never point a development run at the user's real data folder**: pass `YH_DATA_DIR=<scratch>`
(a folder with a `harness.sqlite`, e.g. an APFS clone `cp -c` of `test-harness-root/extreme`),
and `YH_PATH="$PATH"` so no login shell starts and `claude` is found. Launch in the
background, never taking focus:
`open -g -n --env YH_BACKGROUND=1 --env YH_DATA_DIR=… --env YH_PATH="$PATH" --stderr <log> "<app>"`.
Without `YH_BACKGROUND=1` a window nobody looks at stops drawing, and a snapshot shows it as it
was seconds after launch (a finished turn still "working"). Test builds go
to their own folder (`APP=…`), never `build/`, which may be the copy the user has open. A test
run must never put a macOS privacy prompt on screen: never run `screencapture`. Use a
`FEATURES=snapshots` build and `YH_SNAPSHOT=<png>` (`Window::render_to_image`). Use Haiku and a
throwaway folder for real turns.

Launch options: `YH_DATA_DIR`, `YH_CLAUDE_HISTORY=1` (also list Claude Code's own sessions,
`YH_CLAUDE_CONFIG`, `YH_CLAUDE_LIMIT`), `YH_PROJECT=<folder>` (a project of its own, its mode
`YH_MODE`: `ask` shows permission cards, `plan` plan cards), `YH_SELECT=<title or id>`, and
`YH_SEND` / `YH_MODEL` / `YH_EFFORT` / `YH_ANSWER=allow|deny` (one real turn, prompts answered by
themselves, every waiting one by its own id; `YH_ANSWER_DELAY=<s>` answers that much later, so the cards
show and pile up: parallel sub-agents print `PERMISSIONS waiting: 2`). Also `YH_CLICK=<x,y>[;<x,y>…]` (clicks
in window points through the window, a second apart, after `YH_DRAFT_DELAY` + 0.5 s: the composer's pickers,
the plan card's Approve menu; `YH_CLICK_WINDOW=<title>` sends them, and `YH_WHEEL`'s, to that window instead:
`Settings`), `YH_WHEEL=<x,y,dy,count>[;…]` (`count` pixel wheel events of `dy`, positive scrolling up, at that point
through the window as a real wheel goes, same delay; after each step `WHEEL …` prints the open transcript's top row
and every capped tool box that can scroll, `key:offset/max`: for the 320 pt boxes and sideways code blocks; with Settings
open, also where its page and title prompt stand),
`YH_AX_DUMP=<s>` (the window's accessibility tree as VoiceOver reads it, from inside the app so no permission
prompt, printed as `AX …` lines; never dump another app's tree from outside, which needs Accessibility access),
`YH_RUNTIME_LOG=1` (each thread's `claude` started and ended, `RUNTIME …`: its effort, whether `gh stack`'s
instructions are in its prompt, an effort change mid-turn starting it over once the turn ends), `YH_DROP=<x,y>` with `YH_DROP_FILES=<a:b>` (files dragged in from Finder
through GPUI's file-drop events after `YH_DRAFT_DELAY`, held `YH_DROP_HOLD` s (2) so a snapshot shows the
overlay, then dropped), `YH_PREVIEW_MEDIA=1|draft` (the thread's last pictures, or what waits in the composer,
in Quick Look, printing `PREVIEW PASS`; it shows on screen briefly), `YH_OPEN_BROWSER_WINDOW=1` (the open
thread's browser in its own window after `YH_DRAFT_DELAY`, printing where the page went;
`YH_BROWSER_WINDOW_SNAPSHOT=<png>` draws that window, `YH_BROWSER_WINDOW_CLOSE=<s>` then closes it, the pages
going back to the sidebar; with `YH_BROWSER_OPEN=<url>`), `YH_BROWSER_PROBE=<steps.json>` (browser tool calls,
`[{"tool": "click", "args": {"selector": "#go"}}, {"sleep": 1}]`, run on the selected thread's browser at launch
as the agent makes them, a new thread in the first project if none is selected, so give it a `YH_PROJECT`; each
result printed, screenshots next to the file as `<steps>.<n>.jpg`, then `== place …` and `== done`;
`YH_BROWSER_PROBE_QUIT=1` quits after; `YH_BROWSER_AUTO_SHOW=0` keeps the page parked, the sidebar closed;
`YH_BROWSER_PROBE_THREAD=<id>` runs it on that thread while `YH_SELECT` opens another; steps may also be
`{"control": true|false}`, Take Control and Give Back as the panel's button does, and `{"crash": true}`, the tab's web
content process ended as a crash would). Browser panel checks: `YH_BROWSER_ACTIVITY_LOG=1` prints each time the page
shows where the agent acts (`YH_BROWSER_ACTIVITY_SHOT=<png>` draws that overlay alone, which screenshots leave out),
`YH_BROWSER_DIALOG_ANSWER=ok|cancel` answers a page's alert, confirm or prompt through the card you'd click,
`YH_BROWSER_PANE=console|network|none` picks the log strip's pane or folds it (`YH_BROWSER_ERRORS_ONLY=1` filtered),
`YH_BROWSER_CONFIRM_CLEAR=1` shows Clear Browsing Data's question, and Import Sign-ins takes
`YH_COOKIE_SITES=<Cookies.binarycookies>` (a synthetic Safari-format file shown as its sites step; `YH_COOKIE_IMPORT=all`
then selects all and imports, printing `COOKIES imported …`) and `YH_COOKIE_FAILED=access` (what Safari's cookies say
without Full Disk Access). After
changing anything in `browser/`, `webview.rs` or the browser panel or window, run `scripts/browser-fidelity/run.sh
[out dir]` (a few minutes; `SKIP_BUILD=1` reuses the copy in the out dir, `RUNS=panel` for one): it renders test
pages in a plain, unscaled WKWebView (Safari's engine) at every page size and checks the built-in browser against
it, what the page measures and how screenshots look (full page too), over the right sidebar, parked and in its own
window, and its input against a real mouse (hover, exact clicks, pointer drags, HTML drag and drop, sliders,
scrolling). The browser is how Claude sees the UI it builds, so this must pass. Also
`YH_SEND_FILES=<a.png:b.pdf:folder>` (with it: images inline, the rest by path; alone: attached to the
selected thread's composer, as a drop does), `YH_NEW_THREAD=1` (a new thread in the first project at launch:
its empty state), `YH_QUEUE=<text>` (sent `YH_QUEUE_DELAY` s after launch, 3, so it queues behind the
`YH_SEND` turn; `YH_QUEUE_ACTION=now|edit|remove` then does what its buttons do, `YH_QUEUE_ACTION_DELAY` s later), `YH_STOP=<s>` (⌘. that long after launch, through the window: prints `STOP pressed` and what the composer holds 3 s later),
`YH_SEND_SEQ=<a>|<b>|…` (sent to the selected thread one after another, each once the last turn is over: for
`/btw`, `/model`, `/effort`, `/rename`, `/clear`, `/context`, `/usage`, or a second message naming a skill to chain),
`YH_DRAFT=<text>` (typed in the composer:
`/co` opens the command menu, `@no` the file menu; `YH_DRAFT_DELAY`), `YH_FIND=<words>` (Find
opened), `YH_EXPAND_WORK=1` (folded work drawn open), `YH_ACTIONS_PROBE=1` (pin, rename, settle,
delete on the first threads). Also `YH_SNAPSHOT=<png>` (`_DELAY`, `_QUIT`), `YH_BACKGROUND=1`,
`YH_TIMING=1` and `YH_EVENT_LOG=1` (every event, and each plan rebuild). `YH_NOTIFICATIONS=log`
prints notifications and the Dock badge instead (always in tests: a real one asks for permission),
`YH_APPEARANCE=light|dark`, `YH_OPEN_SETTINGS=1`, `YH_INSPECTOR_PANEL=prs|sideChat|subagents|files|browser`, `YH_MOCK_PULLS=<file.json>` (PRs read from
`{"<url>": {…gh pr view's JSON…}}` instead of `gh`), `YH_SIDE_ASK=<text>` (asked in the selected thread's side
chat after `YH_DRAFT_DELAY`) (the right sidebar open on it), `YH_OPEN_FILE=<path[:line[-end]]>`
(opened in the Files tab as a link does, after `YH_DRAFT_DELAY`), `YH_OPEN_MCP=1` (the `/mcp` sheet, its servers printed; `=everywhere` for Settings → MCP; `YH_MCP_ADD_COMMAND=<command line>` or
`YH_MCP_ADD_URL`, `YH_MCP_ADD_SCOPE=local|project|user`, `YH_MCP_ADD_SUBMIT=1`, `YH_MCP_REMOVE=<name>`: they write
Claude Code's config, so project scope in a scratch project, or a scratch `CLAUDE_CONFIG_DIR`; `YH_MCP_BROWSER=log`
prints sign-in pages instead of opening them; `YH_MCP_SELECT=<server>` opens that server's page), `YH_MCP_ELICIT_ANSWER=accept|decline` (what an MCP server asks
mid-call, answered with the form's defaults), `YH_MCP_SIGN_IN_ANSWER=allow|deny` (the card Claude puts up to sign in
to a server; with `YH_MCP_BROWSER=log` each such call prints `MCP sign-in asked: <server>` and the page; never
finish a real sign-in from a test), `YH_STATE_EXPORT=<file.yhstate>` (the whole state exported at launch, its manifest printed),
`YH_STATE_IMPORT=<file>` (imported at launch, development data folders only: never into the real app),
`YH_IMPORT_COOKIES=<Cookies.binarycookies>` (a Safari-format file's cookies into the selected thread's project's
browser: a synthetic file only, never your real browsers, which would ask you for access), `YH_OPEN_PROJECT=1` (the selected thread's project's page: PRs as stacks, repos, worktrees), `YH_OPEN_USAGE=1` (Usage; `YH_USAGE_RANGE=week|month|quarter|halfYear|year`, `YH_USAGE_METRIC=cost`,
`YH_USAGE_HOVER=<days ago>`, `YH_USAGE_PROBE=1`: the pointer moved and scrolled over the chart through the window's events, printing `USAGE PROBE PASS` when the longest main-thread stall is under 100 ms; test it on a clone of `test-harness-root/usage` with `CLAUDE_CONFIG_DIR` at its
`claude-config`, never the real `~/.claude`), `YH_MEMORY_AGENT=importer|reconciler` (with `YH_MEMORY_AGENT_START=1`, `YH_MEMORY_IMPORT_SOURCES=<a:b>`),
`YH_MEMORY_IMPORT=<file or folder>` and `YH_MEMORY_EXPORT=<folder>` (with `YH_OPEN_MEMORY=1`), `YH_OPEN_MEMORY=1` (the project's Memory page,
`YH_MEMORY_TAB=memories|tasks|activity`, after `YH_DRAFT_DELAY`), `YH_SPOTLIGHT=<words>` (⌘K typed, its results
printed as `SPOTLIGHT section · title · subtitle`; `YH_SPOTLIGHT_ENTER=1` with a session ID resumes it),
`YH_RESUME_SESSION=<id or claude --resume …>` (the Resume Claude Code Session sheet filled in, after `YH_DRAFT_DELAY`;
`YH_RESUME_CONFIRM=1` resumes into the suggested project; give it a scratch `CLAUDE_CONFIG_DIR` holding a copied or
synthetic session file, never the real `~/.claude`), `YH_OPEN_TASKS=1` (the header's tasks popover open),
`YH_HOVER=<x,y>` (the pointer moved there in window points after `YH_DRAFT_DELAY`, through the window as a real move:
snapshots of hover states such as a sidebar row's Pin/Settle/… or a project's Memory button),
`YH_REFRESH_LIMITS=1` (the footer's refresh at launch: a tiny Haiku request through `claude`, printing `LIMITS
refreshed` or why not), `YH_THREAD_ACTIONS=<a>,<b>,…` (after `YH_DRAFT_DELAY`, a second apart, on the open thread: `settle`,
`unsettle:<n>`, `delete`, `pin`, `unpin`, `undo` (⌘Z), `new` (⌘N), `memory` (⇧⌘M), `project`, `add:<folder>` (Add
Project), `next`, `inspector`, `settings`, `close` (the close button's quit question); each printed as `ACTION <step>: <what's
open> · <counts> · order: <the sidebar's live threads>`), `YH_QUIT_ANSWER=quit|cancel` (the quit question answered,
printed as `QUIT asked: …`; `YH_QUIT_AFTER=<s>` quits as ⌘Q, `YH_TERMINATE_AFTER=<s>` as Dock → Quit), `YH_MENU_LOG=1`
(each menu bar rebuild), `YH_SNAPSHOT_WINDOW=<title>` (`YH_SNAPSHOT` draws that window instead: `Settings`),
`YH_UPDATE_FEED` with `YH_RESTART_ANSWER=restart|later` (Restart's question answered and printed) and `YH_OPEN_VERSION=1` (the
sidebar's version card open), `YH_SETTLE_MERGED_AFTER=<s>` (when threads whose PRs merged first settle, 20; with
`YH_MOCK_PULLS`, printing `SETTLED merged <id>`), `YH_USAGE_SPLIT=1` (By Model), `YH_USAGE_DAY_PAGE=<n>`,
`YH_PROJECT_FILTER=<words>` (the project page's filter), `YH_MEMORY_OFF_ANSWER=ok|cancel` (Turn Off Everywhere's question),
`YH_STATE_EXPORT_SIGNINS=1`, `YH_STATE_DONE_ANSWER=ok` (Export's done alert printed, not shown), `YH_STATE_IMPORT_ASK=<file>`
(Import State's sheet; `YH_STATE_IMPORT_CONFIRM=1` then replaces, keeping a copy, and restarts), `YH_NOTIFICATION_CLICK=<seconds>` (a click on the last notification posted, which
`YH_NOTIFICATIONS=log` only printed as `NOTIFY <kind>: title · project · body`: it opens that thread, printing `NOTIFY
clicked`; real notifications go through a `UNUserNotificationCenter` delegate, never from a test).
`YH_MEMORY_TAB=memories|tasks|activity`, after `YH_DRAFT_DELAY`), `YH_SPOTLIGHT=<words>` (⌘K typed;
`YH_SPOTLIGHT_ENTER=1` with a session ID resumes it). Transcript: `YH_SCROLL_TO=<row>` (the open
thread's transcript starts at that row; negative counts from the end), `YH_SELECT_PROBE=1` (drags across the
last reply through the window's events, prints `SELECT PASS` and what ⌘C would copy, then clicks it away;
`=keep` leaves it selected for a snapshot; `YH_SELECT_PROBE_DELAY`), `YH_OPEN_STEP_FILE=<Read|Edit|Write>`
(the newest such step's file opens in the Files tab as clicking its path does), `YH_OPEN_DIAGRAM=<file.mmd>`
(its diagram in the Open Large window; `YH_DIAGRAM_EXPORT=<file.png>` saves it as Save as PNG does,
`YH_DIAGRAM_SNAPSHOT=<file.png>` draws that window). These need a selected thread (`YH_SELECT`): any
`*_PROBE` variable stops the last thread from opening again. Project page (with `YH_OPEN_PROJECT=1`):
`YH_PROJECT_PROBE=<steps>` once its repos are read, commas between steps: `expand` (every repo's worktrees
shown), `repos` (scrolled down to them), `remove:<worktree folder name>` (asks as its button does: the
question, or none when nothing's lost), `merge` or `merge:<up to #>` (the first numbered stack's Squash and
Merge question), `new-worktree`, `new-stack`, `add-branch` (their sheets); each question is printed as
`PROJECT asks: …`. `YH_PROJECT_PROBE_ANSWER=ok` then says yes or presses Create (it merges, removes and
pushes for real: the sandbox repo only), with `YH_NEW_WORKTREE_BRANCH=<name>` typed in New Worktree and
`YH_NEW_STACK_BRANCHES=<a:b>` picked in New Stack (Submit off). Right sidebar: `YH_FILES_PROBE=1` (with
`YH_OPEN_FILE`: the file scrolled through in 40 steps, printing how long it took to open and the longest
step's lines took to make, PASS under 50 ms; try `test-harness-root/files`' 100,000-line catalog),
`YH_FILES_WRAP=1` (Wrap Lines on), `YH_FILES_LOG=1` (prints each tree folder read again
and each open file reloaded from disk, with its scroll place, and Find's counts), `YH_FILES_FIND=<words>` (Find in the
first file opened, typed), `YH_PR_PANEL_LOG=1` (what the PRs tab lists, stacks and unsubmitted `gh stack` branches
included, each time it changes, and `PRS activity` / `PRS looking again` when Claude touches PRs),
`YH_SEARCH_PROBE=<words>` (how long until the full-text index, caught up in the background, finds them, and its first
hits), `YH_SUBAGENT_STOP=<seconds>` (Stop pressed on the open sub-agent that
long after it shows, printing `SUBAGENT stop <task>`). Memory: `YH_MEMORY_SETUP=importer|reconciler` (the
specialist's setup over the window, `YH_MEMORY_IMPORT_SOURCES=<a:b>` ticked, `YH_MEMORY_AGENT_NOTE`,
`YH_MEMORY_AGENT_MODEL=haiku`; `YH_MEMORY_SETUP_START=1` presses Start: a real turn),
`YH_MEMORY_ADD_STEP=<text>` (with `YH_OPEN_MEMORY=1`: the first task opened and the step added),
`YH_MEMORY_PROBE=1` (links the first memory to the second, renames the first and restores its version 1, printing
`MEMORY probe: …`). MCP: `YH_MCP_ADD_ENV=NAME=value` (with `YH_MCP_ADD_COMMAND`: an environment variable added),
`YH_MCP_ADD_DESKTOP=1` (Import from Claude Desktop pressed; give it `YH_CLAUDE_DESKTOP_CONFIG=<scratch file>`, never
the real one).

## Performance is the product

Every feature ends with a real Claude turn on Haiku that uses it (a throwaway `YH_PROJECT`,
`YH_MODEL=haiku`), checked by snapshot or the saved data, then a push. Full perf runs
(`scripts/perf-run.sh <results>`: the `extreme` preset, a fake `claude` streaming 20 huge threads,
the probe's phases; `YH_PERF_PHASES`, `YH_PERF_SECONDS`, `SKIP_BUILD=1`, `FORCE=1` on battery)
take ten minutes, so they're kept for the end of big chunks: the user allows one more at most
and one when the rewrite is whole. Judge by frames on time, worst frames and clicks (docs/
performance.md), not averages; no `INVALID` lines; `PERF stall` lines say when and in what
phase a frame ran long. `YH_PERF_FRONT=1` takes over the screen, so only when the user is away. A
slowdown counts once a baseline build shows it isn't there under the same conditions; a fix once
the test fails without it, over several runs. Rules from measurements (docs/architecture.md):

- The transcript is GPUI's `list`; a change splices exactly the rows it touched.
- Views that change rarely are cached; nothing that changes often lives in a view that's always there.
- Parsing and I/O off the main thread; Claude's events applied at most once a frame.
- Hold only recent rows; parked transcripts don't redraw.
- Animate only what fades; hover never changes layout.
- Never send events to the window from inside an entity update; hold off App Nap in test runs.

## Principles

- **Performance and memory over looks.** The user gave up Liquid Glass and native controls for
  stable frames. Still clean and Mac-like: system font, macOS colors in light and dark, standard
  shortcuts.
- **State lives in entities, views draw it.** No domain logic in views.
- **The user's data is sacred.** Keep `harness.sqlite`'s format. Never write to the real data
  folder from a test.
- **CLI protocols change often.** Parsers ignore unknown events and never crash on new fields;
  test them against the recorded sessions in `legacy/Tests/HarnessCoreTests/Fixtures/`.
- **Never extract or reuse CLI credentials.** Always drive the official binary.

## Working autonomously

Push to `rewrite` after each finished feature. Watch the Claude session limit with
`scripts/limits.sh` (it costs one tiny Haiku request). When `five_hour` nears its limit,
commit, push, note the next step in the task, schedule a wake-up just after its reset, and
pause.

## Versions

Nothing is released from `rewrite` until it's the whole app. `main` keeps the frozen app and its
release scripts (`legacy/scripts` here). The pipeline is ready for after the switch:
`RELEASE=1 scripts/bundle.sh` builds the real app (bundle ID `com.mdovhopo.yourharness`, signed with
the app's own identity from `~/.yourharness/signing`), `scripts/dmg.sh` packs it with its `.sha256`,
`scripts/check-dmg.sh <dmg> [identity]` checks it as the updaters will, `scripts/release.sh <version>`
(from `main` only) tests, builds and checks the DMG, then tags and pushes, and
`.github/workflows/release.yml` builds it again and publishes it (run it by hand to test the pipeline
on a branch, artifact only: `gh workflow run release.yml --ref rewrite`), and the updater (`state/updater.rs`) installs it on every copy, the Swift app's included: same
bundle ID and signature. Never run a `RELEASE=1` build here: it shares your app's permissions and
data. Test the updater with `.next` copies and `YH_UPDATE_FEED=<release.json>` (`YH_UPDATE_DOWNLOAD=1`,
`YH_UPDATE_RESTART=1` run the whole flow; `VERSION_OVERRIDE=9.9.9 scripts/bundle.sh` makes the newer copy).
