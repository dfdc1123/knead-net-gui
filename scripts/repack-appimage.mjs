#!/usr/bin/env node

import { existsSync, mkdtempSync, readdirSync, rmSync, statSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import path from "node:path";

const root = fileURLToPath(new URL("../", import.meta.url));
const excludedLibraries = [
  "libwayland-client.so.0",
  "libwayland-cursor.so.0",
  "libwayland-egl.so.1",
  "libwayland-server.so.0",
];

function option(name, fallback) {
  const index = process.argv.indexOf(name);
  if (index === -1) return fallback;
  const value = process.argv[index + 1];
  if (!value || value.startsWith("--")) throw new Error(`${name} requires a value`);
  return value;
}

function selectOne(directory, label, predicate) {
  const matches = readdirSync(directory)
    .filter(predicate)
    .map((name) => path.join(directory, name));
  if (matches.length !== 1) {
    throw new Error(`${label}: expected exactly one entry, found ${matches.length}\n${matches.join("\n")}`);
  }
  return matches[0];
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { encoding: "utf8", ...options });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(
      `${command} failed with status ${result.status}\n${result.stdout ?? ""}${result.stderr ?? ""}`.trim(),
    );
  }
  return result;
}

function assertLibrariesAbsent(appDir) {
  const libraryDirectory = path.join(appDir, "usr", "lib");
  const present = excludedLibraries.filter((name) => existsSync(path.join(libraryDirectory, name)));
  if (present.length > 0) {
    throw new Error(`AppImage still bundles host Wayland libraries:\n${present.join("\n")}`);
  }
}

try {
  const bundleDirectory = path.resolve(
    root,
    option("--bundle-dir", "target/release/bundle/appimage"),
  );
  const appDir = selectOne(bundleDirectory, "AppDir", (name) => name.endsWith(".AppDir"));
  const output = selectOne(bundleDirectory, "AppImage", (name) => name.endsWith(".AppImage"));
  const packager = path.resolve(
    option(
      "--packager",
      path.join(homedir(), ".cache", "tauri", "linuxdeploy-plugin-appimage.AppImage"),
    ),
  );

  if (!existsSync(packager)) throw new Error(`AppImage packager does not exist: ${packager}`);

  const libraryDirectory = path.join(appDir, "usr", "lib");
  for (const name of excludedLibraries) {
    rmSync(path.join(libraryDirectory, name), { force: true });
  }
  assertLibrariesAbsent(appDir);

  rmSync(output, { force: true });
  run(packager, ["--appimage-extract-and-run", `--appdir=${appDir}`], {
    cwd: bundleDirectory,
    env: { ...process.env, ARCH: "x86_64", LDAI_OUTPUT: output },
  });

  if (!existsSync(output) || !statSync(output).isFile() || statSync(output).size === 0) {
    throw new Error(`Repacked AppImage is missing or empty: ${output}`);
  }

  const inspectionDirectory = mkdtempSync(path.join(tmpdir(), "kneadnet-appimage-"));
  try {
    run(output, ["--appimage-extract"], { cwd: inspectionDirectory });
    assertLibrariesAbsent(path.join(inspectionDirectory, "squashfs-root"));
  } finally {
    rmSync(inspectionDirectory, { recursive: true, force: true });
  }

  console.log(`Repacked ${output} without bundled Wayland libraries.`);
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
