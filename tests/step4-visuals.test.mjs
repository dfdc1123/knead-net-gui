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
const breadboardSource = readFileSync(
  new URL("../src/lib/components/BreadboardPreview.svelte", import.meta.url),
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

test("linked selections shrink either diagram when their bounds exceed the viewport", () => {
  assert.match(step4Source, /async function fitDiagramSelection\(target: DiagramTarget\)/);
  assert.match(step4Source, /if \(source === "schematic"\) await fitDiagramSelection\("breadboard"\)/);
  assert.match(step4Source, /if \(source === "breadboard"\) await fitDiagramSelection\("schematic"\)/);
});

test("interactive selections use accent while warnings keep warning semantics", () => {
  assert.match(step4Source, /selected \? 'border border-accent\/40 bg-accent\/10'/);
  assert.equal(step4Source.match(/ring-accent/g)?.length, 2);
  assert.match(step4Source, /drop-shadow\(0 0 5px var\(--color-accent\)\)/);
  assert.match(pickerSource, /\.sch-net-line\.is-selected[\s\S]*stroke: var\(--color-accent\)/);
  assert.match(breadboardSource, /internalConnectionHighlights[\s\S]*stroke="var\(--color-accent\)"/);
  assert.match(pickerSource, /alert alert-warning/);
});
