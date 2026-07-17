import assert from "node:assert/strict";
import test from "node:test";
import { nextBoardHalfSelection } from "../src/lib/boardHalfSelection.js";

test("disabling the only active half switches to the other half", () => {
  assert.deepEqual(
    nextBoardHalfSelection({ useUpperHalf: true, useLowerHalf: false }, "upper", false),
    { useUpperHalf: false, useLowerHalf: true },
  );
  assert.deepEqual(
    nextBoardHalfSelection({ useUpperHalf: false, useLowerHalf: true }, "lower", false),
    { useUpperHalf: true, useLowerHalf: false },
  );
});

test("disabling either half keeps the other active when both are enabled", () => {
  assert.deepEqual(
    nextBoardHalfSelection({ useUpperHalf: true, useLowerHalf: true }, "upper", false),
    { useUpperHalf: false, useLowerHalf: true },
  );
  assert.deepEqual(
    nextBoardHalfSelection({ useUpperHalf: true, useLowerHalf: true }, "lower", false),
    { useUpperHalf: true, useLowerHalf: false },
  );
});

test("enabling a half preserves the state of the other half", () => {
  assert.deepEqual(
    nextBoardHalfSelection({ useUpperHalf: false, useLowerHalf: true }, "upper", true),
    { useUpperHalf: true, useLowerHalf: true },
  );
  assert.deepEqual(
    nextBoardHalfSelection({ useUpperHalf: true, useLowerHalf: false }, "lower", true),
    { useUpperHalf: true, useLowerHalf: true },
  );
});
