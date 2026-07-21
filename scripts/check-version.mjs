#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const root = fileURLToPath(new URL("../", import.meta.url));

function readJson(relativePath) {
  return JSON.parse(readFileSync(path.join(root, relativePath), "utf8"));
}

function option(name) {
  const index = process.argv.indexOf(name);
  if (index === -1) return undefined;
  const value = process.argv[index + 1];
  if (!value || value.startsWith("--")) {
    throw new Error(`${name} requires a value`);
  }
  return value;
}

function assertEqual(actual, expected, label) {
  if (actual !== expected) {
    throw new Error(`${label}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}

function tomlSection(source, name) {
  const header = `[${name}]`;
  const headerStart = source.indexOf(header);
  if (headerStart === -1) throw new Error(`Cargo.toml is missing ${header}`);
  const bodyStart = source.indexOf("\n", headerStart) + 1;
  const remainder = source.slice(bodyStart);
  const nextHeader = remainder.match(/^\[/m);
  return nextHeader ? remainder.slice(0, nextHeader.index) : remainder;
}

function tomlString(section, key) {
  const escapedKey = key.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = section.match(new RegExp(`^${escapedKey}\\s*=\\s*"([^"]+)"\\s*$`, "m"));
  return match?.[1];
}

try {
  const packageJson = readJson("package.json");
  const tauriConfig = readJson("src-tauri/tauri.conf.json");
  const version = packageJson.version;

  if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version)) {
    throw new Error(`package.json version must be a stable SemVer value, got ${version}`);
  }

  assertEqual(packageJson.name, "kneadnet", "frontend package name");
  assertEqual(tauriConfig.version, "../package.json", "Tauri version source");
  assertEqual(tauriConfig.productName, "KneadNet", "Tauri product name");
  assertEqual(tauriConfig.mainBinaryName, "kneadnet", "Tauri main binary");
  assertEqual(
    tauriConfig.identifier,
    "io.github.dfdc1123.kneadnet",
    "Tauri bundle identifier",
  );

  const rootCargo = readFileSync(path.join(root, "Cargo.toml"), "utf8");
  const appCargo = readFileSync(path.join(root, "src-tauri/Cargo.toml"), "utf8");
  assertEqual(tomlString(tomlSection(rootCargo, "workspace.package"), "version"), version, "Cargo workspace version");
  assertEqual(tomlString(tomlSection(rootCargo, "package"), "name"), "knead-net", "core Cargo package name");
  assertEqual(tomlString(tomlSection(appCargo, "package"), "name"), "kneadnet", "desktop Cargo package name");
  if (!/^version\.workspace\s*=\s*true\s*$/m.test(tomlSection(rootCargo, "package"))) {
    throw new Error("knead-net must inherit the workspace version");
  }
  if (!/^version\.workspace\s*=\s*true\s*$/m.test(tomlSection(appCargo, "package"))) {
    throw new Error("kneadnet must inherit the workspace version");
  }

  const tag = option("--tag");
  if (tag !== undefined) {
    assertEqual(tag, `v${version}`, "release tag");
  }

  console.log(`KneadNet version ${version} is consistent.`);
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
