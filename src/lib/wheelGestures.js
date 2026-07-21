const GESTURE_IDLE_MS = 160;
const TRACKPAD_PIXEL_DELTA_LIMIT = 40;
const PINCH_DELTA_LIMIT = 20;
const MIN_ZOOM = 0.5;
const MAX_ZOOM = 3;

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
 * @typedef {object} ZoomWheelEvent
 * @property {number} deltaY
 * @property {number} clientX
 * @property {number} clientY
 */

/** @typedef {(callback: FrameRequestCallback) => number} RequestFrame */
/** @typedef {(handle: number) => void} CancelFrame */

/**
 * @typedef {object} PointerPanOptions
 * @property {RequestFrame} [requestFrame]
 * @property {CancelFrame} [cancelFrame]
 */

/**
 * @typedef {object} PointerPanGesture
 * @property {HTMLDivElement} viewport
 * @property {number} pointerId
 * @property {number} startX
 * @property {number} startY
 * @property {number} startScrollLeft
 * @property {number} startScrollTop
 */

/**
 * @typedef {object} WheelZoomOptions
 * @property {() => number} getZoom
 * @property {(zoom: number) => void} setZoom
 * @property {() => PromiseLike<void> | void} afterRender
 * @property {RequestFrame} [requestFrame]
 * @property {CancelFrame} [cancelFrame]
 */

/**
 * @typedef {object} PendingWheelZoom
 * @property {Exclude<WheelGesture, "pan">} gesture
 * @property {number} deltaY
 * @property {number} clientX
 * @property {number} clientY
 * @property {HTMLDivElement} viewport
 * @property {SVGElement} diagram
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

/**
 * Keep the interaction scale continuous. Rounding belongs in the percentage
 * label; doing it here creates a dead zone for small trackpad pinch deltas.
 * @param {number} zoom
 */
export function clampDiagramZoom(zoom) {
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, zoom));
}

/**
 * Coalesce high-rate pointer reports so a drag writes the scroll position at
 * most once per rendered frame.
 * @param {PointerPanOptions} [options]
 */
export function createPointerPanController(options = {}) {
  const requestFrame = options.requestFrame
    ?? globalThis.requestAnimationFrame.bind(globalThis);
  const cancelFrame = options.cancelFrame
    ?? globalThis.cancelAnimationFrame.bind(globalThis);
  /** @type {PointerPanGesture | null} */
  let gesture = null;
  /** @type {{ x: number, y: number } | null} */
  let pendingPoint = null;
  let animationFrame = 0;

  function flush() {
    animationFrame = 0;
    if (!gesture || !pendingPoint) return;
    const { viewport, startX, startY, startScrollLeft, startScrollTop } = gesture;
    const { x, y } = pendingPoint;
    pendingPoint = null;
    viewport.scrollTo({
      left: startScrollLeft - (x - startX),
      top: startScrollTop - (y - startY),
      behavior: "auto",
    });
  }

  function schedule() {
    if (!animationFrame) animationFrame = requestFrame(flush);
  }

  return {
    /** @param {HTMLDivElement} viewport @param {number} pointerId @param {number} x @param {number} y */
    start(viewport, pointerId, x, y) {
      if (animationFrame) cancelFrame(animationFrame);
      animationFrame = 0;
      pendingPoint = null;
      gesture = {
        viewport,
        pointerId,
        startX: x,
        startY: y,
        startScrollLeft: viewport.scrollLeft,
        startScrollTop: viewport.scrollTop,
      };
    },
    /** @param {number} pointerId @param {number} x @param {number} y */
    move(pointerId, x, y) {
      if (!gesture || gesture.pointerId !== pointerId) return false;
      pendingPoint = { x, y };
      schedule();
      return true;
    },
    /** @param {number} pointerId */
    stop(pointerId) {
      if (!gesture || gesture.pointerId !== pointerId) return false;
      if (animationFrame) cancelFrame(animationFrame);
      animationFrame = 0;
      flush();
      gesture = null;
      return true;
    },
    /** @param {number} pointerId */
    isActive(pointerId) {
      return gesture?.pointerId === pointerId;
    },
    destroy() {
      if (animationFrame) cancelFrame(animationFrame);
      animationFrame = 0;
      pendingPoint = null;
      gesture = null;
    },
  };
}

/**
 * Batch wheel zoom and focal-point correction into one layout update per
 * frame. Events received while Svelte renders are picked up by the next frame.
 * @param {WheelZoomOptions} options
 */
export function createWheelZoomController(options) {
  const { getZoom, setZoom, afterRender } = options;
  const requestFrame = options.requestFrame
    ?? globalThis.requestAnimationFrame.bind(globalThis);
  const cancelFrame = options.cancelFrame
    ?? globalThis.cancelAnimationFrame.bind(globalThis);
  /** @type {PendingWheelZoom | null} */
  let pending = null;
  let animationFrame = 0;
  let rendering = false;
  let destroyed = false;

  function schedule() {
    if (!animationFrame && !rendering && !destroyed) {
      animationFrame = requestFrame(flush);
    }
  }

  async function flush() {
    animationFrame = 0;
    if (!pending || destroyed) return;
    const batch = pending;
    pending = null;
    rendering = true;

    const currentZoom = getZoom();
    const nextZoom = clampDiagramZoom(
      currentZoom * zoomFactorForWheelGesture(batch.gesture, batch.deltaY),
    );
    if (nextZoom !== currentZoom && batch.diagram.isConnected) {
      const before = batch.diagram.getBoundingClientRect();
      if (before.width > 0 && before.height > 0) {
        const focusX = (batch.clientX - before.left) / before.width;
        const focusY = (batch.clientY - before.top) / before.height;
        setZoom(nextZoom);
        await afterRender();

        if (!destroyed && batch.diagram.isConnected) {
          const after = batch.diagram.getBoundingClientRect();
          batch.viewport.scrollTo({
            left: batch.viewport.scrollLeft
              + after.left + focusX * after.width - batch.clientX,
            top: batch.viewport.scrollTop
              + after.top + focusY * after.height - batch.clientY,
            behavior: "auto",
          });
        }
      }
    }

    rendering = false;
    schedule();
  }

  return {
    /**
     * @param {ZoomWheelEvent} event
     * @param {Exclude<WheelGesture, "pan">} gesture
     * @param {HTMLDivElement} viewport
     * @param {SVGElement} diagram
     */
    queue(event, gesture, viewport, diagram) {
      if (event.deltaY === 0 || destroyed) return;
      if (pending && pending.gesture === gesture) {
        // Pinch deltas are continuous and must accumulate. A discrete mouse
        // wheel remains one step per rendered frame regardless of report rate.
        if (gesture === "pinch-zoom") pending.deltaY += event.deltaY;
        else pending.deltaY = event.deltaY;
        pending.clientX = event.clientX;
        pending.clientY = event.clientY;
        pending.viewport = viewport;
        pending.diagram = diagram;
      } else {
        pending = {
          gesture,
          deltaY: event.deltaY,
          clientX: event.clientX,
          clientY: event.clientY,
          viewport,
          diagram,
        };
      }
      schedule();
    },
    destroy() {
      destroyed = true;
      pending = null;
      if (animationFrame) cancelFrame(animationFrame);
      animationFrame = 0;
    },
  };
}
