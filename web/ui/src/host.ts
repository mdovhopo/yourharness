// The page's only way out: calls to the host and events from it. In the app the host is
// the Swift side (web/host/Bridge.swift); anything else that speaks the same JSON can stand
// in for it (a recorded stream in a plain browser, another backend later).

export type WireEntry = {
  id: string;
  k: "user" | "assistant" | "thinking" | "tool" | "notice" | "command";
  t: string;
  n?: string;
  i?: string;
  r?: string;
  rl?: number;
  e?: number;
  d?: number;
  sa?: { d: string; s: string; c?: number };
};

export type ProjectMeta = { id: string; name: string; path: string; source: "db" | "claude" };
export type ThreadMeta = { id: string; projectId: string; title: string; updatedAt: number; sessionId?: string; source: string };
export type Listing = { projects: ProjectMeta[]; threads: ThreadMeta[] };
export type Page = { total: number; from: number; entries: WireEntry[] };

/** One event from a thread's `claude`, as LiveThreads.swift writes it. */
export type HostEvent = { t: string; th: string; [key: string]: any };

type Handler = (events: HostEvent[]) => void;
let handler: Handler = () => {};

declare global {
  interface Window {
    __yh: { recv: (events: HostEvent[]) => void };
    webkit?: { messageHandlers: { host: { postMessage: (body: unknown) => Promise<any> } } };
  }
}

window.__yh = { recv: (events) => handler(events) };

export function onEvents(next: Handler) {
  handler = next;
}

export const inApp = !!window.webkit?.messageHandlers?.host;

export function call<T = any>(method: string, params: Record<string, unknown> = {}): Promise<T> {
  if (!inApp) return Promise.reject(new Error(`no host for ${method}`));
  return window.webkit!.messageHandlers.host.postMessage({ method, params });
}

const api = inApp ? "yhw://app/api/" : "/api/";

export async function fetchListing(): Promise<Listing> {
  return (await fetch(api + "library")).json();
}

export async function fetchPage(thread: string, before: number | undefined, limit: number): Promise<Page> {
  const query = new URLSearchParams({ thread, limit: String(limit) });
  if (before !== undefined) query.set("before", String(before));
  return (await fetch(api + "page?" + query)).json();
}

export function hostLog(s: string) {
  if (inApp) call("log", { s }).catch(() => {});
  else console.log(s);
}
