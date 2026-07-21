import assert from "node:assert/strict";
import test from "node:test";
import {
  clampDiagramZoom,
  createPointerPanController,
  createWheelGestureClassifier,
  createWheelZoomController,
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

test("sub-percent pinch deltas accumulate instead of being rounded away", () => {
  let zoom = 1;
  for (let index = 0; index < 8; index += 1) {
    zoom = clampDiagramZoom(
      zoom * zoomFactorForWheelGesture("pinch-zoom", -0.2),
    );
  }

  assert.ok(zoom > 1.01);
  assert.notEqual(zoom, Math.round(zoom * 100) / 100);
});

test("pointer pan coalesces diagonal movement into one scroll per frame", () => {
  const frames = [];
  const scrolls = [];
  const viewport = {
    scrollLeft: 100,
    scrollTop: 200,
    scrollTo({ left, top }) {
      this.scrollLeft = left;
      this.scrollTop = top;
      scrolls.push({ left, top });
    },
  };
  const pan = createPointerPanController({
    requestFrame(callback) {
      frames.push(callback);
      return frames.length;
    },
    cancelFrame() {},
  });

  pan.start(viewport, 7, 10, 20);
  pan.move(7, 13, 24);
  pan.move(7, 18, 31);

  assert.equal(scrolls.length, 0);
  assert.equal(frames.length, 1);
  frames.shift()();
  assert.deepEqual(scrolls, [{ left: 92, top: 189 }]);
  pan.stop(7);
});

test("pinch zoom batches reports and keeps the focal point stable once per frame", async () => {
  const frames = [];
  const scrolls = [];
  let zoom = 1;
  const viewport = {
    scrollLeft: 50,
    scrollTop: 75,
    scrollTo(position) {
      this.scrollLeft = position.left;
      this.scrollTop = position.top;
      scrolls.push(position);
    },
  };
  const diagram = {
    isConnected: true,
    getBoundingClientRect() {
      return { left: 10, top: 20, width: 200 * zoom, height: 100 * zoom };
    },
  };
  const controller = createWheelZoomController({
    getZoom: () => zoom,
    setZoom: (nextZoom) => (zoom = nextZoom),
    afterRender: async () => {},
    requestFrame(callback) {
      frames.push(callback);
      return frames.length;
    },
    cancelFrame() {},
  });

  controller.queue({ deltaY: -0.2, clientX: 110, clientY: 70 }, "pinch-zoom", viewport, diagram);
  controller.queue({ deltaY: -0.2, clientX: 110, clientY: 70 }, "pinch-zoom", viewport, diagram);

  assert.equal(frames.length, 1);
  assert.equal(scrolls.length, 0);
  await frames.shift()();
  assert.ok(zoom > 1);
  assert.equal(scrolls.length, 1);
  controller.destroy();
});
