# The web rendering experiment

YourHarness's features are where they should be, but over a work day it doesn't feel smooth: no
hangs anymore, yet frames drop whenever a lot streams at once. This folder asks one question:
**does a web rendering layer, built for huge amounts of text, stay smooth where ours doesn't?**

To make the answer mean something, only the rendering layer changes:

- **Same core.** A thin AppKit window holding one `WKWebView`, over the same `HarnessCore`
  (Claude stream parser, permission protocol, session reader, database). Nothing about Claude
  or storage is new.
- **Same data.** The `extreme` preset's database (a clone, never written), and your Claude
  Code history in `~/.claude/projects`, read-only.
- **Same load.** The fake `claude` from `presets/extreme/fake-claude`, 20 huge threads streaming
  at once, through the real process, pipes and parser.
- **Same measurements.** The phases of `PerfProbe` (`open-huge` … `stream-all-switch`), real
  wheel, click and key events sent through the window, frames timed in the page.

## How it's built

```
web/host/   Swift: window, WKWebView, bridge, Claude sessions, history, the probe's host half
web/ui/     SolidJS + TypeScript (Vite): everything you see
web/scripts bundle.sh → build/YourHarnessWeb.app; perf-run.sh, the twin of scripts/perf-run.sh
```

- **One protocol between the two halves**, JSON, in `web/ui/src/host.ts`. The page asks
  (`send`, `interrupt`, `respond`, `setMode`) through `webkit.messageHandlers.host`; history
  comes as `yhw://app/api/page` fetches answered off the main thread; Claude's events come
  back with `__yh.recv([...])`, **one batch per display frame**, never while the last batch is
  still being taken, and with consecutive text deltas of a block joined (`Outbox`). Either half
  can be replaced: another backend, or the page in a plain browser fed a recorded stream.
- **The page holds little.** The newest 160 rows of a thread, earlier pages fetched as you
  scroll up; up to six transcripts stay made for going back and forth, the rest let go of
  their rows; a running thread nobody is looking at keeps its newest 300.
- **One row redraws per delta.** Each row has its own version signal; streamed text changes
  the entry in place and bumps its row. A parked transcript doesn't redraw at all until shown.
- **Rows off screen cost no layout or painting** (`content-visibility: auto`, which keeps each
  row's last real height). The transcript follows the end in the same frame the content grows,
  and keeps what you're reading still when rows above it change height.
- **120 Hz.** WebKit draws pages at 60 Hz even on a 120 Hz screen by default; the host turns
  that off (`PreferPageRenderingUpdatesNear60FPSEnabled`, `YHW_60HZ=1` keeps it).

What's in: projects and threads (the harness database, your Claude Code history, or
`YHW_PROJECT=<folder>`), transcripts with markdown, highlighted code and folded tool calls,
the composer with model and permission mode, Stop (⌘.), approve and deny cards (⌘↩ / ⌘⌫), and
new threads. What isn't: everything else the app does.

## Running it

```sh
web/scripts/bundle.sh                       # build/YourHarnessWeb.app
open build/YourHarnessWeb.app               # your Claude Code history, live turns with your claude
web/scripts/perf-run.sh <results dir>       # the performance run (plugged in; FORCE=1 on battery)
```

Launch options: `YHW_DB_DIR` (a folder with a `harness.sqlite`), `YHW_NO_CLAUDE=1`,
`YHW_CLAUDE_CONFIG`, `YHW_CLAUDE_LIMIT` (newest sessions listed, 400), `YHW_PROJECT`,
`YHW_SELECT=<title or id>`, `YHW_SEND`/`YHW_MODEL`/`YHW_MODE`/`YHW_ANSWER=allow|deny` (one real
turn, logged), `YHW_SNAPSHOT=<png>` (+`_DELAY`, `_QUIT`), `YHW_BACKGROUND=1`, `YHW_TIMING=1`,
`YHW_EVENT_LOG=1`, and for runs `YHW_PERF`, `YHW_PERF_SECONDS`, `YHW_PERF_PHASES`,
`YHW_PERF_FRONT=1`. Use `YH_PATH` as for the app, so no login shell starts.

## Results (2026-11-06, first round)

Same MacBook (120 Hz), on battery, window parked below the screen as `scripts/perf-run.sh` does
without `YH_PERF_FRONT`, 30 s phases, `extreme` preset, 20 fake turns at rate 1. Frames
on time at 120 Hz, worst frame, and peak memory of every process drawing the window (the web
side counts the web content and GPU processes). Native: `scripts/perf-run.sh`, same phases.

| Phase | Native on time | worst | peak | Web on time | worst | peak |
|---|---|---|---|---|---|---|
| open-huge | 97.7% | 118 ms | 225 MB | 98.7% | 16 ms | 134 MB |
| idle-huge | 99.8% | 13 ms | 260 MB | 98.8% | 24 ms | 132 MB |
| scroll-huge | 92.5% | 37 ms | 275 MB | 99.2% | 26 ms | 293 MB |
| scroll-huge-fast | 59.0% | 58 ms | 308 MB | 99.8% | 19 ms | 539 MB |
| switch-huge | 96.2% | 117 ms | 781 MB | 97.0% | 90 ms | 322 MB |
| switch-all-idle | 95.0% | 96 ms | 1716 MB | 97.8% | 47 ms | 303 MB |
| stream-1 | 89.4% | 125 ms | 825 MB | 99.3% | 73 ms | 460 MB |
| stream-all | 90.0% | 88 ms | 1004 MB | 95–98%* | 60–103 ms* | 300–540 MB |
| stream-all-scroll | 85.9% | 94 ms | 752 MB | 97–98%* | 31–74 ms* | 320–450 MB |
| stream-all-switch | 84.2% | 161 ms | 879 MB | 93.9–97.9% | 45–304 ms | 530–690 MB |
| stream-all-typing | — | — | — | 96.8% | 59 ms | 564 MB |

Clicks: switching threads 30–48 ms median on the web side (native 67 ms median while
streaming, 145 ms in a shorter run); a typed key on screen in 10–15 ms median. Opening a
thread's history the first time: 40 ms (the extreme preset) to 3.2 s (a 259 MB Claude Code
session, which `ClaudeSessionArchive` parses whole, as the app does; "Loading…" shows meanwhile).

\* **Open:** in both full runs, `stream-all` and `stream-all-scroll` collapsed (28% and 4% on
time, frames of 450 ms) about five minutes in, with the page's own work small (event batches
under 60 ms, CPU about 55%). Three shorter runs of the same phases, in the same orders, didn't
do it: the ranges above are theirs. Not explained yet; the probe doesn't record per-second
numbers like `seconds.csv`, which would tell the app from the machine. Next run plugged in,
`YHW_PERF_FRONT=1`.

What the round found:

- **WebKit draws pages at 60 Hz** on a 120 Hz screen unless `PreferPageRenderingUpdatesNear60FPSEnabled`
  is turned off (the host does).
- **Boxes that scroll are expensive.** A code block that scrolls sideways gets its own layer in
  WebKit; a transcript of them took about three times the memory (617 MB against 205 MB for the
  web process over the same rows) and had 90 ms frames. Code wraps instead.
- **Not `overflow-wrap`, after all.** A full run with `overflow-wrap: anywhere` on code blocks
  collapsed as above, and removing it seemed to fix it, but only short runs followed, and the
  next full run without it collapsed too. Code blocks no longer have it either way.
- **Counting rows with `count(*)`** cost 90 ms per first visit on 32,000-row threads; the last
  row number is a lookup (`HarnessDatabase.transcriptCount`).
- **Synthetic events into a background window** stopped reaching WebKit partway through the
  night (they worked earlier). The probe then drives the page directly and says `DIRECT`; real
  events are certain only with `YHW_PERF_FRONT=1`.
- **Not yet seen:** the pass bar's 99% of frames and no frame over 50 ms under 20 streams. The
  web side gets 95–98% with a few frames of 60–100 ms; the native app 84–90% with 88–161 ms.

Runs are in `test-harness-root/web-perf/` (`web-final`, `native-30`, `mem*`, `repro*`).
