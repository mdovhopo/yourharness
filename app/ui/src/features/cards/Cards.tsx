import { createSignal, For, Show } from "solid-js";
import { MODES, type PermissionWire, type QuestionWire } from "../../protocol";
import { answer, decidePlan, respond, type ThreadState } from "../../store";
import { renderMarkdown } from "../transcript/markdown";

// What Claude is waiting on you for, above the composer: a tool to allow, questions to answer,
// or a plan to approve.

export function WaitingCard(props: { t: ThreadState; p: PermissionWire }) {
  return (
    <Show
      when={props.p.questions?.length}
      fallback={
        <Show when={props.p.plan !== undefined} fallback={<PermissionCard t={props.t} p={props.p} />}>
          <PlanCard t={props.t} p={props.p} />
        </Show>
      }
    >
      <QuestionCard t={props.t} p={props.p} />
    </Show>
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

function PermissionCard(props: { t: ThreadState; p: PermissionWire }) {
  return (
    <div class="card waiting">
      <div class="card-head">
        Claude wants to use <b>{props.p.n}</b>
        <Show when={props.p.d}>
          <span class="card-detail">{props.p.d}</span>
        </Show>
      </div>
      <pre class="card-input">{prettyInput(props.p.i)}</pre>
      <div class="card-actions">
        <button class="secondary" onClick={() => respond(props.t, false)}>
          Deny <kbd>⌘⌫</kbd>
        </button>
        <button class="primary" onClick={() => respond(props.t, true)}>
          Allow <kbd>⌘↩</kbd>
        </button>
      </div>
    </div>
  );
}

function QuestionCard(props: { t: ThreadState; p: PermissionWire }) {
  const [picked, setPicked] = createSignal<Record<string, string[]>>({});
  const [other, setOther] = createSignal<Record<string, string>>({});
  const toggle = (q: QuestionWire, label: string) =>
    setPicked((old) => {
      const current = old[q.q] ?? [];
      const next = q.multi ? (current.includes(label) ? current.filter((l) => l !== label) : [...current, label]) : [label];
      return { ...old, [q.q]: next };
    });
  const submit = () => {
    const answers: Record<string, string[]> = {};
    for (const q of props.p.questions ?? []) {
      const own = other()[q.q]?.trim();
      answers[q.q] = [...(picked()[q.q] ?? []), ...(own ? [own] : [])];
    }
    answer(props.t, answers);
  };
  return (
    <div class="card waiting">
      <For each={props.p.questions}>
        {(q) => (
          <div class="question">
            <div class="card-head">
              <Show when={q.h}>
                <span class="chip">{q.h}</span>
              </Show>
              {q.q}
            </div>
            <div class="options">
              <For each={q.options}>
                {(o) => (
                  <button class="option" classList={{ on: (picked()[q.q] ?? []).includes(o.label) }} onClick={() => toggle(q, o.label)} title={o.d}>
                    <span class="option-label">{o.label}</span>
                    <Show when={o.d}>
                      <span class="option-detail">{o.d}</span>
                    </Show>
                  </button>
                )}
              </For>
            </div>
            <input class="other" placeholder="Or write your own answer" onInput={(e) => setOther({ ...other(), [q.q]: e.currentTarget.value })} />
          </div>
        )}
      </For>
      <div class="card-actions">
        <button class="secondary" onClick={() => respond(props.t, false)}>
          Skip
        </button>
        <button class="primary" onClick={submit}>
          Answer
        </button>
      </div>
    </div>
  );
}

function PlanCard(props: { t: ThreadState; p: PermissionWire }) {
  const [feedback, setFeedback] = createSignal("");
  return (
    <div class="card waiting plan">
      <div class="card-head">Claude's plan</div>
      <div class="plan-body md" innerHTML={renderMarkdown(props.p.plan ?? "", false)} />
      <textarea class="other" rows={2} placeholder="Feedback to keep planning" onInput={(e) => setFeedback(e.currentTarget.value)} />
      <div class="card-actions">
        <button class="secondary" onClick={() => decidePlan(props.t, null, feedback() || "Keep planning.")}>
          Keep Planning
        </button>
        <For each={MODES.filter(([m]) => m === "auto" || m === "acceptEdits" || m === "ask")}>
          {([mode, label]) => (
            <button class={mode === "auto" ? "primary" : "secondary"} onClick={() => decidePlan(props.t, mode)}>
              Approve: {label}
            </button>
          )}
        </For>
      </div>
    </div>
  );
}
