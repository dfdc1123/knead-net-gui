# Maintaining `kneadnet-bin`

`kneadnet-bin` is a binary AUR package definition. It downloads the official x86-64 Debian package and public examples archive, then installs the upstream binary, desktop entry, icons, AppStream metadata, license, and examples.

Publishing a non-prerelease GitHub Release triggers `.github/workflows/publish-aur.yml`. The workflow downloads and verifies the released Debian and examples assets, updates `PKGBUILD` and `.SRCINFO`, and pushes them to the `kneadnet-bin` AUR Git repository.

## Repository setup

The repository owner must configure AUR access once:

1. Create a dedicated Ed25519 key without a passphrase. Do not reuse a personal login key.
2. Add the public key to the AUR account that maintains or co-maintains `kneadnet-bin`.
3. Store the complete private key as the GitHub Actions repository secret `AUR_SSH_PRIVATE_KEY`.

The workflow verifies the AUR server's published Ed25519 fingerprint before using the key. Never commit either the private key or downloaded build output.

## Automatic release update

After the draft GitHub Release has been inspected and published, the AUR workflow:

1. Checks out the exact released tag and verifies that its package metadata matches the tag.
2. Downloads `SHA256SUMS`, the Debian package, and the public examples archive from that published Release.
3. Verifies both downloaded assets and prepares `PKGBUILD` with the release version, `pkgrel=1`, and exact checksums.
4. Prepares matching `.SRCINFO`, clones the AUR repository, and skips a version already present there.
5. Refuses to replace a newer AUR version, then commits and pushes only `PKGBUILD` and `.SRCINFO`.

Stable `vMAJOR.MINOR.PATCH` Releases are supported. GitHub prereleases are intentionally skipped. A failed run can be retried from its GitHub Actions run after correcting the secret or other external failure.

For a packaging-only fix, increment `pkgrel` and submit it manually without creating another upstream Release. Regenerate `.SRCINFO` from the reviewed `PKGBUILD`:

```bash
makepkg --printsrcinfo > .SRCINFO
```

Check that `.SRCINFO` contains the same version, URLs, dependencies, conflicts, and checksums as `PKGBUILD`. Do not use placeholders or `SKIP` for release assets.

The copies committed in this upstream repository are templates and a record of the last reviewed release. The workflow renders release-specific copies in its temporary workspace; it does not commit generated metadata back to this repository.

## Test

Run at minimum:

```bash
makepkg --verifysource
makepkg --cleanbuild --syncdeps --install
```

When available, also run:

```bash
namcap PKGBUILD
namcap kneadnet-bin-*.pkg.tar.zst
```

Use an Arch clean chroot before submission. After installation, check:

- `kneadnet` launches.
- `kneadnet.desktop` appears once and launches the correct executable.
- All installed icon sizes use the `kneadnet` icon name.
- AppStream recognizes `io.github.dfdc1123.kneadnet`.
- `/usr/share/licenses/kneadnet/LICENSE` exists.
- `/usr/share/doc/kneadnet/examples` contains only the public examples.
- `pacman -Rns kneadnet-bin` removes package-owned files cleanly.

## AUR submission

Normal upstream releases are submitted by `.github/workflows/publish-aur.yml`. For a packaging-only fix, copy only the reviewed `PKGBUILD` and generated `.SRCINFO` into the `kneadnet-bin` AUR clone, inspect the Git diff, and push manually. Do not copy release credentials or build output.

The previous package name was `knead-net-gui`. Any deletion, merge, or replacement request must be handled manually according to current AUR policy; this repository does not perform that external action.
