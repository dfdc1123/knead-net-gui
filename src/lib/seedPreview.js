/**
 * Keep the preview stable unless a completed seed actually improves the best cost.
 *
 * @param {number | null} currentCost
 * @param {number} candidateCost
 */
export function isBetterSeedCost(currentCost, candidateCost) {
  return (
    Number.isFinite(candidateCost) &&
    (currentCost === null || !Number.isFinite(currentCost) || candidateCost < currentCost)
  );
}
