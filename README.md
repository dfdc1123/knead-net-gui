# KneadNet

[English](README.md) | [简体中文](README.zh-CN.md)

> Knead what your nets need.

KneadNet is a cross-platform desktop application that turns KiCad project connectivity into breadboard layouts and jumper-wire suggestions. It combines automatic component placement with a linked schematic, breadboard preview, and assembly checklist.

KneadNet is an early preview intended for small circuits. Generated layouts are suggestions, not electrical verification: check component orientation, pin numbering, power rails, and every connection before applying power.

## Quick start

1. Install and launch KneadNet.
2. Select or drop a folder containing a KiCad project.
3. Choose a project with a `.kicad_pcb` file.
4. Select a breadboard size and any desired power-rail bindings.
5. Choose a computation profile and start the layout search.
6. Follow the schematic, breadboard, and checklist while assembling the circuit.

A `.kicad_pcb` file is required. A same-name `.kicad_sch` file enables schematic preview and cross-selection, but is not required for layout. Both files must be direct children of the selected folder.

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

## What it can do

- Discover `.kicad_pcb` files and matching `.kicad_sch` files in a selected folder.
- Read KiCad PCB nets and through-hole pad geometry as layout input.
- Preview and cross-highlight a matching schematic.
- Work with adjustable 170-, 400-, and 830-tie-point breadboard presets.
- Search for component placements and suggest jumper-wire routes.
- Link selected components and nets across the schematic, breadboard, and assembly list.
- Track assembly progress during the current session.
- Accept folders and KiCad files by drag and drop.

## Download

Download published builds from [GitHub Releases](https://github.com/dfdc1123/knead-net-gui/releases).

| Platform | Recommended file | Notes |
| --- | --- | --- |
| Windows x64 | `KneadNet_<version>_windows_x64-setup.exe` | Installer for most Windows users |
| Windows x64 | `KneadNet_<version>_windows_x64_en-US.msi` | MSI for managed environments |
| macOS Intel / Apple silicon | `KneadNet_<version>_macos_universal.dmg` | Universal application bundle |
| Linux x86-64 | `KneadNet_<version>_linux_amd64.AppImage` | Portable single-file application |
| Debian / Ubuntu x86-64 | `kneadnet_<version>_amd64.deb` | Debian package |
| Fedora / RPM x86-64 | `kneadnet-<version>-1.x86_64.rpm` | RPM package |

Releases also include `SHA256SUMS` for download verification and `KneadNet-examples-<version>.zip` for trying the application with known projects.

Current builds are not code-signed, so Windows SmartScreen or macOS Gatekeeper may show a warning. Only run files downloaded from this repository, and verify their SHA-256 checksum when possible.

An AppImage downloaded through a browser may need its executable bit enabled:

```bash
chmod +x KneadNet_<version>_linux_amd64.AppImage
./KneadNet_<version>_linux_amd64.AppImage
```

`kneadnet-bin` is not currently published to the AUR. Arch Linux users should use the AppImage or another package from GitHub Releases until it becomes available.

## Example projects

The easiest option is to download `KneadNet-examples-<version>.zip` from the same GitHub Release as the application. Extract it, then open one of its project folders in KneadNet.

The examples can also be browsed in the repository's [`examples/` directory](examples/). See [`examples/README.md`](examples/README.md) for a description of each project.

## Platform notes

- **Windows:** installing WebView2 may require an internet connection if a suitable runtime is not already present.
- **Linux:** AppImage compatibility depends on the distribution and system-library versions.
- **macOS:** the universal DMG supports Intel and Apple silicon, but unsigned builds require explicit approval in macOS privacy and security settings.

## Known limitations

- Only through-hole pads and footprints are supported; SMD and mixed-footprint projects may fail to import.
- Project discovery does not scan nested folders.
- Schematic preview and cross-selection require matching schematic and PCB filenames.
- Complex circuits or unusual footprints may not produce a legal or practical layout.
- Results cannot yet be exported as a standalone project or report.
- There is no command-line interface, automatic updater, deep-link handler, or registered KiCad file association.
- Assembly progress is not persisted after the application closes.

## Build from source

Source builds require Node.js 22+, pnpm 11+, a stable Rust toolchain, and the [Tauri 2 prerequisites](https://v2.tauri.app/start/prerequisites/) for your operating system.

```bash
pnpm install --frozen-lockfile
pnpm tauri dev
```

Use `pnpm tauri build` to create packages supported by the current host platform. Development setup, tests, and pull-request guidance are documented in [`CONTRIBUTING.md`](CONTRIBUTING.md).

## Contributing

Bug reports and focused pull requests are welcome. Include the KneadNet version, operating system, package format, KiCad version, reproduction steps, and a minimal project when licensing and privacy permit. Never upload a private schematic without permission.

## License

KneadNet is released under the [GNU General Public License v3.0](LICENSE), identified as `GPL-3.0-only`.
