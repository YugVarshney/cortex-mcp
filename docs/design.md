# Recall-MCP design

This is the design narrative for Recall-MCP: how the system is put together, how
data flows through it, and why each structural choice was made. The
per-decision record lives in [`docs/adr/DECISIONS.md`](adr/DECISIONS.md)
(D-001…D-021); this document does not re-litigate those entries; it references
them and adds the connective tissue: the layering, the recall data path, and
the reasoning that spans multiple decisions. Where a number appears, it was
measured (see [`docs/EVALUATION.md`](EVALUATION.md) for run-by-run provenance).

## What the system is

A local-first semantic memory broker for AI agents, packaged three ways from
one core: an MCP server (stdio + streamable-HTTP) so any MCP-capable agent can
`remember`/`recall`/`forget`/`update_memory`/`list_memories`/`capture`, a plain
HTTP/JSON API (`/v1/*`) with a hand-written OpenAPI document, and a CLI for
scripting that uses the same engine. One SQLite file is the whole store. The
product thesis is **explainable retrieval**: every recall result carries its
score breakdown `{bm25, vector, recency, pinned_boost, total}`, never a
black-box top-k; the web UI renders the breakdown per result, which is the
reason the scorer's components are individually computed and unit-tested
rather than folded into one opaque similarity number.

## Layering

```
crates/
  recall-core/    pure domain. No axum, no tokio, no MCP types.
                  models.rs      Memory, Namespace, RecallParams, MemoryUpdate
                  store.rs       Store trait (the only storage contract)
                  sqlite.rs      the SQLite implementation (FTS5, WAL, crypto,
                                 packed vector cache, maintenance ops)
                  embed.rs       Embedder trait + HashEmbedder + selection
                  onnx_embed.rs  OnnxEmbedder (feature `onnx`, fastembed/ort)
                  openai_embed.rs OpenAIEmbedder (feature `openai`)
                  scorer.rs      HybridScorer — the explainable ranking math
                  crypto.rs      ChaCha20-Poly1305 + HMAC token digests (D-014)
                  capture.rs     deterministic transcript chunking (D-018)
                  export.rs      JSON export/import (plaintext backup format)
  recall-server/  axum REST (/v1/*), rmcp MCP (streamable-HTTP + stdio via the
                  CLI), auth.rs (constant-time API key), ratelimit.rs (token
                  bucket), body limit, metrics.rs (Prometheus), openapi.rs
  recall-cli/     clap surface: serve (HTTP+MCP / --stdio), remember, recall,
                  update, forget, export, import, backup, vacuum
web/              React 19 + Vite SPA — a pure API client (D-006), served from
                  the Rust binary via --web-dir; Playwright + axe e2e
evals/            fixture corpus (40 memories) + golden set (50 queries)
```

The reason for the pure-core crate (D-001) is not aesthetic: the coverage
gate (>= 90% lines and regions on `recall-core`) is only achievable because
the scorer, store, embedders, chunker and crypto are testable without an HTTP
runtime, and the MCP transport is only swappable because nothing in core knows
it exists. The cost is deliberate duplication at the edges (REST, MCP and CLI
each re-validate input), accepted because the alternative (a shared
validation layer typed for async HTTP) would drag axum types back into core.

## The recall data path

One query, in order, with the file that owns each stage:

1. **Candidate generation, keyword side.** FTS5 `MATCH` over the
   external-content index `memories_fts`, trigger-synced to `memories`
   (in `sqlite.rs`). BM25 ranks candidates; the BM25 score is normalized
   against the candidate set, not the corpus, so `norm_bm25` is comparable
   across queries. In encrypted mode (D-014) both documents and queries are
   HMAC token digests, so BM25 sees identical token multiplicities, plaintext
   never touches disk.
2. **Candidate generation, vector side.** An exact cosine scan over the
   namespace's embedding BLOBs. This is the D-002 decision: no sqlite-vec, no
   ANN index, exact results. Three engineering passes made the scan fast
   without making it approximate:
   - **D-016 (slim scan + exact top-k heap):** the vector pass reads only
     `(rowid, embedding, created_at, pinned)`, computes cosine straight from
     BLOB bytes with the query norm hoisted out of the loop
     (`util::cosine_f32_blob*`, bit-identical to the reference, unit-tested),
     and keeps a bounded max-heap of the k best *vector-only* rows. Outside
     the FTS candidate set bm25 is 0, so a row's vector-only total **is** its
     final total; the heap provably contains every row that could reach the
     final top-k. Full rows are fetched only for survivors (≤ candidate_cap +
     k rows). Measured: p95 67.2 → ~34 ms at 10k.
   - **D-019 (packed vector cache):** the first untagged vector pass packs the
     scan columns into contiguous memory (`PackedNamespace`: row-major f32 +
     parallel rowid/created_at/pinned arrays); repeat recalls stop re-reading
     ~10 MB of BLOBs from SQLite. Invalidation is total on any write (writes
     are rare next to reads); tag-filtered recalls bypass the cache (it
     carries no tags); the packed cosine is bit-identical to the reference.
     Measured: criterion `hybrid_k5` 31.8 → 16.2 ms (−49%); exact-harness p95
     26-27 ms at the time of the change. Re-baselined 2026-09-14 after AR-018
     (recalls are read-only): exact-harness p95 20.1-20.9 ms in steady state;
     the 25 ms PRD budget at exactly 10k is met (EVALUATION.md has the
     run-by-run numbers; the first run after a fresh build pays one-time
     packed-cache fill and stays slower).
3. **Scoring.** `scorer.rs::HybridScorer` computes each component
   independently:
   `total = w_bm25·norm_bm25 + w_vector·cosine + w_recency·exp(−Δdays/τ) +
   pinned_boost`, defaults 0.45/0.45/0.10, τ=30d, boost 0.2, all tunable per
   request. Components are summed, never blended before the breakdown is
   recorded; this is what makes the web UI's explanation honest.
4. **Ordering.** Deterministic tie-break: total desc, created_at desc, then
   rowid asc (D-016 changed this from the random UUID id; only
   content-identical duplicates straddling the k-boundary are affected).
5. **Post-processing.** Top-k full rows fetched; recall is read-only over
   `last_accessed_at` (the access-time refresh was removed in the 2026-09-11
   pass, AR-018: it had no consumer and cost a write per recall).

## Design decisions that shape everything else

**SQLite, single file, exact scan (D-002, amended by D-016/D-019).** The
local-first promise is "one file you can back up by copying." SQLite with WAL
delivers it; FTS5 delivers keyword search; the vector index is a BLOB column
plus brute-force cosine. Alternatives rejected: sqlite-vec (extension-loading
risk on windows-gnu, and approximate ANN is the wrong trade at a documented
1M-row ceiling when exactness is what makes breakdowns explainable); a
client-side vector DB (breaks the single-file property). Revisit trigger,
formally measured, not speculative: beyond ~100k memories/namespace, or if the
remaining ~15 ms FTS floor ever matters; LanceDB is the first choice on
windows-gnu when that day comes.

**HashEmbedder by default (D-003), real semantics behind a feature (D-013).**
The default embedder is deterministic feature hashing (FNV-1a to 256 buckets,
sign from the top bit, L2-normalized): zero downloads, platform-stable, and
good enough that the golden-set hit-rate@5 is 0.980 against a 0.85 gate. The
ONNX embedder (all-MiniLM-L6-v2 quantized, 384-dim, behind the `onnx`
feature) scores 1.000 on the same set, and the one lexical miss is fixed by real
semantics, but it requires a runtime DLL at runtime (D-013: ort load-dynamic;
the official ONNX Runtime shared library is loaded via `ORT_DYLIB_PATH`).
Selection is runtime, fail-closed (`--embedder
onnx|hash|openai`; unknown names are typed errors; feature-gated embedders
refuse with the required cargo feature named, never a silent fallback to
`hash`, which would silently corrupt recall quality). A store must be read
with the embedder that wrote it.

**The store trait is `Send`, not `Sync` (D-007).** rusqlite's `Connection` is
`!Sync`; rather than leak a mutex into every method signature,
`AppState::with_store(|store| …)` owns the locking (poison-recovering) and the
trait requires only `Send`. The D-019 cache lives in a `RefCell` inside
`SqliteStore`, sound under the same single-threaded-access argument. A
re-entrant lock bug in the CLI remember path was caught by e2e and fixed by
holding one guard per logical operation; the pattern is now "one guard per
logical operation" everywhere.

**Encryption as an application layer, not a file format (D-014).** ChaCha20-
Poly1305 over `memories.text` with per-record nonces and AAD binding to the
row id; keyword search survives because FTS5 indexes HMAC-SHA256 token
digests under the same key. Alternative rejected: SQLCipher, whole-file
encryption changes the file format for every user, drags an OpenSSL-shaped
dependency onto windows-gnu, and still leaves the FTS content table
plaintext unless separately encrypted. The scope is stated plainly:
`memories.text` only; namespace names, tags, source, embeddings and `export`
output stay plaintext; in-place-upgraded databases may retain remnants in
freelist pages (`vacuum`/`backup` into a fresh keyed DB for strict
guarantees).

**MCP via the official SDK, no fallback (D-004).** `rmcp` 3.4.0 targets the
`2026-07-28` spec; tools are macro-defined. One SDK behavior is documented
rather than fought: on both transports an `initialize` naming `2026-07-28`
is answered `2025-11-25`, the newest version that still has an initialize
handshake, because the 2026-07-28 revision replaced the handshake with
per-request metadata (pinned in both conformance tests). MCP
`remember` and `capture` auto-create namespaces (D-009) because the PRD user
story is an agent with no setup step; REST stays strict (404 on unknown
namespace); the asymmetry is intentional and documented in the tool
description.

**Hardening defaults that keep loopback ergonomics (D-017).** Constant-time
API-key comparison; 1 MiB body limit; optional per-key token-bucket rate
limiting (off by default, the local-first single-user default, with a
shared `anonymous` bucket that also throttles key-guessing when on);
Prometheus metrics; graceful shutdown that drains in-flight requests and
checkpoints the WAL (SIGTERM is a no-op arm on Windows); config precedence
flag > env > JSON file > default, resolved per knob, with secrets never
file-configurable. Rejected: tower-governor (a dependency for ~60 lines of
bucket math) and unauthenticated `/metrics` (leaks store sizes when a key is
set).

**Capture: deterministic chunking in pure core (D-018).** Transcript ingest
splits messages with paragraph → sentence → hard-wrap boundaries (greedy
packing, unicode-safe, deterministic; default 1000 chars, clamped
100..=8000). Each chunk embeds itself and carries `role:<role>` tags, so
tag-filtered recall works without new tables. The chunk/embed/commit pipeline
lives in `recall-server::capture`, shared verbatim by `POST /v1/capture` and
the MCP `capture` tool (added 2026-09-08, closing D-018), so the two surfaces
cannot drift; embedding happens outside the store lock on both paths.
REST-first was deliberate (hooks speak HTTP), with the MCP tool added once
the pipeline proved out.

## Testing and verification

- **Unit/integration:** 126 tests, green (`cargo test --workspace`);
  recall-core alone is 85, covering scorer math, embedder determinism,
  store CRUD, update semantics, tag filtering, FTS sync, crypto properties,
  capture chunking, cache reuse/invalidation policy, and heap-exactness
  against brute force at 300 memories.
- **Quality gate:** golden-set hit-rate@5 ≥ 0.85
  (`crates/recall-core/tests/golden_eval.rs`, 50 queries in
  `evals/golden.json`): Hash 0.980, ONNX 1.000.
- **Latency:** two harnesses on a 10k fixture: criterion
  (`crates/recall-core/benches/recall_latency.rs`) and an exact-percentile
  release-only test (`tests/p95_latency.rs`), because criterion means and
  exact p95 answer different questions. Run-to-run variance is
  ±10-15%; multiple runs are listed in EVALUATION.md, never averaged silently.
- **MCP conformance:** the same initialize → tools/list → tools/call smoke
  over both transports (`recall-server/tests/mcp_http.rs`,
  `recall-cli/tests/mcp_stdio.rs`).
- **E2E + accessibility:** Playwright against one production-shaped server
  (`recall-cli serve --web-dir web/dist`, D-011, no vite dev server in the
  e2e path), axe wcag2a/aa + wcag21a/aa: 0 violations.
- **Coverage:** `cargo llvm-cov` gate on recall-core (lines + regions >= 90;
  cargo-llvm-cov 0.9.1 has no branch fail-under, D-010 amendment). Measured
  locally on the windows-msvc dev machine 2026-09-18: 95.00% lines / 94.03%
  regions (EVALUATION.md has the per-file table); windows-gnu still cannot
  run it (D-010), and linux-gnu CI enforces the same gate remotely.

## Known limitations and debt

Named here rather than left for a reader to trip over:

- The 25 ms PRD p95 budget is met at exactly 10k memories in steady state
  (p95 20.1-20.9 ms; re-baselined 2026-09-14 in EVALUATION.md). Cold-open
  recall (first run after a fresh build) still pays one-time packed-cache
  fill and breaches briefly; beyond ~100k memories/namespace the D-002 ANN
  revisit remains the structural fix.
- The OpenAI embedder compiles and its request/parse logic is tested, but no
  live API call has ever been made (no key); it is not on any default path.
- Encryption scope is `memories.text` only; `export` is plaintext by design.
- The rate limiter keys on the presented API key; distributed key-rotation
  floods are out of its threat model.
- CI is committed but has never executed remotely (GitHub Actions disabled, 2026-09-16); all gates are verified by local execution.
- `/metrics` counts its own request only after the body is rendered;
  scrape-in-scrape self-inclusion is off by one by design.
