// The page's only way out: requests to the backend (through the host) and the patches it
// sends back. Anything that speaks protocol.ts can stand in for the host.
import type { Patch } from "./protocol";

type Handler = (patches: Patch[]) => void;
let handler: Handler = () => {};

declare global {
  interface Window {
    __yh: { recv: (patches: Patch[]) => void };
    webkit?: { messageHandlers: { host: { postMessage: (body: unknown) => Promise<any> } } };
  }
}

window.__yh = { recv: (patches) => handler(patches) };

export function onPatches(next: Handler) {
  handler = next;
}

export const inApp = !!window.webkit?.messageHandlers?.host;

/** A request; big answers come as JSON text and are parsed here. */
export async function call<T = any>(method: string, params: Record<string, unknown> = {}): Promise<T> {
  if (!inApp) throw new Error(`no host for ${method}`);
  const reply = await window.webkit!.messageHandlers.host.postMessage({ method, params });
  return (typeof reply === "string" && (reply.startsWith("{") || reply.startsWith("[")) ? JSON.parse(reply) : reply) as T;
}

export function hostLog(s: string) {
  if (inApp) call("log", { s }).catch(() => {});
  else console.log(s);
}

/** A native menu at a point; resolves to the chosen item's id, or null. */
export function nativeMenu(items: ({ id: string; title: string; destructive?: boolean } | "-")[], x: number, y: number): Promise<string | null> {
  return call("menu", { items: items.map((i) => (i === "-" ? { id: "-" } : i)), x, y });
}

/** A native alert; resolves true when confirmed. */
export function confirmNative(title: string, message: string, ok: string): Promise<boolean> {
  return call("confirm", { title, message, ok });
}
