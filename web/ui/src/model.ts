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

  /** Applies one event from this thread's `claude`. */
  apply(ev: HostEvent) {
    switch (ev.t) {
      case "session":
        this.sessionId = ev.sid;
        break;
      case "b": {
        if (ev.p) return this.subagentStep(ev.p);
        const key = `${ev.m}:${ev.i}`;
        if (this.blocks.has(key)) return;
        const kind: Kind = ev.k === "text" ? "assistant" : ev.k === "thinking" ? "thinking" : "tool";
        const e = new Entry(ev.k === "tool" ? ev.id : key, kind);
        e.toolName = ev.n;
        e.live = true;
        this.blocks.set(key, e);
        this.append(e);
        break;
      }
      case "d": {
        if (ev.p) return;
        const key = `${ev.m}:${ev.i}`;
        let e = this.blocks.get(key);
        if (!e) {
          e = new Entry(key, ev.k === "thinking" ? "thinking" : "assistant");
          e.live = true;
          this.blocks.set(key, e);
          this.append(e);
        }
        if (ev.k === "input") e.toolInput = (e.toolInput ?? "") + ev.s;
        else e.text += ev.s;
        this.touch(e);
        break;
      }
      case "am": {
        if (ev.p) {
          for (const block of ev.b) if (block.k === "tool") this.subagentStep(ev.p);
          return;
        }
        ev.b.forEach((block: any, index: number) => {
          const key = `${ev.m}:${index}`;
          let e = this.blocks.get(key) ?? (block.k === "tool" ? this.tools.get(block.id)?.entry : undefined);
          if (!e) {
            e = new Entry(block.k === "tool" ? block.id : key, block.k === "text" ? "assistant" : block.k === "thinking" ? "thinking" : "tool");
            this.blocks.set(key, e);
            this.append(e);
          }
          if (block.k === "tool") {
            e.toolName = block.n;
            e.toolInput = block.s;
          } else {
            e.text = block.s;
          }
          e.live = false;
          this.touch(e);
        });
        break;
      }
      case "tr": {
        if (ev.p) return;
        for (const r of ev.r) {
          const found = this.tools.get(r.id);
          if (!found) continue;
          found.entry.toolResult = r.s;
          found.entry.resultLength = r.l;
          found.entry.isError = r.e === 1;
          found.entry.live = false;
          found.item.bump();
        }
        this.setChanged(0);
        break;
      }
      case "sa": {
        const found = this.tools.get(ev.id);
        if (found) {
          found.entry.sub = { d: ev.d, s: "running", c: 0 };
          found.item.bump();
        }
        break;
      }
      case "sae": {
        const found = this.tools.get(ev.id);
        if (found) {
          found.entry.sub = { ...(found.entry.sub ?? { d: "" }), s: ev.s, c: ev.c };
          found.item.bump();
        }
        break;
      }
      case "perm":
        this.setPermission({ id: ev.id, tool: ev.n, description: ev.d, input: ev.i });
        break;
      case "notice":
        this.notice(ev.s);
        break;
      case "command":
        this.append(new Entry(`c${Item.next}`, "command", ev.s));
        break;
      case "done":
        this.endTurn();
        if (ev.err && ev.s) this.notice(ev.s);
        break;
      case "exit":
        this.endTurn();
        if (ev.code !== 0) this.notice(`claude exited (${ev.code}) ${ev.s ?? ""}`);
        break;
    }
  }

  private subagentStep(parent: string) {
    const found = this.tools.get(parent);
    if (!found) return;
    const sub = found.entry.sub ?? { d: "Sub-agent", s: "running", c: 0 };
    found.entry.sub = { ...sub, c: (sub.c ?? 0) + 1 };
    found.item.bump();
  }

  private endTurn() {
    for (const e of this.blocks.values()) {
      if (e.live) {
        e.live = false;
        this.touch(e);
      }
    }
    this.blocks.clear();
    batch(() => {
      this.setRunning(false);
      this.setPermission(null);
    });
  }
}
