# Recall-MCP status: v0.4.0 (2026-09-20)

Exact done/partial list. "Done" means: implemented, tests executed and green
locally, committed.

## DONE (2026-09-20 release engineering: 0.4.0 tag + publishability)

1. **Baseline re-verified at as-found HEAD** (`dd2d2d0`): 161 tests
   passed / 0 failed / 3 ignored, llvm-cov recall-core 95.00% lines /
   94.03% regions, fmt + clippy clean — all matching the recorded
   2026-09-18 numbers.
2. **Crate names made publishable** (`029321f`): `cargo publish
   --dry-run` proved `recall-server` and `recall-cli` unpublishable —
   both names are owned on crates.io by unrelated projects (pimlabs/
   recall, OriginalMHV/recall). Published packages renamed to
   `recall-mcp-core` / `recall-mcp-server` / `recall-mcp-cli` (all
   three verified free, 2026-09-20); `[lib]` sections pin the short
   lib names so imports are untouched; the CLI binary stays
   `recall-cli`. Path dependencies now carry `version` (publish
   rejects bare paths). Crate versions in lockstep at workspace 0.4.0;
   the MCP `server_info` version literal followed (the pinning test
   caught the drift, as designed).
3. **Dependency audit**: `cargo audit` (0.22.2, installed via `cargo
   install` — the one tool this pass had to add) over the 396-package
   lock: zero vulnerabilities; one allowed warning for unmaintained
   transitive `paste` (RUSTSEC-2024-0436, via rmcp's macro stack; no
   upstream fix, not a vulnerability).
4. **Release stamped**: CHANGELOG restructured to Keep-a-Changelog
   order (Unreleased on top; the inverted 0.3.1/Unreleased block and
   the stale "crates remain at 0.1.0" line fixed), [0.4.0] - 2026-09-20
   recorded, notes in `releases/NOTES-0.4.0.md`, Releases section in
   README, CITATION.cff corrected from the stale 0.1.0 to 0.4.0.
   Publishing stays owner-gated; publish order core → server → cli.
5. **Not re-run this pass**: the web Playwright suite (last verified
   green with axe AA 0 violations, 2026-09-14/18 records) and the
   cosmic-ray mutation session; no retrieval or web code changed here
   (manifests + one version literal + docs only).

## DONE (2026-09-18 release readiness)

This build re-verified every gate on the current toolchain (windows-msvc,
rustc 1.98.1) and closed the release-blocking gaps.

1. **The coverage gate was realigned with reality.** The recorded
   `cargo llvm-cov --fail-under-branches` flag does not exist in
   cargo-llvm-cov 0.9.1 (branch instrumentation needs nightly; D-010
   amendment); it had never actually executed anywhere. Coverage is now
   **measured** (llvm-cov runs on msvc): recall-core
   **95.00% lines / 94.03% regions**, gate `--fail-under-lines 90
   --fail-under-regions 90` exit 0; recall-server 96.83% / 96.13%. The
   gate in ci.yml, AGENTS.md, and all docs was corrected to lines+regions.
2. **Dependency freshness**: `cargo update` (26 packages, incl. rmcp
   3.3.0 → 3.4.0: the pinned `2025-11-25` handshake re-verified by the
   conformance tests on both transports) and **fastembed 6 → 7** for the
   optional onnx feature (`InitOptions` → `TextInitOptions`; the deprecated
   alias warns in 7). The four `--features onnx` tests ran live with
   onnxruntime-win-x64-1.28.0 via ``ORT_DYLIB_PATH`; model downloaded,
   1.000 hit-rate gate green. web/ deps refreshed within carets (react
   19.3, vite 8.3); `npm audit` 0 vulnerabilities; Playwright 5/5 with axe
   AA 0 violations re-run.
3. **CLI output: embeddings omitted by default**: `remember`/`recall`/
   `update` strip the embedding vector from their JSON unless
   `--show-embedding` (a screen reader would read hundreds of zeros per
   record).. Pinned by a new e2e test; 161
   tests total, 0 failed, 3 ignored diagnostics.
4. **Security re-scan for going public**: no secrets in the working tree
   or any of the 76 commits (pattern scan; `sk-test` fixtures are inert and
   inventoried); no personal paths; client-visible errors stay short (500s
   are generic, details log-only); no key or plaintext logging; the
   encryption docs match the implementation including the
   keyed+invertible-embedder startup warning. One open item: git history carries a
   personal email address as the commit author address;
   rewriting history would change every commit hash, so the
   decision is recorded here rather than acted on.
5. **License audit**: all 395 published locked packages checked at their
   locked versions (crates.io API; cargo-deny not part of the local
   toolchain): all permissive, MIT-compatible; analysis of MIT-vs-Apache-2.0
   recorded, default stays MIT ([docs/LICENSE-AUDIT.md](LICENSE-AUDIT.md)).
6. **Docs for a public repo**: README rebuilt for the Reflow-bar
   (real pasted outputs from executed commands, configuration reference
   table, security/threat-model section, two mermaid diagrams verified
   with `mermaid.parse()`); `CONTRIBUTING.md` and `ideas.md` added;
   `docs/design.md` refreshed (D-021 range, rmcp 3.4.0, CI reality);
   `.gitattributes` pins LF endings.

## DONE (2026-09-16 GitHub migration)

The repository moved to GitHub at `YugVarshney/cortex-mcp`. Docs were synced for
the move: README badge removed while Actions is off and the README/CITATION
URLs point at `YugVarshney`; CITATION `date-released` re-stamped to the last code
change (2026-09-14). README and CHANGELOG state the Actions decision.

1. **GitHub Actions disabled on the repository** (2026-09-16): no paid CI
   minutes. The README carries a CI note recording the decision and the
   zero-cost self-hosted-runner path. The workflow file
   (`.github/workflows/ci.yml`) stays committed, ready for a future runner.
2. **Quality gates at the recorded HEAD**: every gate this toolchain can run
   was executed locally before the push (see "PARTIAL" for the one gate this
   toolchain cannot measure). The push itself is a verifiable record: the
   remote HEAD equals the locally verified commits.

## DONE (2026-09-14 p95 re-baseline)

The p95 re-baseline item closed by measurement, not by a rewrite: recalls have been
read-only since AR-018, and re-baselining the p95 harness against the current
code path shows the 25 ms PRD budget **met at exactly 10k memories** in
steady state: p95 20.1-20.9 ms across four warm/repeat runs (criterion
`hybrid_k5` 15.34 ms), versus the recorded pre-re-baseline 26.2-27.2 ms.
The previously recorded 1.2-2.2 ms breach is measured as resolved; the
improvement is consistent with the AR-018 removal of the per-recall batched
`last_accessed_at` UPDATE (mean 21.7-22.3 → 17.3-17.6 ms; no same-build A/B
possible, so attribution is stated as consistent-with, not proven).

1. **Harness re-baseline + warm-up** (`2933399`): the exact-harness doc
   still claimed the timing included the (AR-018-removed) refresh, and the
   first run after seeding buried one-time costs in the tail: the un-warmed
   first run of the freshly built binary measured p95 32.8 ms (max 101.6 ms),
   packed-cache fill + first page faults. The harness now runs 100
   untimed warm-up recalls before the timed region and reports the warm-up
   size; the budget assertion passes (2 runs recorded). Full run-by-run
   numbers in EVALUATION.md.
2. **D-002 decision: no ANN/storage-engine change at 10k.** D-002's revisit
   trigger ("p95 regresses beyond budget") is not fired at the 10k scale
   anymore; the ANN/packed-vector revisit stays mandatory beyond ~100k
   memories/namespace, unchanged. The optional FTS-pass optimization
   (keyword-only floor 14.8 ms/call cold diagnostic, 10.05 ms criterion)
   stays untriggered at 10k. No retrieval-path code changed in this round.

## DONE (2026-09-14 web UI)

Closed the last P1 gap (inventory #7): the web UI now surfaces the three
server capabilities it was missing, preserving the keyboard/screen-reader
baseline (semantic HTML, sr-only table captions, aria-live status regions,
global `:focus-visible`, no mouse-only interactions; axe AA stays at 0
violations; Playwright scans both the loaded page and the expanded metrics
state).

1. **Pagination**: a "Browse memories" section over `GET /v1/memories`
   (server-driven limit/offset; the unpaginated total comes from
   `X-Total-Count`): page-size select (10/25/50), Previous/Next page buttons
   (disabled at the ends), a live "Page x of y · a-b of n · newest first"
   status region, a stale-response guard for racing page changes, and
   auto-clamp when the store shrinks under the open page.
2. **Metrics view**: `GET /metrics` parsed into a name/value table with row
   headers (plain linear tables, no chart-only presentation) plus the raw
   Prometheus text in a keyboard-operable disclosure (focusable scrollable
   `pre`); Refresh announces the sample count politely.
3. **Capture**: a form over `POST /v1/capture`: free-text namespace with a
   datalist of existing ones (auto-created on capture, D-009; the pre-fill is
   applied once and never clobbers user input), dynamic role/content message
   rows announced as "Message n" fieldset groups with indexed Remove buttons,
   optional comma-separated tags, and the captured count announced. UI
   interpretation recorded: per-message timestamps (`at`) and
   `max_chunk_chars` are not exposed in the form; the server defaults apply
   (`created_at` = now, 1000-char chunk cap); roles are free text (any
   non-empty role is prefixed into the text and tagged `role:<role>`).

Web e2e extended to 5 Playwright tests, all green (functional coverage for
each surface, keyboard-only paging, axe AA on the loaded page and the expanded
metrics state). Vite dev proxy now forwards `/metrics`.

## DONE (2026-09-12 publication)

1. **Dependency freshness**: every direct dependency checked live against
   crates.io (2026-09-12): all at latest except reqwest (0.12.28 → 0.13.5,
   feature renamed `rustls-tls` → `rustls`; the lockfile keeps reqwest 0.12
   alongside it for fastembed's `hf-hub 0.5`, which requires `^0.12`, so
   `--all-features` still resolves). `cargo update` applied (6 transitive
   bumps). `recall-core` checks and tests green with `--features openai`.
2. **`ServerConfig` Debug redaction**: the API key no longer prints verbatim
   in Debug output (a config Debug is exactly what ends up in logs); pinned
   by test.
3. **MCP wire-negotiation truth**: live probes + the rmcp 3.3 source showed
   both transports answer a `2026-07-28` `initialize` with `2025-11-25`
   (that revision replaced the initialize handshake with per-request
   metadata). The previously claimed "2026-07-28 honored on HTTP" was stale;
   corrected across README/ARCHITECTURE/design/ALTERNATIVES/EVALUATION/
   AGENTS/D-004, and the negotiated value is now pinned in both conformance
   tests.
4. **Edge-case tests**: keyed unicode memory round-trip + digest recall
   (multi-byte text through AEAD, HMAC digests, and digest-mode queries).
5. **Publishability**: `CITATION.cff` added; CI actions bumped
   (checkout v5 → v7, upload-artifact v4 → v7; breaking changes reviewed as
   non-affecting); `lcov.info` gitignored; quickstart commands re-executed
   (CLI remember/recall/export/import/backup/vacuum, serve + REST endpoints,
   both MCP handshakes over the wire); no secrets, no absolute personal
   paths, tree clean of temp files.

## DONE (2026-09-11 security hardening)

The 2026-09-11 work addressed the 2026-09-11 adversarial review
(docs/ADVERSARIAL-REVIEW.md): the P1 browser-exploitable loopback surface, two
crypto-hygiene gaps, the MCP/REST pagination drift, the OpenAI dims bug, the
records-required mutation/property testing layers, and every actionable P3 plus
the stale-doc findings. The two v0.3 items below that STATUS previously listed
as NOT DONE (the MCP `capture` tool and ONNX intra-op thread tuning (D-020))
had in fact shipped; they are now correctly listed as DONE.

1. **AR-001 (P1): browser isolation by default**: the Host/Origin guard
   rejects foreign `Host` headers (DNS-rebinding → 421) and cross-origin
   `Origin`s (403); `CorsLayer::permissive()` is gone. CORS is granted only
   for explicit `--allow-origin` opt-ins (repeatable; also in the config
   file); `--allow-host` extends the Host allowlist for non-loopback
   bindings. README no longer describes loopback-without-key as safe.
2. **AR-002: key separation**: HKDF-SHA256 derives distinct AEAD and
   FTS-MAC subkeys (`recall-mcp:v1:aead` / `recall-mcp:v1:fts-mac`);
   pre-HKDF stores are detected at open via the key-check blob and migrated
   in one transaction (plus VACUUM + WAL checkpoint); RFC 5869 vector test
   pins the KDF.
3. **AR-003: embedding-leak disclosure**: D-014 and README now state plainly
   that dense embeddings (onnx/openai) are invertible to text
   (vec2text-style attacks) so keyed mode does not protect text with them;
   `serve` logs a warning when keyed mode runs with an invertible embedder
   (`Embedder::embedding_invertible`).
4. **AR-004: MCP `list_memories` clamp**: shared `MAX_PAGE_LIMIT` (10 000)
   via `page_limit()` on REST and MCP; the store's LIMIT/OFFSET binds
   saturate via `i64::try_from` instead of wrapping. End-to-end test with
   10 001 memories proves the cap.
5. **AR-005: OpenAI dims validated**: response vectors must match the
   configured dims exactly (error names both sides); REST create rejects
   caller-supplied embeddings of the wrong length instead of silently
   zeroing all cosines.
6. **AR-006/AR-007: mutation + property testing**: `mutants.toml` (gated on
   crypto/scorer/util modules) with the completed run recorded in
   EVALUATION.md (2026-09-11: 51 mutants: 45 caught, 0 missed, 6 unviable;
   the 2 first-run survivors were unpinned AAD wire-format literals, closed
   by a format-pinning test and confirmed on the re-run); five proptest
   targets (crypto roundtrip + wrong-AAD, FTS query validity, chunking
   invariants, scorer linearity/bounds, token-bucket invariants).
7. **P3 wave**: AR-011 rmcp 3.3.0 + uuid 1.26.1; AR-012 rate limiter wraps
   auth so key-guessing is throttled; AR-013 keyed upgrades run with
   `secure_delete=ON` and finish with VACUUM + truncated WAL checkpoint
   (forensic canary test); AR-015 scan-cap/candidate-cap approximations
   documented; AR-016 shared reqwest client; AR-017 Web-UI-without-API-key
   limitation documented; AR-018 per-recall `last_accessed_at` writes
   removed (no consumer); AR-019 AGENTS.md tool list corrected; AR-021
   export writes owner-only files and warns about plaintext; AR-022 namespace-isolation phrasing; AR-008 D-021 memory-poisoning threat model
   + untrusted-data framing in MCP tool descriptions.

## DONE (v0.3, unchanged from the 2026-09-07 builds)

- **Packed in-process vector cache (D-019)**: hybrid_k5 31.8 → 16.16 ms
  (−49.1%); exact harness p95 55.9 → 26.2-27.2 ms. The 25 ms PRD budget at
  exactly 10k is still not met on the exact harness.
- **`--embedder onnx|hash|openai` selection** (completing D-013) and
  **ONNX intra-op thread tuning (D-020)**: both shipped and tested
  (`onnx_embed.rs:63-77`, commit 0c2d07e); STATUS previously mislisted
  tuning as NOT DONE.
- **MCP `capture` tool**: shipped (mcp.rs + tests on both transports);
  STATUS previously mislisted it as NOT DONE.
- **Recall fast path (D-016)**, **production hardening (D-017)**,
  **auto-capture ingest (D-018)**, pagination + `/metrics`, CLI
  `backup`/`vacuum`, maintenance ops.

## PARTIAL

1. **Coverage gate (90% lines + regions on recall-core)**: measured locally
   on 2026-09-18 (dev machine now windows-msvc, where llvm-cov runs):
   recall-core **95.00% lines / 94.03% regions**, gate exit 0; recall-server
   measured too (96.83% / 96.13%). Method: cargo-llvm-cov 0.9.1, stable rustc
   1.98.1; numbers in docs/EVALUATION.md. Two caveats: the previously
   recorded `--fail-under-branches` flag does not exist in cargo-llvm-cov 0.9.1
   (branch instrumentation needs nightly, so true branch coverage is unmeasured,
   D-010 amendment), and CI enforcement still waits on a runner: GitHub
   Actions is disabled (2026-09-16), so the corrected gate
   in `.github/workflows/ci.yml` has not executed remotely.
2. **p95 at 10k memories, met in steady state; cold-open caveat**: the
   re-baselined harness measures p95 20.1-20.9 ms at exactly 10k (budget
   25 ms; 2026-09-14 runs in EVALUATION.md). The first run after a fresh
   build still breaches (p95 32.8 ms: one-time packed-cache fill + page
   faults), documented as a cold-open caveat rather than massaged away.
   Beyond ~100k memories/namespace the D-002 ANN revisit remains the
   structural fix.
3. **Web UI**: keyboard/screen-reader accessible, axe-tested; surfaces
   pagination, `/metrics`, and capture (2026-09-14 pass, above). Sends no API
   key (AR-017).
4. **OpenAI embedder**: logic unit-tested (request/parse/dims), selectable
   via `--embedder openai`, but never called live (no key). Not on any
   default code path.
5. **Embeddings/tags plaintext at rest**: documented decision (D-014, D-021,
   AR-003); encrypting them is a v2 candidate, not done.

## NOT DONE

- Config *file for secrets*: the JSON config intentionally excludes the API
  key and encryption key (no secrets on disk); they remain env/flag-only.
- OAuth/multi-user auth (PRD non-goal), TLS, cloud sync, entity graphs,
  consolidation flows, Team Server (multi-user; a different product).
- Generated TS client from OpenAPI (hand-typed `web/src/api.ts`; D-006).
- Any CI execution: Actions are disabled on the repo
  (2026-09-16), so the committed workflow has never run remotely; the
  coverage gate (lines + regions, D-010 amendment) is enforced locally and
  waits on a runner for remote enforcement.
