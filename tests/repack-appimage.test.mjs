import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";

const script = path.resolve("scripts/repack-appimage.mjs");
const excludedLibraries = [
  "libwayland-client.so.0",
  "libwayland-cursor.so.0",
  "libwayland-egl.so.1",
  "libwayland-server.so.0",
];

test("repacked AppImage excludes bundled Wayland libraries", () => {
  const directory = mkdtempSync(path.join(tmpdir(), "kneadnet-repack-test-"));
  try {
    const bundleDirectory = path.join(directory, "bundle");
    const appDir = path.join(bundleDirectory, "kneadnet.AppDir");
    const libraryDirectory = path.join(appDir, "usr", "lib");
    const output = path.join(bundleDirectory, "KneadNet_0.0.0_amd64.AppImage");
    const packager = path.join(directory, "fake-packager.mjs");
    mkdirSync(libraryDirectory, { recursive: true });
    for (const name of excludedLibraries) writeFileSync(path.join(libraryDirectory, name), "bundled");
    writeFileSync(path.join(libraryDirectory, "libkeep.so.1"), "keep");
    writeFileSync(output, "old image");
    writeFileSync(
      packager,
      `#!/usr/bin/env node
import { chmodSync, writeFileSync } from "node:fs";
const appDir = process.argv.find((value) => value.startsWith("--appdir=")).slice(9);
writeFileSync(process.env.LDAI_OUTPUT, \`#!/bin/sh
set -eu
mkdir -p squashfs-root
cp -R \${JSON.stringify(appDir)}/. squashfs-root/
\`);
chmodSync(process.env.LDAI_OUTPUT, 0o755);
`,
    );
    chmodSync(packager, 0o755);

    const stdout = execFileSync(
      process.execPath,
      [script, "--bundle-dir", bundleDirectory, "--packager", packager],
      { encoding: "utf8" },
    );

    assert.match(stdout, /without bundled Wayland libraries/);
    for (const name of excludedLibraries) {
      assert.throws(() => readFileSync(path.join(libraryDirectory, name)));
    }
    assert.equal(readFileSync(path.join(libraryDirectory, "libkeep.so.1"), "utf8"), "keep");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
