import { test } from "node:test";
import assert from "node:assert/strict";
import { streak } from "../src/streak.js";

test("counts consecutive days back from today", () => {
  assert.equal(streak(["2026-11-09", "2026-11-10", "2026-11-11"], "2026-11-11"), 3);
});

test("is zero when today isn't done", () => {
  assert.equal(streak(["2026-11-09", "2026-11-10"], "2026-11-11"), 0);
});
