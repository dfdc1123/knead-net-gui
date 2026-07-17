import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  boardIndexForColumn,
  globalColumnX,
  interBoardGapColumns,
  localColumnForColumn,
  physicalColumnNumber,
  railColumnsForBoard,
  visualBoardGap,
} from "../src/lib/breadboardGeometry.js";

const source = readFileSync(
  new URL("../src/lib/components/BreadboardPreview.svelte", import.meta.url),
  "utf8",
);
const step2Source = readFileSync(
  new URL("../src/lib/components/Step2SelectBoard.svelte", import.meta.url),
  "utf8",
);

test("Step 2 fits every preset against the 830-hole board footprint", () => {
  assert.match(step2Source, /fitReference=\{STEP2_FIT_REFERENCE\}/);
});

test("breadboard canvas prevents native text selection on double click", () => {
  assert.match(source, /class="[^"]*select-none[^"]*"[\s\S]*role="presentation"/);
});

test("selected pin callout text centers without relying on dominant-baseline", () => {
  assert.match(
    source,
    /<text\s+[^>]*x=\{pinLabel\.x\}[^>]*y=\{pinLabel\.y\}[^>]*dy="0\.35em"[^>]*>/s,
  );

  const calloutText = source.match(
    /<text\s+[^>]*x=\{pinLabel\.x\}[^>]*y=\{pinLabel\.y\}[^>]*>/s,
  )?.[0];
  assert.ok(calloutText);
  assert.doesNotMatch(calloutText, /dominant-baseline=/);
});

test("global columns map to consecutive physical breadboards", () => {
  assert.equal(boardIndexForColumn(0, 30, 3), 0);
  assert.equal(boardIndexForColumn(29, 30, 3), 0);
  assert.equal(boardIndexForColumn(33, 30, 3), 1);
  assert.equal(boardIndexForColumn(95, 30, 3), 2);
  assert.equal(localColumnForColumn(33, 30, 3), 0);
  assert.equal(localColumnForColumn(95, 30, 3), 29);
  assert.equal(physicalColumnNumber(29, 30, 3), 30);
  assert.equal(physicalColumnNumber(33, 30, 3), 1);
  assert.equal(physicalColumnNumber(66, 30, 3), 1);

  const boardGap = visualBoardGap(3);
  const lastOnFirst = globalColumnX(29, 30, 3, 12, 18.2, boardGap);
  const firstOnSecond = globalColumnX(33, 30, 3, 12, 18.2, boardGap);
  assert.ok(Math.abs(firstOnSecond - lastOnFirst - (boardGap + 18.2 * 2)) < 1e-9);
});

test("breadboard presets expose their physical inter-board spacing", () => {
  assert.equal(interBoardGapColumns("hole170"), 2);
  assert.equal(interBoardGapColumns("hole400"), 3);
  assert.equal(interBoardGapColumns("hole830"), 3);

  assert.equal(boardIndexForColumn(16, 17, 2), 0);
  assert.equal(boardIndexForColumn(19, 17, 2), 1);
  assert.equal(localColumnForColumn(19, 17, 2), 0);
  assert.equal(physicalColumnNumber(19, 17, 2), 1);
  assert.equal(visualBoardGap(2), 8);
  assert.equal(visualBoardGap(3), 12);
});

test("400-hole power rails restart symmetrically on every board", () => {
  const first = railColumnsForBoard("hole400", 30, 3, 0);
  const second = railColumnsForBoard("hole400", 30, 3, 1);

  assert.deepEqual(first.slice(0, 5), [0, 1, 2, 3, 4]);
  assert.deepEqual(first.slice(-5), [24, 25, 26, 27, 28]);
  assert.deepEqual(second.slice(0, 5), [33, 34, 35, 36, 37]);
  assert.deepEqual(second.slice(-5), [57, 58, 59, 60, 61]);
});

test("800-hole power rails keep two local columns clear on both sides of every board", () => {
  const first = railColumnsForBoard("hole830", 63, 3, 0);
  const second = railColumnsForBoard("hole830", 63, 3, 1);

  assert.equal(first[0], 2);
  assert.equal(first.at(-1), 60);
  assert.equal(second[0], 68);
  assert.equal(second.at(-1), 126);
  assert.ok(!second.includes(63));
  assert.ok(!second.includes(64));
  assert.ok(!second.includes(65));
  assert.ok(!second.includes(66));
  assert.ok(!second.includes(67));
  assert.ok(!second.includes(127));
  assert.ok(!second.includes(128));
});
