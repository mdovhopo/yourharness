import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show } from "solid-js";
import { MODES } from "../../protocol";
import { defaults, interrupt, mentionFiles, models, project, respond, saveDraft, send, setEffort, setMode, setModel, type ThreadState } from "../../store";
import { WaitingCard } from "../cards/Cards";

// The message box: ↩ sends (while Claude works, it joins the turn after the current step),
// ⌘↩ stops the turn and sends right away, ⌘. stops. Drafts are kept per thread.

const drafts = new Map<string, string>();

type Suggestion = { label: string; detail: string; insert: string };
type Image = { base64: string; url: string };

/** Reads a pasted or dropped image as base64. */
function readImage(file: File): Promise<Image | null> {
  return new Promise((resolve) => {
    if (!file.type.startsWith("image/")) return resolve(null);
    const reader = new FileReader();
    reader.onload = () => {
      const url = String(reader.result);
      resolve({ base64: url.slice(url.indexOf(",") + 1), url });
    };
    reader.onerror = () => resolve(null);
    reader.readAsDataURL(file);
  });
}

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
  function onInput() {
    grow();
    updateSuggestions();
    const t = props.t, text = field.value;
    drafts.set(t.id, text);
    clearTimeout(draftTimer);
    draftTimer = window.setTimeout(() => saveDraft(t, text), 600);
  }
  const [images, setImages] = createSignal<Image[]>([]);
  const [suggest, setSuggest] = createSignal<{ items: Suggestion[]; index: number; start: number } | null>(null);
  const addImages = async (files: FileList | File[]) => {
    const read = (await Promise.all([...files].map(readImage))).filter((i): i is Image => !!i);
    if (read.length) setImages([...images(), ...read]);
    return read.length > 0;
  };
  const submit = (now = false) => {
    const text = field.value;
    if (!text.trim() && !images().length) return;
    field.value = "";
    drafts.delete(props.t.id);
    saveDraft(props.t, "");
    grow();
    send(props.t, text, now, images().map((i) => i.base64));
    setImages([]);
    setSuggest(null);
  };

  // The menu under the caret: slash commands at the start, files after an @.
  let mentionTimer = 0;
  const updateSuggestions = () => {
    const caret = field.selectionStart ?? field.value.length;
    const before = field.value.slice(0, caret);
    const slash = /^\/([^\s]*)$/.exec(before);
    if (slash) {
      const q = slash[1].toLowerCase();
      const items = props.t
        .commands()
        .filter((c) => c.name.toLowerCase().includes(q))
        .sort((a, b) => Number(!a.name.toLowerCase().startsWith(q)) - Number(!b.name.toLowerCase().startsWith(q)))
        .slice(0, 12)
        .map((c) => ({ label: "/" + c.name, detail: c.hint ? `${c.hint} · ${c.d}` : c.d, insert: "/" + c.name + " " }));
      setSuggest(items.length ? { items, index: 0, start: 0 } : null);
      return;
    }
    const mention = /(^|\s)@([^\s]*)$/.exec(before);
    if (mention) {
      const start = caret - mention[2].length - 1;
      const t = props.t;
      clearTimeout(mentionTimer);
      mentionTimer = window.setTimeout(async () => {
        const files = await mentionFiles(t, mention[2]);
        if (t !== props.t) return;
        const items = files.slice(0, 12).map((f) => ({ label: f.split("/").pop() ?? f, detail: f, insert: "@" + (f.includes(" ") ? `"${f}"` : f) + " " }));
        setSuggest(items.length ? { items, index: 0, start } : null);
      }, 80);
      return;
    }
    setSuggest(null);
  };
  const accept = (choice: Suggestion) => {
    const s = suggest();
    if (!s) return;
    const caret = field.selectionStart ?? field.value.length;
    field.value = field.value.slice(0, s.start) + choice.insert + field.value.slice(caret);
    const at = s.start + choice.insert.length;
    field.setSelectionRange(at, at);
    setSuggest(null);
    onInput();
  };
  const onKey = (e: KeyboardEvent) => {
    const s = suggest();
    if (s) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const n = s.items.length;
        setSuggest({ ...s, index: (s.index + (e.key === "ArrowDown" ? 1 : n - 1)) % n });
        return;
      }
      if ((e.key === "Enter" || e.key === "Tab") && !e.shiftKey && !e.isComposing) {
        e.preventDefault();
        accept(s.items[s.index]);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setSuggest(null);
        return;
      }
    }
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
    <div
      class="composer-wrap"
      onDragOver={(e) => e.preventDefault()}
      onDrop={(e) => {
        if (e.dataTransfer?.files.length) {
          e.preventDefault();
          addImages(e.dataTransfer.files);
        }
      }}
    >
      <Show when={props.t.permission()}>{(p) => <WaitingCard t={props.t} p={p()} />}</Show>
      <PlanStrip t={props.t} />
      <Show when={suggest()}>
        {(s) => (
          <div class="suggestions">
            <For each={s().items}>
              {(item, i) => (
                <button class="suggestion" classList={{ on: i() === s().index }} onMouseDown={(e) => (e.preventDefault(), accept(item))}>
                  <span class="suggestion-label">{item.label}</span>
                  <span class="suggestion-detail">{item.detail}</span>
                </button>
              )}
            </For>
          </div>
        )}
      </Show>
      <div class="composer">
        <Show when={images().length}>
          <div class="composer-images">
            <For each={images()}>
              {(img, i) => (
                <div class="thumb">
                  <img src={img.url} alt="" />
                  <button class="thumb-remove" title="Remove" onClick={() => setImages(images().filter((_, j) => j !== i()))}>
                    ×
                  </button>
                </div>
              )}
            </For>
          </div>
        </Show>
        <textarea ref={field} rows={1} placeholder={props.t.running() ? "Steer Claude… (⌘↩ to interrupt)" : "Ask Claude…"} onInput={onInput} onKeyDown={onKey}
          onPaste={async (e) => {
            const files = e.clipboardData?.files;
            if (files?.length && [...files].some((f) => f.type.startsWith("image/"))) {
              e.preventDefault();
              await addImages(files);
            }
          }}
          onBlur={() => setTimeout(() => setSuggest(null), 100)}
        />
        <div class="composer-bar">
          <select title="Model" value={currentModel()} onChange={(e) => setModel(props.t, e.currentTarget.value)}>
            <option value="">Default model</option>
            <Show when={!model() && currentModel()}>
              <option value={currentModel()}>{currentModel()}</option>
            </Show>
            <For each={models()}>{(m) => <option value={m.value}>{m.displayName}</option>}</For>
          </select>
          <select title="Reasoning effort" value={currentEffort()} onChange={(e) => setEffort(props.t, e.currentTarget.value)}>
            <option value="">Default effort</option>
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

/** Where Claude is in its checklist: the current step and how many are done. */
function PlanStrip(props: { t: ThreadState }) {
  const state = createMemo(() => {
    const items = props.t.plan();
    if (!items.length) return null;
    const done = items.filter((i) => i.st === "completed").length;
    if (done === items.length && !props.t.running()) return null;
    const current = items.find((i) => i.st === "in_progress") ?? items.find((i) => i.st === "pending");
    return { done, total: items.length, current: current ? current.a || current.s : "All done", items };
  });
  const [open, setOpen] = createSignal(false);
  return (
    <Show when={state()}>
      {(s) => (
        <div class="plan-strip">
          <button class="plan-strip-head" onClick={() => setOpen(!open())}>
            <span class="plan-progress">
              {s().done}/{s().total}
            </span>
            <span class="plan-current">{s().current}</span>
          </button>
          <Show when={open()}>
            <ul class="plan-list">
              <For each={s().items}>{(i) => <li class={i.st}>{i.s}</li>}</For>
            </ul>
          </Show>
        </div>
      )}
    </Show>
  );
}
