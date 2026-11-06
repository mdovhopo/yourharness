// The wire between page and backend (docs/protocol.md). Plain JSON with short keys; nothing
// here assumes which language the backend is written in.

/** One transcript row. */
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
  live?: number;
  sa?: { d: string; s: string; c?: number };
};

export type ProjectWire = { id: string; name: string; path: string; mode: string; home: boolean };

export type ThreadWire = {
  id: string;
  projectId: string;
  title: string;
  updatedAt: number;
  pinned: boolean;
  settledAt?: number;
  running: boolean;
  waiting: boolean;
  model?: string;
  effort?: string;
  draft?: string;
};

export type ModelWire = { value: string; displayName: string; description?: string; supportedEffortLevels: string[]; resolvedModel?: string };

export type LibrarySnapshot = {
  projects: ProjectWire[];
  threads: ThreadWire[];
  defaults: { model: string; effort: string };
  models: ModelWire[];
};

export type QuestionWire = { q: string; h: string; multi: boolean; options: { label: string; d: string }[] };
export type PermissionWire = { id: string; n: string; d: string; i: string; questions?: QuestionWire[]; plan?: string };

/** A thread's rows as fetched: `entries` are rows `from..<total`, at revision `rv`. */
export type ThreadSnapshot = { total: number; from: number; entries: WireEntry[]; rv: number; perm?: PermissionWire; running: boolean };
export type Page = { total: number; from: number; entries: WireEntry[] };

/** Patches, one batch per frame. Row patches carry their thread's revision. */
export type Patch =
  | { t: "add"; th: string; rv: number; e: WireEntry }
  | { t: "app"; th: string; rv: number; id: string; f: "t" | "i"; s: string }
  | { t: "upd"; th: string; rv: number; e: WireEntry }
  | { t: "run"; th: string; rv: number; v: boolean }
  | { t: "perm"; th: string; rv: number; p: PermissionWire | null }
  | { t: "thread"; th: ThreadWire }
  | { t: "threadGone"; th: string }
  | { t: "project"; p: ProjectWire }
  | { t: "models"; v: ModelWire[] };

export const MODES: [string, string][] = [
  ["auto", "Auto"],
  ["ask", "Ask"],
  ["acceptEdits", "Accept Edits"],
  ["plan", "Plan"],
  ["bypass", "Bypass"],
];
