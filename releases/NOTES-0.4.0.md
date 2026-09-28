# Recall-MCP 0.4.0

Released 2026-09-20. Tag: `v0.4.0` (local; pushing and crates.io
publication are owner-gated). Previous recorded project version: 0.3.1
(2026-09-18).

## What this release is

The release that makes the three crates publishable, plus the
release-readiness work recorded on 2026-09-18. Retrieval behavior is
unchanged; the one user-visible CLI change (embedding vectors hidden by
default) landed on 2026-09-18 and is recorded below.

## Crate names changed (published names only)

`cargo publish --dry-run` exposed that the old names were never
publishable: `recall-server` and `recall-cli` on crates.io belong to
unrelated projects (pimlabs/recall and OriginalMHV/recall, checked
2026-09-20). The crates now publish as:

- `recall-mcp-core` (was `recall-core`, name still free but prefixed for
  family consistency)
- `recall-mcp-server` (was `recall-server` — taken)
- `recall-mcp-cli` (was `recall-cli` — taken)

What did not change: the lib targets keep their short names via explicit
`[lib]` sections (`recall_core`, `recall_server`), so every `use` line
is untouched; the CLI binary is still `recall-cli`; the REST API, MCP
surface, and SQLite formats are unchanged. Path dependencies now carry
`version` requirements (publish rejects bare paths once the path is
stripped), and each manifest declares `keywords` and the root README.

## Also in 0.4.0

- Release readiness (2026-09-18): the coverage gate was corrected to
  lines+regions and measured for real — recall-core 95.00% lines /
  94.03% regions, recall-server 96.83% / 96.13%; rmcp updated to 3.4.0
  with the negotiated-version conformance re-verified; fastembed 6 → 7
  verified live against onnxruntime-win-x64-1.28.0; a license audit of
  all 395 locked packages found everything permissive and
  MIT-compatible (docs/LICENSE-AUDIT.md).
- CLI output: `remember`, `recall`, and `update` no longer print the
  stored embedding vector unless `--show-embedding` is passed — a screen
  reader used to announce hundreds of zeros per record. Score
  breakdowns and every other field are unchanged.
- Dependency audit (2026-09-20): `cargo audit` over the full
  396-package lock reports zero vulnerabilities. One allowed warning
  stands, recorded honestly: the transitive `paste` crate
  (RUSTSEC-2024-0436) is unmaintained; it arrives through rmcp's macro
  stack, has no upstream fix, and is not a vulnerability.

## Quality gates at this tag

All executed locally at the tagged HEAD (GitHub Actions is disabled on
this repository):

- 161 tests passed, 0 failed, 3 ignored (latency harnesses on idle
  machines only).
- Coverage gate green: recall-core 95.00% lines / 94.03% regions
  (floor 90/90).
- `cargo fmt --check` and `cargo clippy --workspace --all-targets --
  -D warnings` clean.
- `cargo publish --dry-run` green for `recall-mcp-core`; server and cli
  dry-runs verify everything except the sibling resolution that can
  only exist after core's real publish — publish order: core, server,
  cli.
- `cargo audit`: zero vulnerabilities (one recorded unmaintained
  warning above).

## Publish checklist (owner-gated)

1. A crates.io account; the token exported as `CARGO_REGISTRY_TOKEN`.
2. From a clean tree at the tagged commit, in order, waiting for each
   to land:
   `cargo publish -p recall-mcp-core` → `-p recall-mcp-server` →
   `-p recall-mcp-cli`.
3. Push the `v0.4.0` tag and cut the GitHub Release from this file.
