/** Shared drawing geometry for mapping one logical column axis onto physical boards. */

export const INTER_BOARD_GAP_COLS = 3;

/** @param {number} column @param {number} boardCols @param {number} gapCols */
export function boardIndexForColumn(column, boardCols, gapCols = INTER_BOARD_GAP_COLS) {
  return Math.floor(column / Math.max(1, boardCols + gapCols));
}

/** @param {number} column @param {number} boardCols @param {number} gapCols */
export function localColumnForColumn(column, boardCols, gapCols = INTER_BOARD_GAP_COLS) {
  const stride = Math.max(1, boardCols + gapCols);
  return ((column % stride) + stride) % stride;
}

/** @param {number} column @param {number} boardCols @param {number} gapCols */
export function physicalColumnNumber(column, boardCols, gapCols = INTER_BOARD_GAP_COLS) {
  return localColumnForColumn(column, boardCols, gapCols) + 1;
}

/**
 * @param {number} boardCols
 * @param {number} pitch
 * @param {number} xInset
 */
export function physicalBoardWidth(boardCols, pitch, xInset) {
  return xInset * 2 + Math.max(0, boardCols - 1) * pitch;
}

/**
 * @param {number} column
 * @param {number} boardCols
 * @param {number} gapCols
 * @param {number} pitch
 * @param {number} xInset
 * @param {number} boardGap
 */
export function globalColumnX(column, boardCols, gapCols, pitch, xInset, boardGap) {
  const boardIndex = boardIndexForColumn(column, boardCols, gapCols);
  const localColumn = localColumnForColumn(column, boardCols, gapCols);
  const boardWidth = physicalBoardWidth(boardCols, pitch, xInset);
  return boardIndex * (boardWidth + boardGap) + xInset + localColumn * pitch;
}

/**
 * Global power-rail columns for one physical board. Rail cadence always restarts locally.
 * @param {"hole170" | "hole400" | "hole800"} preset
 * @param {number} boardCols
 * @param {number} gapCols
 * @param {number} boardIndex
 */
export function railColumnsForBoard(preset, boardCols, gapCols, boardIndex) {
  if (preset === "hole170") return [];
  const margin = preset === "hole800" ? 2 : 0;
  const boardStart = boardIndex * (boardCols + gapCols);
  const result = [];
  for (let start = margin; start < boardCols - margin; start += 6) {
    for (let offset = 0; offset < 5 && start + offset < boardCols - margin; offset += 1) {
      result.push(boardStart + start + offset);
    }
  }
  return result;
}
