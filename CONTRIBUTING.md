# Contributing to KneadNet

Thank you for helping improve KneadNet. Keep changes focused, evidence-based, and small enough to review safely.

## Development setup

Install Node.js 22+, pnpm 11+, a stable Rust toolchain, and the platform dependencies listed in the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/). Then run:

```bash
pnpm install --frozen-lockfile
pnpm tauri dev
```

Use `pnpm dev` only when a browser-only frontend preview is sufficient. Native dialogs, drag-and-drop paths, and Tauri commands require `pnpm tauri dev`.

## Code organization

- Keep UI-only work under `src/routes/` and `src/lib/`.
- Keep reusable circuit, placement, and routing logic in the root `knead-net` crate.
- Keep desktop commands and schematic rendering under `src-tauri/src/`.
- Put frontend behavior tests in `tests/*.test.mjs`, Rust unit tests beside their module, and cross-crate tests in `tests/` or `src-tauri/tests/`.
- Use the smallest suitable fixture. Do not turn an internal fixture into a public example without reviewing its purpose and license.

Use two-space indentation in Svelte and TypeScript, standard `rustfmt` formatting in Rust, PascalCase Svelte component names, camelCase TypeScript names, and snake_case Rust names.

## Correctness changes

For a correctness bug, first add a deterministic regression test that fails for the intended reason. Preserve these layout invariants:

- A rejected move restores the complete state.
- Hard legality remains separate from soft optimization cost.
- Cost does not depend on component iteration order.
- Fixed placements, bridged bodies, and existing wires are immutable geometry.
- Invalid results are never written back into a `Layout`.
- Power-rail bindings participate in legality checks.
- A seed is reproducible for the same algorithm version.

Do not weaken assertions or refactor unrelated core code to make a release or packaging change pass.

## Checks

Run the narrowest relevant test while iterating. Before submitting a pull request, run:

```bash
pnpm check
pnpm test:ui
pnpm build
pnpm check:version
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

For packaging changes, also run the applicable checks from [`docs/PACKAGING.md`](docs/PACKAGING.md). Platform-native installers must be smoke-tested on that platform before a release is published.

## Bug reports

Include:

- KneadNet version and release filename.
- Operating system and CPU architecture.
- KiCad version that created the input.
- Reproduction steps, expected behavior, and actual behavior.
- A minimal project when you have the right to share it.

KiCad projects can contain private designs and local paths. Remove confidential material before attaching a reproducer.

## Pull requests

- Explain the user-visible result and why the chosen approach is safe.
- Link the related issue when one exists.
- List every validation command and its result.
- Include before/after screenshots for UI changes.
- Identify any example project used for manual testing.
- Call out platform-specific behavior that was not tested.

Use an imperative, focused subject. Conventional Commit prefixes such as `feat:`, `fix:`, `docs:`, and `chore:` match the existing history.

By contributing, you agree that your contribution is distributed under the repository's [`GPL-3.0-only`](LICENSE) license.
