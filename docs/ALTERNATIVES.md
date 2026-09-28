# Alternatives memo, implemented-stack review (2026-09-06)

For each locked choice: current-state evidence (checked today), tradeoffs,
cost/benefit of switching NOW vs staying, and a recommendation with
confidence. Evidence sources: GitHub API repo stats (stars, `pushed_at`,
issues) pulled 2026-09-06; crates.io API; MCP spec release notes.

| Decision | Verdict | Confidence |
|---|---|---|
| rusqlite (bundled) vs sqlx | **Stay** with rusqlite | High |
| rmcp vs hand-rolled JSON-RPC | **Stay** with rmcp | High |
| HashEmbedder vs ONNX vs API embedders | **Stay default**; add fastembed-rs ONNX behind `onnx` feature | Medium-high |
| axum vs actix-web | **Stay** with axum | High |
| SQLite vs LanceDB / Qdrant | **Stay**; revisit only at >100k memories/namespace or p95 regression | High |
| React+Vite vs Leptos/Dioxus | **Stay** with React+Vite | High |
| Auth: API key vs OAuth resource server | **Stay** API-key locally; OAuth RS only if a hosted/team server ships | High |
| Benchmarks: criterion vs divan | **criterion** as the gate | Medium |
| CI: GitHub Actions | **Yes**, it also unblocks the D-010 coverage gate | High |

## 1. rusqlite (bundled) vs sqlx

**Evidence.** In use: rusqlite 0.40.2, bundled SQLite, WAL, FTS5 (D-002).
sqlx is alive: 17.5k★, 0.9.0 published 2026-05-21 (crates.io), 141.7M
downloads, but note the GitHub repo now resolves under the `transact-rs`
org (redirect from launchbadge/sqlx observed 2026-09-06; crates.io metadata
still says launchbadge). Governance churn is a watch item, not a defect.

**Tradeoffs.** rusqlite is synchronous, which is why `recall-core` is pure
and branch-testable without an async runtime (D-001/D-007). sqlx brings
compile-time-checked SQL and async, but its SQLite support is the weakest of
its three backends, `SQLX_OFFLINE`/prepared-query friction is real, and
adopting it drags tokio into the domain crate. We would also rewrite FTS5
trigger plumbing and the BLOB-embedding scan for zero user-visible gain.

**Switch-now cost/benefit.** Cost: rewrite of the entire store layer +
migration of ~49 unit tests. Benefit: compile-time SQL checks (we already
have an openapi/content drift test discipline and 12 integration tests
covering the SQL paths).

**Recommendation: stay (high).** Revisit only if core grows raw-SQL hotspots
faster than tests can cover. Watch the sqlx org situation before ever
adopting.

## 2. rmcp (official SDK) vs hand-rolled JSON-RPC

**Evidence.** `modelcontextprotocol/rust-sdk`: 3.9k★, pushed 2026-09-06,
official Tier-1 SDK; we're on 3.3.0 targeting spec `2026-07-28` (D-004).
That spec revision is the **largest ever** (statelessness: initialize
handshake and `Mcp-Session-Id` removed; extensions first-class; Tasks
reworked; JSON Schema 2020-12 tool schemas;
[release note](https://blog.modelcontextprotocol.io/posts/2026-07-28-release-candidate/)).

**Tradeoffs.** Hand-rolling buys full control and no SDK API churn, at the
price of personally re-implementing every SEP (stateless `_meta` transport,
`server/discover`, MRTR `requestState`, extensions negotiation). The SDK is
exactly the artifact that absorbs spec drift; since 3.3.0 both transports
answer a `2026-07-28` `initialize` with `2025-11-25` (the newest legacy
handshake version; 2026-07-28 replaced the handshake with per-request
metadata), a behavior change the SDK made deliberately and we pin in the
conformance tests (D-004).

**Switch cost/benefit.** Cost: re-derive protocol correctness against a
moving spec; lose conformance smoke tests' meaning. Benefit: none for a
4-tool server.

**Recommendation: stay (high).** The 2026-07-28 spec velocity is the proof
this was the right call.

## 3. HashEmbedder vs local ONNX vs API embedders

**Evidence.** HashEmbedder (256-dim feature hashing) is lexical-only, so it
cannot match synonyms/paraphrase. Local ONNX is healthy in 2026:
`fastembed-rs` (1.0k★, Apache-2.0, **pushed 2026-08-27**) packages ONNX
embedding models + rerankers on `ort` (2.5k★, **2 open issues**, pushed
2026-09-02, very active). Kalosm exists but is a broader framework, less
focused for this one job. API embedders: `OpenAIEmbedder` is already
implemented behind the `openai` feature (D-003), unexercised against the
live API.

**Tradeoffs.** HashEmbedder: deterministic, offline, tiny, but caps
retrieval quality (our golden-set hit-rate 0.980 is lexical-friendly).
ONNX: real semantic recall, still fully offline, but adds a model download,
ONNX Runtime native dep (Windows GNU compat must be validated, same class
of risk as D-005's bundled-SQLite story) and ~30-100 MB binary/data. API:
best quality-per-model-size but breaks local-first, adds latency, cost, and
a key requirement, precisely the OpenMemory MCP complaint.

**Switch cost/benefit.** Adding `fastembed-rs` behind a non-default `onnx`
feature is incremental (implement `Embedder`, ship a benchmark comparing
HashEmbedder vs ONNX vs OpenAI on `evals/golden.json`), not a rewrite. The
`Embedder` trait (D-003) was built for exactly this.

**Recommendation: keep HashEmbedder as default/test fixture; add ONNX via
fastembed-rs behind `onnx`; keep `openai` opt-in (medium-high).** Do the
windows-gnu `ort` spike before promising it. This is the biggest quality
lever available off the shelf (see COMPETITORS.md rec #5).

## 4. axum vs actix-web

**Evidence.** axum 0.8.9 in use; tokio-native, tower middleware ecosystem,
hand-written OpenAPI served from the binary (D-006). actix-web 4 remains
fast and mature but is the second ecosystem in mindshare for new Rust web
services.

**Tradeoffs.** Both exceed our performance needs by orders of magnitude
(p95 budget 25 ms is spent in SQLite, not the framework). axum's extractor +
middleware model already hosts our API-key auth (D-008) and stateless MCP
HTTP service. actix's actor heritage buys nothing here and its middleware
types don't compose with tower, where our auth/CORS/tracing live.

**Switch cost/benefit.** Cost: full server rewrite, new auth middleware,
new test harness. Benefit: none measurable.

**Recommendation: stay (high).**

## 5. SQLite (+ brute-force cosine) vs LanceDB / Qdrant-embedded / sqlite-vec

**Evidence.** LanceDB: 11.4k★, Apache-2.0, Rust-native embedded retrieval,
actively pushed 2026-09-06, a genuine embedded alternative for ANN at
scale. sqlite-vec: 8.1k★ but C-extension distribution (last push
2026-05-18), blocked on windows-gnu by extension-loading risk (D-002).
Qdrant: designed as a standalone server; no first-class embedded,
Windows-GNU-friendly mode; it is a deployment, not a dependency.

**Tradeoffs.** Exact brute-force cosine is O(namespace rows) per query but
deterministic, explainable, extension-free, and matches our PRD ceiling
(~1M rows, scan_cap 10k default). LanceDB adds a columnar store, its own
on-disk format, and an async API (conflicts with the sync `Store` trait,
D-007) in exchange for sub-linear ANN we don't need yet. One SQLite file
*is* the product promise.

**Switch cost/benefit.** Cost: new storage engine, dual-format migration,
async contagion in core. Benefit: appears only past ~100k memories/namespace
or if p95 recall regresses past 25 ms (unmeasured, see benchmarks below).

**Recommendation: stay (high).** Codify the revisit trigger: benchmark
first (below), then only if numbers breach budget. If we ever need ANN,
LanceDB is the first choice, not sqlite-vec, on windows-gnu.

## 6. UI: React 19 + Vite 8 vs Leptos / Dioxus

**Evidence.** Both Rust options are healthy: Dioxus 39k★ (pushed
2026-09-04, "fullstack app framework for web, desktop, mobile"), Leptos
21.3k★ (pushed 2026-09-04). Our UI is React 19.2.8 + Vite 8 + TS strict +
Tailwind 4 + Playwright/axe with **0 axe violations** (D-012, STATUS).

**Tradeoffs.** Leptos/Dioxus give one-language purity and Wasm size wins,
but: the accessibility tooling (axe-core, testing-library patterns, screen-
reader documentation, ARIA examples) is deepest in the React ecosystem;
and accessibility is the binding constraint here, not
bundle size. Rust UI frameworks still have thinner ARIA patterns, fewer
audited component primitives, and smaller hiring/future-agent-maintenance
pools.

**Switch cost/benefit.** Cost: full UI rewrite + rebuilding a11y guarantees
that are already proven green. Benefit: stylistic unity with the backend.

**Recommendation: stay (high).** The API-client discipline (D-006) means the
UI is disposable; revisit only if a desktop packaging need emerges
(Dioxus would then be the candidate).

## 7. Auth: API-key vs OAuth resource server (planned feature)

**Evidence.** The 2026-07-28 spec overhaul realigned MCP auth with real
OAuth 2.0/OIDC deployments: six SEPs: RFC 9207 `iss` validation (SEP-2468),
DCR `application_type` for desktop/CLI clients (SEP-837), credential
issuer-binding (SEP-2352), refresh-token guidance (SEP-2207), step-up scope
clarifications (SEP-2350/2351)
([source](https://blog.modelcontextprotocol.io/posts/2026-07-28-release-candidate/)).
That machinery targets **remote, multi-tenant deployments**. Our current
API-key scheme (D-008) covers `/v1/*` and `/mcp` with Bearer/X-API-Key and
open-by-default loopback use.

**Tradeoffs.** OAuth resource-server semantics would let enterprise MCP
clients do dynamic registration and token audience validation against a
real AS, meaningless for a single local user, significant for a hosted
team server. API keys are zero-infrastructure, transparent, and match the
local-first promise; their weakness (no expiry/rotation) is acceptable when
the threat model is "loopback plus LAN".

**Recommendation: keep API-key for the local product (high).** If/when a
Team Server ships (COMPETITORS.md rec #3), implement it as an OAuth 2.0
resource server (RFC 9207 `iss`, DCR `application_type=native` for desktop
clients) per the 2026-07-28 SEPs, and that spec section becomes the spec to
conform to, not an alternative to it.

## 8. Benchmarks: criterion vs divan (planned feature)

**Evidence.** criterion: the established standard, statistical change
detection, CI-regression tooling (CodSpeed supports both). divan: 1.4k★,
pushed 2026-07-19, actively maintained; far faster iteration, built-in
allocation tracking, less boilerplate; community (HN thread
[37773599](https://news.ycombinator.com/item?id=37773599)) credits it for
speed/simplicity but notes fewer statistical guarantees than criterion.

**Tradeoffs.** Our PRD metric is a latency budget (p95 < 25 ms on 10k
memories), we need (a) trustworthy distributions for a gate, and (b) a
server-level harness (real SQLite, real query mix), which neither crate
provides out of the box. divan measures niceties (alloc counts) that matter
less than the p95 number; criterion's change detection is what turns a
benchmark into a regression gate.

**Recommendation: criterion for the core hot-path gate
(`HybridScorer`, cosine scan, FTS query), plus a small custom server-level
p95 harness in `cargo test --ignored` style (medium).** divan is fine for
ad-hoc micro-profiling; don't make it the gate.

## 9. CI: GitHub Actions (planned feature)

**Evidence.** GitHub-hosted runners are the de facto standard for public
Rust repos; free tier for public repos; first-class `cargo` caching via
`Swatinem/rust-cache`; Windows + Linux runners match our matrix.

**Decisive point for this repo:** at the time of writing, windows-gnu lacked
the profiler runtime for `cargo llvm-cov` (D-010), so CI was the mechanism
that would finally enforce the coverage gate remotely. The dev machine later
moved to windows-msvc (where llvm-cov runs and the gate was measured locally
in 2026-09-18), but the linux-gnu CI job still enforces it independently.

**Recommendation: GitHub Actions with two jobs (high):** (1) `linux-gnu`:
fmt-check, clippy `-D warnings`, `cargo test --workspace`, llvm-cov gate;
(2) `windows-gnu`: build + `cargo test --workspace`,
plus the web job (`npm run build`, Playwright, axe). Trigger the ONNX
windows-gnu spike job only behind a flag.

## Watchlist (next review: after Team Server decision or 100k-memory trigger)

- sqlx governance (`transact-rs` org) and SQLite backend health all affect
  any future "async core" temptation.
- rmcp minor releases for `2026-07-28` Final-spec conformance suite.
- `ort`/`fastembed-rs` windows-gnu compatibility (the D-005 story repeating).
- Mem0/memU license drift in the local-first cluster (AGPL/NOASSERTION
  neighbors; keep our permissive core clean).
