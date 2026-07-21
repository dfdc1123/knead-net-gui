#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const root = fileURLToPath(new URL("../", import.meta.url));

function option(name, fallback) {
  const index = process.argv.indexOf(name);
  if (index === -1) return fallback;
  const value = process.argv[index + 1];
  if (!value || value.startsWith("--")) throw new Error(`${name} requires a value`);
  return value;
}

try {
  const { version } = JSON.parse(readFileSync(path.join(root, "package.json"), "utf8"));
  const outputDirectory = path.resolve(root, option("--out", "release-assets"));
  const archiveName = `KneadNet-examples-${version}.zip`;
  const archivePath = path.join(outputDirectory, archiveName);
  const prefix = `KneadNet-examples-${version}/`;

  mkdirSync(outputDirectory, { recursive: true });
  execFileSync(
    "git",
    [
      "archive",
      "--format=zip",
      `--prefix=${prefix}`,
      `--output=${archivePath}`,
      "--add-file=examples/README.md",
      "HEAD",
      "LICENSE",
      "examples/NE555+CD4017",
      "examples/SNx4HC00",
      "examples/h-bridge",
      "examples/lm741",
    ],
    { cwd: root, stdio: "inherit" },
  );

  if (statSync(archivePath).size === 0) throw new Error(`${archiveName} is empty`);
  console.log(`Created ${archivePath}`);
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
