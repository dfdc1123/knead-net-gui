const GESTURE_IDLE_MS = 160;
const TRACKPAD_PIXEL_DELTA_LIMIT = 40;
const PINCH_DELTA_LIMIT = 20;

/** @typedef {"pan" | "pinch-zoom" | "wheel-zoom"} WheelGesture */

/**
 * @typedef {object} WheelGestureEvent
 * @property {boolean} ctrlKey
 * @property {number} deltaMode
 * @property {number} deltaX
 * @property {number} deltaY
 * @property {number} timeStamp
 */

/**
 * Browsers expose trackpad scrolling and mouse wheels through the same event.
 * Classify each burst once so inertial deltas cannot switch behavior mid-gesture.
 *
 * @returns {(event: WheelGestureEvent) => WheelGesture}
 */
export function createWheelGestureClassifier() {
  /** @type {WheelGesture | null} */
  let activeGesture = null;
  let lastTimeStamp = Number.NEGATIVE_INFINITY;

  return (event) => {
    const gestureExpired =
      event.timeStamp < lastTimeStamp ||
      event.timeStamp - lastTimeStamp > GESTURE_IDLE_MS;
    if (gestureExpired) activeGesture = null;
    lastTimeStamp = event.timeStamp;

    // Desktop browsers conventionally encode trackpad pinch as Ctrl + wheel.
    if (event.ctrlKey) {
      activeGesture = "pinch-zoom";
      return activeGesture;
    }

    if (activeGesture) return activeGesture;

    const pixelDelta = event.deltaMode === 0;
    const smoothVerticalDelta =
      Math.abs(event.deltaY) < TRACKPAD_PIXEL_DELTA_LIMIT ||
      !Number.isInteger(event.deltaY);
    activeGesture =
      pixelDelta && (event.deltaX !== 0 || smoothVerticalDelta)
        ? "pan"
        : "wheel-zoom";
    return activeGesture;
  };
}

/**
 * Preserve the existing one-step mouse zoom while making trackpad pinch
 * proportional to the gesture delta. Capping protects Ctrl + mouse-wheel.
 *
 * @param {Exclude<WheelGesture, "pan">} gesture
 * @param {number} deltaY
 */
export function zoomFactorForWheelGesture(gesture, deltaY) {
  if (deltaY === 0) return 1;
  if (gesture === "wheel-zoom") return deltaY < 0 ? 1.15 : 1 / 1.15;

  const limitedDelta = Math.max(-PINCH_DELTA_LIMIT, Math.min(PINCH_DELTA_LIMIT, deltaY));
  return Math.exp(-limitedDelta * 0.01);
}
