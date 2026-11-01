# Performance under heavy parallel use

The requirement: 10 to 20 threads working at once, with huge histories, and no visible freezes, hangs or crashes; every click instant, or a loading indicator for what's asynchronous. A lower framerate is acceptable. Measure with `scripts/perf-run.sh <results> YH_PERF_FRONT=1` (see CLAUDE.md), on an M1 Max.

## Fixed, and how it was shown

| Problem | Cause | Evidence |
|---|---|---|
| Freezes of seconds to minutes (the 29.6 s hang in 0.0.13) | Hover animation on whole rows. Content moving under a resting pointer flips hover, and `easeOut` can't merge, so animations stacked on the row's frame | 0 of 6 foreground runs froze with the fix; the same build without it froze on its first run |
| Main thread blocked 1 s or more with many threads opening | Bulk reads held the one reader connection that quick lookups use | Start of 20 threads: 941-1027 ms to 238 ms; test `aBigReadDoesNotHoldUpQuickLookups` |
| Saves stalling long threads | Every save snapshotted every row on the main thread | Saves send changed rows only |
| Expanding a turn of hundreds of tools | All rows made at once, animated | 952-2796 ms to 328-450 ms |
| Blank transcript when a long turn ends | The lazy stack keeps laying out rows that folded away | "Lost its rows" 2-6 a run to 0-4, none found blank by drawing |
| Hitch on the first blank found | The check drew the window to a PNG on the main thread | Only when asked for from the Debug menu now |

## Not solved: clicks while many threads really stream

Measured once phases start timing after every thread is streaming (20 threads, small histories, 45 s phases, after earlier phases): switching threads median about 830 ms, the inspector and a new thread about 500 ms. Streaming alone is far better. Results swing with phase order: the same switch test as the first phase gave a median of 171 ms.

Ruled out by measurement, so don't start here again: streaming rate (a quarter of it is still slow), rows shown (30 rows and 10 turns behave like 90 and 40), sub-agents, leaked views (2-11 text views alive), the GPU and window server (idle), memory pressure, CPU saturation of the machine, and the 30-second file index (it only lists once per folder).

The running turn's row cap (`currentTurnRows`) bounds what a long turn keeps on screen, but no valid long run has shown yet that it helps clicks: the 15-minute soak without it degraded (824 to 2328 ms over a phase), and that soak hasn't been repeated with it.

Still open: the order dependence, and turn ends (`stream-all-end` runs at 9-14 fps with the main thread at 80-90%, with 20 turns ending together). The time is in SwiftUI's own update and layout, not in the app's code.

## Traps in testing this

- **30-second phases mislead.** A send waits for its history, so with huge threads most of a short phase measures loading. Results before the wait for streaming was added (early click tables, `stream-all` in `rel2-*`) overstate how good it was.
- **The display sleeping stops the frames**, which reads as a freeze. `perf-run.sh` now wakes it and keeps it on.
- **A window that's covered isn't drawn**, and `occlusionState` can still say it's visible. Hence floating in front mode.
- **`sample` from the watchdog lands after a stall**, so it can show an idle main thread. Time Profiler only sees running code, so blocked time is missing from it. Use `YH_PERF_FREEZE_AFTER=0.6` and the main thread answer times.
