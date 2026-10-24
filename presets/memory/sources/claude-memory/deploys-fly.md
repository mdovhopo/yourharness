---
name: deploys-fly
description: fly deploy from main, never from a branch
metadata:
  type: project
---

Production deploys go through Fly (`fly deploy`), only from `main`. Staging is `fly deploy -a acme-staging`.
**Why:** a branch deploy took prod down on 2026-09-14.
**How to apply:** check the branch before deploying; see [[short-answers]].
