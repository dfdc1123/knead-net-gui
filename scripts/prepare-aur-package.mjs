#!/usr/bin/env node

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

function option(name) {
  const index = process.argv.indexOf(name);
  if (index === -1) throw new Error(`${name} is required`);
  const value = process.argv[index + 1];
  if (!value || value.startsWith("--")) throw new Error(`${name} requires a value`);
  return value;
}

function replaceOne(content, pattern, replacement, label) {
  const matches = content.match(new RegExp(pattern.source, pattern.flags.includes("g") ? pattern.flags : `${pattern.flags}g`));
  if (matches?.length !== 1) {
    throw new Error(`${label}: expected exactly one match, found ${matches?.length ?? 0}`);
  }
  return content.replace(pattern, replacement);
}

function replaceLines(content, pattern, replacements, label) {
  let index = 0;
  const updated = content.replace(pattern, () => {
    const replacement = replacements[index];
    index += 1;
    return replacement ?? "";
  });
  if (index !== replacements.length) {
    throw new Error(`${label}: expected ${replacements.length} matches, found ${index}`);
  }
  return updated;
}

function readChecksums(file) {
  const checksums = new Map();
  for (const [index, line] of readFileSync(file, "utf8").split(/\r?\n/).entries()) {
    if (line === "") continue;
    const match = line.match(/^([a-f0-9]{64})  ([^/]+)$/);
    if (!match) throw new Error(`${file}:${index + 1}: invalid SHA256SUMS entry`);
    const [, checksum, name] = match;
    if (checksums.has(name)) throw new Error(`${file}: duplicate checksum for ${name}`);
    checksums.set(name, checksum);
  }
  return checksums;
}

export function prepareAurPackage({ tag, checksumsFile, pkgbuildFile, srcinfoFile, outputDirectory }) {
  const match = tag.match(/^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/);
  if (!match) throw new Error(`Expected a stable release tag such as v1.2.3, received ${tag}`);

  const version = tag.slice(1);
  const deb = `kneadnet_${version}_amd64.deb`;
  const examples = `KneadNet-examples-${version}.zip`;
  const checksums = readChecksums(path.resolve(checksumsFile));
  const debChecksum = checksums.get(deb);
  const examplesChecksum = checksums.get(examples);
  if (!debChecksum) throw new Error(`SHA256SUMS does not contain ${deb}`);
  if (!examplesChecksum) throw new Error(`SHA256SUMS does not contain ${examples}`);

  let pkgbuild = readFileSync(path.resolve(pkgbuildFile), "utf8");
  pkgbuild = replaceOne(pkgbuild, /^pkgver=.*$/m, `pkgver=${version}`, "PKGBUILD pkgver");
  pkgbuild = replaceOne(pkgbuild, /^pkgrel=.*$/m, "pkgrel=1", "PKGBUILD pkgrel");
  pkgbuild = replaceOne(
    pkgbuild,
    /^sha256sums_x86_64=\(\n[\s\S]*?^\)$/m,
    `sha256sums_x86_64=(\n  '${debChecksum}'\n  '${examplesChecksum}'\n)`,
    "PKGBUILD sha256sums_x86_64",
  );

  let srcinfo = readFileSync(path.resolve(srcinfoFile), "utf8");
  srcinfo = replaceOne(srcinfo, /^\tpkgver = .*$/m, `\tpkgver = ${version}`, ".SRCINFO pkgver");
  srcinfo = replaceOne(srcinfo, /^\tpkgrel = .*$/m, "\tpkgrel = 1", ".SRCINFO pkgrel");
  srcinfo = replaceLines(
    srcinfo,
    /^\tnoextract = .*$/gm,
    [`\tnoextract = ${deb}`, `\tnoextract = ${examples}`],
    ".SRCINFO noextract",
  );
  srcinfo = replaceLines(
    srcinfo,
    /^\tsource_x86_64 = .*$/gm,
    [
      `\tsource_x86_64 = ${deb}::https://github.com/dfdc1123/knead-net-gui/releases/download/${tag}/${deb}`,
      `\tsource_x86_64 = ${examples}::https://github.com/dfdc1123/knead-net-gui/releases/download/${tag}/${examples}`,
    ],
    ".SRCINFO source_x86_64",
  );
  srcinfo = replaceLines(
    srcinfo,
    /^\tsha256sums_x86_64 = .*$/gm,
    [`\tsha256sums_x86_64 = ${debChecksum}`, `\tsha256sums_x86_64 = ${examplesChecksum}`],
    ".SRCINFO sha256sums_x86_64",
  );

  const resolvedOutputDirectory = path.resolve(outputDirectory);
  mkdirSync(resolvedOutputDirectory, { recursive: true });
  writeFileSync(path.join(resolvedOutputDirectory, "PKGBUILD"), pkgbuild, "utf8");
  writeFileSync(path.join(resolvedOutputDirectory, ".SRCINFO"), srcinfo, "utf8");
  return { version, outputDirectory: resolvedOutputDirectory };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const result = prepareAurPackage({
      tag: option("--tag"),
      checksumsFile: option("--checksums"),
      pkgbuildFile: option("--pkgbuild"),
      srcinfoFile: option("--srcinfo"),
      outputDirectory: option("--out"),
    });
    console.log(`Prepared kneadnet-bin ${result.version} metadata in ${result.outputDirectory}`);
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
