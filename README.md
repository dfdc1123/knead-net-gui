# KneadNet

[English](README.md) | [简体中文](README.zh-CN.md)

> Knead what your nets need.

KneadNet is a cross-platform desktop application that converts electronic schematics into breadboard layouts and routing suggestions. It reads KiCad PCB connectivity and through-hole footprint geometry, searches for a usable component placement, routes jumper wires, and presents the result alongside the schematic and an assembly checklist.

KneadNet is an early preview. It is useful for experimenting with small through-hole circuits, but it is not yet a substitute for checking the circuit and every proposed connection yourself.

## Screenshots

The interface follows a four-step workflow and automatically uses simplified Chinese or English based on the system language.

### Import a KiCad project

![Import a KiCad project and preview its schematic](docs/screenshots/tab1.png)

### Choose a breadboard

![Choose and preview a breadboard](docs/screenshots/tab2.png)

### Compute a layout

![Compute component placement and jumper-wire routing](docs/screenshots/tab3.png)

### Follow the assembly view

![Linked schematic, breadboard, and assembly checklist](docs/screenshots/tab4.png)

## Features

- Finds `.kicad_pcb` and same-name `.kicad_sch` files in a selected folder.
- Uses KiCad PCB nets and through-hole pad geometry as layout input.
- Previews the schematic when a matching schematic file is available.
- Provides 170-, 400-, and 830-tie-point breadboard presets with adjustable length.
- Generates placements with a spectral initializer and parallel simulated annealing.
- Suggests jumper-wire routes after placement.
- Offers quick, standard, and full computation profiles with live progress.
- Links selected components and nets across the schematic, breadboard, and assembly list.
- Tracks component and jumper completion locally during the current session.
- Supports folder and KiCad-file drag and drop in the desktop application.
- Provides simplified Chinese and English UI text.

## How it works

```text
KiCad PCB connectivity and footprint geometry
                    |
                    v
          Circuit and net model
                    |
                    v
       Spectral initial placement
                    |
                    v
     Multi-seed simulated annealing
                    |
                    v
       PathFinder / MST wire routing
                    |
                    v
 Breadboard preview and assembly checklist
```

The `.kicad_pcb` file is required. A same-name `.kicad_sch` file adds the schematic preview and cross-selection, but layout can run without it. Both files must be direct children of the folder selected in KneadNet.

## Download

Published builds are available from [GitHub Releases](https://github.com/dfdc1123/knead-net-gui/releases). Release assets vary for older versions; the cross-platform asset convention below starts with v0.2.0.

| Platform | Choose | Notes |
| --- | --- | --- |
| Windows x64 | `KneadNet_<version>_windows_x64-setup.exe` | NSIS installer for most users |
| Windows x64 | `KneadNet_<version>_windows_x64_en-US.msi` | MSI installer for managed environments |
| macOS Intel / Apple silicon | `KneadNet_<version>_macos_universal.dmg` | Universal application bundle |
| Linux x86-64 | `KneadNet_<version>_linux_amd64.AppImage` | Portable single-file application |
| Debian / Ubuntu x86-64 | `kneadnet_<version>_amd64.deb` | Debian package |
| Fedora / RPM x86-64 | `kneadnet-<version>-1.x86_64.rpm` | RPM package |

Every cross-platform release also contains `SHA256SUMS` and `KneadNet-examples-<version>.zip`. Architecture labels follow platform conventions: `x64` on Windows, `amd64` in Debian/AppImage names, `x86_64` for RPM, and `universal` for a macOS bundle containing Intel and Apple-silicon binaries.

Builds are not currently code-signed. Windows SmartScreen and macOS Gatekeeper may therefore warn before first launch. Confirm that the file came from this repository and verify its SHA-256 value before proceeding. Signing status is recorded in the release notes; do not bypass a warning for a file obtained elsewhere.

To check one downloaded file on Linux:

```bash
grep 'kneadnet_0.2.0_amd64.deb' SHA256SUMS | sha256sum --check
```

Replace the example filename with the asset you downloaded.

AppImage files downloaded from a browser are not necessarily marked executable. Enable and launch one with:

```bash
chmod +x KneadNet_0.2.0_linux_amd64.AppImage
./KneadNet_0.2.0_linux_amd64.AppImage
```

### Arch Linux and AUR

The repository contains a review-ready binary package definition for `kneadnet-bin`. It is intentionally not submitted or updated automatically. After the package is manually published to the AUR, it can be installed with an AUR helper:

```bash
yay -S kneadnet-bin
```

Until then, use a GitHub Release package or build the local [`PKGBUILD`](packaging/aur/PKGBUILD) after replacing its explicit checksum placeholders. The old `knead-net-gui` AUR package belongs to the pre-v0.2.0 naming scheme.

## Quick start

1. Install and launch KneadNet.
2. Select or drop a folder containing a KiCad project.
3. Select a project that has a `.kicad_pcb` file.
4. Choose a breadboard preset, length, active half, and any desired power-rail net bindings.
5. Choose a computation profile and wait for placement and routing to finish. You can interrupt annealing and route the best placement found so far.
6. Use the schematic, breadboard, and checklist together while assembling the circuit.

Treat generated layouts as suggestions. Check component orientation, pin numbering, power rails, and every connection before applying power.

## Example projects

Public examples are described in [`examples/README.md`](examples/README.md).

### Method 1: download the release bundle

Open [GitHub Releases](https://github.com/dfdc1123/knead-net-gui/releases), expand the selected release's assets, and download `KneadNet-examples-<version>.zip`. Extract it, then select one of its individual project folders in KneadNet.

### Method 2: obtain examples from the repository

- Browse the [`examples/` directory on GitHub](https://github.com/dfdc1123/knead-net-gui/tree/main/examples).
- Use GitHub's **Code → Download ZIP** command to download the whole repository. GitHub does not provide a built-in arbitrary-folder ZIP download.
- Clone the repository:

  ```bash
  git clone https://github.com/dfdc1123/knead-net-gui.git
  cd knead-net-gui/examples
  ```

- Or use sparse checkout when only examples are wanted:

  ```bash
  git clone --filter=blob:none --no-checkout https://github.com/dfdc1123/knead-net-gui.git
  cd knead-net-gui
  git sparse-checkout set examples
  git checkout main
  ```

`examples/h-bridge_different_order` is a developer regression fixture and is deliberately excluded from the release examples archive.

## Build from source

Install the following first:

- Node.js 22 or later.
- pnpm 11 or later.
- A stable Rust toolchain with Cargo.
- The [Tauri 2 prerequisites](https://v2.tauri.app/start/prerequisites/) for the target operating system.

Then install dependencies and run the desktop application:

```bash
pnpm install --frozen-lockfile
pnpm tauri dev
```

Build the frontend or platform-native application bundles with:

```bash
pnpm build
pnpm tauri build
```

Tauri can only create native installers for the host platform. The release workflow builds each platform on its corresponding GitHub-hosted runner.

## Platform notes

- **Windows:** the installer uses the WebView2 download bootstrapper when a suitable runtime is missing, so first installation may require internet access.
- **Linux:** AppImage compatibility depends on the distribution baseline. Official release AppImages are built on Ubuntu 22.04 rather than a rolling distribution.
- **macOS:** the DMG is universal, but unsigned builds can require an explicit approval in macOS privacy and security settings.

## Repository structure

```text
src/routes/                 SvelteKit application pages
src/lib/components/         Reusable workflow and breadboard UI
src/input/                  KiCad PCB S-expression parser
src/circuit.rs              Circuit domain model
src/layout/                 Placement, cost, legality, and routing engine
src-tauri/src/              Tauri commands and schematic rendering
src-tauri/tests/            Desktop integration tests
examples/                   Public examples and developer fixtures
docs/screenshots/           README screenshots
packaging/linux/            Linux desktop and AppStream metadata
packaging/aur/              AUR package definition and maintenance notes
scripts/                    Version and release-asset checks
.github/workflows/          CI and draft-release automation
```

## Development and testing

Run the narrowest relevant test while developing. Before opening a pull request, run:

```bash
pnpm check
pnpm test:ui
pnpm build
pnpm check:version
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for code organization, test expectations, and pull-request guidance.

## Packaging and releases

- [`docs/PACKAGING.md`](docs/PACKAGING.md) documents product IDs, package formats, and platform details.
- [`docs/RELEASING.md`](docs/RELEASING.md) contains the release checklist and signing placeholders.
- [`packaging/aur/README.md`](packaging/aur/README.md) explains manual AUR maintenance.

Stable tags must exactly match the version in `package.json` and Cargo metadata. A successful tag workflow creates a draft release; a maintainer inspects and smoke-tests it before publication.

## Known limitations

- Only through-hole pads and footprints are supported. SMD or mixed-footprint projects can fail to import.
- Project discovery scans only the selected folder, not nested directories.
- Schematic and PCB cross-selection requires matching basenames.
- Complex circuits or unusual footprints may not produce a legal or practical layout.
- Generated placements and routes are not electrical verification.
- Results cannot yet be exported as a standalone project or report.
- There is no command-line interface, automatic updater, deep-link handler, or registered KiCad file association.
- Release builds are currently unsigned.

## Contributing

Bug reports and focused pull requests are welcome. Include the KneadNet version, operating system, package format, KiCad version, reproduction steps, and a minimal project when licensing and privacy permit. Never upload a private schematic without permission.

## License

KneadNet is released under the [GNU General Public License v3.0](LICENSE), identified as `GPL-3.0-only`. Repository icons, screenshots, and public examples are distributed with the project under the same license unless a file states otherwise.
