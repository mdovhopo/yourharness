# Automations

An automation is a prompt a project runs by itself: once at a time, or on a schedule. Each run
starts a new thread in the project with the prompt (titled with the automation's name, its
record marked `automationID`), or, for one set to "same thread", sends the prompt to one thread
every time (made by its first run; queued behind a turn that's going). Runs work in the
project's permission mode, so in Ask mode a run waits on its cards like any thread.

## Schedules

`yh-core/src/automations.rs` holds the model, the next-run math (in local time, through
`localtime_r`/`mktime`, so daylight saving and month ends come out right) and the parser that
both the chat and the page use:

| Kind | Written as |
|---|---|
| Once | `in 30m`, `tomorrow 9:00`, `monday 9am` (the next one), `2026-12-01 14:30`, `dec 3 8:00`, `14:30` (today, or tomorrow when it's passed) |
| Every | `every 15m`, `every 2h`, `hourly`: the first run one period on, then every period |
| Daily | `daily 9:00`, `weekdays 8:30`, `weekends 10am`, `mon,thu 18:00`, `mondays 9:00`, `every friday at 5pm` |
| Cron | `cron 0 9 * * 1-5` (five fields, lists, ranges, steps, names; day of month or weekday as cron has it) |

`Schedule::describe` says it back ("Weekdays at 9:00", "Every 2 hours", "Once, tomorrow at 9:00").

## Running

The app's timer (`yh-app/src/state/automations.rs`) wakes when the next run is due (and every
20 seconds at least, for ones made meanwhile), first 8 seconds after launch, and runs what's due. Each run moves the schedule on from now, so however many
runs were missed while the app was closed or the Mac slept, one runs (marked late), then the
schedule carries on. A one-time automation is done after its run. Run Now runs one without
touching the schedule. A run is skipped (and says why in its history) when its project is gone
or 8 threads are already working there. Each automation keeps its last 20 runs.

The timer doesn't run in perf runs or with `YH_AUTOMATIONS=0`. Only one copy of the app may use
a data folder at a time, so nothing runs twice.

## Storage

All automations are one setting, `automations`: a JSON array of records (`id`, `projectId`,
`name`, `prompt`, `schedule` tagged by `kind`, `sameThread`, `threadId`, `model`, `effort`,
`enabled`, `createdAt`, `createdBy`, `nextRun`, `runs`). No schema step, so 0.0.x still opens
the database; a record this version can't read is kept and saved back untouched. Export State
carries them with the other settings.

## From the chat

The `automations` tool server (`yh-core/src/automation_tools.rs`) gives every thread
`list_automations`, `create_automation`, `update_automation` (and pause and resume with
`enabled`), `run_automation` and `delete_automation`, scoped to its own project. `thread`
picks where runs go: `new`, `same`, or `this` (the calling thread every time). The agent is
told to make one only when asked, to write the prompt so it stands alone, and to say back the
schedule and the next run. Settings → Automations (`agentAutomations`) turns the tools off.

## On the page

The project page lists them under Automations (`yh-app/src/ui/automations.rs`): name,
schedule, prompt, next and last run (the last run's thread opens on a click), Run Now, a switch
to pause, and Edit and Delete. New Automation and Edit open a sheet that reads the schedule as
it's typed and says when it first runs. `YH_AUTOMATION_FORM=new|edit` opens it for snapshots.
