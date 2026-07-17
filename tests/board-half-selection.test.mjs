import assert from "node:assert/strict";
import test from "node:test";
import {
  modeAfterHalfClick,
  selectionForBoardHalfMode,
} from "../src/lib/boardHalfSelection.js";

test("three board modes map to the only three valid half selections", () => {
  assert.deepEqual(selectionForBoardHalfMode("upper"), {
    useUpperHalf: true,
    useLowerHalf: false,
  });
  assert.deepEqual(selectionForBoardHalfMode("full"), {
    useUpperHalf: true,
    useLowerHalf: true,
  });
  assert.deepEqual(selectionForBoardHalfMode("lower"), {
    useUpperHalf: false,
    useLowerHalf: true,
  });
});

test("clicking a preview half selects it or expands it back to full", () => {
  assert.equal(modeAfterHalfClick("full", "upper"), "upper");
  assert.equal(modeAfterHalfClick("lower", "upper"), "upper");
  assert.equal(modeAfterHalfClick("upper", "upper"), "full");
  assert.equal(modeAfterHalfClick("full", "lower"), "lower");
  assert.equal(modeAfterHalfClick("upper", "lower"), "lower");
  assert.equal(modeAfterHalfClick("lower", "lower"), "full");
});
