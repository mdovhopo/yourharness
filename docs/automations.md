# Automations

An automation is a prompt YourHarness runs by itself — **per workspace** or **globally** (in Home,
reaching every workspace) — on a schedule, in reaction to a **Mac system event**, **file/folder**
or **git/PR event**, or on demand from the UI, Spotlight (`⌘K`), or the command line (`yh trigger`).

## Where they live

- **Automations page (`⇧⌘A`)**: top-level entry in the sidebar and `View → Automations`
  (`yh-app/src/ui/automations.rs`). Master-detail view grouped by Global and workspace, with
  scope filter, search, **Pause All** kill switch, starter templates, full configuration summary,
  and run history (status, trigger reason, duration, and clickable thread links).
- **Project page**: compact workspace summary linking directly into the Automations page filtered
  to that workspace.
- **Sidebar badges**: threads started by an automation carry a subtle bolt icon.

## Scopes & Thread Policies

- **Scope**:
  - **Workspace**: runs in that project's folder with its git repos, memory, and default mode.
  - **Global**: runs in Home (reaches all workspaces' threads via `mcp__threads`, with an optional
    working directory override).
- **Threads**:
  - **New thread each run**: starts a fresh thread; previous threads stay live.
  - **New thread, settle previous**: starts a fresh thread and settles the previous run's thread
    automatically (when idle, unpinned, and not waiting for you; marks `auto_settle` if still
    running).
  - **Same thread each run**: sends to one persistent thread (created on first run, or `"this"`
    from chat), unsettling it if needed and queuing behind an active turn.
- **Agent & Permissions**: per-automation overrides for Agent (Claude Code / Antigravity), Model,
  Effort, and Permission Mode (`Ask`, `Accept Edits`, `Plan`, `Bypass`, applied per-thread without
  changing the project's mode).

## Triggers

### Schedules (`yh-core/src/automations.rs`)

Next-run math runs in local time through `localtime_r`/`mktime` (handling DST and month ends):

| Kind | Written as |
|---|---|
| Once | `in 30m`, `tomorrow 9:00`, `monday 9am` (the next one), `2026-12-01 14:30`, `dec 3 8:00`, `14:30` |
| Every | `every 15m`, `every 2h`, `hourly`: the first run one period on, then every period |
| Daily | `daily 9:00`, `weekdays 8:30`, `weekends 10am`, `mon,thu 18:00`, `mondays 9:00`, `every friday at 5pm` |
| Cron | `cron 0 9 * * 1-5` (five fields, lists, ranges, steps, names) |

### Mac System, File, Git/PR & CLI Events (`yh-app/src/state/event_triggers.rs`)

Event listeners mount **lazily** (zero background observers when no enabled automation uses them)
and trigger **zero macOS privacy prompts**:

- **Mac System** (`platform.rs::MacEventMonitor`):
  - `MacWake` (`NSWorkspaceDidWakeNotification`)
  - `ScreenUnlock` (`com.apple.screenIsUnlocked`)
  - `AppLaunched` / `AppQuit` (`NSWorkspace` app notifications, optional app name / bundle ID filter)
  - `PowerChanged` (`com.apple.system.powermanagement.poweradapter`, optional `ac` / `battery` filter)
  - `NetworkOnline` (interface online transition)
  - `AppLaunch` (once after YourHarness startup settles)
- **File & Folder Watch** (`FolderWatch` / FSEvents): watches a workspace-relative or user-chosen
  folder with optional glob filter (`*.pdf, docs/**/*.md`), default noise ignores (`.git`,
  `node_modules`, `target`), and burst debounce.
- **Git & Forge Events**: watches `.git/HEAD` & `.git/logs/HEAD` for `BranchChanged` /
  `CommitMade`, and diffs existing `WorkspaceScan` snapshots for `PrReviewRequested`,
  `PrChecksFailed`, and `PrMerged` (baseline seeded on first scan so launch never floods old PRs).
- **Manual / CLI**: `yh trigger <name|slug|id> [--data TEXT] [-f FILE]... [--wait | --follow]`
  over `yh.sock` (great for Apple Shortcuts, Raycast, and git hooks).

Event details are injected into the prompt via `{{event.reason}}`, `{{event.files}}`,
`{{event.app}}`, `{{event.branch}}`, `{{event.payload}}` (or appended automatically in an
`<automation_event>` block when no `{{event.*}}` placeholder is present).

## Guardrails & Running

The runtime (`yh-app/src/state/automations.rs`) enforces per-automation guardrails on automatic
triggers:
- **Global Pause All** (`automationsPaused` setting)
- **Cooldown** (minimum seconds between automatic runs) & **Debounce** (for file bursts)
- **Active hours** (local time window & optional weekdays-only)
- **Overlap policy** (`Skip` if still running, `Queue`, or `Allow` parallel up to 8 running/project)
- **AC power only** (via IOKit `IOPSGetProvidingPowerSourceType`)
- **Shell pre-check command** (run off the main thread; non-zero exit skips invoking the AI agent)
- **Notification policy** (`Always`, `NeedsAttention`, `Silent`)

Schedules catch up once after sleep or app close (marked late). The timer and event watchers don't
run in perf runs or with `YH_AUTOMATIONS=0`. Each automation keeps its last 50 runs.

## Storage

Stored in `settings["automations"]` as a JSON array with full backwards compatibility with v1
records (0.4.0) and 0.0.x databases; unknown entries are preserved untouched. Export State carries
them with the other settings.

## From the chat

The `automations` tool server (`yh-core/src/automation_tools.rs`) gives threads
`list_automations`, `create_automation`, `update_automation`, `run_automation`, and
`delete_automation`. Supports `scope` (`project` / `global`; Home threads default to `global` and
can target any project), `thread` (`new`, `new_settle_previous`, `same`, `this`), `trigger` /
`schedule` (including event shorthands like `wake`, `app_quit:Zoom`, `watch:~/Downloads|*.pdf`,
`pr_failed`, `manual`), `agent`, `model`, `effort`, `mode`, `cooldown`, and `condition_cmd`.
Settings → Automations (`agentAutomations`) turns the tools off.

## Testing flags

- `YH_OPEN_AUTOMATIONS=1`: opens the Automations page after `YH_DRAFT_DELAY`.
- `YH_AUTOMATION_FORM=new|edit`: opens the Create/Edit sheet for snapshots.
- `YH_AUTOMATION_TEST_EVENT=wake|unlock|power:ac|power:battery|app_launched:<App>|app_quit:<App>`:
  injects a synthetic Mac system event after launch.
- `YH_AUTOMATION_LOG=1`: logs triggers, guardrails skips, watcher mounts, and runs.
