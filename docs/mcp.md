# MCP servers (plan)

Goal: everything `/mcp` does in the terminal, done in the app, so there's never a reason to open Claude Code for it. Today typing `/mcp` in a thread gets Claude Code's headless answer: "1 MCP server(s): 0 connected, … Use `/mcp` in the terminal for details."

## Decided (2026-11-04)

From the mockups (`YH_MCP_DESIGNS=<folder>` draws them, see `Support/MCPDesigns.swift`):
- A **composer chip** that only shows when a server needs you, opening a short list.
- **`/mcp`** (and the chip's Manage…) opens the **sheet**: servers grouped by where they apply, with the action each needs and an on/off switch. A row opens the server's **page** (tools, sign in/out, the error and Retry).
- **Add Server** starts from a URL (sign-in in the default browser, for federated login), with Command or JSON as the other ways in. It asks where the server applies: this workspace just for you (the default), this workspace for everyone (`.mcp.json`), or everywhere.
- **Claude asks you to sign in** with a card in the thread, through a tool the app serves.
- **Settings → MCP** lists the servers that apply everywhere, where each is turned off, and every workspace's own.
- No sidebar tab.

Done: the core (below, step 1), MCP prompts in the `/` menu, and the `/mcp` sheet with each server's page (`MCPServersSheet`, `MCPServersModel`). The sheet asks the thread's session when it has one, otherwise a Claude Code process of its own in the thread's folder, kept while the sheet is open so a sign-in's browser can come back to it. Its on/off switch is Claude Code's, saved for the exact folder: per workspace comes with step 4's scopes.

## What the CLI gives us (probed on 2.1.312, 2026-11-03)

All of it over the stream-json `control_request` channel the session already uses. None of it needs a turn, so none of it costs tokens.

| Request | Fields | Reply |
| --- | --- | --- |
| `mcp_status` | — | `{mcpServers: [{name, status, scope, source, transport, config, serverInfo?, error?, tools?: [{name, annotations: {readOnly?, destructive?}}]}]}` |
| `mcp_toggle` | `serverName`, `enabled` | success; **persists** in `~/.claude.json` → `projects[cwd].disabledMcpServers` |
| `mcp_reconnect` | `serverName` | success, or an error ("Connection failed") |
| `mcp_authenticate` | `serverName`, `redirectUri?` | `{authUrl, requiresUserAction, callbackExpected, callbackPort, state}`; **doesn't open a browser** in SDK mode (`skipBrowserOpen`), the CLI listens on `localhost:<callbackPort>` |
| `mcp_oauth_callback_url` | `serverName`, `callbackUrl` | for pasting the redirect URL by hand when the localhost callback can't be reached |
| `mcp_clear_auth` | `serverName` | revokes and forgets the server's tokens |
| `mcp_set_servers` | `servers: {name: config}` | `{added, removed, errors}`; session-only ("dynamic") servers, not written anywhere |

Statuses seen: `connected`, `failed` (with `error`), `needs-auth`, `disabled`, `pending`/connecting. Scopes seen: `project` (`.mcp.json`), `dynamic` (`--mcp-config` / `mcp_set_servers`), `claudeai` (claude.ai connectors, transport `claudeai-proxy`); `user` and `local` from `claude mcp add`.

Also:
- `system/init` carries `mcp_servers: [{name, status, source}]`, enough for a status dot without asking.
- `system/commands_changed` fires when a server connects, is toggled or added, with the full command list. MCP prompts are listed in it as `<server>:<prompt> (MCP)`, but the typed name is `/mcp__<server>__<prompt>` (the server's name with anything but letters, digits, `_` and `-` turned to `_`); `/<server>:<prompt>` isn't taken. `init`'s `slash_commands` uses the typed name. `initialize`'s list rarely has them: servers are still connecting.
- No event when a server's status changes by itself: poll `mcp_status` while the MCP view is open.
- **Security:** in `-p` mode a repo's `.mcp.json` servers connect with no approval (`enabledMcpjsonServers` was empty, no `enableAllProjectMcpServers`). The terminal asks first. So opening a cloned repo in the app already runs whatever commands its `.mcp.json` names.
- Adding and removing servers isn't a control request: it's `claude mcp add-json <name> <json> --scope user|project|local` and `claude mcp remove <name> --scope …`, plus `claude mcp add-from-claude-desktop`.

## Plan

### 1. Core (`HarnessCore`) (done)
- `ClaudeSession.request(_:) async throws -> JSONValue`: control requests that wait for their own reply, matched by `request_id` (today `control()` is fire and forget and replies only come back as events). Keep `.controlFailed` for the rest.
- `MCPServerStatus` (name, status enum with an `unknown(String)` case, scope, transport, error, server info, tools with read-only/destructive flags, a config summary without env values or headers, which can hold secrets). Parsing tolerant of new fields and statuses.
- Session calls: `mcpStatus()`, `setMCPServer(_:enabled:)`, `reconnectMCPServer`, `authenticateMCPServer` → auth URL, `submitMCPCallback`, `clearMCPAuth`.
- Parse `init.mcp_servers` into `SessionInfo`, and `commands_changed` into a new `.commandsChanged` event that updates the project's slash commands.
- Fixtures: a sanitized `mcp_status` reply and a `commands_changed` line under `Tests/HarnessCoreTests/Fixtures/`.

### 2. `/mcp` in the app
- `handleLocally`: `/mcp` opens an MCP sheet for the thread (SwiftUI: opened now and then, per CLAUDE.md). Add it to `appCommands` so the menu says "YourHarness".
- No live session yet (new thread): ask a throwaway process in the thread's folder, as `loadSlashCommands` does.
- One row per server, grouped by scope (Project, Yours, claude.ai, This session): status dot, name, transport chip, tool count, error text. Expanding shows the tools with read-only/destructive chips.
- Actions by status: toggle (enable/disable), Reconnect (failed), Sign In (needs-auth: open `authUrl` with `NSWorkspace`, the CLI's localhost callback finishes it, poll until `connected`; a "Paste redirect URL" field as the fallback), Sign Out (`mcp_clear_auth`).
- A small composer chip only when something needs you ("Linear needs sign-in"), opening the sheet. Nothing when all is well.
- Slash menu: show `mcp__server__prompt` as "server: prompt" with an MCP chip.

### 3. Managing servers
- Add Server sheet: name, stdio (command, args, env) or HTTP/SSE (URL, headers), scope (this project only / all projects / shared in `.mcp.json`). Runs `claude mcp add-json`. Remove runs `claude mcp remove`. Import from Claude Desktop.
- Settings → MCP lists user-scope servers across projects with the same rows.
- After a change, live sessions don't know (to verify): restart them between turns (resume keeps the conversation), or push it with `mcp_set_servers` for the running one.

### URL-only servers and federated login
The main case to get right: paste a URL, sign in, done. The CLI handles the OAuth side for `http`/`sse` servers. It finds the server's auth server, registers itself as a client (Claude Code's published client metadata, or dynamic registration) and does PKCE. Verified up to the sign-in URL against `https://mcp.linear.app/mcp`. Any SSO (Okta, Entra, Google Workspace) happens inside that browser page, so the app never sees it.
- Add Server starts with a single URL field. The name comes from the host (editable), transport `http` (SSE only if asked), scope "All your projects". Adding goes straight to Sign In when the server answers `needs-auth`.
- Sign-in opens in the **default browser**, not the built-in one: the SSO session, passkeys and the managed-device checks companies require live there. The CLI's localhost callback finishes it.
- The `claude` process that started the sign-in holds the callback listener: don't reap it until sign-in ends or is cancelled.
- Advanced, folded away: `--client-id`/`--client-secret` for servers without client registration (common behind company IdPs), `--callback-port` for a redirect URI registered with a fixed port, static `--header`s (a bearer token) for servers without OAuth.

### Where a server applies: workspace or everywhere
Tested 2026-11-04: Claude Code looks for `.mcp.json` in parent folders too (a thread in `repos/api` saw the workspace's), but its `local` scope and per-project on/off (`disabledMcpServers`) are keyed by the **exact** folder: a `local` server added at the workspace root was gone in `repos/api`. Threads run in sub-repos and worktrees, so the app keeps these itself:
- **Everywhere:** Claude Code's `user` scope (`claude mcp add -s user`), so the terminal sees it too.
- **This workspace, for everyone:** `.mcp.json` at the workspace root, committed. Secrets go in as `${VAR}`, never written, and each person is asked for their own value.
- **This workspace, just you:** kept in the app's state, not `~/.claude.json`. Every thread whose project is the workspace gets them through `--mcp-config <generated file in app storage>`, whatever folder it runs in. Secrets live in the Keychain and reach the process as environment variables the config names with `${VAR}`, so they're never on disk or on a command line (to verify: the CLI expands `${VAR}` in `--mcp-config` as it does in `.mcp.json`).
- **On/off per workspace** (turning off an everywhere server in one workspace): kept by the app and applied at session start with `mcp_toggle`, since the CLI's own persisted toggles are per folder. Find out whether a toggle there writes to `~/.claude.json` (it did for a dynamic server) and whether `--strict-mcp-config` plus the app's full list is cleaner (it may drop claude.ai connectors).
- Claude Code's `local` scope servers already there show under the folder they're tied to, with an offer to move them to the workspace.

### Claude asks you to sign in
Done (`MCPSignInTool`, `MCPSignInCard`, `AgentInstructions.mcpSignIn`). Measured on Haiku, fresh threads, 2026-11-04: with a plain instruction Claude called it in 3 of 4; Claude Code's own note ("The following MCP servers require authentication… non-interactive… use /mcp") won the fourth. Quoting that note in the instruction and saying to ignore its advice: 6 of 6 asked. The composer chip is the fallback when Claude doesn't.

Tested: in `-p` mode the CLI gives the model no sign-in tool for a `needs-auth` server, and tells it the session is non-interactive. So Claude answers "run `/mcp` in a terminal". Fix in the app:
- An app-served tool (beside memory/threads/browser), `sign_in_mcp_server(name, reason)`. It puts a sign-in card in the thread (like an approval prompt), runs `mcp_authenticate`, opens the URL in the default browser and waits for `connected`. Then it returns, and Claude carries on in the same turn, with the server's tools arriving as deferred tools. "Not Now" returns a refusal Claude can work around.
- `--append-system-prompt` says to use it instead of pointing at the terminal, overriding the CLI's "non-interactive" note.
- Also answer the CLI's `elicitation` control requests (`mode: "url"` is a server asking you to visit a URL, e.g. step-up auth; `form` asks for fields). The app ignores them today.

### Servers asking mid-call (elicitation) — done
Claude Code forwards an MCP server's `elicitation/create` as a `control_request` with `subtype: "elicitation"`: `mcp_server_name`, `message`, `mode` (`form` or `url`), `requested_schema` and `property_order` for a form, `url` and `elicitation_id` for a page. The answer is a `control_response` whose `response` is `{action: accept|decline|cancel, content}`. `system/elicitation_complete` says a page visit is done. Before this the app ignored them and the turn hung. Now `MCPElicitationCard` asks above the composer (fixture `mcp-elicitation.jsonl`).

### 4. Project `.mcp.json` approval
Ask before a project's `.mcp.json` servers start, as the terminal does: launch with them held back (find how: `--strict-mcp-config` plus our own `--mcp-config` of the approved ones, or the CLI's own `enabledMcpjsonServers`), show "This project wants to run: npx foo-mcp" with Allow / Don't Allow, remember per project. Ties in with the "confirm, but don't annoy" stance.

## Still to find out
- Whether a running session sees `claude mcp add` without a restart (`reload_plugins`?).
- A full OAuth sign-in end to end (needs the user's account on a real server; the URL part is verified).
- Whether `mcp_toggle` on a claude.ai connector persists, and where.
- What `pending` looks like and how long servers take, at launch, with many servers.

## Testing
- Parser and fixture tests; an integration test (`YH_INTEGRATION=1`) with a tiny stdio echo server (tools + prompts), a missing binary (failed) and a toggle round trip, put back afterwards (the toggle writes `~/.claude.json`: use a scratch `CLAUDE_CONFIG_DIR` there).
- `YH_OPEN_MCP=1` opens the sheet at launch, `YH_SNAPSHOT` draws it: checkable in a Dev copy in the background, nothing on screen.
- Never trigger a real sign-in from a test: `mcp_authenticate` is safe (no browser), opening its URL isn't.
