#!/usr/bin/env node

import { copyFileSync, mkdirSync, readFileSync, readdirSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const root = fileURLToPath(new URL("../", import.meta.url));

function option(name, { required = true, fallback } = {}) {
  const index = process.argv.indexOf(name);
  if (index === -1) {
    if (required && fallback === undefined) throw new Error(`${name} is required`);
    return fallback;
  }
  const value = process.argv[index + 1];
  if (!value || value.startsWith("--")) throw new Error(`${name} requires a value`);
  return value;
}

function walk(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const entryPath = path.join(directory, entry.name);
    return entry.isDirectory() ? walk(entryPath) : [entryPath];
  });
}

function selectOne(files, label, predicate) {
  const matches = files.filter(predicate);
  if (matches.length !== 1) {
    throw new Error(`${label}: expected exactly one artifact, found ${matches.length}\n${matches.join("\n")}`);
  }
  return matches[0];
}

try {
  const platform = option("--platform");
  const bundleDirectory = path.resolve(root, option("--bundle-dir"));
  const outputDirectory = path.resolve(root, option("--out", { required: false, fallback: "release-assets" }));
  const { version } = JSON.parse(readFileSync(path.join(root, "package.json"), "utf8"));
  const files = walk(bundleDirectory).filter((file) => path.basename(file).includes(version));
  const normalized = (file) => file.split(path.sep).join("/").toLowerCase();
  const mappings = [];

  if (platform === "windows") {
    mappings.push(
      [selectOne(files, "NSIS installer", (file) => normalized(file).includes("/nsis/") && file.toLowerCase().endsWith(".exe")), `KneadNet_${version}_windows_x64-setup.exe`],
      [selectOne(files, "MSI installer", (file) => normalized(file).includes("/msi/") && file.toLowerCase().endsWith(".msi")), `KneadNet_${version}_windows_x64_en-US.msi`],
    );
  } else if (platform === "linux") {
    mappings.push(
      [selectOne(files, "AppImage", (file) => file.endsWith(".AppImage")), `KneadNet_${version}_linux_amd64.AppImage`],
      [selectOne(files, "Debian package", (file) => path.basename(file) === `kneadnet_${version}_amd64.deb`), `kneadnet_${version}_amd64.deb`],
      [selectOne(files, "RPM package", (file) => path.basename(file) === `kneadnet-${version}-1.x86_64.rpm`), `kneadnet-${version}-1.x86_64.rpm`],
    );
  } else if (platform === "macos") {
    mappings.push(
      [selectOne(files, "macOS disk image", (file) => file.endsWith(".dmg")), `KneadNet_${version}_macos_universal.dmg`],
    );
  } else {
    throw new Error(`Unsupported platform: ${platform}`);
  }

  mkdirSync(outputDirectory, { recursive: true });
  for (const [source, name] of mappings) {
    const destination = path.join(outputDirectory, name);
    copyFileSync(source, destination);
    if (statSync(destination).size === 0) throw new Error(`${destination} is empty`);
    console.log(`${source} -> ${destination}`);
  }
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
