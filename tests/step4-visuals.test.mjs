import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const step4Source = readFileSync(
  new URL("../src/lib/components/Step4Result.svelte", import.meta.url),
  "utf8",
);
const pickerSource = readFileSync(
  new URL("../src/lib/components/SchematicNetPicker.svelte", import.meta.url),
  "utf8",
);
const step1Source = readFileSync(
  new URL("../src/lib/components/Step1SelectFiles.svelte", import.meta.url),
  "utf8",
);

test("schematic canvases keep the KiCad palette on a light theme", () => {
  for (const source of [step1Source, step4Source, pickerSource]) {
    assert.match(
      source,
      /class="[^"]*(?:schematic-host|min-h-0 flex-1 overflow-auto)[^"]*"[\s\S]{0,200}data-theme="nord"/,
    );
  }
});

test("assembly rows preserve the list radius when highlighted", () => {
  assert.equal(step4Source.match(/first:rounded-t-box last:rounded-b-box/g)?.length, 2);
});
