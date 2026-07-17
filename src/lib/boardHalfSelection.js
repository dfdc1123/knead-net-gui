/**
 * Resolve a half-board toggle without ever leaving both halves disabled.
 * Disabling the only active half switches directly to the other half.
 *
 * @param {{ useUpperHalf: boolean, useLowerHalf: boolean }} current
 * @param {"upper" | "lower"} half
 * @param {boolean} enabled
 */
export function nextBoardHalfSelection(current, half, enabled) {
  if (half === "upper") {
    return !enabled && !current.useLowerHalf
      ? { useUpperHalf: false, useLowerHalf: true }
      : { ...current, useUpperHalf: enabled };
  }

  return !enabled && !current.useUpperHalf
    ? { useUpperHalf: true, useLowerHalf: false }
    : { ...current, useLowerHalf: enabled };
}
