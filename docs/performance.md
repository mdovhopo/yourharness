# Performance

The requirement: 10 to 20 threads working at once, with huge histories, and no visible freezes,
hangs or crashes; every click answered in the next frame, or a loading indicator for what's
asynchronous.

## How it's measured

`scripts/perf-run.sh <results>` builds a copy of its own (`test-harness-root/perf-build/`),
clones the `extreme` preset into a fresh data folder (`test-harness-root/perf-run-<time>-<pid>`,
printed as `data:`, to delete after), and launches it with a fake `claude`
(`presets/extreme/fake-claude/`) that streams turns with reasoning, hundreds of tools,
sub-agents and compactions, no tokens spent. The probe (`crates/yh-app/src/probe.rs`) then runs
its phases on the preset's 20 "Huge …" threads (4 of about 32,000 rows, 16 of half that),
through the paths a person takes: real wheel, click and key events sent to the NSWindow.
Frames are timed at the display link, a callback per vsync on the main thread, so a late one is
a frame where the app couldn't show what changed.

Results: `phases.json` (frames on time, worst frames, CPU and memory per phase, and whether the
run was valid), `clicks.json` (each click: when it was answered and when it was done),
`dev.log` (`PERF …` lines). Options:

| | |
|---|---|
| `YH_PERF_PHASES=<a,b>` | Some phases only: `open-huge`, `idle-huge`, `scroll-huge`, `scroll-huge-fast`, `switch-huge`, `switch-all-idle`, `stream-1`, `stream-all`, `stream-all-scroll`, `stream-all-switch`, `stream-all-typing`, … |
| `YH_PERF_SECONDS=<n>` | Each phase's length (30 by default). |
| `YH_PERF_PHASES=stream-all-everything` | Every kind of click in turn while 20 threads stream, each timed by kind (`PERF click <kind>: n · median · worst`, `PERF slow click` over 100 ms); `YH_PERF_ONLY=<groups>` for some, named in `probe::GROUPS`. |
| `YH_PERF_PHASES=stream-all-chaos` | Any of those in any order, plus stop and send; each action logged first (`PERF chaos <n> <action> in <thread>`), so a crash says what led to it; `YH_PERF_CHAOS_SEED=<n>` plays a run again. |
| `YH_PERF_FRONT=1` | The window in front, full screen, the real pointer moving over it: takes over the Mac, only when nobody's using it. Without it the window is parked below the screen. |
| `SKIP_BUILD=1`, `PERF_APP=<path.app>`, `PERF_SEED=<folder>`, `FORCE=1` | Reuse the perf build; build and run another copy (runs side by side, or a baseline); a smaller seed; run on battery. |

After each action in `everything` and `chaos`, the state is checked, and what's off prints as
`PERF WRONG: …` (a pin or settle not undone by ⌘Z, a page left over a thread chosen, ⌘N not
landing on a blank in the right project, Find marks left after closing, the keyboard on
nothing drawn). A release needs no `WRONG`, no panic, no `INVALID`.

Other probes for one view each (all in CLAUDE.md): `YH_USAGE_PROBE` (the usage chart, longest
stall under 100 ms), `YH_FILES_PROBE` (a long file opened and scrolled in Files, under 50 ms a
step), `YH_SEARCH_PROBE` (the full-text index catching up), `YH_SELECT_PROBE`, `YH_WHEEL`,
`YH_SCROLL_TO`, `YH_TIMING=1` and `YH_EVENT_LOG=1`.

To see where time goes, attach Time Profiler to a running copy:
`xcrun xctrace record --template 'Time Profiler' --attach <pid> --time-limit 8s` (with
`DEVELOPER_DIR` set), then `xctrace export … table[@schema="time-profile"]`.

## Results

`scripts/perf-run.sh`, the `extreme` preset, 20 fake turns, window parked below the screen,
30 s phases, App Nap held off, GPUI's inactive-window 30 fps cap off (a perf run's window is
never active). Frames on time at 120 Hz · worst frame; peak footprint over the run. Same
MacBook; the first two columns on battery.

| Phase | 2026-11-06, first build | 2026-11-06, threads and storage | 2026-11-06, right sidebar and browser (AC) | 2026-11-07, whole app (AC) |
|---|---|---|---|---|
| open-huge | 100.0% · 12 ms | 100.0% · 9 ms | 100.0% · 9 ms | 98.9% · 23 ms |
| idle-huge | 99.9% · 27 ms | 99.9% · 24 ms | 100.0% · 15 ms | 99.1% · 49 ms |
| scroll-huge | 98.1% · 17 ms | 99.1% · 17 ms | 99.4% · 19 ms | 95.5% · 30 ms |
| scroll-huge-fast | 98.5% · 15 ms | 99.1% · 15 ms | 99.8% · 16 ms | 93.8% · 28 ms |
| switch-huge | 99.5% · 20 ms | 99.5% · 23 ms | 99.5% · 22 ms | 97.6% · 26 ms |
| switch-all-idle | 99.4% · 25 ms | 99.2% · 26 ms | 99.3% · 28 ms | 97.8% · 30 ms |
| stream-1 | 100.0% · 11 ms | — | 100.0% · 11 ms | 99.0% · 31 ms |
| stream-all | 100.0% · 12 ms | 99.8% · 64 ms | 100.0% · 15 ms | 98.6% · 26 ms |
| stream-all-scroll | 98.7% · 15 ms | 99.8% · 26 ms | 99.8% · 18 ms | 98.9% · 27 ms |
| stream-all-switch | 99.8% · 18 ms | 99.8% · 18 ms | 99.9% · 17 ms | 99.3% · 24 ms |
| stream-all-typing | 98.4% · 23 ms | 94.3% · 26 ms | 98.6% · 46 ms | 94.9% · 35 ms |
| Peak memory | 160 MB | 192 MB | 183 MB | 215 MB |
| Switch click (median / worst, streaming) | 32 / 37 ms | 34 / 51 ms | 32 / 44 ms | see clicks.json |
| Key to screen (median / worst) | 14 / 22 ms | 18 / 29 ms | 15 / 46 ms | see clicks.json |

The whole-app run (2026-11-07, AC power) is valid (no `INVALID`, no `PERF stall`, no frame
over 50 ms). Its scroll phases look lower than the run before, but six alternating 20 s runs of
them on 2026-11-08 put the same build at 96.9–99.0% (scroll-huge) and 97.3–98.7% (fast)
between runs: most of that is run-to-run spread, and the 99.4% before was one run at the top of
it. Compare with three or more alternating runs, never one.

What the profile showed (Time Profiler, 8 s of scroll-huge): the root view renders on every
frame of scrolling, because the list's scroll notifies the transcript view and GPUI dirties its
parents; every child of the root that isn't a cached view is laid out again. The composer
wasn't cached (it grows), and cost 9% of the main thread. It's now a cached view placed at the
height it measured (`Composer::measure`, `RootView::placed_composer`), and gone from the
profile; frames on time didn't move beyond the spread, CPU 2–3 points lower. Selectable text
costs about 9% in all, mostly text layout plain text needs too; its selection registration
2–4%. The rest is GPUI's list laying out the visible rows each frame (about 20%) and text
shaping (18%).

Open: rare long frames (169 ms, 72 ms, 65 ms in three of twelve phases); typing under 20
streams varies between runs (94–99%). Long frames are logged with their phase (`PERF stall`).

GPUI 0.3.11 caps an inactive window at 30 fps (and Low Power Mode too). A run without turning
that off runs every phase at a steady 30 fps: power saving, not stalls. The app keeps the cap
for real use, where a background window at 30 fps is fine.

## The rules that keep it fast

- **The transcript is GPUI's `list`, bottom-aligned**; a change splices exactly the rows it
  touched, and only spliced rows are measured again. Off-screen rows cost nothing.
- **Views that change rarely are cached** (`AnyView::cached`); nothing that changes often lives
  in a view that's always there. A view that grows is cached at the height it measured.
- **Parsing and I/O off the main thread**; Claude's events applied at most once a frame, deltas
  joined.
- **Hold only recent rows** (160 a thread, earlier pages read while scrolling up; six
  transcripts kept made); parked transcripts don't redraw.
- **Answer the click, then do the work**: the next frame shows the selection, what's slow
  follows.
- **Animate only what fades; hover never changes layout.**
- **Never send events to the window from inside an entity update** (GPUI dispatches them
  synchronously, and it re-enters); hold off App Nap in test runs.

## Traps in testing this

- **A window that can't be seen says nothing.** With the display off, the screen locked or
  another Space in front, frames all but stop while the main thread sits idle, and clicks seem
  to take seconds. A run prints `PERF INVALID` when the screen is locked or the
  display asleep (and `valid: false` in `phases.json`): trust nothing with those lines. `pmset -g log | grep "Display is turned"`
  shows the display going off; a Mac left alone locks its screen after a while, whatever
  `caffeinate` says, so check long runs at the end too.
- **App Nap looks like a freeze.** A background app nobody watches gets its timers and display
  link slowed down; test runs hold it off, and `YH_BACKGROUND=1` keeps a parked window drawing.
- **`screencapture` asks.** It puts a privacy dialog on the screen, even when
  `CGPreflightScreenCaptureAccess()` says it's allowed. Use the app's own drawing
  (`FEATURES=snapshots`, `YH_SNAPSHOT`).
- **Events come in by the front door.** Probes send real events through the window, as the
  window server would; an event handed straight to a view skips what a real one goes through.
- **Look at the sequence, not the median.** `clicks.json` and the phases over time: frames
  gone with the main thread idle isn't the app, and a sudden step in click times isn't load
  building up.
- **Compare before believing.** A slowdown counts once a baseline build (`PERF_APP`) doesn't
  show it under the same conditions; a fix counts once the same test fails without it, over
  several runs, as a freeze can be one run in three.
- **A frozen run never exits.** `perf-run.sh` has no time limit: run long ones in the
  background and watch for `PERF stall` lines.
