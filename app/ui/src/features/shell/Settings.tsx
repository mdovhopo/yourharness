import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { MODES } from "../../protocol";
import { changeSetting, models, settings } from "../../store";

// Settings (⌘,): defaults for new threads, titles, notifications, appearance.

export const [settingsOpen, setSettingsOpen] = createSignal(false);

export function Settings() {
  const onKey = (e: KeyboardEvent) => {
    if (e.metaKey && e.key === ",") {
      e.preventDefault();
      setSettingsOpen(true);
    }
    if (e.key === "Escape") setSettingsOpen(false);
  };
  onMount(() => {
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });
  return (
    <Show when={settingsOpen() && settings()}>
      {(s) => (
        <div class="overlay" onMouseDown={(e) => e.target === e.currentTarget && setSettingsOpen(false)}>
          <div class="sheet">
            <h2>Settings</h2>
            <section>
              <h3>New threads</h3>
              <label>
                Model
                <select value={s().model} onChange={(e) => changeSetting("model", e.currentTarget.value)}>
                  <option value="">Claude Code's default</option>
                  <For each={models()}>{(m) => <option value={m.value}>{m.displayName}</option>}</For>
                </select>
              </label>
              <label>
                Effort
                <select value={s().effort} onChange={(e) => changeSetting("effort", e.currentTarget.value)}>
                  <option value="">Claude Code's default</option>
                  <For each={["low", "medium", "high", "xhigh", "max"]}>{(l) => <option value={l}>{l[0].toUpperCase() + l.slice(1)}</option>}</For>
                </select>
              </label>
              <label>
                Permissions
                <select value={s().mode} onChange={(e) => changeSetting("mode", e.currentTarget.value)}>
                  <For each={MODES}>{([v, l]) => <option value={v}>{l}</option>}</For>
                </select>
              </label>
            </section>
            <section>
              <h3>General</h3>
              <label class="toggle">
                <input type="checkbox" checked={s().titles} onChange={(e) => changeSetting("titles", e.currentTarget.checked)} />
                Generate thread titles (Haiku, no tools)
              </label>
              <label class="toggle">
                <input type="checkbox" checked={s().notifications} onChange={(e) => changeSetting("notifications", e.currentTarget.checked)} />
                Notify me when a turn ends or Claude waits for me
              </label>
              <label>
                Appearance
                <select value={s().appearance} onChange={(e) => changeSetting("appearance", e.currentTarget.value)}>
                  <option value="system">System</option>
                  <option value="light">Light</option>
                  <option value="dark">Dark</option>
                </select>
              </label>
            </section>
            <div class="card-actions">
              <button class="primary" onClick={() => setSettingsOpen(false)}>
                Done
              </button>
            </div>
          </div>
        </div>
      )}
    </Show>
  );
}
