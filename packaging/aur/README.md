# Maintaining `kneadnet-bin`

`kneadnet-bin` is a binary AUR package definition for manual review and submission. It downloads the official x86-64 Debian package and public examples archive, then installs the upstream binary, desktop entry, icons, AppStream metadata, license, and examples.

It is not published by GitHub Actions. The committed checksums must match the corresponding public GitHub Release assets.

## Update for a release

1. Publish the corresponding GitHub Release first so its asset URLs are public.
2. Set `pkgver` to the release version and reset `pkgrel=1`. Increment only `pkgrel` for packaging-only fixes.
3. Download `SHA256SUMS` from the release and update the Debian package and examples archive checksums.
4. Add the real maintainer name and email to the `PKGBUILD` comment before AUR submission.
5. Regenerate `.SRCINFO` from the reviewed `PKGBUILD`:

   ```bash
   makepkg --printsrcinfo > .SRCINFO
   ```

6. Check that `.SRCINFO` contains the same version, URLs, dependencies, conflicts, and checksums as `PKGBUILD`.

Do not use placeholders or `SKIP` for release assets.

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

Copy only the reviewed `PKGBUILD` and generated `.SRCINFO` into the `kneadnet-bin` AUR clone, inspect the Git diff, and push manually. Do not copy release credentials or build output.

The previous package name was `knead-net-gui`. Any deletion, merge, or replacement request must be handled manually according to current AUR policy; this repository does not perform that external action.
