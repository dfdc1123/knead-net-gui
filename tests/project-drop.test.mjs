import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { projectTargetFromDrop } from "../src/lib/projectDrop.js";

const step1Source = readFileSync(
  new URL("../src/lib/components/Step1SelectFiles.svelte", import.meta.url),
  "utf8",
);

test("project import listens for native drag/drop and shows a drop target", () => {
  assert.match(step1Source, /onDragDropEvent/);
  assert.match(step1Source, /importDroppedPaths\(event\.payload\.paths\)/);
  assert.match(step1Source, /\{ui\.step1\.dropHere\}/);
});

test("dropping a KiCad file loads its folder and selects its project", () => {
  assert.deepEqual(
    projectTargetFromDrop(["/projects/blinker/blinker.kicad_pcb"]),
    { folder: "/projects/blinker", preferredProject: "blinker" },
  );
  assert.deepEqual(
    projectTargetFromDrop(["C:\\projects\\timer\\timer.KICAD_SCH"]),
    { folder: "C:\\projects\\timer", preferredProject: "timer" },
  );
  assert.deepEqual(projectTargetFromDrop(["C:\\root.kicad_pcb"]), {
    folder: "C:\\",
    preferredProject: "root",
  });
});

test("a supported file takes precedence when several paths are dropped", () => {
  assert.deepEqual(
    projectTargetFromDrop([
      "/projects/blinker/readme.txt",
      "/projects/blinker/blinker.kicad_sch",
    ]),
    { folder: "/projects/blinker", preferredProject: "blinker" },
  );
});

test("dropping one non-KiCad path treats it as a project folder", () => {
  assert.deepEqual(projectTargetFromDrop(["/projects/blinker"]), {
    folder: "/projects/blinker",
    preferredProject: null,
  });
});

test("empty and ambiguous unsupported drops are rejected", () => {
  assert.equal(projectTargetFromDrop([]), null);
  assert.equal(
    projectTargetFromDrop(["/projects/a.txt", "/projects/b.txt"]),
    null,
  );
});
