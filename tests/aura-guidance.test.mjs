import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const dockSource = readFileSync(
  new URL("../src/lib/components/Dock.svelte", import.meta.url),
  "utf8",
);
const step1Source = readFileSync(
  new URL("../src/lib/components/Step1SelectFiles.svelte", import.meta.url),
  "utf8",
);
const appCss = readFileSync(new URL("../src/app.css", import.meta.url), "utf8");

test("the initial project-folder action receives workflow guidance", () => {
  assert.match(
    step1Source,
    /class:aura=\{!folder\}[\s\S]*class:aura-sm=\{!folder\}[\s\S]*class:workflow-next-step=\{!folder\}/,
  );
  assert.match(step1Source, /class:workflow-next-step=\{!folder\}[\s\S]*class="btn btn-primary btn-sm btn-block"/);
});

test("dock guidance uses the same radius as its highlighted control", () => {
  assert.match(dockSource, /class="aura aura-sm workflow-next-step/);
  assert.match(appCss, /\.workflow-next-step\s*\{[\s\S]*--aura-radius:\s*var\(--radius-field\)/);
  assert.match(appCss, /\.workflow-next-step\s*\{[\s\S]*isolation:\s*isolate/);
});
