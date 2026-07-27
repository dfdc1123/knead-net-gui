import assert from "node:assert/strict";
import test from "node:test";

import { uniqueAssemblyWireForNet } from "../src/lib/selectionMappings.js";

const wire = (id, netId, kind = "routed") => ({
  id,
  net_id: netId,
  kind,
});

test("a net maps to its sole physical assembly wire", () => {
  const onlyWire = wire("wire-1", "VCC");
  assert.equal(
    uniqueAssemblyWireForNet([wire("wire-2", "GND"), onlyWire], "VCC"),
    onlyWire,
  );
});

test("a net with zero or multiple physical wires has no unique assembly match", () => {
  assert.equal(uniqueAssemblyWireForNet([wire("wire-1", "GND")], "VCC"), undefined);
  assert.equal(
    uniqueAssemblyWireForNet([wire("wire-1", "VCC"), wire("wire-2", "VCC")], "VCC"),
    undefined,
  );
});

test("air hints do not prevent a sole physical wire from matching", () => {
  const onlyWire = wire("wire-1", "VCC");
  assert.equal(
    uniqueAssemblyWireForNet([wire("air-1", "VCC", "air"), onlyWire], "VCC"),
    onlyWire,
  );
});
