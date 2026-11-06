import { createMemo, For, Show } from "solid-js";
import { Composer } from "./features/composer/Composer";
import { Sidebar } from "./features/sidebar/Sidebar";
import { Transcript } from "./features/transcript/Transcript";
import { FindBar } from "./features/transcript/Find";
import { Settings } from "./features/shell/Settings";
import { Spotlight } from "./features/shell/Spotlight";
import { deck, selectedId, thread } from "./store";

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
          <FindBar />
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
      <Spotlight />
      <Settings />
    </div>
  );
}
