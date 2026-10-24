# Presets

Scratch data folders for running a Dev copy of the app against, so a test run starts from a
known state and never touches your real one. Make one with `scripts/preset.sh`, run it with
`scripts/dev-run.sh`:

```sh
scripts/bundle.sh
scripts/preset.sh legacy-0.0.6                      # → test-harness-root/legacy-0.0.6
scripts/dev-run.sh test-harness-root/legacy-0.0.6 YH_SEARCH_PROBE=timezone
```

Paths inside a preset are written `{{ROOT}}`, the folder it's made in. Each made folder
has a `home` folder, the Home project's, so `claude` never runs in your real home folder.

| Preset | What it is |
|---|---|
| `legacy-0.0.6` | Data as version 0.0.6 saved it (`state.json`, `threads/*.json`), before the database: four threads with tool calls, a big result, a sub-agent, a plan, a settled thread, an attachment. Opening it tests the upgrade. |
| `legacy-big` | Generated: hundreds of threads in the 0.0.6 format, a few with thousands of rows and big tool results (`scripts/preset.sh legacy-big <dir> 0.2` for a fifth of the size). For timing the upgrade, the index and thread switching at scale. |
| `sandbox-prs` | A project on a clone of [yourharness-test-prs](https://github.com/mdovhopo/yourharness-test-prs), where a run may push branches and open PRs. New threads use Haiku. |

The made folders live in `test-harness-root/`, which git ignores.
| `memory` | A project (`acme`) for trying memory: Claude Code-format memory files in `sources/claude-memory` (import them directly) and a ChatGPT memory export in `sources/chatgpt-memory.txt` (for the Importer). New threads use Haiku. |
