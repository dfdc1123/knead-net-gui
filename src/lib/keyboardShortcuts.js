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
  return key === "Enter" || key === "d";
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
