import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show } from "solid-js";
import type { ProjectMeta } from "./host";
import type { ThreadState } from "./model";
import { Transcript } from "./Transcript";
import {
  chooseMode, chooseModel, deck, interrupt, mode, model, newThread, projects, respond, select, selectedId, send, thread, threadList,
} from "./store";

export function App() {
  const selected = createMemo(() => thread(selectedId()));
  return (
    <div class="window">
      <Sidebar />
      <main class="detail">
        <header class="titlebar">
          <span class="title">{selected()?.title() ?? "YourHarness"}</span>
        </header>
        <div class="deck">
          <For each={deck()}>
            {(t) => {
              const shown = () => selectedId() === t.id;
              return (
                <div class="slot" style={{ display: shown() ? "flex" : "none" }}>
                  <Transcript t={t} shown={shown} />
                </div>
              );
            }}
          </For>
          <Show when={!selected()}>
            <div class="empty">Pick a thread, or start one with + next to a project.</div>
          </Show>
        </div>
        <Show when={selected()}>{(t) => <Composer t={t()} />}</Show>
      </main>
    </div>
  );
}

const SHOWN_PER_PROJECT = 12;

function Sidebar() {
  const groups = createMemo(() => {
    const byProject = new Map<string, ThreadState[]>();
    for (const t of threadList()) {
      const list = byProject.get(t.meta.projectId);
      if (list) list.push(t);
      else byProject.set(t.meta.projectId, [t]);
    }
    return projects().map((p) => ({ project: p, threads: byProject.get(p.id) ?? [] }));
  });
  return (
    <nav class="sidebar">
      <div class="sidebar-top" />
      <div class="sidebar-scroll">
        <For each={groups()}>{(g) => <ProjectGroup project={g.project} threads={g.threads} />}</For>
      </div>
    </nav>
  );
}

function ProjectGroup(props: { project: ProjectMeta; threads: ThreadState[] }) {
  const [all, setAll] = createSignal(false);
  const shown = createMemo(() => (all() ? props.threads : props.threads.slice(0, SHOWN_PER_PROJECT)));
  return (
    <section class="project">
      <div class="project-head">
        <span class="project-name" title={props.project.path}>{props.project.name}</span>
        <button class="icon" title="New thread" onClick={() => newThread(props.project)}>+</button>
      </div>
      <For each={shown()}>
        {(t) => (
          <button class="thread-row" data-thread={t.id} classList={{ selected: selectedId() === t.id }} onClick={() => select(t.id)}>
            <span class="thread-title">{t.title()}</span>
            <Show when={t.permission()}>
              <span class="badge">waiting</span>
            </Show>
            <Show when={t.running() && !t.permission()}>
              <span class="dot" />
            </Show>
          </button>
        )}
      </For>
      <Show when={props.threads.length > SHOWN_PER_PROJECT}>
        <button class="more" onClick={() => setAll(!all())}>
          {all() ? "Show less" : `Show ${props.threads.length - SHOWN_PER_PROJECT} more`}
        </button>
      </Show>
    </section>
  );
}

const MODELS = [
  ["", "Default model"],
  ["haiku", "Haiku"],
  ["sonnet", "Sonnet"],
  ["opus", "Opus"],
];
const MODES = [
  ["default", "Ask"],
  ["acceptEdits", "Accept edits"],
  ["auto", "Auto"],
  ["plan", "Plan"],
  ["bypassPermissions", "Bypass"],
];

/** Drafts per thread, kept while the page lives. */
const drafts = new Map<string, string>();

function Composer(props: { t: ThreadState }) {
  let field!: HTMLTextAreaElement;
  const grow = () => {
    field.style.height = "auto";
    field.style.height = Math.min(field.scrollHeight, window.innerHeight * 0.4) + "px";
  };
  const submit = () => {
    const text = field.value;
    if (!text.trim()) return;
    field.value = "";
    drafts.delete(props.t.id);
    grow();
    send(props.t, text);
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      submit();
    }
  };
  const onGlobalKey = (e: KeyboardEvent) => {
    const t = props.t;
    if (e.metaKey && e.key === "." && t.running()) {
      e.preventDefault();
      interrupt(t);
    }
    if (t.permission() && e.metaKey && e.key === "Enter") {
      e.preventDefault();
      respond(t, true);
    }
    if (t.permission() && e.metaKey && e.key === "Backspace") {
      e.preventDefault();
      respond(t, false);
    }
  };
  onMount(() => {
    window.addEventListener("keydown", onGlobalKey);
    onCleanup(() => window.removeEventListener("keydown", onGlobalKey));
  });
  // Moving between threads keeps each one's draft.
  let current = props.t.id;
  const swap = () => {
    if (current === props.t.id) return;
    drafts.set(current, field.value);
    current = props.t.id;
    field.value = drafts.get(current) ?? "";
    grow();
  };
  createEffect(on(() => props.t.id, swap, { defer: true }));
  return (
    <div class="composer-wrap">
      <Show when={props.t.permission()}>
        {(p) => (
          <div class="permission">
            <div class="permission-head">
              Claude wants to use <b>{p().tool}</b>
              <Show when={p().description}>
                <span class="permission-desc">{p().description}</span>
              </Show>
            </div>
            <pre class="permission-input">{prettyInput(p().input)}</pre>
            <div class="permission-actions">
              <button class="secondary" onClick={() => respond(props.t, false)}>Deny <kbd>⌘⌫</kbd></button>
              <button class="primary" onClick={() => respond(props.t, true)}>Allow <kbd>⌘↩</kbd></button>
            </div>
          </div>
        )}
      </Show>
      <div class="composer">
        <textarea
          ref={field}
          rows={1}
          placeholder="Ask Claude…"
          onInput={grow}
          onKeyDown={onKey}
        />
        <div class="composer-bar">
          <select value={model()} onChange={(e) => chooseModel(e.currentTarget.value)}>
            <For each={MODELS}>{([v, l]) => <option value={v}>{l}</option>}</For>
          </select>
          <select value={mode()} onChange={(e) => chooseMode(e.currentTarget.value)}>
            <For each={MODES}>{([v, l]) => <option value={v}>{l}</option>}</For>
          </select>
          <span class="spacer" />
          <Show when={props.t.running()} fallback={<button class="primary send" onClick={submit}>Send</button>}>
            <button class="secondary" onClick={() => interrupt(props.t)}>Stop <kbd>⌘.</kbd></button>
          </Show>
        </div>
      </div>
    </div>
  );
}

function prettyInput(input: string): string {
  try {
    const parsed = JSON.parse(input);
    if (typeof parsed.command === "string") return parsed.command;
    return JSON.stringify(parsed, null, 2);
  } catch {
    return input;
  }
}
