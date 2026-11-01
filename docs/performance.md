# Performance under heavy parallel use

The requirement: 10 to 20 threads working at once, with huge histories, and no visible freezes, hangs or crashes; every click instant, or a loading indicator for what's asynchronous. A lower framerate is acceptable. Measure with `scripts/perf-run.sh <results> YH_PERF_FRONT=1` (see CLAUDE.md), on an M1 Max.

## How the transcript works, and why

A thread's transcript is `TranscriptListView`: a list in AppKit (an `NSScrollView` with its rows laid out by hand), each row's content a small SwiftUI view in a hosting view of its own. Each thread's list is a card in `TranscriptDeck`, kept made while you're away and parked out of the window, where it does nothing. Don't put the transcript back in a `LazyVStack`, and don't put anything that changes often back in the window's own hosting view. That arrangement is what froze and crashed the app.

**The freeze and crash.** A `LazyVStack` with `scrollPosition` could go back and forth for good: its row heights are estimates until measured, and moving your place changes what it measures next. Caught in a log, 100 times a second for 37 s: offset 6016 → -28 → 6016, content height 8174 → 10557 → 8174. It ran inside the window's layout pass, and with hover state and scroll position requests feeding it, it could also run forever inside SwiftUI's update flush, at 100% CPU, where nothing of the app's can stop it (one run: over 7 minutes, until killed). Every pass changed the window's preferences, the toolbar answered by asking for another pass, and AppKit ended the app with "more Update Constraints in Window passes than there are views in the window". That's the signature of the crash in the wild on October 30. Going to the top of a thread while a turn streamed did it in 2 runs of 3.

**In the list, every step is finite.** A row is measured when it comes near the screen, once per width and content. Your place is the row at the top of the screen, and the position follows that row when heights above it change. Scrolling moves a layer, with nothing in SwiftUI running. A row's update touches that row. Rows can't draw blank: what's mounted comes from the geometry.

## Fixed, and how it was shown

| Problem | Cause | Evidence |
|---|---|---|
| Freezes of seconds to forever, then a crash | The lazy stack and the window's layout, above | Crashed 2 of 3 runs; none in the runs below |
| Freezes of seconds to minutes (the 29.6 s hang in 0.0.13) | Hover animation on whole rows. Content moving under a resting pointer flips hover, and `easeOut` can't merge, so animations stacked on the row's frame | 0 of 6 foreground runs froze with the fix; the same build without it froze on its first run |
| Main thread blocked 1 s or more with many threads opening | Bulk reads held the one reader connection that quick lookups use | Start of 20 threads: 941-1027 ms to 238 ms; test `aBigReadDoesNotHoldUpQuickLookups` |
| Saves stalling long threads | Every save snapshotted every row on the main thread | Saves send changed rows only |
| Expanding a turn of hundreds of tools | All rows made at once, animated | Rows of the list now, made as they come near the screen |
| Hitch on the first blank found | The check drew the window to a PNG on the main thread | Only when asked for from the Debug menu now |
| Sidebar rows sliding | Sorted by last activity, so every turn start and end moved them | Stable order by creation; stalls over 250 ms 6 to 1 in two runs each |

## Where it stands

M1 Max, the full `extreme` preset (2 GB, 20 huge threads of 16,000 to 32,000 rows), all 20 streaming, window in front and full screen, pointer moving over it.

| Measured | Result |
|---|---|
| 15-minute soak of random actions (1,783 of them) | No crash, no stall over 1 s, worst 561 ms; 63-68 fps from the first two minutes to the last (the old list degraded over a soak like this) |
| Streaming, nothing else going on | 111 fps, worst frame 137 ms |
| 20 turns ending at once | 82 fps, worst frame 103 ms |
| Scrolling while streaming | 82 fps, worst frame 132 ms |
| Memory | About 2 GB with all 20 histories loaded, 409 MB a minute after the turns end |
| Launch to the first huge thread on screen | 2 s (31,626 rows) |

Clicks, median ms under that load: when the click is answered (first frame), then done (the frame after).

| Click | Answered | Done |
|---|---|---|
| Typing a key | 10 | 20 |
| Open or close a turn's work | 8 | 23 |
| Open a tool call's long output | 55 | 62 |
| Go to another thread (with the right sidebar open and first visits among them) | 117 | 256 |
| Back to a thread from a project page | 276 | 296 |
| Right sidebar tabs | 8-154 | 17-212 |
| Search (⌘K) | 102 | 126 |
| Find (⌘F) | 83 | 108 |
| Jump to top, to end | 51-108 | 87-125 |
| One step of a live window resize | 74 | 83 |

With the right sidebar closed, going to a thread is answered in 17-50 ms, and takes 36-56 ms in all if you've been there and it hasn't changed (first visit under load: about 150 ms, rows included).

Still the slowest, and what to go after next: coming back from a project page (the thread's view is made again), and the first visit to a thread (each row is a hosting view to make; making them ahead of time in idle moments would answer it). Memory with every huge history loaded is high, because a running thread keeps all its rows in memory.

## Traps in testing this

- **A window that can't be seen says nothing.** With the display off, the screen locked or another Space in front, frames all but stop while the main thread sits idle, events are held and applied in batches, and clicks seem to take seconds. For a whole day that read as "clicks take 830 ms, depending on the order of phases": every such run had `windowVisible: false`, and `pmset -g log` shows the display going off and on through them. A run now prints `INVALID` for this: trust nothing with those lines. A Mac left alone locks its screen after a while, whatever `caffeinate` says.
- **`screencapture` asks.** It puts a privacy dialog on the screen, even when `CGPreflightScreenCaptureAccess()` says it's allowed. Use the app's own drawing, which can't show glass.
- **Wheel events have to come in by the front door.** A wheel event handed straight to the window skips the app's monitors, so a code block under it keeps it (`TranscriptWheelRouter`).
- **30-second phases mislead.** A send waits for its history, so with huge threads most of a short phase measures loading. Phases now time from when every thread is streaming.
- **`sample` from the watchdog lands after a stall**, so it can show an idle main thread. Time Profiler only sees running code, so blocked time is missing from it. Use `YH_PERF_FREEZE_AFTER=0.6` and the main thread answer times.
