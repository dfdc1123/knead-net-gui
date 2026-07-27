import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { prepareAurPackage } from "../scripts/prepare-aur-package.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));

function runPrepare(tag, checksums) {
  const directory = mkdtempSync(path.join(tmpdir(), "kneadnet-aur-"));
  const checksumFile = path.join(directory, "SHA256SUMS");
  const outputDirectory = path.join(directory, "out");
  writeFileSync(checksumFile, checksums, "utf8");

  try {
    prepareAurPackage({
      tag,
      checksumsFile: checksumFile,
      pkgbuildFile: path.join(root, "packaging/aur/PKGBUILD"),
      srcinfoFile: path.join(root, "packaging/aur/.SRCINFO"),
      outputDirectory,
    });
    return { status: 0, stderr: "", outputDirectory };
  } catch (error) {
    return {
      status: 1,
      stderr: error instanceof Error ? error.message : String(error),
      outputDirectory,
    };
  }
}

test("prepares matching AUR metadata from stable release checksums", () => {
  const debChecksum = "1".repeat(64);
  const examplesChecksum = "2".repeat(64);
  const result = runPrepare(
    "v1.2.3",
    [
      `${"a".repeat(64)}  KneadNet_1.2.3_linux_amd64.AppImage`,
      `${examplesChecksum}  KneadNet-examples-1.2.3.zip`,
      `${debChecksum}  kneadnet_1.2.3_amd64.deb`,
      "",
    ].join("\n"),
  );

  assert.equal(result.status, 0, result.stderr);
  const pkgbuild = readFileSync(path.join(result.outputDirectory, "PKGBUILD"), "utf8");
  const srcinfo = readFileSync(path.join(result.outputDirectory, ".SRCINFO"), "utf8");

  assert.match(pkgbuild, /^pkgver=1\.2\.3$/m);
  assert.match(pkgbuild, /^pkgrel=1$/m);
  assert.match(pkgbuild, new RegExp(`^  '${debChecksum}'$`, "m"));
  assert.match(pkgbuild, new RegExp(`^  '${examplesChecksum}'$`, "m"));

  assert.match(srcinfo, /^\tpkgver = 1\.2\.3$/m);
  assert.match(srcinfo, /^\tpkgrel = 1$/m);
  assert.match(srcinfo, /^\tnoextract = kneadnet_1\.2\.3_amd64\.deb$/m);
  assert.match(srcinfo, /^\tnoextract = KneadNet-examples-1\.2\.3\.zip$/m);
  assert.match(
    srcinfo,
    /^\tsource_x86_64 = kneadnet_1\.2\.3_amd64\.deb::https:\/\/github\.com\/dfdc1123\/knead-net-gui\/releases\/download\/v1\.2\.3\/kneadnet_1\.2\.3_amd64\.deb$/m,
  );
  assert.match(srcinfo, new RegExp(`^\\tsha256sums_x86_64 = ${debChecksum}$`, "m"));
  assert.match(srcinfo, new RegExp(`^\\tsha256sums_x86_64 = ${examplesChecksum}$`, "m"));
  assert.doesNotMatch(srcinfo, /0\.2\.4/);
});

test("rejects prerelease tags before preparing AUR metadata", () => {
  const result = runPrepare("v1.2.3-rc.1", "");

  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /stable release tag/);
});

test("rejects missing release asset checksums", () => {
  const result = runPrepare(
    "v1.2.3",
    `${"1".repeat(64)}  kneadnet_1.2.3_amd64.deb\n`,
  );

  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /KneadNet-examples-1\.2\.3\.zip/);
});
