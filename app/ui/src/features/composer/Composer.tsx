import { createEffect, createMemo, For, on, onCleanup, onMount, Show } from "solid-js";
import { MODES } from "../../protocol";
import { defaults, interrupt, models, project, respond, saveDraft, send, setEffort, setMode, setModel, type ThreadState } from "../../store";
import { WaitingCard } from "../cards/Cards";

// The message box: ↩ sends (while Claude works, it joins the turn after the current step),
// ⌘↩ stops the turn and sends right away, ⌘. stops. Drafts are kept per thread.

const drafts = new Map<string, string>();

export function Composer(props: { t: ThreadState }) {
  let field!: HTMLTextAreaElement;
  const summary = () => props.t.summary();
  const currentModel = () => summary().model ?? defaults().model ?? "";
  const currentEffort = () => summary().effort ?? defaults().effort ?? "";
  const model = createMemo(() => models().find((m) => m.value === currentModel() || m.resolvedModel === currentModel()));
  const efforts = createMemo(() => model()?.supportedEffortLevels ?? ["low", "medium", "high", "xhigh", "max"]);
  const mode = () => project(summary().projectId)?.mode ?? "auto";

  const grow = () => {
    field.style.height = "auto";
    field.style.height = Math.min(field.scrollHeight, window.innerHeight * 0.4) + "px";
  };
  let draftTimer = 0;
  const onInput = () => {
    grow();
    const t = props.t, text = field.value;
    drafts.set(t.id, text);
    clearTimeout(draftTimer);
    draftTimer = window.setTimeout(() => saveDraft(t, text), 600);
  };
  const submit = (now = false) => {
    const text = field.value;
    if (!text.trim()) return;
    field.value = "";
    drafts.delete(props.t.id);
    saveDraft(props.t, "");
    grow();
    send(props.t, text, now);
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      if (e.metaKey && props.t.permission() && !props.t.permission()!.questions && props.t.permission()!.plan === undefined) return;
      submit(e.metaKey && props.t.running());
    }
  };
  const onGlobalKey = (e: KeyboardEvent) => {
    const t = props.t, p = t.permission();
    if (e.metaKey && e.key === "." && t.running()) {
      e.preventDefault();
      interrupt(t);
    }
    const plain = p && !p.questions && p.plan === undefined;
    if (plain && e.metaKey && e.key === "Enter" && !field.value.trim()) {
      e.preventDefault();
      respond(t, true);
    }
    if (plain && e.metaKey && e.key === "Backspace" && !field.value.trim()) {
      e.preventDefault();
      respond(t, false);
    }
  };
  onMount(() => {
    window.addEventListener("keydown", onGlobalKey);
    onCleanup(() => window.removeEventListener("keydown", onGlobalKey));
    field.value = drafts.get(props.t.id) ?? summary().draft ?? "";
    grow();
  });
  // Moving between threads keeps each one's draft.
  createEffect(
    on(
      () => props.t.id,
      (id) => {
        field.value = drafts.get(id) ?? props.t.summary().draft ?? "";
        grow();
        field.focus();
      },
      { defer: true },
    ),
  );

  return (
    <div class="composer-wrap">
      <Show when={props.t.permission()}>{(p) => <WaitingCard t={props.t} p={p()} />}</Show>
      <div class="composer">
        <textarea ref={field} rows={1} placeholder={props.t.running() ? "Steer Claude… (⌘↩ to interrupt)" : "Ask Claude…"} onInput={onInput} onKeyDown={onKey} />
        <div class="composer-bar">
          <select title="Model" value={currentModel()} onChange={(e) => setModel(props.t, e.currentTarget.value)}>
            <Show when={!model() && currentModel()}>
              <option value={currentModel()}>{currentModel()}</option>
            </Show>
            <For each={models()}>{(m) => <option value={m.value}>{m.displayName}</option>}</For>
          </select>
          <select title="Reasoning effort" value={currentEffort()} onChange={(e) => setEffort(props.t, e.currentTarget.value)}>
            <For each={efforts()}>{(level) => <option value={level}>{level[0].toUpperCase() + level.slice(1)}</option>}</For>
          </select>
          <select title="Permissions" value={mode()} onChange={(e) => setMode(props.t, e.currentTarget.value)}>
            <For each={MODES}>{([value, label]) => <option value={value}>{label}</option>}</For>
          </select>
          <span class="spacer" />
          <Show when={props.t.running()} fallback={<button class="primary send" onClick={() => submit()}>Send</button>}>
            <button class="secondary" onClick={() => interrupt(props.t)}>
              Stop <kbd>⌘.</kbd>
            </button>
          </Show>
        </div>
      </div>
    </div>
  );
}
