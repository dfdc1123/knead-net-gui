import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const tauriManifest = readFileSync(
  new URL("../src-tauri/Cargo.toml", import.meta.url),
  "utf8",
);
const tauriSource = readFileSync(
  new URL("../src-tauri/src/lib.rs", import.meta.url),
  "utf8",
);

test("Linux replaces WebKitGTK page magnification with diagram wheel events", () => {
  assert.match(tauriManifest, /target\.'cfg\(target_os = "linux"\)'\.dependencies/);
  assert.match(tauriSource, /wk-view-zoom-gesture/);
  assert.match(tauriSource, /PropagationPhase::None/);
  assert.match(tauriSource, /GestureZoom::new/);
  assert.match(tauriSource, /new WheelEvent\('wheel'/);
  assert.match(tauriSource, /cfg\(target_os = "linux"\)[\s\S]*linux_pinch_zoom_plugin/);
});
