# Roadmap

Where YourHarness is heading, in priority order. Performance, native polish and day-to-day convenience apply to every item.

## Next

### 1. Workspaces out of the box
Workspace support built in, like T3 Code, where you can follow PRs and related work, but across a whole workspace rather than one repo at a time. Design: [docs/workspaces.md](docs/workspaces.md).

### Browser for agents
A real browser per thread that the agent drives and you watch and take over: run the app, click through it, read the console and network, screenshot what it found. Native WebKit, tools served in-process over the CLI's control channel. Design: [docs/browser.md](docs/browser.md).

### 2. Memory
Memory as a first-class feature: a local store, shared with every agent session, that you can visualize and review automatically.

### 3. Scheduled tasks
Run agents on a schedule: recurring jobs, one-off tasks at a set time, with results waiting in a thread.

### 4. Built-in reports
First-class support for reports. When I ask for an HTML report on something, the harness renders, stores and shares it natively instead of leaving a file somewhere on disk.

### 5. Cloud agents with my permissions
A simple way to run agents in the cloud using the permissions I already have locally. The exact shape is still open.

### 6. One chat for everything
A single chat that can reach across all projects and threads, instead of starting from the right folder every time.

## Later

### Small follow-ups
- Undo for thread actions: a five-second Undo after settling, unpinning or deleting a thread (and ⌘Z), as in T3 Code.
- Settle on merged PRs: comes with thread–PR linking (workspaces phase 2).

### Start agents from mobile
Kick off or check on agents from a phone. Deprioritized for now.
