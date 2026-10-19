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

Integration tests spawn the real `claude` CLI on Haiku and the real `git`/`gh`: `YH_INTEGRATION=1 swift test --filter Integration`.

For manual runs, point the app at a throwaway state so it never scans the user's real projects: `YH_DATA_DIR=<scratch dir> build/YourHarness.app/Contents/MacOS/YourHarness`. Set it up with a `state.json` that lists only folders you're allowed to touch.

## Principles

- **Performance and memory are the product.** Check app footprint (`footprint $(pgrep -x YourHarness)`) after UI changes that touch the transcript. Use Debug → stress transcript.
- **Apple-quality UI.** Native controls, Liquid Glass (`glassEffect`, `.glass` button styles), SF Symbols, standard shortcuts. No web views.
- **`HarnessCore` never imports SwiftUI/AppKit.** Adapters emit `AgentEvent`. The UI only consumes `AgentEvent`.
- **CLI protocols change often.** Parsers ignore unknown event types and must never crash on new fields. When the protocol changes, capture a real session, sanitize it (no local paths or user config), and add it under `Tests/HarnessCoreTests/Fixtures/`.
- **Never extract or reuse CLI credentials.** Always drive the official binary.
