# Storage

Everything the app keeps is in one SQLite file, `harness.sqlite`, in its data folder
(`~/Library/Application Support/YourHarness`, or `YH_DATA_DIR`). Images stay files beside it
(`attachments/`). The code is in `crates/yh-core`: `db.rs` (the database: one writer thread,
reader connections, blobs, the full-text index), `schema.rs` (its steps), `library.rs`
(projects and thread records) and `legacy_import.rs` (the JSON files of 0.0.6 and earlier).

**The format is the one 0.0.x wrote**, table for table and column for column, and records are
kept as the JSON they were read as and changed field by field. So the user's data carries over
from 0.0.x, and 0.0.16 still opens the database after a downgrade. Keep it that way: add, never
change what's there.

The usage view has a database of its own beside it, `usage.sqlite` (`yh-core/src/usage.rs`,
0.0.x's schema): an index of the usage in Claude Code's session files (which files were read
and how far, and each response's tokens by its message and request IDs), sums by fifteen
minutes kept up to date by triggers, and the tokens of side chats and memory agents, which
leave no session file. Export State takes a snapshot of it too (`VACUUM INTO`), and Import
State replaces it, WAL and all: the session files' part could be read again, side chats' and
memory agents' couldn't.

## Tables

| Table | What's in it |
|---|---|
| `settings` | One row per top-level setting or cache (`appearance`, `defaults`, `slashCommands`, …), its value as JSON. A change writes one row. |
| `projects` | One row per project, in sidebar order: its JSON. |
| `threads` | One row per thread: its metadata as JSON, plus `project_id`, `updated_at` and `session_id` to query by. |
| `entries` | Transcript rows, `(thread_id, seq)` in order, with real columns for what queries need (kind, text, tool name, input, result, dates). A sub-agent's run is JSON in `subagent`, the record 0.0.x wrote (`subagent_record.rs`). On the message that started a turn, `changed_files` (JSON, schema step 4) lists the files the turn changed as git saw them (`turn_changes.rs`: tracked files through a copy of the index, untracked ones by size and time), for the work summary's "edited N files". Every write takes the next `rev`. |
| `transcripts` | Per thread: when its transcript was last saved, and how many rows. |
| `blobs` | Tool inputs, results and sub-agent runs over 32 KB, by SHA-256, LZFSE-compressed (the bytes Apple's libcompression makes), stored once however many rows point at them. A big result also keeps its first 20,000 characters (graphemes) in the row: that's what's loaded, and the whole one is read from its blob when asked for. |
| `entries_fts` | FTS5 index over what was said, tool names and inputs, and the start of tool output. Its rowid is the entry's. |
| `index_state` | How far each indexer has read the change feed (`rev`). |
| `memories`, `memory_sources`, `memories_fts` | Memories (scope `global` or `project:<id>`, kind, title, description, body, status active/proposed/archived, pinned, origin, read and search counts, what it was merged into) and the transcript rows each came from. Searchable, a title match counting most. See [memory.md](memory.md). |
| `memory_links`, `memory_versions`, `memory_events` | Links between memories; every version of each; and the log of what was done with them (saves, reads, searches, session starts, merges…), which the analytics are made of. |
| `legacy_imports` | Which old JSON files have been moved in. |

## Writing

Saving returns at once. Writes go through one background thread, in one transaction per batch;
saves that arrive while one is being written collapse (the newest state wins, and a
transcript's changed rows merge). Only rows that changed are sent, so a streaming reply
rewrites one row, not the thread. Reads use connections of their own (WAL), so they never wait
for a write; a read of a transcript with writes still waiting flushes them first. Everything
waiting is written at quit.

A row keeps every column 0.0.x writes, even the ones this app doesn't use yet, so a row read
and saved again comes back unchanged.

## The change feed and the index

Every transcript row write gets a new `rev`. An indexer keeps a bookmark in `index_state` and
reads the rows changed after it in batches, then moves its bookmark: it sees new and edited
rows, never rereads history, and picks up where it stopped after a quit. The full-text index
(`fts`) is the one indexer: it starts a few seconds after launch and runs while the writer is
idle, 500 rows at a time, so a write waits for one batch at most. A database from an older
version is caught up at open.

## Upgrading from the JSON files

Versions up to 0.0.6 kept `state.json` and `threads/<id>.json`. A folder that never ran 0.0.7+
comes over exactly as 0.0.7 would have brought it: the state when the database opens, once;
transcripts one at a time on a background thread from 3 s after launch, each in a transaction
of its own (a transcript read before its turn comes in first). Files are read as 0.0.x read
them: ISO 8601 dates, an entry that can't be decoded costs that entry, not the thread, and a
file that isn't a JSON array of entries counts as empty and is recorded all the same, so it
never comes back. The files are only read, never written or deleted, so an older version still
opens. `state.json` is read only until it's been moved in once, which also means a development
run can be seeded by writing a `state.json` into an empty data folder (presets do).

A record this version can't decode (a newer version's thread) is kept and saved back
untouched; a setting it can't read costs that setting.

## Export and import

File → Export State… (or Settings → General → Data) writes everything the app keeps to one
`.yhstate` file, in the format 0.0.x wrote, so either reads the other's: a zip, made and
opened with `ditto` as Finder does (`yh-core/src/state_archive.rs`, the app's
`ui/state_transfer.rs`):

| In the file | What it is |
|---|---|
| `manifest.json` | `format: "yourharness-state"` and `formatVersion` (1), when it was exported, the app (name, bundle ID, version, build, commit), the Mac (macOS, model, chip, memory, locale, time zone), `claude` and `git` with their paths and versions, the database's schema version, the data folder it came from, and what's inside (project names, counts, size). |
| `data/` | The app's items in its data folder (`state_archive::ITEMS`): `harness.sqlite` and `usage.sqlite` as snapshots (`VACUUM INTO`, after everything waiting is written, so they're consistent and have no WAL), `attachments/`, `memory-imports/`, and the old `state.json` and `threads/` when they're there. |
| `defaults.plist` | Written by 0.0.x (its macOS defaults: window sizes, split views); this app leaves it alone when reading. |
| `browser-cookies.json` | Each project's browser cookies, by project ID, only when you ticked "Include browser sign-ins", as 0.0.x writes them (dates in seconds since 2001). Cookies only, not the browser's other storage. |

Import State… unpacks and checks the file (it's a state, its format is one this version
reads, its database passes `PRAGMA quick_check`), then asks: what's in the file next to what
you have, the projects it holds, threads that are working (they're stopped), and a newer
schema if it's from a newer app. Saying yes saves a copy of the current state into
`backups/` in the data folder first, with sign-ins (untick to skip), moves the unpacked file
into `pending-import/` (`stage`) and restarts. At launch, before the database opens,
`apply_pending` moves the app's items (and the old WAL) aside, writes a marker and moves the
file's in; after the window is up, each of the file's projects gets exactly the file's cookies
(when it has them; otherwise the browser is left as it is), and `pending-import/` goes. Each
step can run again after a crash. Nothing else in the data folder is touched: not
`worktrees/` (checkouts with your work in them), not `backups/`, not a development run's
project folders and logs.

When the app starts keeping something new in its data folder, add it to `ITEMS`. When the
layout changes so an older app can't read it, raise `FORMAT_VERSION`: an older app refuses a
newer file instead of importing part of it. A newer database schema alone doesn't need that (a
newer database opens).

To look inside one, `ditto -x -k <file> <folder>` (or `unzip`): `data/` is a data folder a
development build can run on. To reproduce a bug with someone's state, see CLAUDE.md
(`YH_STATE_IMPORT`).

## Changing the schema

Append a step to `schema::STEPS` (never edit an old one): each step runs once, in order, and
`PRAGMA user_version` records how far a database has got. The steps are 0.0.x's word for word,
so both open the same database. A database from a newer version (higher `user_version`) still
opens; what it added is left alone.

## Checking it

- `cargo test -p yh-core -- db:: legacy_import:: state_archive::`: round trips, row-level saves,
  blobs and LZFSE, search, the index catching up, moving the old files in (dates, lossy entries,
  the state's split), export then import, and the `extreme` preset read whole when it's there.
- `scripts/preset.sh legacy-0.0.6` (or `legacy-big`), then a development build on it with
  `YH_SEARCH_PROBE=<words>`: it upgrades the folder, and the probe prints how long until the
  index finds them and its first hits. `sqlite3 <folder>/harness.sqlite` to look inside.
