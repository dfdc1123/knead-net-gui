/** @typedef {"upper" | "full" | "lower"} BoardHalfMode */

/** @param {BoardHalfMode} mode */
export function selectionForBoardHalfMode(mode) {
  return {
    useUpperHalf: mode !== "lower",
    useLowerHalf: mode !== "upper",
  };
}

/**
 * Clicking the selected single half expands back to the full board.
 * Otherwise the clicked physical half becomes the single active half.
 *
 * @param {BoardHalfMode} currentMode
 * @param {"upper" | "lower"} half
 * @returns {BoardHalfMode}
 */
export function modeAfterHalfClick(currentMode, half) {
  return currentMode === half ? "full" : half;
}
