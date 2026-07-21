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

test("Linux forwards every native pinch sample directly to the webview", () => {
  const scaleChangedHandler = tauriSource.match(
    /gesture\.connect_scale_changed[\s\S]*?\n\s*\}\);/,
  );
  assert.ok(scaleChangedHandler);
  assert.match(scaleChangedHandler[0], /script_webview\.eval/);
  assert.doesNotMatch(tauriSource, /add_tick_callback/);
  assert.doesNotMatch(tauriSource, /LinuxPinchBatch/);
});
