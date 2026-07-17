import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  boardIndexForColumn,
  globalColumnX,
  localColumnForColumn,
  physicalColumnNumber,
  railColumnsForBoard,
} from "../src/lib/breadboardGeometry.js";

const source = readFileSync(
  new URL("../src/lib/components/BreadboardPreview.svelte", import.meta.url),
  "utf8",
);

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
  assert.equal(boardIndexForColumn(0, 30), 0);
  assert.equal(boardIndexForColumn(29, 30), 0);
  assert.equal(boardIndexForColumn(30, 30), 1);
  assert.equal(boardIndexForColumn(89, 30), 2);
  assert.equal(localColumnForColumn(30, 30), 0);
  assert.equal(localColumnForColumn(89, 30), 29);
  assert.equal(physicalColumnNumber(29, 30), 30);
  assert.equal(physicalColumnNumber(30, 30), 1);
  assert.equal(physicalColumnNumber(60, 30), 1);

  const lastOnFirst = globalColumnX(29, 30, 12, 18.2, 16);
  const firstOnSecond = globalColumnX(30, 30, 12, 18.2, 16);
  assert.ok(Math.abs(firstOnSecond - lastOnFirst - (16 + 18.2 * 2)) < 1e-9);
});

test("400-hole power rails restart symmetrically on every board", () => {
  const first = railColumnsForBoard("hole400", 30, 0);
  const second = railColumnsForBoard("hole400", 30, 1);

  assert.deepEqual(first.slice(0, 5), [0, 1, 2, 3, 4]);
  assert.deepEqual(first.slice(-5), [24, 25, 26, 27, 28]);
  assert.deepEqual(second.slice(0, 5), [30, 31, 32, 33, 34]);
  assert.deepEqual(second.slice(-5), [54, 55, 56, 57, 58]);
});

test("800-hole power rails keep two local columns clear on both sides of every board", () => {
  const first = railColumnsForBoard("hole800", 63, 0);
  const second = railColumnsForBoard("hole800", 63, 1);

  assert.equal(first[0], 2);
  assert.equal(first.at(-1), 60);
  assert.equal(second[0], 65);
  assert.equal(second.at(-1), 123);
  assert.ok(!second.includes(63));
  assert.ok(!second.includes(64));
  assert.ok(!second.includes(124));
  assert.ok(!second.includes(125));
});
