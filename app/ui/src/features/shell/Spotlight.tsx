import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import type { SearchHit } from "../../protocol";
import { search, select } from "../../store";

// ⌘K: threads by title, then what was said in them. ↑↓ to move, ↩ to open, Esc to close.

export function Spotlight() {
  const [open, setOpen] = createSignal(false);
  const [hits, setHits] = createSignal<SearchHit[]>([]);
  const [index, setIndex] = createSignal(0);
  let field!: HTMLInputElement;
  let timer = 0;
  const onKey = (e: KeyboardEvent) => {
    if (e.metaKey && e.key === "k") {
      e.preventDefault();
      setOpen(true);
      requestAnimationFrame(() => (field.focus(), field.select()));
    }
  };
  onMount(() => {
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });
  const query = (q: string) => {
    clearTimeout(timer);
    timer = window.setTimeout(async () => {
      setHits(await search(q));
      setIndex(0);
    }, 80);
  };
  const choose = (hit: SearchHit | undefined) => {
    if (!hit) return;
    setOpen(false);
    select(hit.th);
  };
  return (
    <Show when={open()}>
      <div class="overlay" onMouseDown={(e) => e.target === e.currentTarget && setOpen(false)}>
        <div class="spotlight">
          <input
            ref={field}
            placeholder="Search threads"
            onInput={(e) => query(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") setOpen(false);
              if (e.key === "ArrowDown") (e.preventDefault(), setIndex(Math.min(index() + 1, hits().length - 1)));
              if (e.key === "ArrowUp") (e.preventDefault(), setIndex(Math.max(index() - 1, 0)));
              if (e.key === "Enter") choose(hits()[index()]);
            }}
          />
          <Show when={hits().length}>
            <div class="spotlight-results">
              <For each={hits()}>
                {(hit, i) => (
                  <button class="spotlight-hit" classList={{ on: i() === index() }} onMouseDown={() => choose(hit)}>
                    <span class="spotlight-title">{hit.title}</span>
                    <Show when={hit.snippet}>
                      <span class="spotlight-snippet">{hit.snippet}</span>
                    </Show>
                  </button>
                )}
              </For>
            </div>
          </Show>
        </div>
      </div>
    </Show>
  );
}
