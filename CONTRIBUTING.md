# Contributing to Recall-MCP

Thanks for looking at this project. It is an MCP memory broker with one
engine behind three surfaces (MCP, REST, CLI), a pure-domain core crate, and
a small web UI. This file covers what you need to build it, the gates your
change must pass, and the conventions the codebase already follows.

## Setup

- Rust stable (1.88+; the workspace uses edition 2024). Windows needs a C
  compiler for bundled SQLite (MSVC via Visual Studio Build Tools, or
  MinGW-w64 gcc).
- Node 24+ only if you touch `web/`.
- Nothing else. The default build is fully offline; the optional `onnx`
  embedder downloads a model on first use and needs `ORT_DYLIB_PATH` pointing
  at an ONNX Runtime shared library (see `docs/adr/DECISIONS.md` D-013).

## Build and test

```sh
cargo build --workspace
cargo test --workspace
```

The suite includes unit tests, integration tests against the real binary,
MCP conformance tests on both transports (stdio and streamable-HTTP), a
golden-set retrieval eval, and property-based tests. Three tests are
`#[ignore]`d diagnostics (FTS query-plan probe, release-only p95 and cost
harnesses); two more are behind `--features onnx` and need network on first
model download.

## Gates (all of them, before you push)

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo llvm-cov -p recall-core --fail-under-lines 90 --fail-under-regions 90 --summary-only
```

Coverage: 95% lines / 94% regions at the time of writing. New core code
lands with tests in the same commit. cargo-llvm-cov has no branch-level
fail-under on stable Rust (see the D-010 amendment); do not weaken the gate
further.

If you touch `web/`:

```sh
cd web && npm install && npm run build && npx playwright test
```

axe (wcag2a/aa + wcag21a/aa) must stay at 0 violations. The UI is built for
keyboard and screen-reader use first: semantic HTML, labelled controls,
aria-live for async status, visible focus. No mouse-only interactions.

## Conventions

- Conventional commits (`feat:`, `fix:`, `docs:`, `test:`, `chore:`), one
  green change per commit, tests in the same commit as the change.
- `crates/recall-core` is a pure domain crate: no axum, rmcp, tokio, or HTTP
  types. Server and CLI are thin shells; the same engine serves MCP, REST,
  and CLI, and behavior must not drift between them.
- No `unwrap`/`expect` outside tests; every justified clippy opt-out is a
  local `#[allow]` with a comment naming the invariant that makes it safe.
  No new blanket suppressions.
- Ranking behavior changes need a golden-set argument (the eval gate is
  hit-rate@5 >= 0.85) and, if they touch the scorer or crypto modules, a
  cargo-mutants run over those modules.
- Changes to wire behavior (MCP tool shapes, REST paths, config flags) must
  update: the OpenAPI document, the conformance tests, and the docs in the
  same PR.
- Decisions live in `docs/adr/DECISIONS.md` (D-001 onward). If you believe a
  decision is wrong, propose an amendment in your PR instead of silently
  deviating.

## What is in scope right now

Bug fixes, hardening, docs, test coverage, toolchain and dependency
maintenance. Larger product ideas (ANN retrieval beyond ~100k
memories/namespace, cold-open latency work, encrypted embeddings) are
deliberately deferred; see `ideas.md` before proposing an implementation.

## Reporting issues

Include: what you ran, the exact command, expected vs actual, and the
`cargo clippy`/`cargo test` state of your tree. For MCP questions, state the
client and the negotiated protocol version (the server answers `2025-11-25`
to a `2026-07-28` initialize; that revision replaced the handshake with
per-request metadata; see D-004).

## License

By contributing you agree that your contributions are licensed under the
MIT License that covers the repository.
