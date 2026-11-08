# Presets

Scratch data folders for running a development build of the app against, so a test run
starts from a known state and never touches your real one. Make one with `scripts/preset.sh`,
then launch the app on it with `YH_DATA_DIR` (in the background, as CLAUDE.md says), adding
any other `YH_*` variables with more `--env`s:

```sh
scripts/bundle.sh
scripts/preset.sh legacy-0.0.6                      # → test-harness-root/legacy-0.0.6
open -g -n --env YH_BACKGROUND=1 --env YH_DATA_DIR="$PWD/test-harness-root/legacy-0.0.6" \
    --env YH_PATH="$PATH" --env YH_SEARCH_PROBE=timezone --stderr dev.log "build/YourHarness Next.app"
```

Paths inside a preset are written `{{ROOT}}`, the folder it's made in. Each made folder
has a `home` folder, the Home project's, so `claude` never runs in your real home folder.

| Preset | What it is |
|---|---|
| `legacy-0.0.6` | Data as version 0.0.6 saved it (`state.json`, `threads/*.json`), before the database: four threads with tool calls, a big result, a sub-agent, a plan, a settled thread, an attachment. Opening it tests the upgrade. |
| `legacy-big` | Generated: hundreds of threads in the 0.0.6 format, a few with thousands of rows and big tool results (`scripts/preset.sh legacy-big <dir> 0.2` for a fifth of the size). For timing the upgrade, the index and thread switching at scale. |
| `usage` | Generated: a year of Claude Code history in `claude-config/` (sessions, sub-agents' files, forks, three eras of models) and a limits reading, for the usage view: run on `test-harness-root/usage` with `CLAUDE_CONFIG_DIR=$PWD/test-harness-root/usage/claude-config` and `YH_OPEN_USAGE=1`. `scripts/preset.sh usage <dir> 4` makes four times as much (1.4 GB) for timing. |
| `sandbox-prs` | A project on a clone of [yourharness-test-prs](https://github.com/mdovhopo/yourharness-test-prs), where a run may push branches and open PRs. New threads use Haiku. |
| `files` | A project (`shop`, a git repo) with code, a 100,000-line JSON file, a 2 MB one-line minified file, a folder of 3,000 files, a picture, a binary and ignored folders, and a thread whose reply mentions paths every way it can (and some that mustn't link), for the Files tab: run on `test-harness-root/files` with `YH_OPEN_FILE=src/checkout/cart.ts:10-14`, and `YH_FILES_PROBE=1` to time it. |
| `files-workspace` | A workspace (`platform`, a shell repo) holding two gitignored clones, `api` and `web`, and a thread with paths in each, for the Files tab across repos. |
| `memory` | A project (`acme`) for trying memory: Claude Code-format memory files in `sources/claude-memory` (import them directly) and a ChatGPT memory export in `sources/chatgpt-memory.txt` (for the Importer). New threads use Haiku. |
| `extreme` | Generated: 20 huge threads (4 of about 32,000 rows and 30 compactions, 16 of half that, about 2 GB), 100 ordinary ones, and a fake `claude` in `fake-claude/` that streams turns (reasoning, hundreds of tools, sub-agents, compactions) with no tokens. For `scripts/perf-run.sh <results>`, which records FPS, CPU and memory per phase and click times while up to 20 run in parallel. |

The made folders live in `test-harness-root/`, which isn't tracked.
