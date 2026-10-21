# Browser

Status: built, apart from what's under Still to build. Follows T3 Code's collaborative browser, done natively.

## Goal

Give each thread a real browser that the agent can drive and you can watch and take over: run the app it's building, look at it, click through it, read the console and network, and show you what it found. Testing a web change should be as easy for the agent as running the unit tests, and you should see every step.

## What T3 Code does

T3 Code has a per-thread browser panel. Its agents drive it through an MCP server (`mcp__t3-code__preview_*`) inside T3's own server:
- **Engine:** a downloaded headless Chrome (about 120 MB) driven by Playwright over CDP. Desktop tabs are Electron webviews relayed over CDP.
- **Transport:** an HTTP MCP endpoint on localhost, guarded by a per-session bearer token. For Claude it's registered through the CLI's stdin control channel, so the token never appears in `argv`, and `mcp__t3-code__*` is pre-approved.
- **Snapshots:** Playwright's ARIA snapshot with element refs (`aria-ref=…`), capped near 20 KB. Claude Code moves bigger MCP results to a file, and the refs are lost. Passwords are masked.
- **Shared control:** the agent owns the tabs it opens. You can "Take control", which blocks agent actions but not reads, and invalidates refs ("Take a fresh snapshot"). A pending JS dialog blocks actions until it's answered.
- **Around it:**
  - device presets and freeform sizes;
  - an element picker and annotations sent to the composer;
  - screenshots and recordings;
  - localhost servers found by port scanning, shown in the empty state;
  - browser profiles, with cookie import from other browsers;
  - a floating mini-player when an agent opens a tab.
- **Gaps:** there are no console or network views for the user; those reach only the agent, and the user gets DevTools.

We keep the shape: a panel per thread, a typed tool set, refs from snapshots, shared control, and pre-approval. Underneath, we differ.

## Decisions

### Engine: WebKit (`WKWebView`), not a bundled Chromium

- It's already on every Mac: no download, no second browser to keep updated, and it's Safari's engine, which your users have.
- Each tab costs one WebContent process (tens of MB, outside the app's footprint), not a Chromium instance.
- The trade-off is that Chrome-only bugs aren't caught. "Open in Default Browser" is always one click away, and driving the user's Chrome over CDP can come later as a second engine behind the same tools.
- `CLAUDE.md` says "no web views". That rule is about the app's own UI, which stays native. A page under test is web content by definition. The panel around it (URL bar, console, network, controls) is SwiftUI and Liquid Glass.

### Transport: in-process MCP over the CLI's control channel

The app tells the CLI it hosts an MCP server (`initialize` with `sdkMcpServers: ["browser"]`). The CLI then sends every MCP message to us as a `control_request` of subtype `mcp_message`, and we answer with `{mcp_response: …}` on the same stdin/stdout pipe we already use.

Spiked with the installed CLI (2.1.305) and a 60-line Python host:
- The handshake (`initialize`, `notifications/initialized`, `tools/list`) runs, and `tools/call` arrives with `_meta["claudecode/toolUseId"]`, which ties each call to its transcript row.
- It works under `--strict-mcp-config` with built-in tools off. `init` reports the server as `{status: connected, source: sdk}`.
- An image in the tool result reaches the model: a test image came back as "Green".

That means no port, no token and no `--mcp-config`. Nothing on the machine but the `claude` process we started can reach the tools. It's simpler and safer than T3's HTTP endpoint. Other CLIs (Codex) can get a localhost HTTP transport for the same tool set later.

### Tools are pre-approved

The browser is sandboxed to the app: its own cookie store, and nothing it does touches your files. So `mcp__browser__*` goes in `--allowedTools`, as T3 does. There's a Settings toggle, "Let Claude use the browser" (on), with a per-project override later.

When it's off:
- the server isn't registered;
- the system prompt doesn't mention it, because describing tools that don't exist talks the model out of using Playwright or curl.

### Real input events, not synthetic DOM ones

Clicks and keys are sent to the `WKWebView` as `NSEvent`s at the element's position. That way pages see trusted events (`isTrusted`), focus, hover and default actions just as with a person. Scripted `element.click()` is the fallback when an element can't be hit (covered or off screen). Spike 3 checks this.

## What you see

The right sidebar gets a third tab: **Browser**, next to Side Chat and Sub-agents.

- **Address bar** (a glass bar at the top of the panel; the window toolbar holds the sidebar's tabs):
  - back/forward and reload;
  - the URL field;
  - device size;
  - the element picker;
  - Take Control / Give Back;
  - a More menu: Open in Default Browser, Web Inspector, Clear Browsing Data, light/dark page.
- **Tabs:** a thin tab strip appears only when the thread has more than one tab.
- **Page:** fills the panel. When the agent acts, a ring briefly marks the element it clicked or typed into, so you can follow along.
- **Console and Network** (T3 has neither):
  - a drawer under the page with two lists;
  - console messages and uncaught errors, with level filters;
  - requests with method, status, type and time, failed ones in red.
  - Both lists are native, searchable and capped, the same buffers the agent reads.
- **Empty state:** "Claude opens pages here when it tests your app." It lists recent URLs and the local servers this thread started (see Local servers below).
- **Opening:** when the agent opens a page in the thread you're looking at, the sidebar opens on Browser. A setting covers this, "Show the browser when Claude opens it" (on).
- **Taking over:** you can click and type in the page at any time. Take Control makes it explicit: the agent's actions fail with "The user has taken control of the browser", while its reads still work.
- **Element picker:** pick an element to put it in the composer as a chip with:
  - its role, name and a CSS selector;
  - a short HTML excerpt;
  - a cropped screenshot, which goes in as an image attachment.

  Annotations (drawing, CSS tweaks) come later.
- **Transcript:**
  - browser calls render as their own rows: "Open localhost:3000", "Click 'Sign in'", "Type into Email";
  - a screenshot shows as a thumbnail you can click;
  - clicking a row shows the Browser tab.
- **Web Inspector:**
  - `isInspectable` is on, so Safari's full Web Inspector can attach (Develop menu);
  - the More menu explains this if Safari's Develop menu is off.

## Tools

Server name `browser`, so Claude sees `mcp__browser__<tool>`.
- **Targets:** every action takes a `ref` from the latest snapshot, or a CSS `selector`; `click` also takes `x`/`y`.
- **Tabs:** every tool takes an optional `tab`. With one tab, it's implied.
- **Results:** short text: what happened, the URL and title if they changed, and the count of new console errors and failed requests since the last call. That way the agent notices breakage without asking.

| Tool | Does | Returns |
|---|---|---|
| `status` | Tabs, URL, title, loading, viewport, who has control, a pending dialog | text |
| `open` | A new tab at `url` (`show` defaults to true) | tab, status |
| `navigate` | `url`, or `back` / `forward` / `reload`; waits for `load` (or `domContentLoaded`, `none`), up to 60 s | status |
| `snapshot` | The page as an outline of landmarks, headings, text and interactive elements with refs (`button "Sign in" [ref=e12]`), plus the viewport, scroll position and recent errors. Password values masked. Capped at 20 KB, dropping logs first and interactive elements last | text |
| `screenshot` | The viewport, a full page or one `ref`, downscaled for the model | image |
| `click` | `ref`/`selector`/`x,y`, button, click count | result |
| `type` | Text into `ref` or the focused element; `clear`, `submit` | result |
| `press` | A key with modifiers (Enter, Escape, ⌘A…) | result |
| `hover`, `select`, `scroll`, `drag` | As named; `select` for native `<select>` | result |
| `wait_for` | Text, a selector or a URL to appear, all of them by `timeout` | result |
| `console` | Messages and uncaught errors since a point, by level | text |
| `network` | Requests since a point: method, URL, status, type, time, failure reason; `failed_only` | text |
| `evaluate` | A JS expression in the page (async allowed), returning JSON up to 64 KB | text |
| `resize` | A preset (desktop, tablet, phone) or width × height; `color_scheme` light/dark | status |
| `dialog` | Accept or dismiss a pending alert/confirm/prompt, with text | result |
| `upload` | Absolute file paths into a file input or open chooser | result |
| `close` | Closes a tab the agent opened | — |

How the snapshot works:
- It's JS in an isolated `WKContentWorld`, so the page can't see or spoof it.
- It computes roles and accessible names (ARIA first, then the HTML semantics), keeps what's visible, and gives elements refs in a `WeakMap`.
- Refs carry a generation that changes on navigation and on Take Control, so a stale ref fails with "Take a fresh snapshot".

How console and network are captured:
- **Console:** hooked in the page world at document start: `console.*`, `error` and `unhandledrejection`.
- **Network:** `fetch`/XHR are wrapped, `PerformanceObserver` covers every resource, and navigation responses come from the delegate.
- **Buffers:** ring buffers per tab (2,000 entries each).

## Local servers

T3 scans every listening port. We look only at what this thread started. Two sources:
- **Output:** `http://localhost:…` and `127.0.0.1` URLs in Bash output and background task output.
- **Processes:** `lsof -a -iTCP -sTCP:LISTEN` limited to the processes under the thread's `claude` process, run when a Bash call finishes.

What it gives you:
- found servers show in the Browser empty state;
- localhost links in the transcript open in the thread's browser (⌘-click opens the default browser);
- the agent's instructions say it can start a dev server in the background and `open` it.

## Lifecycle and memory

- **Created on demand:** a thread's browser exists only once the agent calls `open`, or you type a URL.
- **Cookies:** one persistent `WKWebsiteDataStore(forIdentifier:)` per project, so logins survive and stay out of Safari and other projects. Clear Browsing Data resets it. A private tab uses a non-persistent store.
- **Idle teardown:** tabs of threads you aren't looking at are closed after 30 minutes idle. The URL and history are kept, so the next `open`/`status` or your visit brings them back.
- **Ending:** settling or deleting a thread closes its tabs. Quitting closes all of them.
- **Limits:** at most 8 tabs per thread. Popups become tabs.
- **Sub-agents** share the thread's browser and open their own tab when working in parallel (the tool description says so).

## Architecture

- **HarnessCore** (no WebKit):
  - `MCPToolServer`: tool definitions and JSON-RPC handling (`initialize`, `tools/list`, `tools/call`), pure and fixture-tested.
  - `ClaudeSession` registers servers in `initialize` and answers `mcp_message` control requests by calling them.
  - The tool calls themselves go through a `Sendable` protocol, so the core never sees a browser.
- **App:**
  - `BrowserSession`, one per thread: tabs, control state, console and network buffers.
  - `BrowserTab`: one `WKWebView`, its delegates and injected scripts.
  - `BrowserTools`: implements the tool set on the main actor.
  - Scripts (snapshot, refs, console/network hooks, picker) live as bundled `.js` resources.
  - `Views/BrowserPanel.swift`: the sidebar tab. A transcript row type for browser calls.
- **Instructions:** one paragraph in `--append-system-prompt` when enabled. It says:
  - the browser is in the app and the user sees it;
  - prefer it over Playwright or curl for checking UI;
  - start dev servers in the background;
  - take a snapshot before acting;
  - take a screenshot to check how things look.

## What the spikes found

1. In-process MCP over the control channel works with the installed CLI, with no wait between `initialize` and the first message (integration test `MCPIntegrationTests`), and `--allowedTools mcp__browser` keeps the tools from asking.
2. Image results reach the model.
3. **Input.** `NSEvent`s sent straight to the web view arrive as trusted events, in a key window or not, on screen or off. Two catches, both handled:
   - WebKit makes the web view the window's first responder when it's clicked, which would pull the keyboard out of your composer. `_setShouldSuppressFirstResponderChanges:` stops that for the agent's own events (turned off again after a round trip, so your clicks focus the page as usual).
   - Without the keyboard focus, key presses still fire `keydown` and their default actions (Backspace, Tab, Enter submitting a form), but typed characters don't insert. `type` inserts text with `insertText:` instead (real `beforeinput`/`input` events). ⌘A/⌘C/⌘X/⌘V are menu commands WebKit runs only for the focused view, so they're done in the page, with a clipboard of the browser's own.
4. **Hidden rendering.** WebKit treats a page in an occluded or off-screen window as hidden: timers drop to 1 Hz, `requestAnimationFrame` stops, `visibilityState` is `hidden`. With `_setWindowOcclusionDetectionEnabled:` off, a page in any window that's ordered in runs at full speed and screenshots fine; out of a window it's suspended either way. So pages not on screen wait in an off-screen, never-key panel (it doesn't keep the app open), and moving a web view between windows keeps its state. Occlusion detection is off only while the agent is using the browser and for two minutes after. Both switches are private WebKit API, looked up first: without them, pages are throttled and the agent's clicks take focus, but nothing breaks.
5. **Snapshots.** Measured on real pages: a login form 1 KB, TodoMVC 1 KB, Hacker News' front page 17.5 KB with all 30 stories, a long Wikipedia article and apple.com capped at 18 KB with text and off-screen parts dropped first. Layout tables are flattened, boxes around a single link collapse into it, and punctuation between links is dropped.

## Still to build

- Recordings and annotations.
- Localhost URLs inside code spans aren't links, so they don't open in the browser yet.
- The same tools for Codex over localhost HTTP; Chrome over CDP as a second engine.

## How it's tested

`YH_BROWSER_PROBE=<steps.json>` runs tool calls at launch and prints the results (see CLAUDE.md). The MCP plumbing has unit tests and an integration test against the real CLI.

## Decided

- A thread's tabs reopen at their last URLs when its browser is next used; the agent's next `open` takes over a reopened tab.
- The side chat gets the read-only tools (`status`, `snapshot`, `screenshot`, `wait_for`, `console`, `network`) on the thread's browser.
- Cookie import from Safari and Chrome is wanted. Built as Import Cookies… in the More menu: you pick a browser and profile, then the sites; nothing is read until you pick, and nothing is selected by default. Safari's file needs Full Disk Access; Chromium values are decrypted with PBKDF2/AES-128-CBC from the browser's Safe Storage keychain item (macOS asks you), dropping the domain hash newer databases prefix. The readers are tested on synthetic files in `CookieImportTests`.
