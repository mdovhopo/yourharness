# Workspaces

Status: agreed design. Phase 1 (read-only workspace and repo awareness) is implemented; the rest is not. Roadmap item 1.

## What a workspace is

A workspace is the base for day-to-day work on a company or project. It's usually a git repo (the *shell*) holding the shared AI setup: `CLAUDE.md`/`AGENTS.md`, skills, agents, rules, docs on how things work, and scripts that clone the real repos. The real repos live inside it, gitignored. The shell contains no product code. It's an empty frame, ready to use, which makes it a natural way to onboard people and share AI setup.

```
acme-workspace/            ← shell repo
  CLAUDE.md, .claude/      ← shared AI setup
  scripts/clone.sh
  repos/                   ← gitignored
    api/   (own .git, own CLAUDE.md)
    web/   (own .git)
```

The nesting is what makes it work: Claude Code loads `CLAUDE.md` from the working directory and every parent, and nested ones on demand. **Every layout the harness creates keeps sub-repos at the same relative path under a shell checkout.**

Workspaces come in three levels, and the harness supports all of them:

| Level | Shape |
|---|---|
| Folder | Any directory with 2 or more git repos at any depth (group folders allowed) |
| Shell | That directory is itself a git repo that ignores its repos |
| Declared | The shell also lists what *can* be cloned: a manifest, or compose `build.context`s |

Opening a folder-level workspace can offer to turn it into a shell repo.

## Principles

- **Discovery first, config optional.** With no config at all, the harness finds repos, PRs, stacks and tickets.
- **Drive official CLIs** (`git`, `gh`, `glab`) and never handle their credentials.
- **Never edit the user's files to make the harness work.** Generated files (compose overrides, worktrees) live in the harness's own storage.
- **Seamless.** Anything missing (no ticket, no PR, no remote) simply isn't rendered. No empty states, no "untracked" buckets.

## Repo discovery

Sources are pluggable and merged:

1. Nested `.git` scan, recursive, skipping `node_modules` and similar.
2. Compose files: `build.context` paths under the workspace.
3. Optional manifest entries.
4. Forge org listing (`gh repo list <org>`, `glab repo list`), for repos you can access.

Each repo is in one of three states:
- **Cloned:** a real `.git`.
- **Known:** referenced by the workspace but not cloned yet. This includes the empty placeholder folders some clone scripts create so compose doesn't fail.
- **Available:** in the org, but not referenced by the workspace.

## The thread footprint

The unit of work is the **thread**. There's no separate task object. Each thread builds up a footprint, discovered rather than declared:

- **Affected repos**, with derived state: dirty → committed → pushed → PR open → merged.
  - Sources: Edit/Write tool paths, Bash working directories, FSEvents during the thread's turns, per-turn snapshots, and commits created during turns.
- **PRs/MRs:**
  - Sources: forge URLs in tool output (e.g. `gh pr create` / `glab mr create`), branches the thread created or checked out, and manual links.
- **Tickets:**
  - Sources: issue keys the user mentions in the thread.
- **Branches and commits:** the detail under each repo, not the headline.

Threads that share a ticket or a PR group together automatically, so a follow-up thread joins earlier work across sessions. Manual linking covers what discovery misses. A thread with no ticket shows no ticket UI.

## Forges

GitHub through `gh`, GitLab through `glab`. One batched GraphQL query per forge covers every repo in the workspace, instead of a request per repo. PR state refreshes on a gentle poll, when a turn finishes, and when the app becomes active.

## Stacks

Stacks are discovered from forge data alone: **a PR or MR whose base branch is another one's head branch is stacked on it.** Following those links rebuilds any stack, however it was made: plain git, Graphite, spr or forge-native stacking.

PR relations form a small graph:
- **Stacked on** (same repo): an ordered chain, shown as a vertical stack with CI and review state for each layer.
- **Ships with** (across repos): sibling PRs from the same thread or ticket, with a suggested merge order.

Actions, built on plain git:
- **New layer:** branch from the top of the stack.
- **Restack:** `git rebase --update-refs` (git 2.38+) moves every branch in the stack at once.
- **Submit:** push every layer, create or retarget PRs/MRs with the right bases, and keep a stack-navigation block in each description.

## Tickets

For now a ticket is just an issue key the user mentioned in the thread, shown as a plain chip. There's no tracker integration.

- **Keys:**
  - Linear and Jira style: `ABC-123`.
  - GitHub style: `#123` and `org/repo#123`.
- **Grouping:** threads that mention the same key group together.
- **Deferred:**
  - Links: learning URL templates from PR bodies.
  - Details (title, status, assignee): Linear first, then Jira. Getting them means either the harness holding its own tracker token in the Keychain, or going through Claude's tracker MCP servers. That decision is deferred along with the feature.

## Change tracking across sessions

- **Per-turn snapshots.** After each turn, for each repo that got dirty, build a commit through a temporary index (`GIT_INDEX_FILE=… git add -A && git write-tree && git commit-tree`). Store it under `refs/yourharness/<thread>/<turn>`. This leaves the user's index, stash and branches alone, includes untracked files, respects `.gitignore` and survives restarts.
- **What it gives you:**
  - Per-thread and per-turn diffs across repos.
  - Rewind per turn across repos. Claude's own checkpoints don't work in `-p` mode and miss changes made through Bash.
  - Aggregate diffs that hold up however many sessions worked on something.
- **One writing thread per checkout at a time.** Read-only threads can run in parallel. Parallel writing is what worktrees are for.
- **Cleanup.** Snapshot refs are garbage-collected when a thread is archived.

## Worktrees

A worktree of the shell has no sub-repos, since they're gitignored, so worktrees are **composite**:

```
<harness storage>/worktrees/acme/<name>/   ← shell worktree
  repos/api/   ← git worktree of acme/repos/api
  repos/web/   ← git worktree of acme/repos/web
```

- **Location: outside the workspace.** Otherwise the main checkout's agent greps duplicates, and the parent-directory `CLAUDE.md` lookup loads the shell's rules twice.
- **Stable paths.** Claude stores sessions per working directory, and `--resume` only finds them there.
- **Which repos to include.** Repos are picked when the worktree is created, and more can be added later. Other repos get a read-only link to the main checkout, and the permission channel and hooks keep writes inside the worktree.
- **Dependencies.** `node_modules` and build caches are copied with APFS clonefile (`cp -c`), which is instant and uses no extra disk until files change. The repo's install step then reconciles them. Python venvs bake in absolute paths, so they get reinstalled instead.
- **Gitignored env files** are copied in, as declared or detected.
- **Lifecycle hooks:** `setup`, `start`, `stop` and `teardown` per repo. The harness runs them with `YH_WORKTREE`, `YH_PORT_BASE` and `COMPOSE_PROJECT_NAME` set.

## Environments (Docker)

Workspaces often use one large compose file with hardcoded host ports, relative bind mounts (`./repos/x:/code`) and shared env files. The harness never edits that file. It generates an override in its own storage and runs `docker compose -f <theirs> -f <ours> -p <project>`. There are two modes:

- **Swap-in (default):** one shared stack. A worktree overrides only the services it changes, pointing their bind mounts at the worktree. Switching the active worktree recreates just those containers, so RAM stays flat.
- **Isolated (opt-in, for small stacks):** a separate compose project with remapped ports through Compose's `!override`/`!reset` tags, and one block of ports per worktree.

Environments start on demand, stop after a period of idleness (like idle `claude` processes), and are torn down on archive. The UI shows running containers and their memory use.

Secrets come from the workspace's own scripts (`make get-secret` or similar). The harness never fetches secrets itself.

## Onboarding

1. Paste a workspace URL, and the harness clones the shell.
2. Discover repos and check access for each (`gh repo view` / `glab repo view`).
3. Run the clone and bootstrap scripts.
4. A health panel: required tools, Docker, and whether `claude` and `gh`/`glab` are signed in.

Improvements to the shell's skills and rules made during work become PRs to the shell. That's also where team-level memory (roadmap item 2) is meant to live.

## Config

Small, optional, in two layers:
- **Shared, in the shell repo** (one tool-neutral file, name to be decided):
  - extra repo sources and ignores
  - lifecycle hooks
  - env files to copy
  - Docker mode
- **Personal, in the harness:** hidden repos, a preferred stacking tool, per-user overrides.

## UI

- **Thread header and inspector:** repo chips with state, ticket chips (when there are tickets), and PR stacks.
- **Workspace overview, "what's being worked on":** active threads with affected repos, PRs and tickets. Can be grouped by ticket or by repo.
- **Repo view:** which threads touch each repo right now. This is the conflict radar.

## Phases

1. **Workspace and repo awareness:** detection at all three levels, repo states, branch and dirty and ahead/behind, read-only PR/MR visibility on GitHub and GitLab, stack discovery.
2. **Thread footprint:** affected repos, PR linking from tool output, mentioned ticket keys, per-turn snapshots, rewind.
3. **Composite worktrees:** lifecycle hooks, clonefile dependencies, env files. Stack actions (new layer, restack, submit).
4. **Environments:** generated compose overrides, swap-in and isolated modes, idle shutdown.
5. **Onboarding from a URL,** PRs back to the shell.

Later: tracker integration (Linear, then Jira).

## Open questions

- The name and format of the shared config file.
- How strictly to enforce "writes stay inside the worktree": block, or warn.
- Merge-order hints for cross-repo PRs: inferred from compose `depends_on`, or declared.
