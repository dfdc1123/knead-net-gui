import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  assemblyNavigationOffset,
  isAssemblyCompletionKey,
  isAuraActivationKey,
} from "../src/lib/keyboardShortcuts.js";

const layoutSource = readFileSync(
  new URL("../src/routes/+layout.svelte", import.meta.url),
  "utf8",
);
const step4Source = readFileSync(
  new URL("../src/lib/components/Step4Result.svelte", import.meta.url),
  "utf8",
);

test("aura actions use only the two requested activation keys", () => {
  assert.equal(isAuraActivationKey("Enter"), true);
  assert.equal(isAuraActivationKey(" "), true);
  assert.equal(isAuraActivationKey("Spacebar"), false);
  assert.equal(isAuraActivationKey("d"), false);
  assert.match(layoutSource, /<svelte:window[\s\S]*onkeydown=\{triggerAuraAction\}/);
});

test("assembly navigation supports arrows and vim directions", () => {
  for (const key of ["ArrowUp", "ArrowLeft", "k", "h"]) {
    assert.equal(assemblyNavigationOffset(key), -1);
  }
  for (const key of ["ArrowDown", "ArrowRight", "j", "l"]) {
    assert.equal(assemblyNavigationOffset(key), 1);
  }
  assert.equal(assemblyNavigationOffset("Enter"), 0);
});

test("assembly completion is intentionally one-way on Enter or d", () => {
  assert.equal(isAssemblyCompletionKey("Enter"), true);
  assert.equal(isAssemblyCompletionKey("d"), true);
  assert.equal(isAssemblyCompletionKey("D"), false);
  assert.equal(isAssemblyCompletionKey(" "), false);
  assert.match(step4Source, /<svelte:window onkeydown=\{handleAssemblyShortcut\}/);
  assert.match(step4Source, /setPartCompleted\(part\.id, true\)/);
  assert.match(step4Source, /setWireCompleted\(wire\.id, true\)/);
});
