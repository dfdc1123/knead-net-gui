import assert from "node:assert/strict";
import test from "node:test";
import { createServer } from "vite";

test("Tailwind skips Svelte virtual style modules in development", async () => {
  const server = await createServer({
    configFile: new URL("../vite.config.ts", import.meta.url).pathname,
    server: { middlewareMode: true, hmr: false },
  });

  try {
    await assert.doesNotReject(() =>
      server.transformRequest(
        "/src/lib/components/SchematicNetPicker.svelte?svelte&type=style&lang.css",
      ),
    );
  } finally {
    await server.close();
  }
});
