import { createMemo, createSignal, onCleanup, onMount, Show } from "solid-js";
import { loadEarlier, selectedId, thread } from "../../store";
import type { Item } from "../../store/thread";

// Find in thread (⌘F): rows whose text matches, newest first; ↩ steps back, ⇧↩ forward, Esc
// closes. Past the oldest row held, earlier history is fetched and searched too.

const [open, setOpen] = createSignal(false);
const [query, setQuery] = createSignal("");
const [current, setCurrent] = createSignal<Item | null>(null);

const matches = (item: Item, q: string) =>
  item.entries.some((e) => e.text.toLowerCase().includes(q) || (e.toolInput ?? "").toLowerCase().includes(q) || (e.toolResult ?? "").toLowerCase().includes(q));

/** The rows that match, oldest first, in the open thread. */
export const hits = createMemo(() => {
  const q = query().trim().toLowerCase();
  const t = thread(selectedId());
  if (!open() || q.length < 2 || !t) return new Set<Item>();
  return new Set(t.items().filter((item) => matches(item, q)));
});

export const findCurrent = current;

function scrollTo(item: Item) {
  setCurrent(item);
  requestAnimationFrame(() => document.querySelector(`[data-item="${item.key}"]`)?.scrollIntoView({ block: "center" }));
}

async function step(back: boolean) {
  const t = thread(selectedId());
  if (!t) return;
  const list = [...hits()];
  const at = current() ? list.indexOf(current()!) : list.length;
  const next = back ? at - 1 : at + 1;
  if (next >= 0 && next < list.length) return scrollTo(list[next]);
  if (back && t.before() > 0) {
    // Nothing older held: fetch the page before and look again.
    while (t.before() > 0) {
      await loadEarlier(t);
      const older = [...hits()].filter((i) => !list.includes(i));
      if (older.length) return scrollTo(older[older.length - 1]);
    }
  }
}

export function FindBar() {
  let field!: HTMLInputElement;
  const onKey = (e: KeyboardEvent) => {
    if (e.metaKey && e.key === "f") {
      e.preventDefault();
      setOpen(true);
      requestAnimationFrame(() => (field?.focus(), field?.select()));
    }
    if (open() && e.metaKey && e.key === "g") {
      e.preventDefault();
      step(!e.shiftKey);
    }
  };
  onMount(() => {
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });
  const count = () => hits().size;
  const position = () => {
    const c = current();
    return c && hits().has(c) ? [...hits()].indexOf(c) + 1 : 0;
  };
  return (
    <Show when={open()}>
      <div class="find-bar">
        <input
          ref={field}
          placeholder="Find in thread"
          value={query()}
          onInput={(e) => {
            setQuery(e.currentTarget.value);
            setCurrent(null);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              step(!e.shiftKey);
            }
            if (e.key === "Escape") {
              setOpen(false);
              setCurrent(null);
            }
          }}
        />
        <span class="find-count">{query().trim().length < 2 ? "" : count() ? `${position() || "–"} of ${count()}` : "No matches"}</span>
        <button class="icon" title="Previous (↩)" onClick={() => step(true)}>
          ↑
        </button>
        <button class="icon" title="Next (⇧↩)" onClick={() => step(false)}>
          ↓
        </button>
        <button class="icon" title="Close (Esc)" onClick={() => (setOpen(false), setCurrent(null))}>
          ×
        </button>
      </div>
    </Show>
  );
}
