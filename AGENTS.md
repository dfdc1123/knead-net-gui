# Repository Guidelines

## Project Structure & Module Organization

This repository combines a SvelteKit frontend, a Tauri desktop shell, and a Rust layout engine.

- `src/routes/` contains application pages; reusable Svelte components live in `src/lib/components/`.
- `src/input/`, `src/circuit.rs`, and `src/layout/` implement KiCad parsing, circuit models, placement, cost calculation, and routing.
- `src-tauri/src/` exposes desktop commands and schematic processing; `src-tauri/tests/` contains integration tests.
- `static/` holds web assets, while `src-tauri/icons/` holds packaged application icons.
- `examples/folders/` provides complete KiCad projects for manual testing; `docs/screenshots/` contains README imagery.

Keep UI-only changes under the Svelte tree and reusable algorithmic logic in the root Rust crate.

## Build, Test, and Development Commands

Use pnpm and a stable Rust toolchain:

- `pnpm install` installs frontend and Tauri CLI dependencies.
- `pnpm tauri dev` runs the desktop application with hot reload.
- `pnpm dev` starts only the Vite frontend.
- `pnpm check` synchronizes SvelteKit types and runs TypeScript/Svelte diagnostics.
- `pnpm build` creates the static frontend build; `pnpm tauri build` packages the desktop app.

## Coding Style & Naming Conventions

Use two-space indentation in Svelte and TypeScript, and standard `rustfmt` formatting in Rust. Keep TypeScript strict and prefer explicit exported types. Name Svelte components in PascalCase (`BreadboardPreview.svelte`), TypeScript identifiers in camelCase, and Rust modules/functions in snake_case. Follow existing Tailwind/DaisyUI patterns instead of adding isolated CSS where practical.

## Verification

For focused changes, run the narrowest relevant test first. Rust unit tests belong in a module-local `#[cfg(test)] mod tests`; cross-crate tests belong in `src-tauri/tests/`. Use descriptive snake_case names and minimal `examples/` fixtures where useful. UI changes must pass `pnpm check` and be exercised in `pnpm tauri dev`.

Before declaring a task complete, run:

- `cargo fmt --check`
- Relevant crate or module tests, for example `cargo test -p knead-net layout::cost`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`

## Change Discipline

- Verify reported issues against the current `HEAD` before modifying code.
- Reproduce correctness bugs with a deterministic regression test first; it must fail against the old implementation for the intended reason.
- Apply the smallest coherent production-code fix. Never weaken assertions to make tests pass.
- Do not perform unrelated refactors or repository-wide formatting.
- Preserve public APIs unless the task explicitly requires changing them.
- Do not commit unless explicitly requested.

## Layout and SA Invariants

- Rejected moves restore the complete state.
- Hard legality is separate from soft optimization cost, which is invariant to component iteration order.
- Fixed placements, bridged bodies, and existing wires are immutable geometry.
- Invalid results must not be written back into `Layout`.
- A `Bridged` placement must completely and uniquely represent a valid two-pin component.
- Power-rail bindings participate in legality validation.
- Seed reproducibility means the same algorithm version plus the same seed.

## Commit & Pull Request Guidelines

Recent history generally follows Conventional Commit prefixes such as `feat:`, `fix:`, and `docs:`, with optional scopes like `fix(step4):`. Keep subjects imperative and focused. Pull requests should explain the user-visible effect, link related issues, and list validation commands. Include before/after screenshots for UI changes and identify any KiCad example used for manual verification.

## Completion Report

Report the confirmed root cause, tests added, files changed, commands run and results, remaining risks, and any assumptions or unverified paths.
