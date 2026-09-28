# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [0.4.0] - 2026-09-20

- **Publishable crate names** (2026-09-20): the published package names are
  now `recall-mcp-core`, `recall-mcp-server`, and `recall-mcp-cli` — the old
  `recall-server` and `recall-cli` names are owned by unrelated projects on
  crates.io (pimlabs/recall, OriginalMHV/recall), so they could never have
  been published. Lib targets keep their short names (`recall_core`,
  `recall_server`) via explicit `[lib]` sections and the CLI binary keeps
  its user-facing `recall-cli` name, so no import, command, or API changed.
  The path-dependency entries now carry `version` requirements (publish
  rejects them without) and every crate manifest gained `keywords` and the
  root README. Verified free on crates.io 2026-09-20; publish order is
  core, then server, then cli.
- **Release readiness** (2026-09-18): coverage gate corrected to
  lines+regions (the recorded `--fail-under-branches` flag does not exist in
  cargo-llvm-cov 0.9.1; D-010 amendment) and **measured for real**:
  recall-core 95.00% lines / 94.03% regions, recall-server 96.83% / 96.13%.
  `cargo update` (rmcp 3.4.0, negotiated-version conformance re-verified) +
  fastembed 6 → 7 with the onnx feature verified live against
  onnxruntime-win-x64-1.28.0. License audit of all 395 published locked
  packages (docs/LICENSE-AUDIT.md): all permissive, MIT-compatible. Security
  re-scan clean (tree + full history). README rebuilt for public release
  (executed quickstart outputs, configuration reference, security/threat
  model, parse-verified mermaid diagrams); CONTRIBUTING.md and ideas.md
  added.
- **CLI output: embeddings omitted by default** (2026-09-18): `remember`,
  `recall`, and `update` print their JSON without the stored embedding
  vector unless `--show-embedding` is passed. The embedding vector carried
  no signal for a human at a terminal and screen readers announced it as
  hundreds of zeros per record; score breakdowns and all other fields are
  unchanged, and the REST/MCP API surface is unaffected.
- **Dependency audit** (2026-09-20): `cargo audit` over the full 396-package
  lock reports zero vulnerabilities; the only finding is an allowed warning
  for the unmaintained transitive `paste` 1.0.15 (RUSTSEC-2024-0436, via
  rmcp's macro stack — no fix exists upstream and it is not a vulnerability).

## [0.3.1] - 2026-09-18

- Shipped binary version aligned with the recorded project status (v0.3.1). The full milestone history lives in docs/STATUS.md; the sections below carry the milestones that predate the crate-version split.

- **GitHub migration** (2026-09-16): repository moved to the GitHub
  repo `YugVarshney/cortex-mcp`. GitHub Actions is disabled on
  the repository (no paid Actions minutes); every quality gate was
  verified by local execution at the recorded HEAD. Doc sync for the move:
  README/CITATION URLs point at `YugVarshney`, the
  CI badge is removed, README status tag corrected to v0.3.1, CITATION
  `date-released` re-stamped to 2026-09-14.

- **p95 re-baseline** (2026-09-14): the p95 harness re-measured
  against the current (post-AR-018, read-only-recall) code path: the 25 ms
  PRD budget is met at exactly 10k memories in steady state (p95
  20.1-20.9 ms; criterion `hybrid_k5` 15.34 ms), closing the previously
  recorded 1.2-2.2 ms breach. The harness now warms up 100 untimed recalls
  before the timed region (cold first-run p95 32.8 ms documented as a
  one-time-cost caveat), and its stale refresh note was corrected. D-002's
  ANN revisit stays deferred beyond ~100k memories/namespace, no
  retrieval-path code changed. Numbers in `docs/EVALUATION.md`.
- **Web UI** (2026-09-14): the console now surfaces the three server
  capabilities it was missing, preserving the keyboard/screen-reader baseline:
  server-driven pagination over `GET /v1/memories` (page-size select, live
  status region, stale-response guard), a `/metrics` view (name/value table
  plus keyboard-operable raw Prometheus disclosure), and a capture form over
  `POST /v1/capture` (dynamic message rows, namespace datalist, aria-live
  result). Playwright e2e extended to 5 tests incl. keyboard-only paging and
  axe AA scans of the loaded and expanded states. Details in `docs/STATUS.md`.
- **Publication readiness** (2026-09-12): dependency freshness (reqwest 0.12 →
  0.13, feature rename `rustls-tls` → `rustls`; every other direct dependency
  verified at its crates.io latest), `ServerConfig` Debug now redacts the API
  key, negotiated MCP protocol version pinned on both transports with the
  docs corrected to match (rmcp 3.3 answers `2025-11-25` to a `2026-07-28`
  initialize; that revision replaced the handshake with per-request
  metadata), keyed unicode round-trip test, `CITATION.cff`, CI actions bumped
  (checkout v7, upload-artifact v7), 160 tests green.
- **v0.3.1** (2026-09-11): adversarial-review hardening: same-origin CORS
  default with Host validation, HKDF key separation with store migration,
  OpenAI dims validation, MCP/REST page-limit parity, atomic import,
  rate-limiter-before-auth, mutation + property testing layers (mutation
  score recorded in `docs/EVALUATION.md`), doc true-ups.
- **v0.3** (2026-09-07): production hardening: rate limiting, `/metrics`,
  transcript auto-capture, hot backup (operations surface documented in the
  README).
- **v0.2** (2026-09-06): measured status/evaluation work, including
  p95 breach, plus ONNX and encryption sections.
- **v0.1**: initial implementation (see git history).
