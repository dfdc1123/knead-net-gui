#!/usr/bin/env node

import { createHash } from "node:crypto";
import { createReadStream, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
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

async function sha256(file) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return hash.digest("hex");
}

try {
  const { version } = JSON.parse(readFileSync(path.join(root, "package.json"), "utf8"));
  const directory = path.resolve(root, option("--dir", "release-assets"));
  const expected = [
    `KneadNet_${version}_windows_x64-setup.exe`,
    `KneadNet_${version}_windows_x64_en-US.msi`,
    `KneadNet_${version}_linux_amd64.AppImage`,
    `kneadnet_${version}_amd64.deb`,
    `kneadnet-${version}-1.x86_64.rpm`,
    `KneadNet_${version}_macos_universal.dmg`,
    `KneadNet-examples-${version}.zip`,
  ].sort();
  const actual = readdirSync(directory)
    .filter((name) => name !== "SHA256SUMS")
    .sort();

  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`Release asset mismatch.\nExpected:\n${expected.join("\n")}\nActual:\n${actual.join("\n")}`);
  }

  const checksumLines = [];
  for (const name of expected) {
    const file = path.join(directory, name);
    if (!statSync(file).isFile() || statSync(file).size === 0) {
      throw new Error(`${name} is missing or empty`);
    }
    checksumLines.push(`${await sha256(file)}  ${name}`);
  }
  writeFileSync(path.join(directory, "SHA256SUMS"), `${checksumLines.join("\n")}\n`, "utf8");
  console.log(`Verified ${expected.length} release assets and wrote SHA256SUMS.`);
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
