# MCP servers

Everything `/mcp` does in the terminal, done in the app, so there's never a reason to open
Claude Code for it. Run headless, `/mcp` only gets Claude Code's short answer ("1 MCP
server(s): 0 connected, … Use `/mcp` in the terminal for details."), so the app does it
itself.

## What the app has

- A **composer chip** that only shows when a server needs you (one to sign in to, or one that
  won't start), opening `/mcp` (`composer.rs`, `mcp_attention_chip`).
- **`/mcp`** opens the **sheet** (`ui/mcp.rs`): servers grouped by where they apply, with the
  action each needs (turn on or off, reconnect, sign in, sign out). A row opens the server's
  **page**: its tools, sign in/out, the error. The sheet asks the thread's `claude` (started in
  its folder if it isn't running) and asks again every 2 s while it's open. The app's own
  servers (browser, memory, threads, harness: transport `sdk`) aren't listed: they aren't yours
  to switch off, and `/mcp disable all` would turn off Claude's browser and memory too.
- **Add Server** starts from a URL (sign-in in the default browser, for federated login), with
  a command or pasted JSON as the other ways in (one config, `{"name": {…}}` or a whole
  `mcpServers` block), and Import from Claude Desktop (its `claude_desktop_config.json`). It
  asks where the server applies: everywhere (`user`), this project just for you (`local`), or
  its `.mcp.json` (`project`). It runs `claude mcp add-json` / `claude mcp remove` in the
  thread's folder, so the terminal sees them too. Advanced for URLs: headers, the OAuth client
  ID, a client secret (through `MCP_CLIENT_SECRET`, as Claude Code reads it) and the callback
  port.
- **Claude asks you to sign in** with a card in the thread, through a tool the app serves
  (`mcp__harness__sign_in_to_mcp_server`, `state/harness_tools.rs`).
- **Servers asking mid-call** (elicitation): a form or a page to visit, asked in a card above
  the composer.
- **Settings → MCP**: the same views with no thread, asking Claude Code in Home's folder and
  showing only what applies everywhere (`user`, your organization's, claude.ai connectors). No
  on/off switches there (Claude Code keeps those per folder), and adding from Settings always
  adds everywhere.
- MCP prompts in the `/` menu.

A running session notices a `user` or `project` server added to the config about a second
later; `mcp_reconnect` with its name then connects it (retried for 3 s). A `local` one it
never notices, since it reads the folder's config at start, so its `claude` starts over between
turns and the next message resumes the session. Removing always starts it over.

## What the CLI gives us (probed on 2.1.312, 2026-11-03)

All of it over the stream-json `control_request` channel the session already uses
(`ClaudeProcess::request`, which waits for the reply matched by `request_id`). None of it
needs a turn, so none of it costs tokens.

| Request | Fields | Reply |
| --- | --- | --- |
| `mcp_status` | — | `{mcpServers: [{name, status, scope, source, transport, config, serverInfo?, error?, tools?: [{name, annotations: {readOnly?, destructive?}}]}]}` |
| `mcp_toggle` | `serverName`, `enabled` | success; **persists** in `~/.claude.json` → `projects[cwd].disabledMcpServers` |
| `mcp_reconnect` | `serverName` | success, or an error ("Connection failed") |
| `mcp_authenticate` | `serverName`, `redirectUri?` | `{authUrl, requiresUserAction, callbackExpected, callbackPort, state}`; **doesn't open a browser** in SDK mode (`skipBrowserOpen`), the CLI listens on `localhost:<callbackPort>` |
| `mcp_oauth_callback_url` | `serverName`, `callbackUrl` | for pasting the redirect URL by hand when the localhost callback can't be reached |
| `mcp_clear_auth` | `serverName` | revokes and forgets the server's tokens |
| `mcp_set_servers` | `servers: {name: config}` | `{added, removed, errors}`; session-only ("dynamic") servers, not written anywhere |

Statuses seen: `connected`, `failed` (with `error`), `needs-auth`, `disabled`,
`pending`/connecting. Scopes seen: `project` (`.mcp.json`), `dynamic` (`--mcp-config` /
`mcp_set_servers`), `claudeai` (claude.ai connectors, transport `claudeai-proxy`); `user` and
`local` from `claude mcp add`. Parsing keeps unknown statuses and ignores new fields.

Also:
- `system/init` carries `mcp_servers: [{name, status, source}]`, enough for a status dot without asking.
- `system/commands_changed` fires when a server connects, is toggled or added, with the full command list. MCP prompts are listed in it as `<server>:<prompt> (MCP)`, but the typed name is `/mcp__<server>__<prompt>` (the server's name with anything but letters, digits, `_` and `-` turned to `_`); `/<server>:<prompt>` isn't taken. `init`'s `slash_commands` uses the typed name. `initialize`'s list rarely has them: servers are still connecting.
- No event when a server's status changes by itself: hence asking again while the sheet is open.
- **Security:** in `-p` mode a repo's `.mcp.json` servers connect with no approval (`enabledMcpjsonServers` was empty, no `enableAllProjectMcpServers`). The terminal asks first. So opening a cloned repo in the app already runs whatever commands its `.mcp.json` names.
- Adding and removing servers isn't a control request: it's `claude mcp add-json <name> <json> --scope user|project|local` and `claude mcp remove <name> --scope …`.

## URL-only servers and federated login

The main case to get right: paste a URL, sign in, done. The CLI handles the OAuth side for
`http`/`sse` servers. It finds the server's auth server, registers itself as a client (Claude
Code's published client metadata, or dynamic registration) and does PKCE. Verified up to the
sign-in URL against `https://mcp.linear.app/mcp`. Any SSO (Okta, Entra, Google Workspace)
happens inside that browser page, so the app never sees it.

- Sign-in opens in the **default browser**, not the built-in one: the SSO session, passkeys and
  the managed-device checks companies require live there. The CLI's localhost callback
  finishes it; a "paste the redirect URL" field is the fallback.
- The `claude` process that started the sign-in holds the callback listener, so it's kept until
  sign-in ends or is cancelled.

## Claude asks you to sign in

In `-p` mode the CLI gives the model no sign-in tool for a `needs-auth` server and tells it the
session is non-interactive, so Claude would answer "run `/mcp` in a terminal". The app serves
`mcp__harness__sign_in_to_mcp_server(name, reason)`: it puts a sign-in card in the thread,
runs `mcp_authenticate`, opens the URL in the default browser and returns once the server is
`connected`. Claude carries on in the same turn, with the server's tools arriving as deferred
tools. "Not Now" returns a refusal Claude can work around.

What the agent is told (`harness_tools::INSTRUCTIONS`) quotes Claude Code's own note ("The
following MCP servers require authentication… non-interactive… use /mcp") and says to ignore
its advice. Measured on Haiku, fresh threads: with a plain instruction Claude called the tool
in 3 of 4, Claude Code's note winning the fourth; quoting the note, 6 of 6. The composer chip
is the fallback when Claude doesn't.

## Servers asking mid-call (elicitation)

Claude Code forwards an MCP server's `elicitation/create` as a `control_request` with
`subtype: "elicitation"`: `mcp_server_name`, `message`, `mode` (`form` or `url`),
`requested_schema` and `property_order` for a form, `url` and `elicitation_id` for a page. The
answer is a `control_response` whose `response` is `{action: accept|decline|cancel, content}`
(`ClaudeProcess::respond_elicitation`). `system/elicitation_complete` says a page visit is
done. Unanswered, the turn hangs, so the card above the composer always asks.

## Where a server applies: workspace or everywhere (design)

Tested 2026-11-04: Claude Code looks for `.mcp.json` in parent folders too (a thread in
`repos/api` saw the workspace's), but its `local` scope and per-project on/off
(`disabledMcpServers`) are keyed by the **exact** folder: a `local` server added at the
workspace root was gone in `repos/api`. Threads run in sub-repos and worktrees, so the app
would keep these itself:
- **Everywhere:** Claude Code's `user` scope, so the terminal sees it too (built).
- **This workspace, for everyone:** `.mcp.json` at the workspace root, committed. Secrets go in as `${VAR}`, never written, and each person is asked for their own value.
- **This workspace, just you:** kept in the app's state, not `~/.claude.json`. Every thread whose project is the workspace gets them through `--mcp-config <generated file in app storage>`, whatever folder it runs in. Secrets live in the Keychain and reach the process as environment variables the config names with `${VAR}`, so they're never on disk or on a command line (to verify: the CLI expands `${VAR}` in `--mcp-config` as it does in `.mcp.json`).
- **On/off per workspace** (turning off an everywhere server in one workspace): kept by the app and applied at session start with `mcp_toggle`, since the CLI's own persisted toggles are per folder. Find out whether a toggle there writes to `~/.claude.json` (it did for a dynamic server) and whether `--strict-mcp-config` plus the app's full list is cleaner (it may drop claude.ai connectors).
- Claude Code's `local` scope servers already there show under the folder they're tied to, with an offer to move them to the workspace.

## Next: project `.mcp.json` approval

Ask before a project's `.mcp.json` servers start, as the terminal does: launch with them held
back (find how: `--strict-mcp-config` plus our own `--mcp-config` of the approved ones, or the
CLI's own `enabledMcpjsonServers`), show "This project wants to run: npx foo-mcp" with Allow /
Don't Allow, remember per project. Ties in with the "confirm, but don't annoy" stance.

## Still to find out

- A full OAuth sign-in end to end (needs the user's account on a real server; the URL part is verified).
- Whether `mcp_toggle` on a claude.ai connector persists, and where.
- What `pending` looks like and how long servers take, at launch, with many servers.

## Testing

- `cargo test -p yh-core -- claude::` covers the control requests and elicitations as parsed.
- `YH_OPEN_MCP=1` opens the sheet at launch and prints its servers (`=everywhere` for
  Settings → MCP), `YH_SNAPSHOT` draws it: checkable in a development build in the background,
  nothing on screen. `YH_MCP_SELECT=<server>` opens a server's page.
- Adding and removing: `YH_MCP_ADD_URL` or `YH_MCP_ADD_COMMAND` (with `YH_MCP_ADD_ENV`),
  `YH_MCP_ADD_SCOPE`, `YH_MCP_ADD_SUBMIT=1`, `YH_MCP_REMOVE=<name>`, `YH_MCP_ADD_DESKTOP=1` with
  `YH_CLAUDE_DESKTOP_CONFIG=<scratch file>`. They write Claude Code's config: a scratch project
  and a scratch `CLAUDE_CONFIG_DIR` (toggles write `~/.claude.json`).
- `YH_MCP_BROWSER=log` prints sign-in pages instead of opening them; `YH_MCP_SIGN_IN_ANSWER`
  and `YH_MCP_ELICIT_ANSWER` answer the cards.
- Never trigger a real sign-in from a test: `mcp_authenticate` is safe (no browser), opening its URL isn't.
