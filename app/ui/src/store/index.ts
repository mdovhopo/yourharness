import { batch, createSignal } from "solid-js";
import { call, hostLog, onPatches } from "../host";
import type { LibrarySnapshot, ModelWire, Page, Patch, ProjectWire, ThreadSnapshot } from "../protocol";
import { Entry, ThreadState } from "./thread";

export { Entry, ThreadState } from "./thread";

/** Rows fetched per page of history. */
export const PAGE = 160;
/** Transcripts kept made after you leave them, for going back and forth. */
const KEPT = 6;

const [projects, setProjects] = createSignal<ProjectWire[]>([]);
const [threadList, setThreadList] = createSignal<ThreadState[]>([]);
const [selectedId, setSelectedId] = createSignal<string | null>(null);
/** The transcripts that are made, the open one first. */
const [deck, setDeck] = createSignal<ThreadState[]>([]);
const [models, setModels] = createSignal<ModelWire[]>([]);
const [defaults, setDefaults] = createSignal({ model: "", effort: "" });

export { projects, threadList, selectedId, deck, models, defaults };

const byId = new Map<string, ThreadState>();
export const thread = (id: string | null) => (id ? byId.get(id) : undefined);
export const project = (id: string) => projects().find((p) => p.id === id);

/** How long applying each batch of patches took, for the probe. */
export const recvTimes: number[] = [];

onPatches((patches: Patch[]) => {
  const started = performance.now();
  batch(() => {
    for (const patch of patches) {
      switch (patch.t) {
        case "thread": {
          const known = byId.get(patch.th.id);
          // A turn ending in the thread you're looking at is read already.
          if (patch.th.unread && selectedId() === patch.th.id && document.hasFocus()) call("read", { th: patch.th.id }).catch(() => {});
          if (known) known.setSummary(patch.th);
          else {
            const t = new ThreadState(patch.th);
            byId.set(t.id, t);
            setThreadList((old) => [t, ...old]);
          }
          break;
        }
        case "threadGone": {
          const gone = byId.get(patch.th);
          byId.delete(patch.th);
          setThreadList((old) => old.filter((t) => t.id !== patch.th));
          setDeck((old) => old.filter((t) => t !== gone));
          if (selectedId() === patch.th) setSelectedId(null);
          break;
        }
        case "reload": {
          // Rows changed under it (a cut-off turn recovered): fetch them again.
          const t = byId.get(patch.th);
          if (t?.loaded) {
            t.unload();
            ensureLoaded(t);
          }
          break;
        }
        case "commands":
          byId.get(patch.th)?.setCommands(patch.v);
          break;
        case "plan":
          byId.get(patch.th)?.setPlan(patch.v);
          break;
        case "project":
          setProjects((old) => (old.some((p) => p.id === patch.p.id) ? old.map((p) => (p.id === patch.p.id ? patch.p : p)) : [...old, patch.p]));
          break;
        case "models":
          setModels(patch.v);
          break;
        default:
          byId.get(patch.th)?.apply(patch);
      }
    }
  });
  recvTimes.push(performance.now() - started);
});

export async function loadLibrary() {
  const snapshot = await call<LibrarySnapshot>("library");
  const states = snapshot.threads.map((summary) => {
    const t = new ThreadState(summary);
    byId.set(t.id, t);
    return t;
  });
  batch(() => {
    setProjects(snapshot.projects);
    setThreadList(states);
    setModels(snapshot.models ?? []);
    setDefaults(snapshot.defaults);
  });
  hostLog(`UI library: ${snapshot.projects.length} projects, ${states.length} threads`);
}

/** Opens a thread's rows: the newest page, then its patches as they come. */
export function ensureLoaded(t: ThreadState): Promise<void> {
  if (t.loaded) return Promise.resolve();
  if (!t.loading) {
    t.expectSnapshot();
    t.loading = (async () => {
      const started = performance.now();
      const snap = await call<ThreadSnapshot>("subscribe", { th: t.id, limit: PAGE });
      const fetched = performance.now();
      t.loadSnapshot(snap.entries, snap.from, snap.rv, snap.perm ?? null, snap.running, snap.commands, snap.plan);
      if (timing) hostLog(`UI load ${t.title()}: fetch ${(fetched - started).toFixed(0)} ms, rows ${(performance.now() - fetched).toFixed(0)} ms (${snap.entries.length} rows)`);
    })();
  }
  return t.loading;
}

export let timing = true;

let loadingEarlier = false;
/** The page of history before what's held. Returns the rows added. */
export async function loadEarlier(t: ThreadState): Promise<number> {
  if (loadingEarlier || t.before() === 0) return 0;
  loadingEarlier = true;
  try {
    const page = await call<Page>("page", { th: t.id, before: t.before(), limit: PAGE });
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
  // A transcript no longer made lets go of its rows, and the backend stops sending them.
  for (const gone of old) {
    if (!next.includes(gone)) {
      call("unsubscribe", { th: gone.id }).catch(() => {});
      gone.unload();
    }
  }
  ensureLoaded(t);
  if (t.summary().unread) call("read", { th: id }).catch(() => {});
}

export async function newThread(p: ProjectWire) {
  const id = await call<string>("newThread", { projectId: p.id });
  // The backend's `thread` patch adds it; it may land a frame after this answer.
  for (let i = 0; i < 60 && !byId.has(id); i++) await new Promise((r) => requestAnimationFrame(r));
  select(id);
}

/** `images` are base64, as pasted or dropped. */
export function send(t: ThreadState, text: string, now = false, images: string[] = []) {
  const trimmed = text.trim();
  if (!trimmed && !images.length) return;
  call("send", { th: t.id, text: trimmed, now, images }).catch((error) => hostLog(`send failed: ${error}`));
}

/** Files in the thread's folder matching what follows an @. */
export const mentionFiles = (t: ThreadState, q: string) => call<{ v: string[] }>("files", { th: t.id, q }).then((r) => r.v);

export const interrupt = (t: ThreadState) => call("interrupt", { th: t.id }).catch(() => {});
export const respond = (t: ThreadState, allow: boolean) => {
  const p = t.permission();
  if (p) call("respond", { th: t.id, id: p.id, allow }).catch(() => {});
};
export const answer = (t: ThreadState, answers: Record<string, string[]>) => {
  const p = t.permission();
  if (p) call("answer", { th: t.id, id: p.id, answers }).catch(() => {});
};
export const decidePlan = (t: ThreadState, approve: string | null, feedback = "") => {
  const p = t.permission();
  if (p) call("plan", { th: t.id, id: p.id, approve, feedback }).catch(() => {});
};
export const setMode = (t: ThreadState, mode: string) => call("setMode", { th: t.id, mode });
export const setModel = (t: ThreadState, model: string) => call("setModel", { th: t.id, model });
export const setEffort = (t: ThreadState, effort: string) => call("setEffort", { th: t.id, effort });
export const rename = (t: ThreadState, title: string) => call("rename", { th: t.id, title });
export const pin = (t: ThreadState, on: boolean) => call("pin", { th: t.id, on });
export const settle = (t: ThreadState, on: boolean) => call("settle", { th: t.id, on });
export const remove = (t: ThreadState) => call("delete", { th: t.id });
export const saveDraft = (t: ThreadState, text: string) => call("draft", { th: t.id, text }).catch(() => {});
