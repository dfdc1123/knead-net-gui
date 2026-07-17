import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const pageSource = readFileSync(new URL("../src/routes/+page.svelte", import.meta.url), "utf8");
const step2Source = readFileSync(
  new URL("../src/lib/components/Step2SelectBoard.svelte", import.meta.url),
  "utf8",
);
const pickerUrl = new URL("../src/lib/components/SchematicNetPicker.svelte", import.meta.url);

test("Step 2 receives the rendered schematic and uses a modal net picker", () => {
  assert.match(pageSource, /<Step2SelectBoard[^>]*\{schematicSvg\}/s);
  assert.match(step2Source, /<SchematicNetPicker/);
  assert.doesNotMatch(step2Source, /<select[\s\S]*top-negative-power-net/);
});

test("schematic picker uses the recommended dialog modal and selects only network hits", () => {
  const pickerSource = readFileSync(pickerUrl, "utf8");
  assert.match(pickerSource, /<dialog[^>]*class="modal"/);
  assert.match(pickerSource, /closest<SVGElement>\("\[data-net\]"\)/);
  assert.match(pickerSource, /allowedNetNames\.includes\(net\)/);
});
