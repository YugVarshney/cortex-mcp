# Recall-MCP architecture

Describes the **shipped v0.3 design** (2026-09-11 revision; the pre-implementation
v1 sketch (sqlite-vec, trait `Scorer`, `--embedding-api`, utoipa) was abandoned
during build-out; history is in git, decisions in docs/adr/DECISIONS.md).

**Stack:** Rust 2024 (GNU toolchain, Windows) · Axum 0.8 + Tokio · SQLite via
rusqlite (bundled, FTS5) · rmcp 3.3 (targets the MCP 2026-07-28 spec; the
wire `initialize` handshake answers `2025-11-25`, see D-004) · React 19 +
Vite 8 + TypeScript + Tailwind 4 · `cargo test` + `cargo llvm-cov` +
Playwright/axe.

## Components

```
crates/
  recall-core/       # pure domain: models, Store trait + SqliteStore (FTS5, WAL,
                     # BLOB embeddings, packed vector cache, maintenance ops),
                     # crypto (ChaCha20-Poly1305 + keyed FTS digests), Embedder
                     # trait (HashEmbedder default; OnnxEmbedder/OpenAIEmbedder
                     # behind cargo features), concrete HybridScorer, capture
                     # chunking, export/import
  recall-server/     # axum REST API (/v1/*), hand-written OpenAPI document,
                     # /metrics (Prometheus 0.0.4), auth + rate-limit + origin
                     # guard middleware, rmcp MCP tools over streamable-HTTP and
                     # stdio, shared capture pipeline
  recall-cli/        # clap CLI: serve (HTTP+MCP / --stdio), remember, recall,
                     # update, export, import, backup, vacuum
web/                 # React 19 + Vite 8 + TS UI; talks to the HTTP API only
evals/               # fixture corpus (40 memories) + golden set (50 queries)
```

Rationale (ADR-001): workspace crates keep the domain pure (no axum/rmcp/tokio
types in core), which is what makes the coverage gate (lines + regions >= 90)
achievable and the MCP transport swappable.

## Data model (SQLite, single file; D-002 as amended)

```sql
store_meta(key TEXT PK, value TEXT)           -- at-rest mode marker + key-check blob
namespaces(id TEXT PK, name TEXT UNIQUE, created_at INTEGER)
memories(id TEXT PK, namespace_id TEXT FK REFERENCES namespaces ON DELETE CASCADE,
         text TEXT NOT NULL,                  -- ciphertext in keyed mode (D-014)
         fts_tokens TEXT NOT NULL DEFAULT '', -- plaintext text OR keyed HMAC digests
         tags TEXT /*json*/, embedding BLOB /*f32 LE, nullable*/,
         source TEXT DEFAULT 'agent', pinned INTEGER DEFAULT 0,
         created_at INTEGER, last_accessed_at INTEGER /*write-time only, AR-018*/)
memories_fts(fts_tokens)   -- FTS5, external-content over memories, trigger-synced
```

There is **no vector extension**: embedding similarity is a brute-force exact
cosine scan over the BLOBs in rowid order (D-002 amended; sqlite-vec was
abandoned), with an exact top-k candidate heap and a slim scoring-columns scan
(D-016), served from a packed in-memory vector cache that any write invalidates
(D-019). `scan_cap` (default 10 000 rows/namespace) bounds the vector scan;
keyword candidates are capped at `candidate_cap` (200); both bounds are
documented approximations (README "Ranking pipeline").

## Encryption at rest (D-014, optional per database)

`RECALL_MCP_KEY` (64 hex) or `RECALL_MCP_KEY_FILE`. The 256-bit master key is
HKDF-SHA256-split into an AEAD subkey (`recall-mcp:v1:aead`) and an FTS-MAC
subkey (`recall-mcp:v1:fts-mac`; key separation, AR-002). `memories.text`
is `enc:v1:<base64(nonce‖ct‖tag)>` under ChaCha20-Poly1305 (random 96-bit
nonce per record, safe to ~2^32 encryptions; AAD binds the row id); the FTS
index holds HMAC-SHA256 token digests truncated to 128 bits, so BM25 works on
ciphertext. A stored key-check blob makes a wrong key fail at open. Plaintext
databases are upgraded in place on first keyed open (transaction, with
`secure_delete=ON`, VACUUM + truncated WAL checkpoint, AR-013); pre-HKDF
stores are migrated automatically. Documented residual plaintext:
`tags`/`source`/embeddings (invertible for dense embedders, AR-003) and
`export` output.

## Embedders

Trait `Embedder` (`dimensions/name/embed/embedding_invertible`): `HashEmbedder`
(deterministic feature hashing, 256-dim, offline, default), `OnnxEmbedder`
(all-MiniLM-L6-v2 quantized, 384-dim, behind the `onnx` feature + fastembed),
`OpenAIEmbedder` (behind the `openai` feature; configured via
`RECALL_MCP_OPENAI_*`, response dims validated, AR-005). Selection is
`--embedder`/env/config-file resolved in core (`select_embedder`) and carried
on `ServerConfig` so REST/MCP/capture embed identically. There is no trait
`Scorer`: ranking is the concrete `HybridScorer` struct.

## API surface (HTTP)

- `POST /v1/namespaces` · `GET /v1/namespaces`
- `POST /v1/memories` `{namespace, text, tags?, pinned?, embedding?}` → 201
  (caller vectors must match the embedder's dims, AR-005)
- `GET /v1/memories?namespace=&limit=&offset=` → `X-Total-Count` pagination,
  limit clamped at `MAX_PAGE_LIMIT` (10 000, AR-004)
- `PATCH /v1/memories/{id}` (partial update, re-embeds on text change)
- `DELETE /v1/memories/{id}` · `GET /v1/stats`
- `POST /v1/recall` `{namespace, query, k?, tau_days?, weights?, pinned_boost?, tags?}`
  → ranked `[memory + breakdown]` (`breakdown: {bm25, vector, recency,
  pinned_boost, total}`)
- `POST /v1/capture` `{namespace, transcript, tags?, max_chunk_chars?}` (D-018)
- `GET /healthz`, `GET /openapi.json` (hand-written document, ADR-007),
  `GET /metrics` (Prometheus 0.0.4)
- MCP over streamable-HTTP (`/mcp`) and stdio (`recall-cli serve --stdio`):
  six tools (`remember`, `recall`, `forget`, `update_memory`,
  `list_memories`, `capture`) with JSON-Schema input schemas.

Middleware (outermost first): Host/Origin guard (AR-001: foreign Host → 421,
cross-origin Origin → 403, CORS only for `--allow-origin` opt-ins) → rate
limiter (token bucket per presented key, AR-012) → API-key auth (constant-time)
→ body limit (1 MiB default) → tracing/metrics.

## Ranking

`total = w_bm25·norm_bm25 + w_vector·cosine + w_recency·exp(-Δdays/tau_days) +
pinned_boost`; defaults 0.45 / 0.45 / 0.10 / 30 d / 0.2, all tunable per
request with validation (`k ∈ [1,100]`, finite weights, `tau_days > 0`,
`tau_days: null` disables recency). Deterministic tie-break: total desc,
created_at desc, id asc.

## ADRs (summary; full text in docs/adr/DECISIONS.md)

- ADR-001 Rust workspace with pure-core crate: testability + coverage target; web/MCP as shells.
- ADR-002 (amended) SQLite + FTS5 + brute-force exact cosine (sqlite-vec abandoned); 1M-row ceiling documented.
- ADR-003 HashEmbedder default: deterministic tests + true offline; onnx/openai as pluggable opt-in features.
- ADR-004 rmcp SDK; no hand-rolled fallback; spec version pinned.
- ADR-005 GNU toolchain on Windows (no MSVC Build Tools dependency).
- ADR-006 UI is a pure API client (no SSR); OpenAPI hand-written (D-006).
- D-007…D-021 cover the shared-store shape, API-key auth (D-008), the
  namespace/capture semantics (D-009, D-018), coverage honesty (D-010), local
  ONNX embeddings (D-013), encryption at rest (D-014), update semantics
  (D-015), the recall fast path (D-016), hardening defaults (D-017), thread
  tuning (D-020), and the memory-poisoning threat model (D-021).

## Testing strategy

- recall-core: unit tests (scorer properties, embedder determinism, store CRUD,
  crypto roundtrips, export/import) + property-based tests (`proptest`: crypto
  roundtrip/wrong-AAD, FTS query validity, chunking invariants, scorer
  linearity, token-bucket invariants) + mutation testing (`cargo-mutants`,
  `mutants.toml` gates `crypto.rs`/`scorer.rs`; first recorded run: 51
  mutants, 45 caught, 0 missed, 6 unviable; results in EVALUATION.md).
- recall-server: router-level `oneshot` integration tests + MCP conformance
  over streamable-HTTP and stdio (initialize → tools/list → tools/call, error
  mapping, auth).
- Golden-set eval test: 50 QA pairs, hit-rate@5 ≥ 0.85 gate.
- e2e: real-binary CLI tests over sockets; UI Playwright + axe AA.
- Latency: exact p95 harness (ignored by default, release runs) + criterion.
- Coverage: `cargo llvm-cov --fail-under-lines 90 --fail-under-regions 90`
  on recall-core (cargo-llvm-cov 0.9.1 has no branch fail-under; D-010
  amendment), enforced in CI (linux-gnu) and measured locally on windows-msvc.
