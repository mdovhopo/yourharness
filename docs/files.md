# Files

The right sidebar's Files tab shows a thread's folder (its worktree when it has one, the
project otherwise) and the files opened from it, read-only. Paths in the chat link to it.

## Where the code is

| Piece | File |
|---|---|
| A folder's file list (`git ls-files`, or a walk outside a repo) | `crates/yh-core/src/files.rs` (`files::list`), shared with `@` mentions |
| Finding paths, issue and PR numbers and bare addresses in replies | `crates/yh-core/src/links.rs` (`Linker`) |
| The tab: Go to File, the tree, tabs, the viewer, Find, Wrap Lines | `crates/yh-app/src/ui/inspector.rs` (the Files part of the right sidebar) |
| Opening a link from the chat | `inspector::open_link`, through the window's `FileLinks` |
| Watching the folder (FSEvents) | `crates/yh-app/src/state/file_watch.rs` (`FolderWatch`) |

## Links

When a thread's transcript is made, and again after its turns, its folder's files and its
repo are read off the main thread (`TranscriptView::load_links`: `files::list`, the folder's
`origin`) into a `Linker`: a set of the paths and a map from each name to the paths with it.
It's swapped in only when its `revision` changed (a hash of the files and the repo), since
text linked against it is linked again then.

Drawing never touches the disk. Paths are looked up in the `Linker` alone: the exact path, an
absolute path inside the folder, then the end of a path or a bare name when exactly one file
ends that way, with an optional line (`:12`, `:12-20`, `#L12`, `#L12-L20`). Paths outside the
folder, URLs and ambiguous names aren't linked: a name that could be several files isn't
linked rather than linked to the wrong one. Clicking one opens it in the Files tab at those
lines (`open_link`); a web address opens in your browser.

## The tab

- **The tree** is read a folder at a time as it's opened, as Finder does, off the main thread:
  everything on disk shows, and what git ignores is dimmed rather than hidden.
- **Tabs.** A single click in the tree opens a preview tab (in italics) that the next one
  replaces; a double click, or Keep Open, keeps it.
- **Reading a file** happens off the main thread: over 8 MB, its first 8 MB shows, with a note;
  a line over 5,000 characters is cut, with a note; a NUL in the first 8,000 bytes makes it a
  binary file (its size, and Open in Default App); pictures are drawn from their file. Lines are
  coloured by `markdown::highlight_line`, from what each starts inside (a comment or string from
  lines above), worked out once per read.
- **Only what's seen is made.** The viewer is a list of lines; Wrap Lines makes a list that
  measures each line, the first time it's wrapped.
- **Watching.** Only the open thread's folder is watched (`FolderWatch`, through `notify`):
  events are passed on in batches 250 ms after they stop (an editor's save or a `git checkout`
  is many events), so the main thread hears once per burst. The tree reads again the folders
  that changed, and an open file that changed reloads, keeping its place. Another thread's
  folder catches up when it shows again.

## Checking it

- `cargo test -p yh-core -- files:: links::`.
- `scripts/preset.sh files-workspace` for a workspace: paths in both clones link.
- `scripts/preset.sh files`, then a development build on `test-harness-root/files` with
  `YH_OPEN_FILE=<path[:line[-end]]>` (the 100,000-line catalog, or `src/checkout/cart.ts:10-14`)
  and `YH_FILES_PROBE=1`: the file scrolled through in 40 steps, printing how long it took to
  open and the longest step's lines took to make (PASS under 50 ms). `YH_FILES_LOG=1` prints
  each folder read again and each file reloaded; `YH_FILES_FIND=<words>` types in Find;
  `YH_FILES_WRAP=1` turns Wrap Lines on; `YH_SNAPSHOT` draws it.
