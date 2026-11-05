import { createSignal, batch, type Accessor, type Setter } from "solid-js";
import type { HostEvent, ThreadMeta, WireEntry } from "./host";

// A thread's rows as the page holds them. Only the newest page or two of a long history is
// here; earlier pages come from the host when you scroll up to them. Streaming changes an
// entry in place and bumps its item's version, so exactly one row redraws.

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
    e.toolName = w.n;
    e.toolInput = w.i;
    e.toolResult = w.r;
    e.resultLength = w.rl;
    e.isError = w.e === 1;
    e.date = w.d;
    e.sub = w.sa;
    return e;
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

export function itemsFrom(entries: Entry[]): Item[] {
  const items: Item[] = [];
  for (const e of entries) {
    const last = items[items.length - 1];
    if (isWork(e.kind)) {
      if (last?.type === "work") last.entries.push(e);
      else items.push(new Item("work", [e]));
    } else {
      items.push(new Item(e.kind === "user" ? "user" : e.kind === "assistant" ? "text" : "notice", [e]));
    }
  }
  return items;
}

export type Permission = { id: string; tool: string; description: string; input: string };

export class ThreadState {
  meta: ThreadMeta;
  items: Accessor<Item[]>;
  setItems: Setter<Item[]>;
  running: Accessor<boolean>;
  setRunning: Setter<boolean>;
  permission: Accessor<Permission | null>;
  setPermission: Setter<Permission | null>;
  title: Accessor<string>;
  setTitle: Setter<string>;
  /** Rows of the history before the first one held. */
  before: Accessor<number>;
  setBefore: Setter<number>;
  private loadedFlag = false;
  /** Whether its rows have arrived: the transcript says "Loading…" until then. */
  isLoaded: Accessor<boolean>;
  private setLoaded: Setter<boolean>;
  get loaded() {
    return this.loadedFlag;
  }
  set loaded(value: boolean) {
    this.loadedFlag = value;
    this.setLoaded(value);
  }
  loading: Promise<void> | null = null;
  /** Its transcript is made (open or kept for going back): all its rows stay. */
  inDeck = false;
  sessionId?: string;
  /** Streaming blocks by message and index, tool calls by ID. */
  private blocks = new Map<string, Entry>();
  private tools = new Map<string, { entry: Entry; item: Item }>();
  private itemOf = new Map<Entry, Item>();
  /** Live rows by ID, for the host's changes. */
  private byId = new Map<string, Entry>();
  /** Bumped on anything that changes what the transcript shows. */
  changed: Accessor<number>;
  private setChanged: Setter<number>;

  constructor(meta: ThreadMeta) {
    this.meta = meta;
    this.sessionId = meta.sessionId;
    [this.items, this.setItems] = createSignal<Item[]>([]);
    [this.running, this.setRunning] = createSignal(false);
    [this.permission, this.setPermission] = createSignal<Permission | null>(null);
    [this.title, this.setTitle] = createSignal(meta.title);
    [this.before, this.setBefore] = createSignal(0);
    [this.changed, this.setChanged] = createSignal(0, { equals: false });
    [this.isLoaded, this.setLoaded] = createSignal(false);
  }

  get id() {
    return this.meta.id;
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
    if (e.kind === "tool") this.tools.set(e.id, { entry: e, item });
  }

  /** Puts a new entry at the end, folding work into the work row before it. */
  append(e: Entry): Item {
    const items = this.items();
    const last = items[items.length - 1];
    if (isWork(e.kind) && last?.type === "work") {
      last.entries.push(e);
      this.track(e, last);
      last.bump();
      this.setChanged(0);
      return last;
    }
    const item = new Item(isWork(e.kind) ? "work" : e.kind === "user" ? "user" : e.kind === "assistant" ? "text" : "notice", [e]);
    this.track(e, item);
    this.setItems(items.concat(item));
    // Nobody is looking: keep the newest rows, the rest go back to "earlier".
    if (!this.inDeck && items.length > 300) this.trim(200);
    this.setChanged(0);
    return item;
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
        if (x.kind === "tool") this.tools.delete(x.id);
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
    });
    this.blocks.clear();
    this.tools.clear();
    this.itemOf.clear();
    this.byId.clear();
    this.loaded = false;
    this.loading = null;
  }

  private touch(e: Entry) {
    this.itemOf.get(e)?.bump();
    this.setChanged(0);
  }

  notice(text: string) {
    this.append(new Entry(`n${Item.next}`, "notice", text));
  }

  /** Applies one change from the host, which owns the rows (ThreadStore.swift). */
  apply(ev: HostEvent) {
    switch (ev.t) {
      case "session":
        this.sessionId = ev.sid;
        break;
      case "add": {
        const e = Entry.from(ev.e);
        e.live = ev.e.live === 1;
        this.byId.set(e.id, e);
        this.append(e);
        break;
      }
      case "app": {
        const e = this.byId.get(ev.id);
        if (!e) return;
        if (ev.f === "i") e.toolInput = (e.toolInput ?? "") + ev.s;
        else e.text += ev.s;
        this.touch(e);
        break;
      }
      case "upd": {
        const e = this.byId.get(ev.e.id);
        if (!e) return;
        const w = ev.e;
        e.text = w.t;
        e.toolName = w.n;
        e.toolInput = w.i;
        e.toolResult = w.r;
        e.resultLength = w.rl;
        e.isError = w.e === 1;
        e.sub = w.sa;
        e.live = w.live === 1;
        this.touch(e);
        break;
      }
      case "run":
        this.setRunning(ev.v);
        break;
      case "perm":
        this.setPermission(ev.id ? { id: ev.id, tool: ev.n, description: ev.d, input: ev.i } : null);
        break;
    }
  }
}
