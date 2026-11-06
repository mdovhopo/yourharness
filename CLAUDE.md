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
  committed). Today only `gpui-pre-macos`: `GPUI_KEEP_DRAWING=1` draws a window parked off
  screen.
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
themselves). Also
`YH_SEND_FILES=<a.png:b.jpg>` (images with it), `YH_DRAFT=<text>` (typed in the composer:
`/co` opens the command menu, `@no` the file menu; `YH_DRAFT_DELAY`), `YH_FIND=<words>` (Find
opened), `YH_EXPAND_WORK=1` (folded work drawn open), `YH_ACTIONS_PROBE=1` (pin, rename, settle,
delete on the first threads). Also `YH_SNAPSHOT=<png>` (`_DELAY`, `_QUIT`), `YH_BACKGROUND=1`,
`YH_TIMING=1` and `YH_EVENT_LOG=1` (every event, and each plan rebuild). `YH_NOTIFICATIONS=log`
prints notifications and the Dock badge instead (always in tests: a real one asks for permission),
`YH_APPEARANCE=light|dark`, `YH_OPEN_SETTINGS=1`, `YH_INSPECTOR_PANEL=prs|sideChat|subagents|files|browser`, `YH_MOCK_PULLS=<file.json>` (PRs read from
`{"<url>": {…gh pr view's JSON…}}` instead of `gh`), `YH_SIDE_ASK=<text>` (asked in the selected thread's side
chat after `YH_DRAFT_DELAY`) (the right sidebar open on it), `YH_OPEN_FILE=<path[:line[-end]]>`
(opened in the Files tab as a link does, after `YH_DRAFT_DELAY`), `YH_OPEN_MCP=1` (the `/mcp` sheet, its servers printed; `YH_MCP_ADD_COMMAND=<command line>` or
`YH_MCP_ADD_URL`, `YH_MCP_ADD_SCOPE=local|project|user`, `YH_MCP_ADD_SUBMIT=1`, `YH_MCP_REMOVE=<name>`: they write
Claude Code's config, so project scope in a scratch project, or a scratch `CLAUDE_CONFIG_DIR`; `YH_MCP_BROWSER=log`
prints sign-in pages instead of opening them), `YH_MCP_ELICIT_ANSWER=accept|decline` (what an MCP server asks
mid-call, answered with the form's defaults), `YH_MCP_SIGN_IN_ANSWER=allow|deny` (the card Claude puts up to sign in
to a server; with `YH_MCP_BROWSER=log` each such call prints `MCP sign-in asked: <server>` and the page; never
finish a real sign-in from a test), `YH_STATE_EXPORT=<file.yhstate>` (the whole state exported at launch, its manifest printed),
`YH_STATE_IMPORT=<file>` (imported at launch, development data folders only: never into the real app),
`YH_IMPORT_COOKIES=<Cookies.binarycookies>` (a Safari-format file's cookies into the selected thread's project's
browser: a synthetic file only, never your real browsers, which would ask you for access), `YH_OPEN_USAGE=1` (Usage; `YH_USAGE_RANGE=week|month|quarter|halfYear|year`, `YH_USAGE_METRIC=cost`,
`YH_USAGE_HOVER=<days ago>`; test it on a clone of `test-harness-root/usage` with `CLAUDE_CONFIG_DIR` at its
`claude-config`, never the real `~/.claude`), `YH_OPEN_MEMORY=1` (the project's Memory page,
`YH_MEMORY_TAB=memories|tasks|activity`, after `YH_DRAFT_DELAY`), `YH_SPOTLIGHT=<words>` (⌘K typed;
`YH_SPOTLIGHT_ENTER=1` with a session ID resumes it).

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
`scripts/release.sh <version>` (from `main` only) tags it and `.github/workflows/release.yml` publishes
it, and the updater (`state/updater.rs`) installs it on every copy, the Swift app's included: same
bundle ID and signature. Never run a `RELEASE=1` build here: it shares your app's permissions and
data. Test the updater with `.next` copies and `YH_UPDATE_FEED=<release.json>` (`YH_UPDATE_DOWNLOAD=1`,
`YH_UPDATE_RESTART=1` run the whole flow; `VERSION_OVERRIDE=9.9.9 scripts/bundle.sh` makes the newer copy).
