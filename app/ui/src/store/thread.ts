import { batch, createSignal, type Accessor, type Setter } from "solid-js";
import type { Patch, PermissionWire, ThreadWire, WireEntry } from "../protocol";

// A thread as the page mirrors it: its summary (always), and while it's open, its newest rows
// at the backend's revision. Earlier rows are fetched as you scroll up. Streaming changes a row
// in place and bumps its item's version, so exactly one row redraws.

export type Kind = WireEntry["k"];

export class Entry {
  id: string;
  kind: Kind;
  text: string;
  toolName?: string;
  toolInput?: string;
  toolResult?: string;
  resultLength?: number;
  isError = false;
  date?: number;
  sub?: { d: string; s: string; c?: number };
  /** Still being written by Claude. */
  live = false;
  /** Rendered markdown, kept while the text doesn't change. */
  html?: string;
  htmlFor = -1;

  constructor(id: string, kind: Kind, text = "") {
    this.id = id;
    this.kind = kind;
    this.text = text;
  }

  static from(w: WireEntry): Entry {
    const e = new Entry(w.id, w.k, w.t);
    e.assign(w);
    return e;
  }

  assign(w: WireEntry) {
    this.text = w.t;
    this.toolName = w.n;
    this.toolInput = w.i;
    this.toolResult = w.r;
    this.resultLength = w.rl;
    this.isError = w.e === 1;
    this.date = w.d;
    this.sub = w.sa;
    this.live = w.live === 1;
  }
}

/** One row on screen: a message, or a run of thinking and tool calls folded into one. */
export class Item {
  static next = 0;
  key = Item.next++;
  type: "user" | "text" | "work" | "notice";
  entries: Entry[];
  version: Accessor<number>;
  private setVersion: Setter<number>;
  constructor(type: Item["type"], entries: Entry[]) {
    this.type = type;
    this.entries = entries;
    [this.version, this.setVersion] = createSignal(0, { equals: false });
  }
  bump() {
    this.setVersion(0);
  }
}

const isWork = (k: Kind) => k === "thinking" || k === "tool";
const itemType = (k: Kind): Item["type"] => (isWork(k) ? "work" : k === "user" ? "user" : k === "assistant" ? "text" : "notice");

export function itemsFrom(entries: Entry[]): Item[] {
  const items: Item[] = [];
  for (const e of entries) {
    const last = items[items.length - 1];
    if (isWork(e.kind) && last?.type === "work") last.entries.push(e);
    else items.push(new Item(itemType(e.kind), [e]));
  }
  return items;
}

export class ThreadState {
  summary: Accessor<ThreadWire>;
  setSummary: Setter<ThreadWire>;
  items: Accessor<Item[]>;
  setItems: Setter<Item[]>;
  /** Rows of the history before the first one held. */
  before: Accessor<number>;
  setBefore: Setter<number>;
  /** What Claude is waiting on you for, while the thread is open. */
  permission: Accessor<PermissionWire | null>;
  setPermission: Setter<PermissionWire | null>;
  private openRunning: Accessor<boolean | null>;
  private setOpenRunning: Setter<boolean | null>;
  isLoaded: Accessor<boolean>;
  private setLoaded: Setter<boolean>;
  /** Bumped on anything that changes what the transcript shows. */
  changed: Accessor<number>;
  private setChanged: Setter<number>;

  loading: Promise<void> | null = null;
  /** Its transcript is made (open or kept for going back): all its rows stay. */
  inDeck = false;
  /** The backend's revision of this thread's rows that the page has. */
  rv = -1;
  /** Patches that arrived before the snapshot they follow. */
  private waiting: Patch[] | null = null;
  private itemOf = new Map<Entry, Item>();
  private byId = new Map<string, Entry>();

  constructor(summary: ThreadWire) {
    [this.summary, this.setSummary] = createSignal(summary);
    [this.items, this.setItems] = createSignal<Item[]>([]);
    [this.before, this.setBefore] = createSignal(0);
    [this.permission, this.setPermission] = createSignal<PermissionWire | null>(null);
    [this.openRunning, this.setOpenRunning] = createSignal<boolean | null>(null);
    [this.isLoaded, this.setLoaded] = createSignal(false);
    [this.changed, this.setChanged] = createSignal(0, { equals: false });
  }

  get id() {
    return this.summary().id;
  }
  get loaded() {
    return this.isLoaded();
  }
  title = () => this.summary().title;
  /** Running, from the thread's own topic while open (it's newer), else from its summary. */
  running = () => this.openRunning() ?? this.summary().running;
  waitingForYou = () => (this.isLoaded() ? this.permission() !== null : this.summary().waiting);

  /** The snapshot the backend gave on subscribing; patches that came first are applied after. */
  loadSnapshot(entries: WireEntry[], from: number, rv: number, perm: PermissionWire | null, running: boolean) {
    this.rv = rv;
    batch(() => {
      this.prepend(entries.map(Entry.from), from);
      this.setPermission(perm);
      this.setOpenRunning(running);
      this.setLoaded(true);
    });
    const waiting = this.waiting ?? [];
    this.waiting = null;
    for (const patch of waiting) this.apply(patch);
  }

  /** Called before subscribing: patches from now on wait for the snapshot. */
  expectSnapshot() {
    this.waiting = [];
  }

  /** History rows, oldest first, put in front of what's held. */
  prepend(entries: Entry[], before: number) {
    const items = itemsFrom(entries);
    for (const item of items) for (const e of item.entries) this.track(e, item);
    batch(() => {
      this.setItems((old) => items.concat(old));
      this.setBefore(before);
      this.setChanged(0);
    });
  }

  private track(e: Entry, item: Item) {
    this.itemOf.set(e, item);
    this.byId.set(e.id, e);
  }

  /** A new row at the end, folding work into the work row before it. */
  private append(e: Entry) {
    const items = this.items();
    const last = items[items.length - 1];
    if (isWork(e.kind) && last?.type === "work") {
      last.entries.push(e);
      this.track(e, last);
      last.bump();
    } else {
      const item = new Item(itemType(e.kind), [e]);
      this.track(e, item);
      this.setItems(items.concat(item));
      if (!this.inDeck && items.length > 300) this.trim(200);
    }
    this.setChanged(0);
  }

  private touch(e: Entry) {
    this.itemOf.get(e)?.bump();
    this.setChanged(0);
  }

  /** Keeps the newest `keep` rows; the ones before go back to "earlier", fetched again if wanted. */
  trim(keep: number) {
    const items = this.items();
    if (items.length <= keep) return;
    const dropped = items.slice(0, items.length - keep);
    let count = 0;
    for (const old of dropped) {
      count += old.entries.length;
      for (const x of old.entries) {
        this.itemOf.delete(x);
        this.byId.delete(x.id);
      }
    }
    batch(() => {
      this.setItems(items.slice(items.length - keep));
      this.setBefore(this.before() + count);
    });
  }

  /** Lets go of the rows; they're fetched again on the next visit. */
  unload() {
    batch(() => {
      this.setItems([]);
      this.setBefore(0);
      this.setPermission(null);
      this.setOpenRunning(null);
      this.setLoaded(false);
    });
    this.itemOf.clear();
    this.byId.clear();
    this.loading = null;
    this.waiting = null;
    this.rv = -1;
  }

  /** One patch of this thread's rows, in revision order. */
  apply(patch: Patch) {
    if (this.waiting) {
      this.waiting.push(patch);
      return;
    }
    if (!("rv" in patch) || !this.isLoaded() || patch.rv <= this.rv) return;
    this.rv = patch.rv;
    switch (patch.t) {
      case "add": {
        const e = Entry.from(patch.e);
        this.append(e);
        break;
      }
      case "app": {
        const e = this.byId.get(patch.id);
        if (!e) return;
        if (patch.f === "i") e.toolInput = (e.toolInput ?? "") + patch.s;
        else e.text += patch.s;
        this.touch(e);
        break;
      }
      case "upd": {
        const e = this.byId.get(patch.e.id);
        if (!e) return;
        e.assign(patch.e);
        this.touch(e);
        break;
      }
      case "run":
        this.setOpenRunning(patch.v);
        break;
      case "perm":
        this.setPermission(patch.p);
        break;
    }
  }
}
