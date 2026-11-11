# Demo videos and screenshots

Made from the real app on demo data, with no screen recording (so no privacy prompts): a
`FEATURES=snapshots` build runs a script (`YH_DEMO`, `crates/yh-app/src/demo.rs`) that clicks,
types, scrolls and waits on real Claude turns through the window, and draws each frame with
`render_to_image` into `ffmpeg`, with a drawn pointer and the browser's page captured from WebKit
and laid in. Captions and camera zooms in the script are written next to the take with their
times; `compose.py` cuts takes into the finished video.

```sh
brew install ffmpeg; python3 -m venv /tmp/yh-media-venv && /tmp/yh-media-venv/bin/pip install numpy pillow
APP=test-harness-root/demo/app/"YourHarness Next.app" FEATURES=snapshots scripts/bundle.sh

scripts/demo/build-data.sh                 # projects in /Users/Shared/Projects + real turns → data-seed, work-seed
scripts/demo/pair-phone.sh                 # the demo Simulator paired over Tailscale (phone app built into
                                           # test-harness-root/demo/phone-dd, Simulator ID in demo/phone-udid)

EXTRA_ENV="--env YH_MODEL=sonnet" scripts/demo/record.sh scripts/demo/scenes/overview.json system
scripts/demo/record-phone.sh scripts/demo/scenes/phone.json scripts/demo/phone/steps.json dark
/tmp/yh-media-venv/bin/python -I scripts/demo/compose.py scripts/demo/cuts/overview.json
scripts/demo/record.sh scripts/demo/scenes/stills.json dark
scripts/demo/stills.sh test-harness-root/demo/takes/stills-dark.mp4 test-harness-root/demo/out/stills dark
```

- The projects live in `/Users/Shared/Projects`, not in this repo: commands Claude runs show their
  paths (no home folder in them), and inside the repo its CLAUDE.md reached the demo turns.
- The phone: `YH_PHONE_DEMO` (`mobile/ios/YourHarness/Demo.swift`) acts out `phone/*.json` while
  `simctl io recordVideo` films the Simulator; the Mac serves phones over Tailscale only, so the
  connection shown is the real one. A cut's `phone` clip puts both side by side, lined up by the
  take's `-sync.json`; `phone_blur` hides the tailnet address. Shut the demo Simulator down for
  Mac-only takes: it slows the Mac's frame grabs.
- `appearance` steps switch light and dark mid-take (record with `system`); a cut's clips take a
  `theme` for the backdrop.
- `scenes/*.json`: what happens, step by step (see the top of `demo.rs` for the steps). Live
  turns differ every take: check the take before cutting.
- `cuts/*.json`: clips of takes (`from`, `to`, `speed` for time-lapses), title cards; makes the
  MP4, its `.srt` and a poster still. Music is `music.py`, generated (no licence to track), mixed
  at -30 LUFS.
- Record when the Mac is otherwise quiet: a build or a simulator running alongside starves the
  recorder (`DEMO frames take … ms to grab` in the take's log should stay near 10 ms).
