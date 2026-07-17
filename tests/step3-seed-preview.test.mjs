import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { isBetterSeedCost } from "../src/lib/seedPreview.js";

const source = readFileSync(
  new URL("../src/lib/components/Step3Compute.svelte", import.meta.url),
  "utf8",
);

test("completed seeds replace the preview only when their cost improves", () => {
  assert.equal(isBetterSeedCost(null, 120), true);
  assert.equal(isBetterSeedCost(120, 119), true);
  assert.equal(isBetterSeedCost(120, 120), false);
  assert.equal(isBetterSeedCost(120, 121), false);
  assert.equal(isBetterSeedCost(120, Number.NaN), false);
});

test("Step 3 shows that remaining seeds are still searching", () => {
  assert.match(source, /loading loading-dots loading-sm/);
  assert.match(source, /ui\.step3\.remainingSeeds/);
  assert.match(source, /seed_result/);
});
