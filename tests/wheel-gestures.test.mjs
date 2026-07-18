import assert from "node:assert/strict";
import test from "node:test";
import {
  createWheelGestureClassifier,
  zoomFactorForWheelGesture,
} from "../src/lib/wheelGestures.js";

function wheel(overrides = {}) {
  return {
    ctrlKey: false,
    deltaMode: 0,
    deltaX: 0,
    deltaY: 0,
    timeStamp: 0,
    ...overrides,
  };
}

test("trackpad pinch and two-finger scrolling map to separate gestures", () => {
  const classify = createWheelGestureClassifier();

  assert.equal(classify(wheel({ ctrlKey: true, deltaY: -3 })), "pinch-zoom");
  assert.equal(
    classify(wheel({ deltaX: 4.25, deltaY: 8.5, timeStamp: 200 })),
    "pan",
  );
});

test("discrete mouse wheel input keeps zoom behavior", () => {
  const classify = createWheelGestureClassifier();

  assert.equal(classify(wheel({ deltaMode: 1, deltaY: 3 })), "wheel-zoom");
  assert.equal(classify(wheel({ deltaY: 100, timeStamp: 200 })), "wheel-zoom");
});

test("a gesture stays classified while its delta shape changes", () => {
  const classify = createWheelGestureClassifier();

  assert.equal(classify(wheel({ deltaY: 2.5 })), "pan");
  assert.equal(classify(wheel({ deltaY: 80, timeStamp: 40 })), "pan");

  assert.equal(classify(wheel({ deltaY: 100, timeStamp: 240 })), "wheel-zoom");
  assert.equal(classify(wheel({ deltaY: 4, timeStamp: 280 })), "wheel-zoom");
});

test("pinch zoom overrides an active two-finger pan gesture", () => {
  const classify = createWheelGestureClassifier();

  assert.equal(classify(wheel({ deltaX: 2, deltaY: 5 })), "pan");
  assert.equal(
    classify(wheel({ ctrlKey: true, deltaY: -2, timeStamp: 30 })),
    "pinch-zoom",
  );
});

test("mouse zoom stays stepped while trackpad pinch scales continuously", () => {
  assert.equal(zoomFactorForWheelGesture("wheel-zoom", -100), 1.15);
  assert.equal(zoomFactorForWheelGesture("wheel-zoom", 100), 1 / 1.15);
  assert.ok(zoomFactorForWheelGesture("pinch-zoom", -2) > 1);
  assert.ok(zoomFactorForWheelGesture("pinch-zoom", 2) < 1);
  assert.equal(
    zoomFactorForWheelGesture("pinch-zoom", -100),
    zoomFactorForWheelGesture("pinch-zoom", -20),
  );
});
