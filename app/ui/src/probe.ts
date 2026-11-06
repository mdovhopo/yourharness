import { call, hostLog } from "./host";
import type { ThreadState } from "./store/thread";
import { ensureLoaded, recvTimes, select, selectedId, send, threadList } from "./store";

// The performance run (`YH_PERF=<folder>`), the web twin of the native app's PerfProbe:
// the same phases on the same threads ("Huge …" in the extreme preset) under the same fake
// `claude`, through the same paths a person takes (real wheel, click and key events from the
// host). Frames are timed here with requestAnimationFrame: a frame that's late here is one
// where the page couldn't show what changed.

type Config = { seconds: number; phases: string; screenHz: number; rate: string; front: boolean };

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const frame = () => new Promise<number>((r) => requestAnimationFrame(r));
/** After the next frame has been painted: what a person sees. */
const painted = async () => {
  await frame();
  await frame();
  return performance.timeOrigin + performance.now();
};

class Frames {
  deltas: number[] = [];
  private last = 0;
  private running = false;
  start() {
    this.deltas = [];
    this.last = 0;
    this.running = true;
    const tick = (t: number) => {
      if (!this.running) return;
      if (this.last) this.deltas.push(t - this.last);
      this.last = t;
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  }
  stop(period: number) {
    this.running = false;
    let vsyncs = 0, missed = 0, hitches = 0, over50 = 0, over100 = 0, worst = 0;
    for (const d of this.deltas) {
      const spent = Math.max(1, Math.round(d / period));
      vsyncs += spent;
      missed += spent - 1;
      if (spent > 1) hitches++;
      if (d > 50) over50++;
      if (d > 100) over100++;
      worst = Math.max(worst, d);
    }
    const sorted = [...this.deltas].sort((a, b) => a - b);
    const total = this.deltas.reduce((a, b) => a + b, 0);
    return {
      fps: total ? (this.deltas.length / total) * 1000 : 0,
      framesMissedPct: vsyncs ? (missed / vsyncs) * 100 : 0,
      onTimePct: vsyncs ? ((vsyncs - missed) / vsyncs) * 100 : 0,
      hitches,
      framesOver50ms: over50,
      framesOver100ms: over100,
      worstFrameMs: worst,
      p99FrameMs: sorted[Math.floor(sorted.length * 0.99)] ?? 0,
      frames: this.deltas.length,
    };
  }
}

export async function runProbe() {
  const config: Config = await call("probe.config");
  const log = (s: string) => call("probe.log", { s }).catch(() => {});
  const huge = () => threadList().filter((t) => t.title().startsWith("Huge")).sort((a, b) => a.title().localeCompare(b.title()));
  const threads = huge();
  if (!threads.length) {
    log("FAIL: no threads named Huge");
    return call("probe.done");
  }
  const seconds = config.seconds;

  // The page's frame period: WebKit may render pages at 60 Hz on a 120 Hz screen.
  const frames = new Frames();
  frames.start();
  await sleep(1500);
  const warm = frames.stop(1000 / config.screenHz);
  const sorted = [...frames.deltas].sort((a, b) => a - b);
  const median = sorted[Math.floor(sorted.length / 2)] ?? 1000 / 60;
  const period = median < 12 ? 1000 / config.screenHz : 1000 / 60;
  log(`page frames: median ${median.toFixed(2)} ms → judging against ${(1000 / period).toFixed(0)} Hz (screen ${config.screenHz} Hz, warm-up fps ${warm.fps.toFixed(0)})`);

  // A pointer moving over the window now and then, as yours does: hover is part of the cost.
  let pointer = true;
  (async () => {
    while (pointer) {
      await call("probe.move", { x: 20 + Math.random() * (innerWidth - 40), y: 60 + Math.random() * (innerHeight - 160) });
      await sleep(500);
    }
  })();

  const clicks: any[] = [];
  // Real events from the host when they land; a window in the background can stop taking them
  // from WebKit, and then the page is driven directly (as the native probe drives its views),
  // which skips WebKit's own input and scrolling path. Logged, and kept in the results.
  let realEvents = true;
  const direct = (why: string) => {
    if (!realEvents) return;
    realEvents = false;
    log(`DIRECT: ${why}; driving the page directly from here on`);
  };
  let phaseName = "";

  const click = async (el: Element) => {
    el.scrollIntoView({ block: "nearest" });
    await frame();
    if (!realEvents) {
      const sent = performance.timeOrigin + performance.now();
      (el as HTMLElement).focus();
      (el as HTMLElement).click();
      return sent;
    }
    const r = el.getBoundingClientRect();
    return (await call("probe.click", { x: r.left + Math.min(r.width / 2, 60), y: r.top + r.height / 2 })) as number;
  };

  const transcriptCenter = () => {
    const el = document.querySelector(".slot:not([style*='none']) .transcript") as HTMLElement | null;
    const r = el?.getBoundingClientRect();
    return { el, x: r ? r.left + r.width / 2 : innerWidth / 2, y: r ? r.top + r.height / 2 : innerHeight / 2 };
  };

  const switchTo = async (t: ThreadState, attempt = 1): Promise<void> => {
    let row = document.querySelector(`[data-thread="${CSS.escape(t.id)}"]`);
    if (!row) {
      // Behind its project's "Show more", as for a person.
      const more = [...document.querySelectorAll(".more")].find((b) => b.textContent?.startsWith("Show") && !b.textContent.includes("less"));
      if (more) {
        await click(more);
        await frame();
        row = document.querySelector(`[data-thread="${CSS.escape(t.id)}"]`);
      }
    }
    if (!row) return log(`FAIL: no sidebar row for ${t.title()}`);
    const sent = await click(row);
    const start = performance.now();
    while (selectedId() !== t.id && performance.now() - start < 1000) await frame();
    if (selectedId() !== t.id) {
      // The first click on a window in the background can go to activating it.
      if (attempt < 3) {
        log(`click on ${t.title()} didn't select it; clicking again`);
        return switchTo(t, attempt + 1);
      }
      if (realEvents) {
        direct("clicks don't reach the window");
        return switchTo(t, 1);
      }
      log(`FAIL: couldn't select ${t.title()}`);
      select(t.id);
      await painted();
      return;
    }
    const wasLoaded = t.loaded;
    const answered = await painted();
    // Held already, the frame that showed the selection showed the rows too.
    let done = answered;
    if (!wasLoaded) {
      await ensureLoaded(t);
      done = await painted();
    }
    clicks.push({ phase: phaseName, click: "switch", answeredMs: answered - sent, doneMs: done - sent, rows: t.items().length });
  };

  const scrolling = async (until: number, speed = 40) => {
    let direction = 1, count = 0, lowest = Infinity, highest = -Infinity;
    while (performance.now() < until) {
      const { el, x, y } = transcriptCenter();
      const before = el?.scrollTop ?? 0;
      if (realEvents) await call("probe.wheel", { x, y, dy: speed * direction });
      else if (el) el.scrollTop -= speed * direction;
      if (realEvents && el && count === 20 && highest - lowest < 1 && el.scrollTop === before) direct("wheel events don't reach the window");
      count++;
      if (el) {
        lowest = Math.min(lowest, el.scrollTop);
        highest = Math.max(highest, el.scrollTop);
      }
      if (count % 200 === 0) direction = -direction;
      await sleep(8);
    }
    if (highest - lowest < 100) log(`FAIL: scrolling moved the transcript only ${Math.round(highest - lowest)} px`);
    else log(`scrolled across ${Math.round(highest - lowest)} px`);
  };

  const switching = async (list: ThreadState[], until: number, every = 1500) => {
    let index = threads.indexOf(list.find((t) => t.id === selectedId()) ?? list[0]);
    while (performance.now() < until) {
      index = (index + 1) % list.length;
      await switchTo(list[index]);
      await sleep(every);
    }
  };

  const typing = async (until: number) => {
    const field = document.querySelector(".composer textarea") as HTMLTextAreaElement | null;
    if (!field) return log("FAIL: no composer");
    await click(field);
    const letters = "the quick brown fox jumps over the lazy dog ";
    let i = 0;
    while (performance.now() < until) {
      const character = letters[i++ % letters.length];
      const seen = new Promise<void>((r) => field.addEventListener("input", () => r(), { once: true }));
      let sent: number;
      if (realEvents) sent = await call("probe.key", { s: character });
      else {
        sent = performance.timeOrigin + performance.now();
        field.focus();
        document.execCommand("insertText", false, character);
      }
      const landed = await Promise.race([seen.then(() => true), sleep(1000).then(() => false)]);
      if (!landed && realEvents) {
        direct("keys don't reach the composer");
        continue;
      }
      const shown = await painted();
      clicks.push({ phase: phaseName, click: "key", answeredMs: shown - sent, doneMs: shown - sent });
      await sleep(90);
      if (i % 60 === 0) {
        field.value = "";
        field.dispatchEvent(new Event("input"));
      }
    }
    field.value = "";
    field.dispatchEvent(new Event("input"));
  };

  const startTurns = async (list: ThreadState[], length: number) => {
    const begin = performance.now();
    const marks = new Map(list.map((t) => [t, t.items().length]));
    for (const t of list) if (!t.running()) send(t, `PERF seconds=${Math.round(length)} rate=${config.rate}`);
    // Open threads show their rows; the others only say they're running.
    const streaming = (t: ThreadState) => {
      if (!t.loaded) return t.running();
      const items = t.items();
      return t.running() && items.length > (marks.get(t) ?? 0) + 1 && items[items.length - 1].type !== "user";
    };
    while (!list.every(streaming) && performance.now() - begin < 120_000) await sleep(200);
    log(`${list.length} turns streaming after ${((performance.now() - begin) / 1000).toFixed(1)} s`);
    return performance.now() + length * 1000;
  };

  const waitForTurnsToEnd = async () => {
    const limit = performance.now() + 60_000;
    while (threads.some((t) => t.running()) && performance.now() < limit) await sleep(250);
  };

  const phases: [string, () => Promise<void>][] = [
    ["open-huge", async () => {
      await switchTo(threads[0]);
      await sleep(5000);
    }],
    ["idle-huge", async () => { await sleep(seconds * 1000); }],
    ["scroll-huge", () => scrolling(performance.now() + seconds * 1000)],
    ["scroll-huge-fast", () => scrolling(performance.now() + seconds * 1000, 200)],
    ["switch-huge", () => switching(threads.slice(0, 4), performance.now() + seconds * 1000)],
    ["switch-all-idle", async () => {
      for (const t of threads) await ensureLoaded(t);
      await switching(threads, performance.now() + seconds * 1000);
    }],
    ["stream-1", async () => {
      const end = await startTurns(threads.slice(0, 1), seconds);
      await sleep(end - performance.now());
    }],
    ["stream-all", async () => {
      const end = await startTurns(threads, seconds);
      await sleep(end - performance.now());
    }],
    ["stream-all-scroll", async () => {
      const end = await startTurns(threads, seconds);
      await scrolling(end);
    }],
    ["stream-all-switch", async () => {
      const end = await startTurns(threads, seconds);
      await switching(threads, end);
    }],
    ["stream-all-typing", async () => {
      const end = await startTurns(threads, seconds);
      await typing(end);
    }],
  ];

  const only = config.phases ? new Set(config.phases.split(",")) : null;
  log(`${threads.length} huge threads, phases ${only ? [...only].join(",") : "all"}, ${seconds} s each`);
  await switchTo(threads[0]);
  await sleep(1000);

  for (const [name, body] of phases) {
    if (only && !only.has(name)) continue;
    phaseName = name;
    if (name.startsWith("stream")) await waitForTurnsToEnd();
    await call("probe.sample");
    const memory: number[] = [], cpu: number[] = [], webCPU: number[] = [];
    let sampling = true;
    const sampler = (async () => {
      while (sampling) {
        await sleep(1000);
        const s = await call("probe.sample");
        memory.push(s.totalMB);
        cpu.push(s.totalCPU);
        webCPU.push(s.webCPU ?? 0);
      }
    })();
    const started = performance.now();
    const clicksBefore = clicks.length;
    recvTimes.length = 0;
    frames.start();
    await body();
    const recv = [...recvTimes].sort((a, b) => a - b);
    const stats = frames.stop(period);
    sampling = false;
    await sampler;
    const end = await call("probe.sample");
    const avg = (xs: number[]) => (xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : 0);
    const phaseClicks = clicks.slice(clicksBefore);
    const result = {
      phase: name,
      seconds: (performance.now() - started) / 1000,
      ...stats,
      judgedHz: 1000 / period,
      footprintEndMB: end.totalMB,
      footprintPeakMB: Math.max(end.totalMB, ...memory),
      hostMB: end.hostMB,
      webMB: end.webMB,
      gpuMB: end.gpuMB ?? 0,
      cpuPct: avg(cpu),
      webCpuPct: avg(webCPU),
      realEvents,
      eventBatches: recv.length,
      eventBatchWorstMs: recv[recv.length - 1] ?? 0,
      eventBatchP99Ms: recv[Math.floor(recv.length * 0.99)] ?? 0,
      clicks: phaseClicks.length,
      clickWorstMs: Math.max(0, ...phaseClicks.map((c) => c.doneMs)),
      clickMedianMs: phaseClicks.length ? phaseClicks.map((c) => c.doneMs).sort((a, b) => a - b)[Math.floor(phaseClicks.length / 2)] : 0,
      domNodes: document.getElementsByTagName("*").length,
    };
    await call("probe.phase", result);
    await call("probe.clicks", { list: phaseClicks });
    log(
      `${name}: ${stats.fps.toFixed(0)} fps, on time ${stats.onTimePct.toFixed(1)}%, worst ${stats.worstFrameMs.toFixed(0)} ms, ` +
        `p99 ${stats.p99FrameMs.toFixed(0)} ms, >50ms ${stats.framesOver50ms}, events worst ${result.eventBatchWorstMs.toFixed(0)} ms, mem ${result.footprintPeakMB.toFixed(0)} MB, cpu ${result.cpuPct.toFixed(0)}%` +
        (phaseClicks.length ? `, clicks median ${result.clickMedianMs.toFixed(0)} ms worst ${result.clickWorstMs.toFixed(0)} ms` : ""),
    );
  }
  pointer = false;
  await waitForTurnsToEnd();
  await call("probe.done");
}

export function maybeRunProbe() {
  call("probe.config")
    .then(() => runProbe())
    .catch(() => {});
}

export { hostLog };
