# Style guide: toolchain conventions (Cargo, rustfmt, clippy, performance)

Sources (live 2026-09-06):
- Cargo Book (official): <https://doc.rust-lang.org/cargo/> (workspaces,
  features, registry/semver conventions)
- rustfmt: <https://github.com/rust-lang/rustfmt> (default style; the tool
  is authoritative: do not fight it with a heavily customized
  `rustfmt.toml`)
- Clippy: <https://doc.rust-lang.org/clippy/> (lint groups, `#[allow]` etiquette)
- The Rust Performance Book: <https://nnethercote.github.io/perf-book/>
  (profiling-first methodology; relevant to the p95 < 25 ms recall budget)
- Feature naming per Rust API Guidelines **C-FEATURE**; metadata per **C-METADATA**

## Cargo conventions we adopt

- **Workspace-first**: shared deps pinned once in the workspace
  `Cargo.toml` via `[workspace.dependencies]` and `workspace = true` in
  members (cargo 1.64+ feature; keeps the three crates in lockstep).
- **Semver discipline** (Cargo Book: "Semver Compatibility"): pre-1.0 crates
  treat minor as breaking: bump `0.x` when a public type changes; document
  in release notes (C-RELNOTES).
- **Features are additive, noun-named** (C-FEATURE): `openai` exists; new
  ones follow (`onnx`, `sqlcipher`). No `default = [...]` surprises: the
  default build stays offline and dependency-light (D-003).
- **C-METADATA**: every published crate carries `description`, `license`,
  `repository`, `readme`, `keywords`, `categories`.
- **Bundled builds**: `rusqlite` stays `features = ["bundled"]` so windows-gnu
  (D-005) needs no system SQLite; document gcc as the only external prereq.

## rustfmt

- **Default style, `cargo fmt --all` in CI/pre-commit.** No `rustfmt.toml`
  unless a concrete need appears; the rustfmt project explicitly treats the
  default style as the ecosystem norm and stable-channel options are frozen.

## Clippy

- Gate: `cargo clippy --workspace --all-targets -- -D warnings` (0-warning
  bar already met 2026-09-06).
- Opt **in** per-crate, with justification comments, to a curated set of
  `pedantic` lints (e.g. `clippy::unwrap_used`, `clippy::expect_used`,
  `clippy::dbg_macro`, `clippy::todo`) instead of blanket `#![warn(clippy::pedantic)]`
  noise. `#[allow(...)]` requires a one-line reason.
- Never silence a lint by deleting the check it pointed at.

## Performance methodology (Performance Book)

- Measure before optimizing: the book's workflow is profile → identify →
  fix → re-measure. For the p95 < 25 ms / 10k-memories PRD budget, build a
  criterion benchmark on `HybridScorer` + the recall scan path *before*
  touching code (see docs/ALTERNATIVES.md, benchmarks section).
- Default low-risk wins documented by the book: avoid unnecessary
  allocation/clones in the hot path, prefer iterators over collect-then-scan,
  avoid bounds-check-avoiding `unsafe` (we don't need it), build with
  release + `codegen-units=1` for the shipped binary.

## Testing conventions (Cargo Book + ecosystem norm)

- Unit tests inline (`#[cfg(test)] mod tests`) in `recall-core`; integration
  tests per crate in `tests/`; one doc-test per public API item (C-EXAMPLE).
- Commands are non-negotiable: `cargo test --workspace`,
  `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`.
- windows-gnu caveat: `cargo llvm-cov` cannot run on windows-gnu (no profiler
  runtime, D-010); it runs on windows-msvc and is enforced on linux-gnu CI.
  Gate: lines + regions >= 90 on recall-core (cargo-llvm-cov 0.9.1 has no
  branch fail-under: D-010 amendment).
