# Files

The right sidebar's Files tab shows a thread's folder (its worktree when it has one, the
project otherwise) and the files opened from it, read-only. Paths in the chat link to it.
The Home project has none: a home folder isn't a project, and its Desktop, Documents and
Downloads would each ask for access.

## Where the code is

| Piece | File |
|---|---|
| Finding paths in text, deciding which are the folder's files | `HarnessCore/Files/FileLinker.swift` (`FileLinker`, `FileReference`, `FileLink`) |
| Reading a file: text, line starts, binary, picture, too big | `HarnessCore/Files/FileReader.swift` |
| A thread's tabs, preview tab, open folders, what's saved | `Model/FilesModel.swift`, `SavedFiles` in `Model/Store.swift` (thread JSON, `files`) |
| The tab: header, Go to File, tabs bar, the tree and file split | `Views/FilesPanel.swift` |
| The tree (`NSOutlineView`) and the folder watcher (FSEvents) | `Views/FileTreeView.swift` |
| The viewer (`NSTextView`, line numbers, the file watcher) | `Views/FileViewer.swift` |
| Links in replies and your messages | `InlineMarkdown` in `Views/MarkdownView.swift`, `UserMessageView` |
| Links on Read, Edit and Write steps | `ToolCallView.fileTarget` |

## Links

`AppModel.loadFileLinker(for:)` lists the thread's folder once, off the main thread
(`FileIndex.list`, as `@` mentions do: `git ls-files` there and in each repo found inside it,
so a workspace's clones count though its shell repo ignores them; a walk of what's outside any
repo in a plain folder; at most `FileIndex.limit` paths), and builds a `FileLinker`: a set of the paths and a map from each
name to the paths with it. It's listed again after each turn, and replaced only when the files
changed (`revision`, a hash of them), since text linked against it is built again then.

Rendering never touches the disk. A paragraph is linked only if it has a `/` or a `.`, then
one regular expression finds runs of path characters with an optional line (`:12`,
`:12-20`, `:12:5`, `#L12`, `#L12-L20`), and each is looked up: the exact path, an absolute
path inside the folder, then the end of a path or a bare name when exactly one file ends that
way. Paths outside the folder, `../`, URLs and ambiguous names aren't linked. Code spans are
linked too (it's how Claude names files), other links and URLs are left alone. The link is
`yh-file:///abs/path?line=12&end=20`; the thread's `openURL` opens it in the Files tab
(`FileLinkAction`), ⌘-click in the file's own app.

`FileLinker` goes through SwiftUI's environment, which compares values on every update. Its
tables are behind a class and it compares by revision, so that's O(1); comparing the sets
made switching threads slower. For the same reason the side chat's link handler is set on the
side chat panel, not on the whole right sidebar, which updates on every switch even hidden.

## Performance

- **Nothing big on the main thread.** A file is read (memory-mapped), decoded, split into
  lines and highlighted in a detached task into an `NSTextStorage`, which the viewer swaps in
  whole (`replaceTextStorage`). Folders are listed off the main thread too.
- **Only what's seen exists.** The text view lays out non-contiguously (only what's on
  screen); the line numbers are drawn only for visible lines, by binary search in the line
  starts. The tree reads a folder when it's opened, never the whole tree. Of the tabs, only
  the last four seen hold their text (32 MB at most, `RecentFiles`), so going between them
  shows the file in the same frame and only checks its date, off the main thread; the others
  are a path and, while the app runs, a scroll position. That text goes when you switch
  threads (the sidebar's panels are made per thread).
- **Limits.** Text over 8 MB shows its first 8 MB (cut at a line). A line over 5,000 characters
  is cut, with a note above the text: AppKit lays out a line whole, and a minified file's 2 MB
  line held the main thread for a second. Highlighting stops at 1 MB or 20,000 lines. A NUL in the first 8 KB is a binary file; pictures are decoded at up
  to 3,000 px. Tabs past 24 close oldest first.
- **Watching.** The open file is watched with a dispatch source on its descriptor; a change
  reloads it after 120 ms and keeps your place (a save that replaces the file is a change, and
  the new file is watched after). The tree has one FSEvents stream on the folder (0.3 s) and
  reads again only folders it has read; `.git` is ignored.

Measured with the `files` preset (`YH_FILES_PROBE=5`): a 7.4 MB, 100,000-line file shows in
about 60 ms from the click, with the main thread held at most about 20 ms; files after the
first show in 15–40 ms; scrolling through it holds the main thread under 20 ms a step. The
first file in a thread costs about 50 ms more (making the tab and the viewer), in line with
the first show of any sidebar tab.

A tab selects on the click itself, by the event's click count: a double-tap gesture beside a
tap gesture makes SwiftUI hold every single click until a double click can't come any more.
Synthetic clicks don't reach those gestures, so the probes can't see this; check by hand.

Real input (`YH_FILES_PROBE_INPUT=1`), main thread per step: the wheel over that file 7 ms
(worst about 22), over a 3,000-row tree 4 ms, the divider 5 ms, a key in Go to File about 20 ms.
The file's cost is AppKit's: about a quarter of it is `usedRectForTextContainer`, but answering
that from the line count didn't help (7.4 to 7.0 ms) and got the height wrong, so it's left to
AppKit. Resizing the window costs about 95 ms a step whichever tab shows (about 80 with the
sidebar closed), in SwiftUI's layout of the whole window.

## Checking it

- `swift test --filter "FileLinkerTests|FileReaderTests|FilesModelTests"`.
- `scripts/preset.sh files-workspace` for a workspace: paths in both clones link, and their
  folders aren't dimmed.
- `scripts/preset.sh files`, then `scripts/dev-run.sh test-harness-root/files YH_FILES_PROBE=5`
  (PASS: every stall under 50 ms) and `YH_OPEN_FILE=src/checkout/cart.ts:10-14` with
  `YH_SNAPSHOT` to look at it.
- After changes to links or anything the transcript or the right sidebar reads from the
  environment: the switch probe on a big state whose projects have files (`legacy-big`, with
  its project folders filled), against the build before the change.
- After changes to the tab's layout: `YH_NAV_PROBE` (it shows the Files tab, wide and narrow).
