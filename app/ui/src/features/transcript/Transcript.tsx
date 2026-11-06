import { createEffect, For, on, onCleanup, onMount, Show, type Accessor } from "solid-js";
import { Row } from "./Rows";
import { loadEarlier } from "../../store";
import type { ThreadState } from "../../store/thread";

// The transcript: one scrolling column of rows, every held row in the DOM. Rows off screen
// skip layout and painting (`content-visibility: auto`, which keeps each row's last real
// height), so a long one costs memory, not frames. What it does itself:
// - follows the end while you're at the end, in the same frame the content grows;
// - keeps what you're reading still when rows above it change height or arrive;
// - fetches earlier history as you near the top.

export function Transcript(props: { t: ThreadState; shown: Accessor<boolean> }) {
  let scroller!: HTMLDivElement;
  let content!: HTMLDivElement;
  let following = true;
  let savedTop = 0;
  let adjusting = false;
  const heights = new WeakMap<Element, number>();

  const atEnd = () => scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 4;
  const toEnd = () => {
    adjusting = true;
    scroller.scrollTop = scroller.scrollHeight;
  };

  const rows = new ResizeObserver((entries) => {
    if (!props.shown()) return;
    if (following) return toEnd();
    // Rows above what you're reading changed height: move by as much, so it stays put.
    const top = scroller.scrollTop;
    let shift = 0;
    for (const entry of entries) {
      const el = entry.target as HTMLElement;
      const height = entry.borderBoxSize[0]?.blockSize ?? el.offsetHeight;
      const before = heights.get(el);
      heights.set(el, height);
      if (before !== undefined && el.offsetTop + before <= top) shift += height - before;
    }
    if (shift) {
      adjusting = true;
      scroller.scrollTop = top + shift;
    }
  });
  onCleanup(() => rows.disconnect());

  const observe = (el: HTMLElement) => {
    rows.observe(el);
    onCleanup(() => rows.unobserve(el));
  };

  /** Rows held for a thread you're at the end of: more came in as you read back. */
  const KEEP = 240;
  const onScroll = () => {
    if (adjusting) {
      adjusting = false;
      following = atEnd();
      return;
    }
    const was = following;
    following = atEnd();
    // Back at the end after reading back: let go of what was fetched on the way up.
    if (following && !was && props.t.items().length > KEEP) {
      props.t.trim(KEEP);
      toEnd();
    }
    if (scroller.scrollTop < 1200 && props.t.before() > 0) earlier();
  };

  const earlier = async () => {
    const oldHeight = scroller.scrollHeight;
    const oldTop = scroller.scrollTop;
    const added = await loadEarlier(props.t);
    if (!added) return;
    // The new rows went in above: keep the same row under your eyes.
    adjusting = true;
    scroller.scrollTop = oldTop + (scroller.scrollHeight - oldHeight);
  };

  // Anything new in this thread: to the end if you're following, before this frame paints.
  createEffect(
    on(props.t.changed, () => {
      if (props.shown() && following) toEnd();
    }),
  );

  createEffect(
    on(props.shown, (shown) => {
      if (shown) {
        if (following) toEnd();
        else {
          adjusting = true;
          scroller.scrollTop = savedTop;
        }
      } else {
        savedTop = scroller.scrollTop;
        if (following) props.t.trim(KEEP);
      }
    }),
  );

  onMount(() => {
    // The first rows arrive with the page: start at the end.
    toEnd();
    const growth = new ResizeObserver(() => {
      if (props.shown() && following) toEnd();
    });
    growth.observe(content);
    onCleanup(() => growth.disconnect());
  });

  return (
    <div class="transcript" ref={scroller} onScroll={onScroll}>
      <div class="transcript-content" ref={content}>
        <Show when={!props.t.isLoaded()}>
          <div class="loading">Loading…</div>
        </Show>
        <Show when={props.t.before() > 0}>
          <div class="earlier">{props.t.before().toLocaleString()} earlier rows</div>
        </Show>
        <For each={props.t.items()}>
          {(item) => (
            <div class="item" ref={observe}>
              <Row item={item} shown={props.shown} />
            </div>
          )}
        </For>
      </div>
    </div>
  );
}
