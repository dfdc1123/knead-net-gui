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
const step2Source = readFileSync(
  new URL("../src/lib/components/Step2SelectBoard.svelte", import.meta.url),
  "utf8",
);
const step3Source = readFileSync(
  new URL("../src/lib/components/Step3Compute.svelte", import.meta.url),
  "utf8",
);
const panelSource = readFileSync(
  new URL("../src/lib/components/Panel.svelte", import.meta.url),
  "utf8",
);
const appCssSource = readFileSync(
  new URL("../src/app.css", import.meta.url),
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

test("interactive selections use the shared purple highlight while warnings keep warning semantics", () => {
  assert.match(appCssSource, /--color-highlight: oklch\(74\.229% 0\.133 311\.379\)/);
  assert.match(step4Source, /selected \? 'border border-accent\/40 bg-accent\/10'/);
  assert.equal(step4Source.match(/selection-ring/g)?.length, 2);
  assert.match(step4Source, /drop-shadow\(0 0 5px var\(--color-highlight\)\)/);
  assert.match(
    step4Source,
    /\.sch-component\.is-selected \.sch-component-hit[\s\S]*fill: var\(--color-highlight\)[\s\S]*stroke: var\(--color-highlight\)/,
  );
  assert.match(pickerSource, /\.sch-net-line\.is-selected[\s\S]*stroke: var\(--color-highlight\)/);
  assert.match(breadboardSource, /internalConnectionHighlights[\s\S]*stroke="var\(--color-highlight\)"/);
  assert.match(pickerSource, /alert alert-warning/);
});

test("workflow pages share the same outer spacing", () => {
  const pageClass =
    'class="mx-auto flex h-full min-h-0 w-full max-w-[1920px] flex-col gap-4 overflow-hidden p-6"';
  for (const source of [step1Source, step2Source, step3Source, step4Source]) {
    assert.ok(source.includes(pageClass));
  }
});

test("badges use stable metadata, summary, selection, and state semantics", () => {
  const workflowSources = [step1Source, step2Source, step3Source, step4Source, pickerSource];
  for (const source of workflowSources) {
    assert.doesNotMatch(source, /badge-(?:neutral|secondary)/);
  }
  assert.match(step1Source, /badge badge-ghost badge-sm">\{projects\.length\}/);
  assert.match(step2Source, /badge badge-ghost badge-sm">\{ui\.step2\.columns/);
  assert.match(pickerSource, /badge badge-accent max-w-full/);
  assert.equal(step4Source.match(/\? 'badge-success' : 'badge-outline'/g)?.length, 3);
});

test("workflow cards share the DaisyUI panel wrapper", () => {
  assert.match(panelSource, /card min-h-0 border border-base-300 bg-base-100 shadow-sm/);
  for (const source of [step1Source, step2Source, step3Source, step4Source]) {
    assert.match(source, /import Panel from "\.\/Panel\.svelte"/);
    assert.doesNotMatch(
      source,
      /card min-h-0[^"\n]*border border-base-300 bg-base-100 shadow-sm/,
    );
  }
});

test("workflow pages omit redundant helper copy and board metadata", () => {
  assert.doesNotMatch(step1Source, /ui\.step1\.subtitle/);
  assert.doesNotMatch(step2Source, /autoBoardHint|powerRailHint|withRails|withoutRails/);
  assert.doesNotMatch(step4Source, /ui\.step4\.subtitle|ui\.step4\.(?:boards|columns)/);
});
