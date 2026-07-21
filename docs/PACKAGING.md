# Packaging KneadNet

This document records the product identities, generated formats, and platform-specific packaging decisions that must remain stable across releases.

## Product identities

| Purpose | Value |
| --- | --- |
| Product and display name | `KneadNet` |
| Tagline | `Knead what your nets need.` |
| Desktop executable | `kneadnet` |
| Core Rust crate | `knead-net` |
| Linux package | `kneadnet` |
| Tauri/macOS bundle ID | `io.github.dfdc1123.kneadnet` |
| Linux desktop file ID | `kneadnet.desktop` |
| AppStream component ID | `io.github.dfdc1123.kneadnet` |
| AUR binary package | `kneadnet-bin` |

The repository URL remains `https://github.com/dfdc1123/knead-net-gui`. The repository name is not an installed product identity.

The main Tauri product name stays `KneadNet` for Windows, macOS, and window chrome. `src-tauri/tauri.linux.conf.json` deliberately overrides only the Linux technical product name to `kneadnet`, preventing Tauri from transforming the Debian/RPM package and desktop filename into `knead-net` while the tracked desktop entry still displays `KneadNet`.

The old `knead-net-gui` executable, bundle ID, and AUR package belong to the v0.1.0 preview. v0.2.0 intentionally starts a clean product identity; it does not implement an in-place migration from v0.1.0.

## Version source

`package.json` is the Tauri version source. Cargo packages inherit the matching value from `[workspace.package]` in the root `Cargo.toml`. Run:

```bash
pnpm check:version
```

For a release tag, also run:

```bash
node scripts/check-version.mjs --tag v0.2.0
```

Only stable `vMAJOR.MINOR.PATCH` tags are accepted. Prerelease tags are not currently supported because the Windows MSI and downstream package-version policies have not been defined for them.

## Release assets

| Format | Stable asset name |
| --- | --- |
| Windows NSIS | `KneadNet_<version>_windows_x64-setup.exe` |
| Windows MSI | `KneadNet_<version>_windows_x64_en-US.msi` |
| Linux AppImage | `KneadNet_<version>_linux_amd64.AppImage` |
| Debian | `kneadnet_<version>_amd64.deb` |
| RPM | `kneadnet-<version>-1.x86_64.rpm` |
| macOS universal DMG | `KneadNet_<version>_macos_universal.dmg` |
| Public examples | `KneadNet-examples-<version>.zip` |
| Checksums | `SHA256SUMS` |

Tauri's native filenames are normalized by `scripts/collect-release-assets.mjs`. `scripts/verify-release-assets.mjs` fails unless exactly one non-empty asset of every expected type is present.

No portable Windows archive is produced. It would bypass the installer, WebView2 bootstrapper, uninstall metadata, and upgrade policy without providing a better supported installation path.

## Linux integration

The canonical desktop entry is `packaging/linux/kneadnet.desktop`. It deliberately has no MIME declarations because KneadNet does not accept command-line file arguments or register KiCad file types.

The AppStream component is `packaging/linux/io.github.dfdc1123.kneadnet.metainfo.xml`. Screenshot URLs use immutable release-tag paths, and each published version records its actual publication date.

Expected installed files include:

```text
/usr/bin/kneadnet
/usr/share/applications/kneadnet.desktop
/usr/share/metainfo/io.github.dfdc1123.kneadnet.metainfo.xml
/usr/share/icons/hicolor/.../apps/kneadnet.png
```

Debian packages install the GPL text as `/usr/share/doc/kneadnet/copyright`; RPM/AppImage use `/usr/share/licenses/kneadnet/LICENSE`.

Validate metadata with:

```bash
desktop-file-validate packaging/linux/kneadnet.desktop
appstreamcli validate --no-net packaging/linux/io.github.dfdc1123.kneadnet.metainfo.xml
```

Official AppImages are built on Ubuntu 22.04 to avoid linking against a newer rolling-distribution baseline.

## Windows

The workflow builds x64 NSIS and MSI installers. Both display `KneadNet` and install `kneadnet.exe`.

The WiX upgrade code is explicitly fixed in `tauri.conf.json`; do not change it after v0.2.0. Downgrades are disabled. WebView2 uses the download-bootstrapper mode, which keeps installers small but can require internet access when the runtime is absent.

The repository does not contain certificate data or a signing command. Add signing only after a maintainer provisions a certificate and documents secret handling. Test install, launch, reinstall, upgrade, uninstall, Start Menu entries, icons, and uninstall metadata on Windows before publication.

## macOS

The release workflow installs both Rust macOS targets and produces one `universal-apple-darwin` DMG. The bundle name is `KneadNet` and the bundle ID is `io.github.dfdc1123.kneadnet`.

Signing and notarization are intentionally absent until Apple identities and secrets exist. Test the `.app` metadata, Intel/Apple-silicon slices, first launch, Gatekeeper behavior, and drag-to-Applications installation on macOS.

## Local bundle checks

On Linux, the most useful local package command is:

```bash
pnpm tauri build --ci --no-sign --bundles deb
```

Inspect the actual package name, control metadata, executable, desktop entry, icons, AppStream file, and license. RPM and AppImage builds require their platform tools and a compatible build host. Windows and macOS installers cannot be fully validated from Linux.

## Signing and updates

Current releases are unsigned and no updater plugin is configured. Do not create placeholder certificates, private keys, publisher identities, updater endpoints, or signatures. The missing signing identities and updater policy are release risks, not configuration values to invent.

## Third-party licenses

The bundled GPL file covers KneadNet itself; it does not replace notices required by dependencies. The current npm inventory reports Apache-2.0, BSD-3-Clause, CC-BY-4.0, ISC, MIT, and MPL-2.0 metadata with no missing license field. A complete Rust notice bundle is not generated by the tools currently installed in the repository.

Before publishing binaries, review a license inventory generated from the locked Rust and npm dependency graphs with a pinned, documented tool. Inspect AppImage contents separately because it bundles shared libraries from the Linux build image. Add any required copyright notices or source offers to the release rather than assuming the project GPL file covers them.
