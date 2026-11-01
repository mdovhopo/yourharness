# Computer use

Status: parked on the `computer-use` branch. Investigation only: nothing is built, and the spikes below aren't finished.

## Goal

Let the agent use Mac apps while you follow along in a right sidebar panel, as with the browser, so agents can work in parallel while you use the Mac. Wanted for two uses: driving real Mac apps, and testing apps the agent builds, YourHarness above all.

## Decisions

- **Two modes behind one tool set.** A mirror of a window on this Mac comes first. A virtual Mac follows.
- **The tools are our own in-process MCP server**, as `mcp__browser__` is (see docs/browser.md), so the panel knows what the agent does and can draw its cursor and clicks.
- **The panel is view-only by default.** Hovering says so, and a toggle gives you control. Taking control pauses the agent after its current action. Handing back gives it a fresh screenshot and a note that things changed. A stop button and a global shortcut are always there.
- **On by default, and each use asks.** A prompt offers "Don't ask again", remembered per app in Settings, where it can be revoked. Settings can turn the whole feature off, which removes the tools. macOS's own permissions are requested only at first use.
- **Terminal, password managers and System Settings ask every time**, with no "Don't ask again".
- **The real pointer is a separate tool**, `request_foreground_control(app, reason)`, enforced by the tool and not by instructions: `click` and `type` use background routes only and return `needs_foreground_control` when those fail. Approval is once or per app, ends with the turn unless remembered, and times out as denied. Touching the mouse or keyboard pauses the agent.
- **One thread at a time holds the real pointer**, and each thread uses one app.
- **The virtual Mac is optional.** Nothing is downloaded until you press "Set Up Virtual Mac…" in Settings, which shows the size first. Agents can't start it, and its tools exist only once it's set up. "Remove Virtual Mac" deletes it. A panel switch picks This Mac or Virtual Mac per thread, and a project can default to it. The app ships with the `com.apple.security.virtualization` entitlement, which `scripts/bundle.sh` doesn't set yet.
- **Frames go from an `IOSurface` to a `CALayer` in AppKit**, so SwiftUI never sees them.

## Build order

1. Tools: screenshots and background input
2. Panel: view-only mirror and the agent's cursor
3. Stop button and global shortcut, before anything can take the real pointer
4. Approval flow, Settings list and off switch
5. Take control toggle
6. Virtual Mac behind the same tools
7. Probes

## What the spikes found

Measured 2026-11-01 on a MacBookPro18,2 with 32 GB of RAM, macOS 27.0.1, Claude Code 2.1.312.

**Waiting for approval.** A tool call waited 6 minutes without the CLI timing it out, and the turn then finished normally (`aToolCallCanWaitForTheUser`).

**Capture (ScreenCaptureKit).**
- One stream of a 600 × 432 pt window animating at 60 fps at 2x scale, asking for 30 fps: 31 fps, 2.0% CPU. Four streams: 20.8 fps each, 4.4% CPU. That CPU is the capturing process only, not the system's processes doing the capture, so the true cost is higher.
- A minimized window gives no frames, and a screenshot of it fails with `-3811`. The panel needs a state for this.
- `-3811` also came up when capturing a window that wasn't the app's real one.

**Input without focus.**
- Accessibility works on native apps: `AXPress` in Calculator and setting `AXValue` in TextEdit, with the pointer and the front app unchanged.
- Keys through `CGEvent.postToPid` reached Calculator, TextEdit and a custom-drawn window, also without taking focus.
- **Clicks and scrolling delivered nothing** through `postToPid`, with or without the window fields (91, 92), or through `SLEventPostToPid` with `CGEventSetWindowLocation`. **Unresolved:** the control, a plain click with the target frontmost, failed too, so this is either a test bug or a change in macOS 27.
- VS Code: inconclusive. It listed 715 Accessibility elements with `AXManualAccessibility` once, but typed keys changed no pixels, and the next run found no text area.
- VS Code takes focus when launched, even with `activates = false`. Handing it back to the previous app worked.

**Permissions.**
- An app launched with `open` from inside YourHarness is its own responsible process, so macOS doesn't attribute it to YourHarness.
- `CGPreflightScreenCaptureAccess` doesn't change in a running process after a grant: check from a fresh launch.
- A test app in `/tmp` asked for permissions but wasn't listed in Settings. Accessibility took once it was in `~/Applications`, though we can't tell if that was the move or the attempts to add it by hand.
- Settings now calls the Accessibility page "Device Control and Data Access".
- The feature has to handle both permissions being refused or missing from the list, with a way to open the right page.

**Virtual Mac.**
- Guest: macOS 27.2 (26B123), at least 2 CPUs and 4 GB of RAM.
- The restore image is a 25.6 GB download. Installing took 4.5 minutes and used 25 GB of a sparse 60 GB disk. Setup needs 55–60 GB free at its peak, then about 25 GB once the image is deleted.
- The restore catalog fails with `VZErrorDomain 10001` without the entitlement.

## Still to find out

1. **Clicks:** a run with the real pointer, to show whether the test window receives clicks at all, then background delivery again. This decides how often the agent needs your pointer.
2. **Electron and canvas apps:** VS Code again, and a real canvas app.
3. **Another Space:** does capture continue for a window there?
4. **Virtual Mac:** first boot, whether Setup Assistant has to be clicked through by hand, RAM, suspend time, how the tools drive the guest (probably a helper over vsock), getting test builds into it, and cloning a set-up image as a base.
5. **Screenshots through the CLI:** quality and coordinates at this size. Image results are known to reach the model, but the mapping between screenshot and screen points hasn't been tried with a model.
6. **Testing without prompts:** how a Dev copy gets these permissions. Otherwise most tests need a fake capture and input layer.

## Running the spikes

`scripts/computer-use-spike/` has the test programs. They're investigation code, not part of the app.

- `host.swift` is a small app that holds the permissions and loads a plugin, so changing the plugin doesn't ask again. An ad hoc signature changes with every build of the app itself.
- `spike.swift` is the plugin: modes `check`, `run` and `run2`, writing to `/tmp/yh-cu-spike/results.log`. It only opens Calculator, TextEdit on a scratch file, VS Code on a throwaway profile, and its own window.
- `vm-install.swift` installs macOS into `~/.yh-vm-spike` from a `restore.ipsw` there. Sign it with the virtualization entitlement.

Keep the plugin outside anything other users can write to, and take the host's permissions away when done: it loads whatever plugin is at that path.
