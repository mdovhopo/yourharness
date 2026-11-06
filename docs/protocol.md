# Page ⇄ backend protocol

The contract between `app/ui` and the backend (today `HarnessBackend` through `app/host`;
later Rust). JSON only, short stable keys. Chunk 1 formalizes this into topics with revisions
(docs/architecture.md); until then, this is what's on the wire.

## Requests (page → backend)

`window.webkit.messageHandlers.host.postMessage({method, params})`, answered with a promise.

| method | params | |
|---|---|---|
| `send` | `th`, `text`, `sid`, `projectId`, `model`, `mode` | Send a message; starts or resumes the thread's `claude` |
| `interrupt` | `th` | Stop the turn |
| `respond` | `th`, `id`, `allow` | Answer a permission request |
| `setMode`, `setModel` | `th`, `mode` / `model` | |
| `config`, `log`, `quit` | | Development |
| `probe.*` | | The performance run (scripts/perf-run.sh) |

Fetched, not pushed: `yhw://app/api/library` (projects, threads) and
`yhw://app/api/page?thread=&before=&limit=` (`{total, from, entries}`, a page of history).

## Patches (backend → page)

`__yh.recv([...])`, one batch per display frame. Rows use `WireEntry` (`app/ui/src/host.ts`):
`id`, `k` (user, assistant, thinking, tool, notice, command), `t` text, `n` tool name, `i` tool
input, `r` result head, `rl` its full length, `e` error, `sa` sub-agent, `live`.

| t | fields | |
|---|---|---|
| `add` | `th`, `e` | New row at the end |
| `app` | `th`, `id`, `f` (`t` text, `i` input), `s` | Streamed text appended to a row |
| `upd` | `th`, `e` | A row's new state |
| `run` | `th`, `v` | Running or not |
| `perm` | `th`, `id`?, `n`, `d`, `i` | Waiting for permission; no `id`: answered |
| `session` | `th`, `sid` | The Claude session ID |
