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

scripts/demo/make-workspace.sh             # projects/ → test-harness-root/demo/work, as git repos
scripts/demo/seed.sh                       # real Sonnet turns on them → test-harness-root/demo/data
SEED=seed2 scripts/demo/seed.sh "" haiku   # a few more threads, memories and a task
# then keep the seeded state: cp -cR …/demo/work …/demo/work-seed; cp -cR …/demo/data …/demo/data-seed

EXTRA_ENV="--env YH_MODEL=sonnet" scripts/demo/record.sh scripts/demo/scenes/overview.json light
/tmp/yh-media-venv/bin/python -I scripts/demo/compose.py scripts/demo/cuts/overview-light.json
scripts/demo/record.sh scripts/demo/scenes/stills.json dark
scripts/demo/stills.sh test-harness-root/demo/takes/stills-dark.mp4 test-harness-root/demo/out/stills dark
```

- `scenes/*.json`: what happens, step by step (see the top of `demo.rs` for the steps). Live
  turns differ every take: check the take before cutting.
- `cuts/*.json`: clips of takes (`from`, `to`, `speed` for time-lapses), title cards; makes the
  MP4, its `.srt` and a poster still. Music is `music.py`, generated (no licence to track), mixed
  at -30 LUFS.
- Record when the Mac is otherwise quiet: a build or a simulator running alongside starves the
  recorder (`DEMO frames take … ms to grab` in the take's log should stay near 10 ms).
