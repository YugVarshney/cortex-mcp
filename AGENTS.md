# AGENTS.md — Recall-MCP

## What this is (3 lines)
Local-first semantic memory broker for AI agents: a Rust MCP server
(stdio + streamable-HTTP, built on the SDK for the 2026-07-28 spec — the wire
`initialize` handshake answers `2025-11-25`, see D-004) exposing
remember/recall/forget/update_memory/
capture/list_memories over one SQLite file, plus REST, OpenAPI, web UI, CLI.
Retrieval is hybrid BM25 + cosine + recency decay with a per-result score
breakdown — never a black-box top-k. Read docs/PRD.md and docs/STATUS.md
before planning work.

## Architecture boundaries (hard rules)
- `crates/recall-core` is a pure domain crate: no axum, no rmcp, no tokio,
  no HTTP/JSON types. Models, `Store` trait, `Embedder` trait, `HybridScorer`.
- `recall-server` (axum + rmcp) and `recall-cli` (clap) are thin shells over
  core. Same engine behind REST, MCP, and CLI — no logic drift between them.
- Decisions live in docs/adr/DECISIONS.md (D-001…D-021). Do not silently
  deviate: if a decision is wrong, propose an amendment first.

## Style guides (load before writing code)
- docs/style-guides/rust-api-guidelines.md (C-* checklist; core is the public API)
- docs/style-guides/rust-error-handling.md (typed errors in core; anyhow only in app crates)
- docs/style-guides/rust-tooling-conventions.md (Cargo/rustfmt/clippy/perf conventions)

## Testing policy
- Gate commands: `cargo test --workspace`, `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`. All green before
  any commit is "done".
- Coverage: 90% lines AND regions on recall-core via
  `cargo llvm-cov -p recall-core --fail-under-lines 90 --fail-under-regions 90`.
  (Measured 2026-09-18 on windows-msvc: 95.00% lines / 94.03% regions.
  cargo-llvm-cov 0.9.1 has no branch fail-under — `--branch` needs nightly —
  so true branch coverage is a recorded tooling gap, D-010 amendment.)
- windows-gnu caveat: llvm-cov cannot run on windows-gnu (D-010) — measure on
  the windows-msvc dev machine or let linux-gnu CI enforce it.
  Never estimate or fabricate a coverage number.
- Web: `cd web && npm run build && npx playwright test` (axe AA must stay 0 violations).
- Retrieval quality: evals/ golden set gates hit-rate@5 >= 0.85 in tests.

## Commits
Conventional commits (`feat:`, `fix:`, `docs:`, `test:`, `chore:`), one
green slice per commit, tests in the same commit as the change they cover.

## Accessibility (web/)
The web UI targets screen-reader users. Semantic HTML first; every control
keyboard-operable with visible `:focus-visible`; labels bound to inputs;
aria-live for async status; test with @axe-core/playwright, keep WCAG 2.1 AA
at 0 violations. No mouse-only or hover-only interactions.

## Verify before claiming
"Done" = implemented, tests executed and green locally, committed.
Never claim a pass, benchmark, or coverage number you did not run. State
partial/incomplete work plainly in docs/STATUS.md — reviews check it.
