# YourHarness

Native macOS (26+, Apple Silicon only) harness over AI coding CLIs, in **Rust on GPUI**, one
process. Read docs/architecture.md first: the crates, threads and their `claude`, the tools the
app serves, storage, the browser, and what keeps it fast. The other docs cover one area each
(storage, browser, files, MCP, memory, design, workspaces, performance).

## Layout

- `crates/yh-core`: storage (`db.rs`, `harness.sqlite` in the format 0.0.x wrote, so 0.0.16
  still opens it after a downgrade; `library.rs`, project and thread records; `legacy_import.rs`,
  the JSON files of 0.0.6 and earlier), `claude` processes and stream-json (`claude.rs`), Claude
  Code history (`history.rs`), crash recovery (`recovery.rs`), markdown and diagrams, memory, git
  and forges, the in-app MCP servers. No GPUI. `cargo test -p yh-core` reads the `extreme`
  preset when it's there.
- `crates/yh-app`: the app.
  - `state/`: what it knows, no drawing. `workspace.rs` owns projects, threads, selection,
    each thread's `claude` and every request (send, stop, allow, pin, rename…), and emits
    `WorkspaceEvent`s; `thread.rs` holds a thread's rows.
    `thread_tools.rs` and `harness_tools.rs` serve agents the `threads` and `harness` tools.
  - `ui/`: `root.rs` (window, transcript deck), `sidebar.rs`, `transcript.rs`, `rows.rs`,
    `composer.rs` (with the permission, question and plan cards), `inspector.rs` (the right
    sidebar), the pages (`project.rs`, `memory.rs`, `usage.rs`), `settings.rs`, sheets.
  - `browser/` and `webview.rs` (the agents' browser, WKWebViews over the window),
    `platform.rs` (AppKit), `probe.rs` (the perf run), `theme.rs`.
- `patches/`, `scripts/vendor.sh`: GPUI crates we patch, unpacked into `vendor/` (not
  committed). `gpui-pre-macos`: `GPUI_KEEP_DRAWING=1` draws a window parked off screen;
  `gpui-pre`: cached views draw anew while VoiceOver listens (a reused one dropped out of its
  tree); `gpui-component`: an icon-only button is named by its tooltip; menus drawn as macOS's (13 pt rows,
  a hairline edge, one shadow, the lit row in `list_active`, which `theme.rs` sets to the soft accent).
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

A development build (`YourHarness Next.app`) has the bundle ID `com.mdovhopo.yourharness.next`,
so it never shares permissions or data with the user's installed app
(`/Applications/YourHarness.app`). **Never point a development run at the user's real data folder**: pass `YH_DATA_DIR=<scratch>`
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
the plan card's Approve menu; `r<x,y>` right-clicks, for context menus, and `h<x,y>` only moves the pointer, a menu
row lit; `YH_CLICK_WINDOW=<title>` sends them, and `YH_WHEEL`'s, to that window instead:
`Settings`), `YH_DRAG=<x1,y1,x2,y2>` (the left button dragged between those window points, same delay, then `DRAG selected …`: what ⌘C would copy), `YH_WHEEL=<x,y,dy,count[,dx]>[;…]` (`count` pixel wheel events of `dy`, positive scrolling up, `dx` sideways, at that point
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
`{"control": true|false}`, Take Control and Give Back as the panel's button does, `{"keyboard": true|false}`, the
Typing in Page toggle (`YH_TYPE_KEYS_CLICKS` then shows where your clicks put the keyboard), and `{"crash": true}`, the tab's web
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
`YH_SCROLL_BARS=always` shows the transcript's and sub-agent detail's scroll bars all the time, for snapshots (they follow the Mac's Show scroll bars setting otherwise). `YH_TYPE_KEYS=h,i,shift-space,x,space,y` types into the composer as real key events sent through the application (key equivalents first, as a key press goes; `shift-<key>`, `space`, `return`, letters and digits), `YH_TYPE_KEYS_DELAY` seconds after launch (3), then prints `TYPE composer holds "…"`; `YH_TYPE_KEYS_CLICKS=0.88,0.4;0.4,0.93` clicks those fractions of the window first (the browser's page, then the composer, as a person would) instead of focusing the composer. `YH_DRAFT=<text>` (typed in the composer:
`/co` opens the command menu, `@no` the file menu; `YH_DRAFT_DELAY`), `YH_FIND=<words>` (Find
opened), `YH_EXPAND_WORK=1` (folded work drawn open), `YH_ACTIONS_PROBE=1` (pin, rename, settle,
delete on the first threads). Also `YH_SNAPSHOT=<png>` (`_DELAY`, `_QUIT`), `YH_DEMO=<script.json>` (a scripted demo recorded to video with a drawn pointer, captions and zooms for the edit; `crates/yh-app/src/demo.rs`, scripts/demo/README.md), `YH_WINDOW_SIZE=<w>x<h>`, `YH_BACKGROUND=1`,
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
refreshed` or why not), `YH_ACCOUNT_ACTIONS=add,cancel:1,switch:1,move:1,off:0,on:0,signin:1,signout:1,refresh:1,remove:1,list`
(the Claude accounts' buttons after `YH_DRAFT_DELAY`, a second apart, by index, the main account 0: `switch` picks new threads'
account, `move` moves the open thread; each printed as `ACCOUNTS …` with the list and the open thread's account after; `add` and `signin` run `claude auth login`, which opens the browser, so give the run a `YH_PATH`
whose first folder has a stand-in `open` script; added accounts' folders link into Claude Code's own folder, so point
`CLAUDE_CONFIG_DIR` at a scratch one unless the test needs the real sign-in; `YH_ACCOUNT_REMOVE_ANSWER=ok|cancel`
answers Settings' Remove question), `YH_THREAD_ACTIONS=<a>,<b>,…` (after `YH_DRAFT_DELAY`, a second apart, on the open thread: `settle`,
`unsettle:<n>`, `delete`, `pin`, `unpin`, `undo` (⌘Z), `new` (⌘N), `memory` (⇧⌘M), `project`, `add:<folder>` (Add
Project), `next`, `inspector`, `settings`, `close` (the close button's quit question), `key:<keystroke>` (typed through the window: `key:alt-cmd-down`; not `cmd-,`, commas split steps); each printed as `ACTION <step>: <what's
open> · <counts> · order: <the sidebar's live threads>`), `YH_QUIT_ANSWER=quit|cancel` (the quit question answered,
printed as `QUIT asked: …`; `YH_QUIT_AFTER=<s>` quits as ⌘Q, `YH_TERMINATE_AFTER=<s>` as Dock → Quit), `YH_MENU_LOG=1`
(each menu bar rebuild), `YH_SNAPSHOT_WINDOW=<title>` (`YH_SNAPSHOT` draws that window instead: `Settings`),
`YH_PHONES=1` (Settings → Phone's server on at launch, not saved; `YH_PHONES_BIND=127.0.0.1` keeps it
off the network, so a Mac with its firewall on never asks; `YH_PHONES_LOG=1` prints the pairing link;
`YH_PHONES_TAILSCALE_ONLY=1` over Tailscale only; `YH_TAILSCALE=0`, `YH_TAILSCALE_STATUS=<file>`,
`YH_TAILSCALE_WHOIS=<file>` stand in for Tailscale; docs/mobile.md),
`YH_UPDATE_FEED` with `YH_RESTART_ANSWER=restart|later` (Restart's question answered and printed) and `YH_OPEN_VERSION=1` (the
sidebar's version card open), `YH_SETTLE_MERGED_AFTER=<s>` (when threads whose PRs merged first settle, 20; with
`YH_MOCK_PULLS`, printing `SETTLED merged <id>`), `YH_USAGE_SPLIT=1` (By Model), `YH_USAGE_DAY_PAGE=<n>`,
`YH_PROJECT_FILTER=<words>` (the project page's filter), `YH_MEMORY_OFF_ANSWER=ok|cancel` (Turn Off Everywhere's question),
`YH_STATE_EXPORT_SIGNINS=1`, `YH_STATE_DONE_ANSWER=ok` (Export's done alert printed, not shown), `YH_STATE_IMPORT_ASK=<file>`
(Import State's sheet; `YH_STATE_IMPORT_CONFIRM=1` then replaces, keeping a copy, and restarts), `YH_NOTIFICATION_CLICK=<seconds>` (a click on the last notification posted, which
`YH_NOTIFICATIONS=log` only printed as `NOTIFY <kind>: title · project · body`: it opens that thread, printing `NOTIFY
clicked`; real notifications go through a `UNUserNotificationCenter` delegate, never from a test).
Transcript: `YH_SCROLL_TO=<row>` (the open
thread's transcript starts at that row; negative counts from the end), `YH_SELECT_PROBE=1` (drags across the
last reply through the window's events a move every 40 ms, prints `SELECT drawn while dragging: n PASS` (the rows painted the selection as it grew), `SELECT PASS` and what ⌘C would copy, then clicks it away;
`=keep` leaves it selected for a snapshot, `=hold` keeps the button down, mid-drag; `YH_SELECT_PROBE_DELAY`), `YH_OPEN_STEP_FILE=<Read|Edit|Write>`
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
`YH_MEMORY_SEED=<n>` (with `YH_OPEN_MEMORY=1`: n made-up memories, linked and some read, plus 21 tasks in every state, added to the project first; scratch data only, it adds again each launch), `YH_MEMORY_GRAPH_HOVER=<index or title>` (with `YH_MEMORY_TAB=graph`: the pointer moved over that dot through the window, printing `GRAPH hover … at x,y`), `YH_MEMORY_PROBE=1` (links the first memory to the second, renames the first and restores its version 1, printing
`MEMORY probe: …`). MCP: `YH_MCP_ADD_ENV=NAME=value` (with `YH_MCP_ADD_COMMAND`: an environment variable added),
`YH_MCP_ADD_DESKTOP=1` (Import from Claude Desktop pressed; give it `YH_CLAUDE_DESKTOP_CONFIG=<scratch file>`, never
the real one).

## Performance is the product

Every feature ends with a real Claude turn on Haiku that uses it (a throwaway `YH_PROJECT`,
`YH_MODEL=haiku`), checked by snapshot or the saved data, then a push. Full perf runs
(`scripts/perf-run.sh <results>`: the `extreme` preset, a fake `claude` streaming 20 huge threads,
the probe's phases; `YH_PERF_PHASES`, `YH_PERF_SECONDS`, `SKIP_BUILD=1`, `FORCE=1` on battery,
`PERF_APP=<path.app>` for a copy of its own; each run gets a fresh `test-harness-root/perf-run-<time>-<pid>`
data clone, printed as `data:`, to delete after). Before a release, run the two
release checks: `YH_PERF_PHASES=stream-all-everything` (every kind of click in turn while 20
threads stream, each timed into clicks.json by kind and summed up as `PERF click <kind>: n ·
median · worst` and `PERF slow click` over 100 ms: typing, a long draft, every right sidebar tab
with the browser shown and hidden, a long file opened and wheeled in Files, the right sidebar's
edge dragged, ⌘K, ⌘F with ⌘G, jumps, folded work and its biggest output, earlier turns, pin and
settle with ⌘Z, collapse, the project, Memory and Usage pages, Settings opened and closed, ⌘N,
the jobs strip, a live window resize, the sidebar hidden; `YH_PERF_ONLY=<groups>` for some, named
in `probe::GROUPS`) and `YH_PERF_PHASES=stream-all-chaos` (any of those in any order, plus stop
and send, on the full preset, 120 s or more; each action is logged first as `PERF chaos <n> <action>
in <thread>`, so a crash says what led to it, and `YH_PERF_CHAOS_SEED=<n>` plays a run again).
After each action the state is checked, and what's off prints as `PERF WRONG: …` (a pin or settle
not undone by ⌘Z, a page left over a thread chosen, ⌘N not landing on a blank in the right project,
an open thread with no transcript, Find marks left after closing, Spotlight or Settings left open,
the keyboard on nothing drawn); a release needs none, and no panic. Shortcuts go in as key
events through the window (one that doesn't land is `WRONG`, and its action is dispatched
instead: the keyboard left on a field or page that's gone reaches nothing); `PERF stall` lines name the action they came after. The fake `claude`
also leaves background jobs running, for the jobs strip. Full runs
take ten minutes, so run them only when the user asks or before a release. Judge by frames on time, worst frames and clicks (docs/
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
  test them against the recorded sessions in `crates/yh-core/fixtures/`. When the protocol
  changes, capture a real session, sanitize it (no local paths or user config) and add it there.
- **Never extract or reuse CLI credentials.** Always drive the official binary.

## Working autonomously

Commit and push after each finished feature. Watch the Claude session limit with
`scripts/limits.sh` (it costs one tiny Haiku request). When `five_hour` nears its limit,
commit, push, note the next step in the task, schedule a wake-up just after its reset, and
pause.

## Versions and releases

The version is in `VERSION` (semver). When a feature lands, add a line to CHANGELOG.md under
**Unreleased**, in the reader's terms. `scripts/release.sh <version>` (from `main` only, with a
clean tree: stash only the user's own uncommitted files, by name, around it, and put them back)
tests, files the Unreleased notes under the version, builds and checks the DMG, then tags,
pushes and installs the app it built in /Applications; the tag's workflow
(`.github/workflows/release.yml`) builds it again and publishes it on GitHub Releases with that
version's notes (run it by hand to test the pipeline on a branch, artifact only:
`gh workflow run release.yml --ref <branch>`).

`RELEASE=1 scripts/bundle.sh` builds the real app (`build/YourHarness.app`, bundle ID
`com.mdovhopo.yourharness`), `scripts/dmg.sh` packs it with its `.sha256`, and
`scripts/check-dmg.sh <dmg> [identity]` checks it as the updaters will. Releases are signed with
the app's own self-signed certificate, "YourHarness Self-Signed", so macOS sees every build as
the same app and keeps the permissions it was given, and the updater (`state/updater.rs`) only
installs updates signed the same way; it updates every copy, 0.0.x's included (same bundle ID
and signature). The certificate and its passwords live in `~/.yourharness/signing` (a lost
certificate means a new identity, and macOS asking for everything again) and in the repo's
`SIGNING_P12` / `SIGNING_P12_PASSWORD` secrets; `scripts/bundle.sh` signs in a keychain of its
own (`scripts/signing-keychain.sh`), never the login keychain, so signing never prompts.

Never run a `RELEASE=1` build for testing: it shares your app's permissions and data. Test the
updater with `.next` copies and `YH_UPDATE_FEED=<release.json>` (`YH_UPDATE_DOWNLOAD=1`,
`YH_UPDATE_RESTART=1` run the whole flow; `YH_AUTO_UPDATE=1` turns on Install Automatically for the run, with `YH_UPDATE_IDLE` / `YH_UPDATE_COUNTDOWN` / `YH_UPDATE_SNOOZE` seconds, `YH_UPDATE_TOAST_ANSWER=later|now` pressing the first countdown's button and `YH_UPDATE_SCREEN_OFF=1` pretending the screen is locked; `VERSION_OVERRIDE=9.9.9 scripts/bundle.sh` makes the
newer copy).
