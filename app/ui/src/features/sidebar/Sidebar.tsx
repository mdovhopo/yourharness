import { createMemo, createSignal, For, Show } from "solid-js";
import { confirmNative, nativeMenu } from "../../host";
import type { ProjectWire } from "../../protocol";
import { newThread, pin, remove, rename, select, selectedId, settle, threadList, projects, type ThreadState } from "../../store";

// Projects with their threads: pinned first, then the newest; settled ones on a shelf of
// their own. Right-click (or the thread's ⋯) opens a native menu.

const SHOWN = 12;

export function Sidebar() {
  const groups = createMemo(() => {
    const byProject = new Map<string, ThreadState[]>();
    for (const t of threadList()) {
      const id = t.summary().projectId;
      const list = byProject.get(id);
      if (list) list.push(t);
      else byProject.set(id, [t]);
    }
    const list = projects().map((p) => ({ project: p, threads: byProject.get(p.id) ?? [] }));
    // Home first, then the others as saved.
    return list.sort((a, b) => Number(b.project.home) - Number(a.project.home));
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

function ProjectGroup(props: { project: ProjectWire; threads: ThreadState[] }) {
  const [all, setAll] = createSignal(false);
  const [shelf, setShelf] = createSignal(false);
  const sorted = createMemo(() => {
    const live: ThreadState[] = [], settled: ThreadState[] = [];
    for (const t of props.threads) (t.summary().settledAt ? settled : live).push(t);
    live.sort((a, b) => Number(b.summary().pinned) - Number(a.summary().pinned) || b.summary().updatedAt - a.summary().updatedAt);
    settled.sort((a, b) => (b.summary().settledAt ?? 0) - (a.summary().settledAt ?? 0));
    return { live, settled };
  });
  const shown = createMemo(() => (all() ? sorted().live : sorted().live.slice(0, SHOWN)));
  return (
    <section class="project">
      <div class="project-head">
        <span class="project-name" title={props.project.path}>{props.project.home ? "Home" : props.project.name}</span>
        <button class="icon" title="New Thread" onClick={() => newThread(props.project)}>
          +
        </button>
      </div>
      <For each={shown()}>{(t) => <ThreadRow t={t} />}</For>
      <Show when={sorted().live.length > SHOWN}>
        <button class="more" onClick={() => setAll(!all())}>
          {all() ? "Show less" : `Show ${sorted().live.length - SHOWN} more`}
        </button>
      </Show>
      <Show when={sorted().settled.length > 0}>
        <button class="more" onClick={() => setShelf(!shelf())}>
          {shelf() ? "Hide settled" : `Settled (${sorted().settled.length})`}
        </button>
        <Show when={shelf()}>
          <For each={sorted().settled}>{(t) => <ThreadRow t={t} />}</For>
        </Show>
      </Show>
    </section>
  );
}

function ThreadRow(props: { t: ThreadState }) {
  const [editing, setEditing] = createSignal(false);
  const t = props.t;
  const menu = async (e: MouseEvent) => {
    e.preventDefault();
    const s = t.summary();
    const choice = await nativeMenu(
      [
        { id: "rename", title: "Rename…" },
        { id: "pin", title: s.pinned ? "Unpin" : "Pin" },
        { id: "settle", title: s.settledAt ? "Bring Back" : "Settle" },
        "-",
        { id: "delete", title: "Delete…", destructive: true },
      ],
      e.clientX,
      e.clientY,
    );
    if (choice === "rename") setEditing(true);
    if (choice === "pin") pin(t, !s.pinned);
    if (choice === "settle") settle(t, !s.settledAt);
    if (choice === "delete" && (await confirmNative(`Delete “${s.title}”?`, "Its history is removed from YourHarness. Claude Code's own session file stays.", "Delete"))) remove(t);
  };
  const commit = (value: string) => {
    setEditing(false);
    const title = value.trim();
    if (title && title !== t.title()) rename(t, title);
  };
  return (
    <div
      class="thread-row"
      data-thread={t.id}
      classList={{ selected: selectedId() === t.id, unread: !!t.summary().unread && selectedId() !== t.id }}
      onClick={() => !editing() && select(t.id)}
      onDblClick={() => setEditing(true)}
      onContextMenu={menu}
    >
      <Show when={t.summary().pinned}>
        <span class="pin" title="Pinned">
          ●
        </span>
      </Show>
      <Show
        when={editing()}
        fallback={<span class="thread-title">{t.title()}</span>}
      >
        <input
          class="rename"
          value={t.title()}
          ref={(el) => requestAnimationFrame(() => (el.focus(), el.select()))}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit(e.currentTarget.value);
            if (e.key === "Escape") setEditing(false);
          }}
          onBlur={(e) => commit(e.currentTarget.value)}
        />
      </Show>
      <Show when={t.waitingForYou()}>
        <span class="badge">waiting</span>
      </Show>
      <Show when={t.running() && !t.waitingForYou()}>
        <span class="dot" />
      </Show>
    </div>
  );
}
