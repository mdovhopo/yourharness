# Memory

Each project (workspace) has its own memory, kept by the app in its database, not in files
an agent writes. Every thread in the project starts with it, agents use it through tools the
app serves, and you can see and change all of it in the Memory view.

The code: `Sources/HarnessCore/Memory/` (no UI imports, all tested in `MemoryTests`) and the
app's `Model/AppModel+Memory.swift` and `Views/Memory*.swift`.

## Tasks

Your asks are memories too, of kind `task`: a title, a state (to do, in progress, blocked, done),
steps with the done ones ticked, and a dated log of what happened, with every thread that worked
on it. Agents keep them with `mcp__memory__track`: before starting multi-step work (or work that
goes on later), after each step, when blocked (and why), and when done. Every session's index
starts with the open tasks and their next step, and the ones done in the last week, so "carry on
with dark mode" lands where the last thread stopped. A step ticked or a note added is logged as
progress (`progressed`), not a rewrite; every change is a version, so restoring one brings its
progress back. The Memory view's **Tasks** tab shows them by state with a progress bar and next
step; you can tick steps, add them or change the state in a task's detail; a thread's toolbar
shows the tasks it's working on (state and steps done), with a popover of their steps.

## What a memory is

One fact, preference, decision or pointer: a title, a one-line description (what the index
shows), a body, and a kind, after Claude Code's own: `user` (who you are), `feedback` (how you
want agents to work), `project` (work, goals and decisions the code doesn't record) and
`reference` (where to look things up). It can be pinned (always given in full), linked to other
memories, archived (kept, no longer given to agents) or merged into another. It remembers where
it was learned (the thread and tool call), every version, how often agents read it and how often
it came up in their searches. Scope is `project:<project id>`; a removed project's ID is kept
by folder, so adding the folder again brings its memory back.

## How agents get it

- **The index**, appended to the system prompt at every launch (`MemoryPrompt`): how to use
  memory, then one line a memory (`- 1a2b3c4d Title — description`), grouped by kind, most read
  and most recent first, under 8 KB (about 2,000 tokens); pinned ones in full, under 4 KB more.
  What doesn't fit is a search away, and the prompt says so.
- **Tools**, `mcp__memory__*` (`MemoryToolServer`), served inside the app over the CLI's control
  channel like the browser's, and pre-approved since they only touch the app's database:
  `search`, `read`, `save`, `update`, `forget` (archives), `link`. The side chat gets `search`
  and `read`. IDs are the first eight characters; any unique prefix works. `save` refuses a
  title the workspace already has, kinds it doesn't know, memories over 6,000 characters, and
  anything that looks like a key or token, and names similar memories so the agent can merge.
- **One memory, not two**: with Settings → General → Memory → "Turn off Claude Code's own memory
  files" (on by default), sessions run with `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`. Bring what
  Claude Code had remembered in with Import.

## What's recorded

Every save, read, search (what was asked, what it found), session start (how many memories, how
many bytes), archive, merge, link, pin, import and export is a row in `memory_events`, with who
did it (`agent`, `user`, `importer`, `reconciler`) and in which thread. That's what the Memory
view's Insights and Activity are made of (`MemoryStats`): size and token cost per session, the
share of memories read in the last 30 days, the search hit rate and the searches that found
nothing, memories never read, likely duplicates (titles sharing most words), memories with no
links, growth and activity by day, kinds and ages. The log keeps a year.

## The Memory view

Click a project's name in the sidebar: its page opens on its pull requests, with **Pull Requests |
Memory** in the middle of the toolbar (Home has no workspace, so its page is its memory). ⇧⌘M,
the brain button on a project's row and its menu go straight to Memory. Memory's own tabs sit in
its header: Memories, Tasks, Insights, Activity, Graph.
- **Memories**: search, filter by kind, in use or archived; edit title, description, kind and
  body (Markdown), pin, archive, delete, link; see how agents use it, where it was learned
  (click to open the thread at that point) and its history, with Restore This Version.
- **Insights**: the figures above, in plain sentences first ("11 memories have never been read
  in over a week. Each still costs a line in every session."), then charts and lists.
- **Activity**: everything that happened, as sentences, by day, each linked to its thread.
- **Graph**: memories as dots (by kind, bigger the more they're read) and links as lines.

## Import and export

- **Export** (JSON or a Markdown folder). JSON brings everything back. The Markdown folder is
  Claude Code's memory format (a file a memory with `name`/`description`/`metadata.type`
  frontmatter, `[[links]]`, a `MEMORY.md` index), readable by people and other tools.
- **Import File…** takes either back without an agent, and so does **Import Claude Code's
  Memory** for the project's folder (`~/.claude/projects/<folder>/memory`, offered when it has
  any). A memory the workspace already has (same ID or title) is updated only if the incoming
  one is newer; IDs another workspace uses are replaced, links following.
- Anything else goes to **the Importer**.

## The specialists

Pick their model and reasoning effort in their window (before starting, or in the toolbar; a
change applies from the next message), or the default for both in Settings → General → Memory:
memory is worth a strong model. Two agents with their own windows, one conversation per project each, that outlive the window
until New Session. They run Claude Code with only Read, Glob, Grep, AskUserQuestion and the full
memory tools (also `list`, `stats`, `merge`, `unlink`, `restore`, `pin`), no MCP servers of
yours, nothing saved to Claude Code's history (`MemoryPrompt.specialist` has their instructions).
A column on the right shows what the session has changed so far; each change opens the memory.
- **The Importer** reads what you give it (Claude Code's memory, instruction files it finds in
  the project, files and folders you pick, pasted text) with those folders added to the session
  (`--add-dir`), splits it into atomic memories, sorts them into kinds, skips secrets, one-offs
  and what the workspace already has, and reports what it did and what it wasn't sure about.
- **The Reconciler** starts from `stats` and `list`, then proposes merges, fixes, archives,
  tighter wording and links, and asks before applying (unless told not to). Every change keeps
  the old version.

## ⌘K

Search Everything (⌘K, or Search at the top of the sidebar) is a Spotlight-style panel over the
window: thread titles and actions match as you type; then the full-text indexes add memories and
tasks from every project and what was said in threads (one hit a thread, what was said before
tool input, none for a thread already found by title). ↑↓ and ↩ to go; a conversation hit opens
the thread with Find on the matched word. With nothing typed it lists recent threads and actions
(New Thread, Open Memory, Open Tasks, Import, Reconcile, Add Project, Resume, Settings).

## Checking it

- `swift test --filter MemoryTests`: the schema upgrade, versions, links, merges, counts, the
  index budget, every tool, both file formats, Claude Code's folder names.
- `scripts/preset.sh memory <dir>` makes a project with Claude Code-format memory files and a
  ChatGPT export to import. Make it outside this repo (the scratchpad, `/tmp`): a project inside
  it reads the repo's CLAUDE.md too. Then `scripts/dev-run.sh <dir>` with:
  - `YH_SEND_MESSAGE="Remember that …"`: a real turn that should save; a second launch with a
    question should read it (`sqlite3 <dir>/harness.sqlite "select * from memory_events"`).
  - `YH_MEMORY_IMPORT=<dir>/sources/claude-memory`: a direct import.
  - `YH_MEMORY_AGENT=importer YH_MEMORY_AGENT_START=1 YH_MEMORY_IMPORT_SOURCES=<file> YH_MEMORY_AGENT_MODEL=haiku`,
    or `YH_MEMORY_AGENT=reconciler YH_MEMORY_AGENT_START=1 YH_MEMORY_AGENT_NOTE="apply directly"`.
  - `YH_SEND_MESSAGE="Plan X in steps, do only the first"`: the agent should `track` a task;
    a second launch with "continue X" should read it and move it along.
  - `YH_SPOTLIGHT=<words>` opens ⌘K with them typed.
  - `YH_OPEN_MEMORY=1 YH_MEMORY_TAB=insights` (or `tasks`), `YH_MEMORY_SEED=2000` (made-up memories and
    activity), `YH_MEMORY_EXPORT=<folder>`, with `YH_SNAPSHOT` (and `YH_SNAPSHOT_WINDOW=Memory
    Importer` for a specialist's window).
