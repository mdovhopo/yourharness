import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import type { SearchHit } from "../../protocol";
import { resumeSession, search, select } from "../../store";

// ⌘K: threads by title, then what was said in them. ↑↓ to move, ↩ to open, Esc to close.

export function Spotlight() {
  const [open, setOpen] = createSignal(false);
  const [hits, setHits] = createSignal<SearchHit[]>([]);
  const [index, setIndex] = createSignal(0);
  /** A pasted session ID: offered as "Resume Claude Code session". */
  const [session, setSession] = createSignal<string | null>(null);
  const [error, setError] = createSignal("");
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
    const id = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/i.exec(q);
    setSession(id ? id[0] : null);
    setError("");
    clearTimeout(timer);
    timer = window.setTimeout(async () => {
      setHits(await search(q));
      setIndex(0);
    }, 80);
  };
  const resume = async () => {
    try {
      await resumeSession(session()!);
      setOpen(false);
    } catch (e) {
      setError(String(e).replace(/^Error: /, ""));
    }
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
              if (e.key === "Enter") session() ? resume() : choose(hits()[index()]);
            }}
          />
          <Show when={session()}>
            <div class="spotlight-results">
              <button class="spotlight-hit on" onMouseDown={resume}>
                <span class="spotlight-title">Resume Claude Code session</span>
                <span class="spotlight-snippet">{error() || session()}</span>
              </button>
            </div>
          </Show>
          <Show when={!session() && hits().length}>
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
