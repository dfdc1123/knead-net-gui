import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const tauriSource = readFileSync(
  new URL("../src-tauri/src/lib.rs", import.meta.url),
  "utf8",
);
const layoutSource = readFileSync(
  new URL("../src/routes/+layout.svelte", import.meta.url),
  "utf8",
);

test("Windows enables WebView2 pinch input without browser zoom hotkeys", () => {
  assert.match(tauriSource, /SetIsPinchZoomEnabled\(true\)/);
  assert.match(tauriSource, /SetIsZoomControlEnabled\(false\)/);
  assert.match(tauriSource, /cfg\(windows\)[\s\S]*windows_pinch_zoom_plugin/);
});

test("pinch outside a diagram cannot page-scale the whole application", () => {
  assert.match(layoutSource, /function preventBrowserPinchZoom\(event: WheelEvent\)/);
  assert.match(layoutSource, /if \(event\.ctrlKey\) event\.preventDefault\(\)/);
  assert.match(layoutSource, /<svelte:window[^>]*onwheel=\{preventBrowserPinchZoom\}/);
});
