# Releasing KneadNet

KneadNet releases are built from stable SemVer tags. The workflow verifies the source, builds every supported platform, creates checksums and an examples archive, and then creates a **draft** GitHub Release. Publication is always a maintainer decision.

## Before tagging

1. Choose a stable version such as `0.2.0`. Do not move or reuse a previously published tag.
2. Update both version sources:
   - `package.json`
   - `[workspace.package].version` in `Cargo.toml`
3. Refresh lockfiles:

   ```bash
   pnpm install --lockfile-only
   cargo check --workspace
   ```

4. Review user-visible changes and generated GitHub release-note categories.
5. Confirm that public examples contain no private designs and have redistribution permission.
6. Review the third-party dependency and AppImage library license inventory described in `docs/PACKAGING.md`, and include any required notices or source offers.
7. Confirm that README download instructions and `docs/PACKAGING.md` still match the workflow.
8. Run the complete checks:

   ```bash
   pnpm install --frozen-lockfile
   pnpm check:version
   pnpm check
   pnpm test:ui
   pnpm build
   cargo fmt --check
   cargo test --workspace --locked
   cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
   desktop-file-validate packaging/linux/kneadnet.desktop
   appstreamcli validate --no-net packaging/linux/io.github.dfdc1123.kneadnet.metainfo.xml
   ```

9. Build and inspect at least one native bundle locally when the host permits it.
10. Commit and push the release preparation. Wait for CI to pass on that exact commit.

## Tagging

Verify the intended version explicitly:

```bash
node scripts/check-version.mjs --tag v0.2.0
```

Create an annotated tag on the tested commit and push it:

```bash
git tag --annotate v0.2.0 --message "KneadNet v0.2.0"
git push origin v0.2.0
```

Use a signed tag only when a maintained signing key and verification policy exist.

## Workflow behavior

The release workflow:

1. Rejects a tag that differs from Cargo/package/Tauri metadata.
2. Re-runs frontend checks and the complete Rust test/clippy suite.
3. Builds Windows x64 NSIS/MSI, Linux x86-64 AppImage/Deb/RPM, and a macOS universal DMG.
4. Renames artifacts to the documented stable convention.
5. Packages only the reviewed public examples.
6. Fails if any expected asset is missing, duplicated, or empty.
7. Writes SHA-256 values for every downloadable asset.
8. Creates or refreshes a draft Release. It refuses to overwrite an already published Release.

The workflow never publishes to the AUR and never publishes the GitHub Release automatically.

## Inspecting the draft

Before publication:

- Confirm all documented files exist and no unexpected files are present.
- Download the assets and verify `SHA256SUMS` from a separate machine where practical.
- Inspect file properties, icons, version, architecture, product name, publisher/signing state, and license.
- Smoke-test:
  - Windows NSIS install, launch, reinstall/upgrade, Start Menu entry, and uninstall.
  - Windows MSI install, upgrade behavior, repair/uninstall metadata, and WebView2 bootstrap.
  - AppImage executable bit, launch, desktop integration, and behavior on an older supported distribution.
  - Debian and RPM install, launch, menu entry, AppStream discovery, and removal.
  - macOS DMG mount, drag installation, universal binary slices, launch, and Gatekeeper warning.
- Open at least one bundled example and complete a layout calculation.
- Edit generated release notes so they describe only behavior supported by the code.
- State clearly that builds are unsigned.

Publish the draft only after these checks pass.

## AUR follow-up

After the GitHub Release is public:

1. Follow `packaging/aur/README.md` to update `pkgver`, `pkgrel`, checksums, and `.SRCINFO`.
2. Test source verification and package installation in a clean Arch environment.
3. Inspect the installed executable, desktop entry, icons, AppStream metadata, license, and examples.
4. Submit or update `kneadnet-bin` manually.
5. Retire or request an appropriate merge/deletion of the old `knead-net-gui` package only after checking AUR policy.

## Post-release checks

- Confirm the Releases page shows the intended version as latest.
- Download one asset from the public page and verify its checksum.
- Confirm README links and example instructions work without authentication.
- Confirm the source tag points to the exact reviewed commit.
- Check the release workflow and CI logs for warnings hidden by successful steps.
- Record any signing, installer, or package issue before beginning the next release.

## Optional signing inputs

No signing secrets are currently configured.

- Windows needs a code-signing certificate, secure certificate import, password secret, timestamp service, and a documented renewal process.
- macOS needs a Developer ID Application identity, certificate/password, Apple account or App Store Connect credentials, Team ID, notarization, and stapling.
- A future Tauri updater would additionally need a stable HTTPS endpoint, an offline-protected signing private key, and the corresponding public key in application configuration.

Choose and document secret names only when these identities exist. Never commit signing material or print secrets in workflow logs.

## Maintainer-owned repository settings

These are intentionally not changed by repository files:

- GitHub repository description, homepage, and topics.
- Private vulnerability reporting or a security contact email.
- Release environment protection rules.
- Windows and Apple signing identities.
- AUR maintainer name and email.
