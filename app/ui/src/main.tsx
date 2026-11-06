import { render } from "solid-js/web";
import { App } from "./App";
import { call, hostLog, inApp } from "./host";
import { maybeRunProbe } from "./probe";
import { loadLibrary, newThread, projects, respond, select, send, setMode, setModel, thread, threadList, selectedId } from "./store";
import { createRoot, createEffect, on } from "solid-js";
import "./styles.css";

render(() => <App />, document.getElementById("root")!);

loadLibrary().then(async () => {
  if (!inApp) return;
  const config = await call("config");
  for (const flag of String(config.flags || "").split(",").filter(Boolean)) document.documentElement.classList.add(flag);
  if (config.select) {
    const t = threadList().find((t) => t.title() === config.select || t.id === config.select);
    if (t) select(t.id);
    // What the open transcript shows, for telling a blank one.
    setTimeout(() => {
      const el = document.querySelector(".slot:not([style*='none']) .transcript") as HTMLElement | null;
      if (!el) return hostLog("DIAG no transcript");
      const view = el.getBoundingClientRect();
      const items = [...el.querySelectorAll(".item")] as HTMLElement[];
      const inView = items.filter((i) => { const r = i.getBoundingClientRect(); return r.bottom > view.top && r.top < view.bottom; });
      hostLog(`DIAG scrollTop ${el.scrollTop} scrollHeight ${el.scrollHeight} client ${el.clientHeight} items ${items.length} inView ${inView.length} ` +
        inView.slice(0, 5).map((i) => `${i.firstElementChild?.className}:${Math.round(i.getBoundingClientRect().height)}:${(i.textContent ?? "").slice(0, 30)}`).join(" | ") +
        ` last: ${items.slice(-3).map((i) => `${Math.round(i.getBoundingClientRect().height)}:${(i.textContent ?? "").length}`).join(",")}`);
    }, Number(config.diagAfter || 6000));
  }
  if (config.expand) document.documentElement.classList.add("expand-work");
  if (config.send) liveCheck(config);
  maybeRunProbe();
});

/** `YH_SEND`: one real turn from a new thread, logged as it goes, for checking the live path. */
async function liveCheck(config: { send: string; model: string; mode: string; answer: string; quit: boolean; project: string; image: string }) {
  const project = projects().find((p) => !config.project || p.name === config.project) ?? projects()[0];
  if (!project) return hostLog("LIVE FAIL: no project");
  await newThread(project);
  const t = thread(selectedId())!;
  if (config.model) await setModel(t, config.model);
  if (config.mode) await setMode(t, config.mode);
  createRoot(() => {
    createEffect(on(t.permission, (p) => {
      if (!p) return;
      hostLog(`LIVE permission: ${p.n} ${p.i.slice(0, 120)}`);
      if (config.answer) setTimeout(() => respond(t, config.answer === "allow"), 500);
    }));
    createEffect(on(t.running, (running, was) => {
      if (was && !running) {
        const rows = t.items().map((i) => `${i.type}:${i.entries.map((e) => `${e.kind}=` + (e.kind === "tool" ? `${e.toolName}${e.isError ? "!" : ""}` : e.text.slice(0, 60)).replace(/\n/g, " ")).join("|")}`);
        hostLog(`LIVE done\n  ${rows.join("\n  ")}`);
        if (config.quit) setTimeout(() => call("quit"), 300);
      }
    }));
  });
  send(t, config.send, false, config.image ? [config.image] : []);
}
