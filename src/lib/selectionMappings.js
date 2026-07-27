/**
 * Return a physical assembly wire only when a net maps to exactly one wire.
 *
 * @template {{ kind?: string, net_id?: string }} T
 * @param {T[]} wires
 * @param {string} netId
 * @returns {T | undefined}
 */
export function uniqueAssemblyWireForNet(wires, netId) {
  let match;
  for (const wire of wires) {
    if (wire.kind === "air" || wire.net_id !== netId) continue;
    if (match) return undefined;
    match = wire;
  }
  return match;
}
