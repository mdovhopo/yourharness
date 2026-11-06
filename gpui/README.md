# The GPUI rendering experiment

The web experiment (`web/`) asked whether a web rendering layer stays smooth where SwiftUI
doesn't. This folder asks the same question of a third stack: **GPUI**, Zed's GPU-drawn,
immediate-mode UI framework, in Rust. The project started on GPUI before moving to Swift for
Apple looks, and a Rust backend is the plan after the UI rewrite, so this is also a first look at
the whole app in Rust.

Like the web POC, only the rendering layer is meant to differ. Here, though, the whole thing is
Rust: there's no HarnessCore underneath. It reads the same tables, files and stream-json directly.

- **Same data.** The `extreme` preset's database (a clone, opened read-only), and your Claude
  Code history in `~/.claude/projects`, read-only.
- **Same load.** The fake `claude` from `presets/extreme/fake-claude`, 20 huge threads streaming
  at once, through a real process, pipes and parser.
- **Same measurements.** The web probe's phases (`open-huge` … `stream-all-typing`), with real
  wheel, click and key events sent through the NSWindow. Frames are timed at the display link,
  where the web side used `requestAnimationFrame`.

## How it's built

```
src/main.rs      window, menus, launch options
src/library.rs   projects, threads, pages of rows (harness.sqlite, Claude Code history)
src/history.rs   Claude Code session files, read as HarnessCore's ClaudeSessionArchive does
src/claude.rs    a claude process per thread: ClaudeSession's arguments, stream-json → Event
src/model.rs     a thread's rows; a delta changes its entry and re-measures one list row
src/app.rs       sidebar, transcript deck, composer, live turns
src/rows.rs      one kind of row per item type (user, text, work, notice)
src/markdown.rs  pulldown-cmark → styled text blocks, a light code highlighter
src/input.rs     the composer's field (GPUI's own text input example)
src/probe.rs     the performance run; src/platform.rs, the Mac side of it
```

- **GPUI's `list`** holds the transcript, aligned to the bottom so it follows the end. Rows
  are measured once and again only when spliced (`ListState::splice`): a streamed delta
  re-measures exactly the one row it changed. Off-screen rows are neither laid out nor drawn.
- **Cached views.** The sidebar is an `AnyView::cached`, so it redraws only when told something
  changed. While Claude streams, a frame draws the transcript and little else. A thread nobody
  is looking at isn't drawn at all.
- **Claude's events**: parsed on a reader thread per process. The main thread takes what's
  arrived at most once a frame (8 ms), with consecutive deltas of a block joined, as the web
  host's `Outbox` does.
- **The page holds little**, as on the web: the newest 160 rows of a thread, with earlier pages
  read as you scroll up. Six transcripts stay made for going back and forth. A running thread
  nobody is looking at keeps its newest 300 rows.
- **GPUI 0.2.2 from crates.io, patched** (`patches/gpui-0.2.2.patch`, applied by
  `scripts/vendor.sh` into `vendor/`, which isn't committed). The patch keeps the display link
  running for a window parked off screen (`GPUI_KEEP_DRAWING=1`), and it can write a drawn frame
  to a PNG (`GPUI_SNAPSHOTS=1`), because `screencapture` raises a privacy prompt. Shaders compile
  at launch (`runtime_shaders`): Xcode 26 doesn't ship the Metal toolchain.

What's in: projects and threads (the harness database, your Claude Code history, or
`YHG_PROJECT=<folder>`), and transcripts with markdown, highlighted code and folded tool calls.
Also the composer with model and permission mode (click to cycle), Stop (⌘.), approve and deny
cards (⌘↩ / ⌘⌫), and new threads (+). What isn't: everything else the app does. The composer is
one line (GPUI has no multi-line text field short of Zed's editor), and inline code keeps the
body font (GPUI's highlights can't change the font).

## Running it

Needs the arm64 Rust toolchain (`rustup`, stable).

```sh
gpui/scripts/bundle.sh                     # build/YourHarnessGPUI.app
open build/YourHarnessGPUI.app             # your Claude Code history, live turns with your claude
gpui/scripts/perf-run.sh <results dir>     # the performance run (plugged in; FORCE=1 on battery)
```

Launch options: `YHG_DB_DIR` (a folder with a `harness.sqlite`), `YHG_NO_CLAUDE=1`,
`YHG_CLAUDE_CONFIG`, `YHG_CLAUDE_LIMIT` (newest sessions listed, 400), `YHG_PROJECT`,
`YHG_SELECT=<title or id>`, and `YHG_SEND` / `YHG_MODEL` / `YHG_MODE` / `YHG_ANSWER=allow|deny`
(one real turn, prompts answered by themselves). Also `YHG_SNAPSHOT=<png>` (+`_DELAY`, `_QUIT`),
`YHG_BACKGROUND=1`, `YHG_TIMING=1` and `YHG_EVENT_LOG=1`. For runs: `YHG_PERF`,
`YHG_PERF_SECONDS`, `YHG_PERF_PHASES`, `YHG_PERF_RATE`, `YHG_PERF_FRONT=1`, and `YHG_APP_NAP=1`
(lets macOS nap the app, see below). Use `YH_PATH` as for the app, so no login shell starts.

## Results

### 2026-11-06, first round

Same MacBook (120 Hz), on battery, window parked below the screen, 30 s phases, `extreme` preset,
20 fake turns at rate 1: the conditions of `web/README.md`'s table, whose web and native columns
are copied here (taken a day earlier). The columns show frames on time at 120 Hz, the worst
frame, and the peak footprint. GPUI draws in-process, so its footprint is the whole app; the web
column adds the content and GPU processes. The GPUI column is the last of two full runs with
App Nap held off (`full-2`, `full-3`; the other agreed within a point). Every phase used real
events and every run was valid.

| Phase | Native on time | worst | peak | Web on time | worst | peak | GPUI on time | worst | peak |
|---|---|---|---|---|---|---|---|---|---|
| open-huge | 97.7% | 118 ms | 225 MB | 98.7% | 16 ms | 134 MB | 100.0% | 10 ms | 80 MB |
| idle-huge | 99.8% | 13 ms | 260 MB | 98.8% | 24 ms | 132 MB | 100.0% | 13 ms | 80 MB |
| scroll-huge | 92.5% | 37 ms | 275 MB | 99.2% | 26 ms | 293 MB | 99.4% | 16 ms | 116 MB |
| scroll-huge-fast | 59.0% | 58 ms | 308 MB | 99.8% | 19 ms | 539 MB | 99.7% | 14 ms | 120 MB |
| switch-huge | 96.2% | 117 ms | 781 MB | 97.0% | 90 ms | 322 MB | 99.5% | 19 ms | 114 MB |
| switch-all-idle | 95.0% | 96 ms | 1716 MB | 97.8% | 47 ms | 303 MB | 99.4% | 29 ms | 121 MB |
| stream-1 | 89.4% | 125 ms | 825 MB | 99.3% | 73 ms | 460 MB | 100.0% | 12 ms | 122 MB |
| stream-all | 90.0% | 88 ms | 1004 MB | 95–98% | 60–103 ms | 300–540 MB | 100.0% | 14 ms | 139 MB |
| stream-all-scroll | 85.9% | 94 ms | 752 MB | 97–98% | 31–74 ms | 320–450 MB | 99.9% | 14 ms | 149 MB |
| stream-all-switch | 84.2% | 161 ms | 879 MB | 93.9–97.9% | 45–304 ms | 530–690 MB | 99.8% | 15 ms | 154 MB |
| stream-all-typing | — | — | — | 96.8% | 59 ms | 564 MB | 98.9% | 28 ms | 162 MB |

Clicks: switching threads is 24–32 ms median, 45 ms worst, with 20 threads streaming (web 30–48
ms median, native 67 ms). A typed key is on screen in 15–16 ms median, 22 ms worst (web 10–15
ms). Main-thread CPU is about 55% under 20 streams, the same as the web page's.

Against the pass bar (≥99% on time, no frame over 50 ms under 20 streams, switch ~50 ms, a key in
a frame, memory ≤ 1.5× the current app): every stream phase meets it, at about a sixth of the
native app's memory. The typing phase came in at 98.9% in one run and 99.4% in a 10 s run.

What the round found:

- **App Nap was the collapse.** The first full run collapsed about five minutes in to a steady 9
  fps (117 ms frames) with CPU around 12%: the main thread idle, nothing wrong in the app. macOS
  naps a background app that nobody seems to watch, slowing its timers and display link, and a
  parked test window is exactly that. Holding an `NSActivity` (user-initiated, latency-critical)
  for the run fixed it (`platform::hold_off_app_nap`; `YHG_APP_NAP=1` turns it back on). The web
  runs' unexplained collapse "about five minutes in" has the same signature, and the web host
  doesn't hold off App Nap. That is worth re-running before trusting its stream-all numbers.
- **GPUI's list measures what it shows.** Rows above what's been drawn have no height yet, so
  pixel scroll offsets undercount. That's fine for following and for the wheel, which only reach
  the next rows. The probe reports rows crossed as well as pixels.
- **Events into a parked window**: GPUI reads `locationInWindow`, and a synthetic wheel event
  the window server can't place has its location in screen coordinates. The probe sets it so the
  two agree. Clicks and keys go straight to `NSWindow sendEvent:`. They're sent outside any GPUI
  update, because they dispatch synchronously and would otherwise re-enter the app.
- **Not yet seen:** `YHG_PERF_FRONT=1` (in front, a real pointer), a plugged-in run, your real
  Claude Code history at size (opening a 259 MB session parses it whole, as the web host does),
  and an hour of real use.

Runs are in `test-harness-root/gpui-perf/` (`full-1` napped, `full-2`, `full-3`, `typing`).
