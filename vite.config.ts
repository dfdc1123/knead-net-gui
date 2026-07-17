import { defineConfig } from "vite";
import { sveltekit } from "@sveltejs/kit/vite";
import tailwindcss from "@tailwindcss/vite";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

const tailwindPlugins = tailwindcss();
const tailwindServePlugin = tailwindPlugins.find(
  (plugin) => plugin.name === "@tailwindcss/vite:generate:serve",
);

if (
  tailwindServePlugin?.transform &&
  typeof tailwindServePlugin.transform === "object"
) {
  const transform = tailwindServePlugin.transform;
  const originalHandler = transform.handler;
  transform.handler = function (code, id, ...args) {
    // Svelte owns these virtual CSS modules. Tailwind's pre-transform can see
    // the uncompiled component source here and try to parse its script as CSS.
    if (id.includes(".svelte?svelte&type=style")) return;
    return originalHandler.call(this, code, id, ...args);
  };
}

// https://vite.dev/config/
export default defineConfig({
  plugins: [sveltekit(), tailwindPlugins],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host ?? false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
});
