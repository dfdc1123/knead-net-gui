/** @param {string} key */
export function isAuraActivationKey(key) {
  return key === "Enter" || key === " ";
}

/** @param {string} key */
export function assemblyNavigationOffset(key) {
  if (["ArrowUp", "ArrowLeft", "k", "h"].includes(key)) return -1;
  if (["ArrowDown", "ArrowRight", "j", "l"].includes(key)) return 1;
  return 0;
}

/** @param {string} key */
export function isAssemblyCompletionKey(key) {
  return key === "Enter" || key === " " || key === "d";
}

/** @typedef {"components" | "wires" | "last"} AssemblyDirectJump */

/**
 * @param {{ altKey: boolean, ctrlKey: boolean, key: string, metaKey: boolean, shiftKey: boolean }} event
 * @returns {AssemblyDirectJump | null}
 */
export function assemblyDirectJumpCommand(event) {
  if (event.altKey || event.metaKey) return null;
  if (event.ctrlKey && !event.shiftKey) {
    if (event.key === "b") return "components";
    if (event.key === "f") return "wires";
    return null;
  }
  if (!event.ctrlKey && event.key === "G") return "last";
  return null;
}

/** @typedef {"pending" | "trigger" | null} VimStartSequenceResult */

/**
 * @param {number} [timeoutMs]
 * @returns {(key: string, timeStamp: number) => VimStartSequenceResult}
 */
export function createVimStartSequence(timeoutMs = 600) {
  let firstGTime = Number.NEGATIVE_INFINITY;

  return (key, timeStamp) => {
    if (key !== "g") {
      firstGTime = Number.NEGATIVE_INFINITY;
      return null;
    }
    if (timeStamp >= firstGTime && timeStamp - firstGTime <= timeoutMs) {
      firstGTime = Number.NEGATIVE_INFINITY;
      return "trigger";
    }
    firstGTime = timeStamp;
    return "pending";
  };
}

/** @param {{ altKey: boolean, ctrlKey: boolean, metaKey: boolean, shiftKey: boolean }} event */
export function hasShortcutModifier(event) {
  return event.altKey || event.ctrlKey || event.metaKey || event.shiftKey;
}

/** @param {EventTarget | null} target */
export function isTextEditingTarget(target) {
  if (!(target instanceof Element)) return false;
  return Boolean(
    target.closest(
      "input, textarea, select, [contenteditable]:not([contenteditable='false'])",
    ),
  );
}
