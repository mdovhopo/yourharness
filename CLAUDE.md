# YourHarness

Native macOS (26+, Apple Silicon only) harness over AI coding CLIs. Being rewritten: the UI is
web (SolidJS in one `WKWebView`), the backend Swift for now (Rust later). Read
docs/architecture.md first: layers, the protocol, the plan and what each chunk covers. The
SwiftUI app this replaces is frozen in `legacy/` (tag `swift-app-frozen`): read it for what a
feature does, never port it one for one. `legacy/CLAUDE.md` has its notes.

## Layout

- `Sources/HarnessCore`: CLI adapters, parsers, storage (`harness.sqlite`), git, forge. No UI imports.
- `Sources/HarnessBackend`: everything the app knows and does, no UI. Owns state, emits patches.
- `app/host`: the AppKit window, the web view, the bridge, native-only pieces. Thin.
- `app/ui`: the page (SolidJS + TypeScript, Vite). Mirrors backend topics, holds view state only.
- `docs/protocol.md`: the contract between page and backend.

## Build and run

`xcode-select` may point at the Command Line Tools, which can't build. Always:

```sh
export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
swift build && swift test
(cd app/ui && pnpm typecheck)
scripts/bundle.sh                 # → build/YourHarness Next.app (APP=<path> for elsewhere)
```

The bundle ID is `com.mdovhopo.yourharness.next` until the switch, so it never shares
permissions or data with the user's app (`/Applications/YourHarness.app`, the frozen 0.0.16).
**Never point a development run at the user's real data folder**: pass `YH_DATA_DIR=<scratch>`
(a folder with a `harness.sqlite`, e.g. an APFS clone `cp -c` of `test-harness-root/extreme`),
and `YH_PATH="$PATH"` so no login shell starts (the user's `.zshrc` reads from `~/Downloads`
and macOS would ask them for access). Launch in the background, never taking focus:
`open -g -n --env YH_DATA_DIR=… --env YH_PATH="$PATH" --stderr <log> "<app>"`. Test builds go
to their own folder (`APP=test-harness-root/web-test/… scripts/bundle.sh`), never `build/`,
which may be the copy the user has open; quit test copies by their own PID or `YH_QUIT=1`,
never by name or path pattern. A test run must never put a macOS privacy prompt on screen;
never run `screencapture`, use `YH_SNAPSHOT=<png>`.

Launch options (see `app/host` and `app/ui/src/main.tsx`): `YH_DATA_DIR`, `YH_NO_CLAUDE=1`,
`YH_CLAUDE_CONFIG`, `YH_PROJECT=<folder>`, `YH_SELECT=<title or id>`, `YH_SEND`/`YH_MODEL`/
`YH_MODE`/`YH_ANSWER=allow|deny`/`YH_QUIT=1` (one real turn, logged, use Haiku and a throwaway
folder), `YH_SNAPSHOT=<png>` (`_DELAY`, `_QUIT`), `YH_BACKGROUND=1`, `YH_TIMING=1`,
`YH_EVENT_LOG=1`, `YH_FLAGS=<classes>` (turn one thing off to measure it).

## Performance is the product

Every chunk ends with `scripts/perf-run.sh <results>` (the `extreme` preset, a fake `claude`
streaming 20 huge threads, the native probe's phases; `YH_PERF_PHASES`, `YH_PERF_SECONDS`,
`SKIP_BUILD=1`, `FORCE=1` on battery) compared with the last chunk's numbers in
docs/performance.md. Judge by frames on time, worst frames and clicks, not averages. Before
trusting a run: no `INVALID` lines; `DIRECT` means synthetic events didn't reach the window and
the page was driven directly (fine for comparing, but `YH_PERF_FRONT=1` is the real test and
takes over the screen: only when the user is away). A slowdown counts once a baseline build
shows it isn't there under the same conditions; a fix once the test fails without it, over
several runs. Rules from measurements (docs/web-experiment.md):

- Nothing that scrolls inside a row; code wraps. One scroller per column.
- Rows redraw alone (a version per row); parked transcripts don't redraw; hold only recent rows.
- Patches reach the page once a frame, batched; the page never reduces raw events.
- Animate opacity and transform only; hover never changes layout.
- Check memory of all processes (host, web content, GPU), not just the app's.

## Principles

- **Apple-quality look**, drawn on the web: system font, macOS colors in light and dark,
  native-feeling controls, SF Symbols as SVG where needed. Shared tokens in one stylesheet.
- **The backend owns state; the page mirrors it.** Requests are intents, patches say what
  changed. Keep the wire language-neutral (Rust later).
- **CLI protocols change often.** Parsers ignore unknown events and never crash on new fields;
  capture real sessions, sanitized, under `Tests/HarnessCoreTests/Fixtures/`.
- **Never extract or reuse CLI credentials.** Always drive the official binary.

## Versions

Work happens on `rewrite`, pushed after each feature. Nothing is released from it until it is
the whole app. `main` keeps the frozen app and its release scripts (`legacy/scripts` here).
