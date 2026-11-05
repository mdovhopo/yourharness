import { batch, createSignal } from "solid-js";
import { call, fetchListing, fetchPage, hostLog, onEvents, type HostEvent, type ProjectMeta } from "./host";
import { Entry, ThreadState } from "./model";

/** Rows fetched per page of history. */
export const PAGE = 160;
/** Transcripts kept made after you leave them, for going back and forth. */
const KEPT = 6;

const [projects, setProjects] = createSignal<ProjectMeta[]>([]);
const [threadList, setThreadList] = createSignal<ThreadState[]>([]);
const [selectedId, setSelectedId] = createSignal<string | null>(null);
/** The transcripts that are made, the open one first. */
const [deck, setDeck] = createSignal<ThreadState[]>([]);
const [model, setModel] = createSignal(localStorage.getItem("model") ?? "");
const [mode, setMode] = createSignal(localStorage.getItem("mode") ?? "default");

export { projects, threadList, selectedId, deck, model, mode };

/** Logs how long loading a thread took. */
export let timing = true;
const byId = new Map<string, ThreadState>();
export const thread = (id: string | null) => (id ? byId.get(id) : undefined);

export function chooseModel(value: string) {
  localStorage.setItem("model", value);
  setModel(value);
  const t = thread(selectedId());
  if (t?.running()) call("setModel", { th: t.id, model: value || "default" });
}

export function chooseMode(value: string) {
  localStorage.setItem("mode", value);
  setMode(value);
  const t = thread(selectedId());
  if (t) call("setMode", { th: t.id, mode: value }).catch(() => {});
}

/** How long applying each batch of events took, for the probe. */
export const recvTimes: number[] = [];
onEvents((events: HostEvent[]) => {
  const started = performance.now();
  batch(() => {
    for (const ev of events) byId.get(ev.th)?.apply(ev);
  });
  recvTimes.push(performance.now() - started);
});

export async function loadLibrary() {
  const listing = await fetchListing();
  const states = listing.threads.map((meta) => {
    const t = new ThreadState(meta);
    byId.set(t.id, t);
    return t;
  });
  states.sort((a, b) => b.meta.updatedAt - a.meta.updatedAt);
  batch(() => {
    setProjects(listing.projects);
    setThreadList(states);
  });
  hostLog(`UI library: ${listing.projects.length} projects, ${states.length} threads`);
}

export function ensureLoaded(t: ThreadState): Promise<void> {
  if (t.loaded) return Promise.resolve();
  if (!t.loading) {
    t.loading = (async () => {
      if (t.id.startsWith("new:")) {
        t.loaded = true;
        return;
      }
      const started = performance.now();
      const page = await fetchPage(t.id, undefined, PAGE);
      const fetched = performance.now();
      t.prepend(page.entries.map(Entry.from), page.from);
      t.loaded = true;
      if (timing) hostLog(`UI load ${t.title()}: fetch ${(fetched - started).toFixed(0)} ms, rows ${(performance.now() - fetched).toFixed(0)} ms (${page.entries.length} rows)`);
    })();
  }
  return t.loading;
}

let loadingEarlier = false;
/** The page of history before what's held. Returns the rows added. */
export async function loadEarlier(t: ThreadState): Promise<number> {
  if (loadingEarlier || t.before() === 0) return 0;
  loadingEarlier = true;
  try {
    const page = await fetchPage(t.id, t.before(), PAGE);
    t.prepend(page.entries.map(Entry.from), page.from);
    return page.entries.length;
  } finally {
    loadingEarlier = false;
  }
}

export function select(id: string) {
  const t = byId.get(id);
  if (!t) return;
  const old = deck();
  const next = [t, ...old.filter((x) => x !== t)].slice(0, KEPT);
  for (const x of old) x.inDeck = false;
  for (const x of next) x.inDeck = true;
  batch(() => {
    setSelectedId(id);
    setDeck(next);
  });
  // A transcript no longer made lets go of its rows, unless Claude is still writing them.
  for (const gone of old) if (!next.includes(gone) && !gone.running() && !gone.id.startsWith("new:")) gone.unload();
  ensureLoaded(t);
}

export function newThread(project: ProjectMeta) {
  const id = "new:" + crypto.randomUUID();
  const t = new ThreadState({ id, projectId: project.id, title: "New thread", updatedAt: Date.now(), source: project.source });
  byId.set(id, t);
  setThreadList((old) => [t, ...old]);
  select(id);
}

export async function send(t: ThreadState, text: string) {
  const trimmed = text.trim();
  if (!trimmed) return;
  await ensureLoaded(t);
  const user = new Entry(`u${Date.now()}`, "user", trimmed);
  batch(() => {
    t.append(user);
    t.setRunning(true);
    if (t.title() === "New thread") t.setTitle(trimmed.split("\n")[0].slice(0, 60));
    t.meta.updatedAt = Date.now();
  });
  try {
    await call("send", { th: t.id, text: trimmed, sid: t.sessionId ?? "", projectId: t.meta.projectId, model: model(), mode: mode() });
  } catch (error) {
    batch(() => {
      t.notice(`Couldn't send: ${error}`);
      t.setRunning(false);
    });
  }
}

export function interrupt(t: ThreadState) {
  call("interrupt", { th: t.id }).catch(() => {});
}

export function respond(t: ThreadState, allow: boolean) {
  const p = t.permission();
  if (!p) return;
  t.setPermission(null);
  call("respond", { th: t.id, id: p.id, allow }).catch(() => {});
}
