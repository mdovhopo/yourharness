# YourHarness

Native macOS (26+, Apple Silicon only) harness over AI coding CLIs. SwiftUI app plus the `HarnessCore` library. See README.md for the architecture.

## Build

`xcode-select` on this machine may point at the Command Line Tools, which can't build. Always prefix:

```sh
export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
swift build
swift test
scripts/bundle.sh   # → build/YourHarness.app
```

## Versions and releases

The user's main app is `/Applications/YourHarness.app`, installed by releases; `build/YourHarness.app` is the development build. The version is in `VERSION` (semver, from 0.0.1). When a feature lands, add a line to CHANGELOG.md under **Unreleased**, in the reader's terms. `scripts/release.sh <version>` cuts a release (changelog, tag, push, install); the tag's workflow publishes the DMG on GitHub Releases.

Integration tests spawn the real `claude` CLI on Haiku and the real `git`/`gh`: `YH_INTEGRATION=1 swift test --filter Integration`.

For manual runs, point the app at a throwaway state so it never scans the user's real projects, and launch it in the background so it never takes focus from what the user is typing: `open -g -n --env YH_DATA_DIR=<scratch dir> build/YourHarness.app` (one `--env` per variable). Running the executable directly brings the window to the front. Set it up with a `state.json` that lists only folders you're allowed to touch. Add `YH_SIMULATE_STREAMING=1` to stream the markdown sample (`StressTest.markdownSample`) into the selected thread at launch, for checking rendering and streaming without clicking. `YH_OPEN_DIAGRAM=<file.mmd>` opens a Mermaid file in the large diagram window. `YH_SEND_MESSAGE=<text>` (plus optional `YH_SEND_IMAGES=<a.png:b.png>`) sends a message from a new thread at launch (a real Claude turn plus title generation), so keep the state's folders throwaway. `YH_SEND_IMAGES` alone attaches the images to the selected thread's composer instead, as a drop does. `YH_RESUME_SESSION=<session id>` opens the Resume Claude Code Session sheet filled in. `YH_APPEARANCE=light|dark` overrides the Appearance setting without saving it. `YH_OPEN_SETTINGS=1` opens Settings. `YH_SCROLL_PROBE=<points>` scrolls the open thread up (negative: down) in trackpad-sized steps and prints how often a row on screen moved by more or less than the step, the jump count; `YH_SCROLL_PROBE=log` prints the scroll geometry instead. Run it after transcript layout changes. The right sidebar's open state is saved (`showsInspector` in `state.json`), so a scratch state can open it.

## Principles

- **Performance and memory are the product.** Check app footprint (`footprint $(pgrep -x YourHarness)`) after UI changes that touch the transcript. Use Debug → stress transcript.
- **Apple-quality UI.** Native controls, Liquid Glass (`glassEffect`, `.glass` button styles), SF Symbols, standard shortcuts. No web views. Draw from `Views/Theme.swift` so the window stays coherent: content sits on the `.canvas` (set once under the detail column), boxes use `.card()`, labels use `Chip`, radii come from `Theme`. Glass is for what floats above content (composer, prompts, buttons), not for transcript rows.
- **`HarnessCore` never imports SwiftUI/AppKit.** Adapters emit `AgentEvent`. The UI only consumes `AgentEvent`.
- **CLI protocols change often.** Parsers ignore unknown event types and must never crash on new fields. When the protocol changes, capture a real session, sanitize it (no local paths or user config), and add it under `Tests/HarnessCoreTests/Fixtures/`.
- **Never extract or reuse CLI credentials.** Always drive the official binary.
