import { createMemo, createSignal, For, Show, type Accessor } from "solid-js";
import { entryHTML, escapeHTML } from "./markdown";
import type { Entry, Item } from "../../store/thread";

// One component per kind of row. Each reads its item's version, so a streamed delta redraws
// that one row. `shown` is false while the transcript is parked: a parked row keeps what it
// last drew and catches up when it's shown again, so threads you aren't looking at cost no
// markdown work.

export function Row(props: { item: Item; shown: Accessor<boolean> }) {
  const item = props.item;
  switch (item.type) {
    case "user":
      return <UserRow item={item} shown={props.shown} />;
    case "text":
      return <TextRow item={item} shown={props.shown} />;
    case "work":
      return <WorkRow item={item} shown={props.shown} />;
    default:
      return <NoticeRow item={item} shown={props.shown} />;
  }
}

/** The item's version while shown; while parked, nothing to track, so nothing redraws. */
function whileShown<T>(props: { item: Item; shown: Accessor<boolean> }, compute: () => T): Accessor<T> {
  return createMemo<T>((previous) => {
    if (!props.shown() && previous !== undefined) return previous;
    props.item.version();
    return compute();
  }, undefined as T);
}

function UserRow(props: { item: Item; shown: Accessor<boolean> }) {
  const text = whileShown(props, () => props.item.entries[0].text);
  return (
    <div class="row user">
      <div class="bubble">{text()}</div>
    </div>
  );
}

function TextRow(props: { item: Item; shown: Accessor<boolean> }) {
  const html = whileShown(props, () => entryHTML(props.item.entries[0]));
  return <div class="row text md" innerHTML={html()} />;
}

function NoticeRow(props: { item: Item; shown: Accessor<boolean> }) {
  const e = props.item.entries[0];
  const html = whileShown(props, () => (e.kind === "command" ? entryHTML(e) : escapeHTML(e.text)));
  return <div class={`row notice ${e.kind === "command" ? "md" : ""}`} innerHTML={html()} />;
}

/** A tool call's one-line gist: the command, the file, the pattern. */
export function toolGist(e: Entry): string {
  const input = e.toolInput ?? "";
  let parsed: any = null;
  if (input.startsWith("{")) {
    try {
      parsed = JSON.parse(input);
    } catch {
      parsed = null;
    }
  }
  if (e.sub) return e.sub.d;
  if (parsed) {
    const value = parsed.command ?? parsed.file_path ?? parsed.path ?? parsed.pattern ?? parsed.url ?? parsed.query ?? parsed.description ?? parsed.prompt;
    if (typeof value === "string") return value.split("\n")[0].slice(0, 160);
  }
  return input.replace(/\s+/g, " ").slice(0, 160);
}

function summary(entries: Entry[]): string {
  let tools = 0, thinking = 0;
  const names = new Map<string, number>();
  for (const e of entries) {
    if (e.kind === "tool") {
      tools++;
      const name = e.toolName ?? "Tool";
      names.set(name, (names.get(name) ?? 0) + 1);
    } else thinking++;
  }
  const top = [...names.entries()].sort((a, b) => b[1] - a[1]).slice(0, 4).map(([n, c]) => (c > 1 ? `${n} ×${c}` : n));
  const parts: string[] = [];
  if (tools) parts.push(`${tools} tool call${tools === 1 ? "" : "s"}`);
  if (thinking && !tools) parts.push("Thought");
  return parts.join(" · ") + (top.length ? ` — ${top.join(", ")}` : "");
}

function WorkRow(props: { item: Item; shown: Accessor<boolean> }) {
  const [open, setOpen] = createSignal(false);
  const state = whileShown(props, () => {
    const entries = props.item.entries;
    const last = entries[entries.length - 1];
    return {
      summary: summary(entries),
      live: last.live || (last.kind === "tool" && last.toolResult === undefined && last.sub?.s === "running"),
      latest: last.kind === "tool" ? `${last.toolName ?? "Tool"} ${toolGist(last)}` : "Thinking…",
      count: entries.length,
    };
  });
  return (
    <div class="row work" classList={{ open: open() }}>
      <button class="work-head" onClick={() => setOpen(!open())}>
        <span class="chevron">›</span>
        <span class="work-summary">{state().summary}</span>
        <Show when={state().live}>
          <span class="work-latest">{state().latest}</span>
        </Show>
      </button>
      <Show when={open()}>
        <div class="work-body">
          <For each={props.item.entries}>{(e) => <WorkEntry entry={e} item={props.item} shown={props.shown} />}</For>
        </div>
      </Show>
    </div>
  );
}

function WorkEntry(props: { entry: Entry; item: Item; shown: Accessor<boolean> }) {
  const [open, setOpen] = createSignal(false);
  const e = props.entry;
  const view = whileShown(props, () => ({
    head: e.kind === "tool" ? toolGist(e) : e.text.split("\n")[0].slice(0, 160),
    error: e.isError,
    result: e.toolResult,
    sub: e.sub ? `${e.sub.s}${e.sub.c ? ` · ${e.sub.c} steps` : ""}` : "",
  }));
  return (
    <div class="step" classList={{ error: view().error }}>
      <button class="step-head" onClick={() => setOpen(!open())}>
        <span class="tool-name">{e.kind === "tool" ? e.toolName : "Thinking"}</span>
        <span class="step-gist">{view().head}</span>
        <Show when={view().sub}>
          <span class="step-sub">{view().sub}</span>
        </Show>
      </button>
      <Show when={open()}>
        <Show when={e.kind === "tool"} fallback={<div class="md thinking" innerHTML={entryHTML(e)} />}>
          <pre class="step-input">{e.toolInput}</pre>
          <Show when={view().result !== undefined}>
            <pre class="step-result">
              {view().result}
              {(e.resultLength ?? 0) > (view().result?.length ?? 0) ? `\n… ${(e.resultLength! - view().result!.length).toLocaleString()} more characters` : ""}
            </pre>
          </Show>
        </Show>
      </Show>
    </div>
  );
}
