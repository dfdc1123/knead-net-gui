import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const source = readFileSync(
  new URL("../src/lib/components/BreadboardPreview.svelte", import.meta.url),
  "utf8",
);

test("selected pin callout text centers without relying on dominant-baseline", () => {
  assert.match(
    source,
    /<text\s+[^>]*x=\{pinLabel\.x\}[^>]*y=\{pinLabel\.y\}[^>]*dy="0\.35em"[^>]*>/s,
  );

  const calloutText = source.match(
    /<text\s+[^>]*x=\{pinLabel\.x\}[^>]*y=\{pinLabel\.y\}[^>]*>/s,
  )?.[0];
  assert.ok(calloutText);
  assert.doesNotMatch(calloutText, /dominant-baseline=/);
});
