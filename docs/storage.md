# Storage

Everything the app keeps is in one SQLite file, `harness.sqlite`, in its data folder
(`~/Library/Application Support/YourHarness`, or `YH_DATA_DIR`). Images stay files beside it
(`attachments/`). The code is `Sources/HarnessCore/Storage/` (`HarnessDatabase`, no UI imports)
and the app's `Model/Store.swift`, which turns its state into rows and back.

The usage view has a database of its own beside it, `usage.sqlite` (`HarnessCore/Usage/UsageStore.swift`):
an index of the usage in Claude Code's session files (which files were read and how far, and
each response's tokens by its message and request IDs), sums by fifteen minutes kept up to date
by triggers, and the tokens of side chats and memory agents, which leave no session file.
Export State takes a snapshot of it too (`VACUUM INTO`), and Import State replaces it, WAL and
all: the session files' part could be read again, side chats' and memory agents' couldn't.

## Tables

| Table | What's in it |
|---|---|
| `settings` | One row per top-level setting or cache (`appearance`, `defaults`, `slashCommands`, …), its value as JSON. A change writes one row. |
| `projects` | One row per project, in sidebar order: its JSON. |
| `threads` | One row per thread: its metadata as JSON, plus `project_id`, `updated_at` and `session_id` to query by. |
| `entries` | Transcript rows, `(thread_id, seq)` in order, with real columns for what queries need (kind, text, tool name, input, result, dates). A sub-agent's run is JSON in `subagent`. Every write takes the next `rev`. |
| `transcripts` | Per thread: when its transcript was last saved, and how many rows. |
| `blobs` | Tool inputs, results and sub-agent runs over 32 KB, by SHA-256, LZFSE-compressed, stored once however many rows point at them. A big result also keeps its first 20,000 characters in the row: that's what's loaded (`toolResultRef` names the whole one, `fullToolResult` reads it). |
| `entries_fts` | FTS5 index over what was said, tool names and inputs, and the start of tool output. Its rowid is the entry's. |
| `index_state` | How far each indexer has read the change feed (`rev`). |
| `memories`, `memory_sources`, `memories_fts` | Memories (scope `global` or `project:<id>`, kind, title, description, body, status active/proposed/archived, pinned, origin, read and search counts, what it was merged into) and the transcript rows each came from. Searchable, a title match counting most. See [memory.md](memory.md). |
| `memory_links`, `memory_versions`, `memory_events` | Links between memories; every version of each; and the log of what was done with them (saves, reads, searches, session starts, merges…), which the analytics are made of. |
| `legacy_imports` | Which old JSON files have been moved in. |

## Writing

`saveState` and `saveTranscript` return at once. What's waiting is written on one background
queue in one transaction; saves that arrive while one is being written collapse into the latest,
and a load sees what's waiting before it's on disk. A transcript save compares each row with
what the last save wrote (shared strings, so the comparison is a pointer check for rows that
didn't change) and writes only rows that differ: a streaming reply rewrites one row, not the
thread. Reads use a second connection (WAL), so they never wait for a write.

The app saves a transcript only while it's loaded; the state is saved 300 ms after the last
change, and at once at quit (`AppModel.saveNow`, then `Store.flush`).

## The change feed

Every transcript row write gets a new `rev`. An indexer keeps a bookmark and reads
`changes(after: bookmark)` in batches, then sets its bookmark: it sees new and edited rows,
never rereads history, and picks up where it stopped after a quit. The full-text index is the
first indexer (a few seconds after writes stop, in batches of 500). Memory extraction is meant
to be the next one:

```swift
let database = store.database
let changes = database.changes(after: database.bookmark("memory"))
// … read them, database.saveMemory(…) what's worth keeping …
if let last = changes.last { database.setBookmark("memory", rev: last.rev) }
```

`search(_:projectID:limit:)` searches every transcript, best match first; `findMemories`
searches memories (any word, the last as a prefix).

## Upgrading from the JSON files

Versions up to 0.0.6 kept `state.json` and `threads/<id>.json`. On first launch the state is
moved in before the app reads it; each transcript is moved in the first time its thread opens
(or is saved), and the rest in the background a few seconds after launch. An entry this version
can't read costs that entry, not the thread. The files are left as they were, so an older
version still opens. `state.json` is read only until it's been moved in once, which also means
a development run can be seeded by writing a `state.json` into an empty data folder (presets do).

A row this version can't decode (a newer version's thread) is kept and saved back untouched; a
setting it can't read costs that setting.

If the database can't be opened (damaged), it's moved aside as `harness.sqlite.unreadable-<time>`
and the app starts again from the JSON files.

## Export and import

File → Export State… (or Settings → General → Data) writes everything the app keeps to one
`.yhstate` file, a zip (`StateArchive` in `Sources/HarnessCore/Storage/`, the app's
`Model/StateTransfer.swift`):

| In the file | What it is |
|---|---|
| `manifest.json` | `format: "yourharness-state"` and `formatVersion` (1), when it was exported, the app (name, bundle ID, version, build, commit), the Mac (macOS, model, chip, memory, locale, time zone), every CLI and tool with its path and version or why it didn't run, the database's schema version, the data folder it came from, and what's inside (project names, counts, size). |
| `data/` | The app's items in its data folder (`StateArchive.items`): `harness.sqlite` and `usage.sqlite` as snapshots (`VACUUM INTO`, after everything waiting is written, so they're consistent and have no WAL), `attachments/`, `memory-imports/`, and the old `state.json` and `threads/` when they're there. |
| `defaults.plist` | The app's macOS defaults (window sizes and places, split views, the Settings tab), without `lastRunVersion`. |
| `browser-cookies.json` | Each project's browser cookies, by project ID, only when you ticked "Include browser sign-ins". Cookies only, not the browser's other storage. |

Import State… unpacks and checks the file (it's a state, its format is one this version
reads, its database passes `PRAGMA quick_check`), then asks: what's in the file next to what
you have, the projects it holds, threads that are working (they're stopped), and a newer
schema if it's from a newer app. Saying yes saves a copy of the current state into
`backups/` in the data folder first, with sign-ins (untick to skip), moves the unpacked file
into `pending-import/` and restarts. At launch, before the database opens,
`StateArchive.applyPending` moves the app's items (and the old WAL) aside, writes a marker,
moves the file's in, and the app applies the defaults; after the window is up, each of the
file's projects gets exactly the file's cookies (when it has them; otherwise the browser is
left as it is), and `pending-import/` goes. Each step can run again after a crash. Nothing
else in the data folder is touched: not `worktrees/` (checkouts with your work in them), not
`backups/`, not a development run's project folders and logs. A Home folder that doesn't
exist on this Mac goes back to your home folder.

When the app starts keeping something new in its data folder, add it to `StateArchive.items`.
When the layout changes so an older app can't read it, raise `StateArchive.formatVersion`: an
older app refuses a newer file instead of importing part of it. A newer database schema
alone doesn't need that (a newer database opens).

To look inside one, `ditto -x -k <file> <folder>` (or `unzip`): `data/` is a data folder a Dev
copy can run on. To reproduce a bug with someone's state, see CLAUDE.md (`YH_STATE_IMPORT`).

## Changing the schema

Add a step to `Schema.steps` (never edit an old one): each step runs once, in order, in a
transaction, and `PRAGMA user_version` records how far a database has got. A database from a
newer version (higher `user_version`) still opens; what it added is left alone.

## Checking it

- `swift test --filter "HarnessDatabaseTests|StoreMigrationTests"`: round trips, row-level
  saves, blobs, search, the change feed, memories, moving the old files in, a damaged database.
- `swift test --filter StateArchiveTests`: export then import over another state brings back
  exactly what was exported and leaves worktrees and logs alone; a swap cut off partway
  finishes; files that aren't states, or are from a newer format, are refused.
- `YH_MIGRATION_SOURCE=<data folder> swift test --filter RealDataMigration` moves a copy of a
  real data folder in and checks every thread and row came through.
- `scripts/preset.sh legacy-0.0.6` (or `legacy-big`) then `scripts/dev-run.sh <folder>
  YH_SEARCH_PROBE=<words>`: a Dev copy upgrades it, and the probe prints how long the move and
  the index took and what a search finds. `sqlite3 <folder>/harness.sqlite` to look inside.
