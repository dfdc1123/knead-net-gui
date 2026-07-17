/** Shared drawing geometry for mapping one logical column axis onto physical boards. */

/** @param {number} column @param {number} boardCols */
export function boardIndexForColumn(column, boardCols) {
  return Math.floor(column / Math.max(1, boardCols));
}

/** @param {number} column @param {number} boardCols */
export function localColumnForColumn(column, boardCols) {
  const safeBoardCols = Math.max(1, boardCols);
  return ((column % safeBoardCols) + safeBoardCols) % safeBoardCols;
}

/** @param {number} column @param {number} boardCols */
export function physicalColumnNumber(column, boardCols) {
  return localColumnForColumn(column, boardCols) + 1;
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
 * @param {number} pitch
 * @param {number} xInset
 * @param {number} boardGap
 */
export function globalColumnX(column, boardCols, pitch, xInset, boardGap) {
  const boardIndex = boardIndexForColumn(column, boardCols);
  const localColumn = localColumnForColumn(column, boardCols);
  const boardWidth = physicalBoardWidth(boardCols, pitch, xInset);
  return boardIndex * (boardWidth + boardGap) + xInset + localColumn * pitch;
}

/**
 * Global power-rail columns for one physical board. Rail cadence always restarts locally.
 * @param {"hole170" | "hole400" | "hole800"} preset
 * @param {number} boardCols
 * @param {number} boardIndex
 */
export function railColumnsForBoard(preset, boardCols, boardIndex) {
  if (preset === "hole170") return [];
  const margin = preset === "hole800" ? 2 : 0;
  const boardStart = boardIndex * boardCols;
  const result = [];
  for (let start = margin; start < boardCols - margin; start += 6) {
    for (let offset = 0; offset < 5 && start + offset < boardCols - margin; offset += 1) {
      result.push(boardStart + start + offset);
    }
  }
  return result;
}
