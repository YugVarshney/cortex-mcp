# Build log

A dated engineering journal, not a report: the problems hit during
development, in the order they were hit, with the evidence that proved each
one and the change that followed. The clean final state hides exactly what a
reviewer most wants to see: a plan that turned out wrong, a tool that failed
silently, a budget that was missed and reported anyway. Decisions behind the
changes live in `docs/adr/DECISIONS.md`; this file records how each landed in
practice. All times IST. Every claim below was executed for real
(GNU toolchain on Windows); nothing is reconstructed from memory alone; each entry cross-references the commit that carries it.


## 2026-09-06, v0.1: the whole vertical, in one night

### The vertical slice landed before the research pass

**What.** Between 00:53 and 02:29 the project went from PRD to a working
system: PRD + architecture baseline (`8e2a2fa`), `recall-core` with the
SQLite+FTS5 store, hash embedder and hybrid scorer (`e0d27d2`), the axum
server with rmcp MCP over streamable-HTTP and API-key auth (`a03aa80`), the
CLI with serve/remember/recall/export/import plus a stdio MCP conformance
test (`d6c55e5`), the golden-set retrieval eval (`d104ed4`), the React 19 web
UI with Playwright + axe (`cdb86bb`), and the decision log D-001..D-012
(`ce24d7a`).

**Why this order.** Core-first was not dogma; it was the only order in which
the coverage gate (D-001's whole point) could be enforced at every later
commit. Server and CLI are thin shells; a shell cannot be tested before the
thing it shells.

**Evidence it worked.** Golden-set eval hit 0.980 hit-rate@5 against the 0.85
PRD gate on the first run (`d104ed4`); the MCP conformance smoke
(initialize → tools/list → tools/call) passed over both transports the same
night.

### llvm-cov cannot run on windows-gnu, measured and worked around in CI, not faked

**What.** The 90/90 line/branch coverage gate is core to the project's quality
bar, and `cargo llvm-cov` fails on `stable-x86_64-pc-windows-gnu` with
`error[E0463]: can't find crate for 'profiler_builtins`, the profiler
runtime is not distributed for windows-gnu.

**Evidence.** Three workarounds attempted in order and documented in D-010:
llvm-tools component (same error); nightly-gnu +
`CARGO_UNSTABLE_BUILD_STD=std,panic_abort` + rust-src (same error); adding
`profiler_builtins` to build-std (build fails inside profiler_builtins),
compiler-rt sources not shipped). Alternatives surveyed: cargo-tarpaulin
(Linux-only), grcov (same `-C instrument-coverage` requirement → same missing
runtime), MSVC toolchain (excluded by D-005, the GNU-toolchain decision).

**Consequence.** The gate runs in CI on a linux-gnu job with the standard
command; on the windows-gnu toolchain coverage is reported as **not
measurable**, never guessed.
(The repo has no `origin` remote yet, so CI itself has not executed, stated
in STATUS rather than papered over.)

### rmcp negotiates different spec versions per transport, observed not assumed

**What.** The MCP conformance tests showed streamable-HTTP honoring the
client's `2026-07-28` request while stdio negotiates rmcp's default
`2025-11-25`.

**Evidence.** Both conformance tests (HTTP: `recall-server/tests/mcp_http.rs`;
stdio: `recall-cli/tests/mcp_stdio.rs`) assert their negotiated versions.

**Consequence.** Documented as SDK behavior in D-004 rather than fought; the
server-info test pins the spec so a future SDK bump that changes it is a
visible diff.

### A re-entrant lock bug the e2e suite caught

**What.** The CLI `remember` path deadlocked: it acquired the store guard,
then called a store method that tried to acquire it again.

**Evidence.** Caught by the CLI e2e against the real binary, not by unit
tests; the unit tests exercised the store directly and never went through
the shared `Arc<Mutex<dyn Store>>`.

**Consequence.** Fixed by holding one guard per logical operation; that
pattern is now the codebase-wide rule (D-007).


## 2026-09-06 (evening), v0.2: the benchmark, real embeddings, encryption

### The ONNX spike answered the open windows-gnu question, and the first attempt failed loudly

**What.** ALTERNATIVES.md §3 left "local semantic embeddings on windows-gnu"
as an open question. The spike (`spikes/onnx-spike`, kept in the repo)
attempted `fastembed` with default features first.

**Evidence.** Attempt 1 failed to build: ort 2.0.0-rc.13
(`ort/download-binaries`) ships only windows-MSVC prebuilts, saying `no prebuilt
binaries available for target x86_64-pc-windows-gnu`. Attempt 2
(`ort-load-dynamic` + `hf-hub-rustls-tls`, loading the official
`onnxruntime-win-x64-1.28.0.dll` at runtime via `ORT_DYLIB_PATH`) worked: the
MSVC-built DLL is C ABI; `LoadLibrary` needs no GNU link step. Measured in the
spike: all-MiniLM-L6-v2-Q, 384-dim, paraphrase-vs-unrelated cosine 0.759 vs
0.041, 3 texts in ~8.5 ms.

**Consequence.** `OnnxEmbedder` landed behind the non-default `onnx` feature
(`6c68639`), Embedder trait contract preserved (empty text → zero vector;
inference serialized behind a mutex; failures yield a zero vector, never a
crash). Golden-set eval with the same 0.85 gate: **hit-rate@5 = 1.000 (50/50)**
vs HashEmbedder's 0.980, and the known lexical miss ("which integration do
leavers miss most") is fixed by real semantics. Batch size pinned to one text
per call: same-batch-shape embeddings were verified bit-identical, and
variable batch shapes were not trusted to stay so.

### The PRD's own latency budget was breached, and the breach was shipped as the finding

**What.** The PRD budget is p95 recall < 25 ms. The first real measurement on
a 10k-memory fixture (the documented worst case for an exact scan) read
**67.2 ms** (`4b54722`, criterion + an exact-percentile harness built because
criterion means and exact p95 answer different questions).

**Evidence.** Two harnesses, same fixture: criterion `hybrid_k5` 80.5 ms mean;
exact harness p95 67.2-72.5 ms across runs. Run-to-run variance is ±10-15%,
so EVALUATION.md lists every run and never averages
silently.

**Consequence.** v0.2 shipped with the breach stated in STATUS/EVALUATION
(`5c29da2`) and D-002's revisit trigger formally armed. The fix was *not*
swapping storage engines first; that is D-016/D-019, the next day. The
important discipline: the breach was measured, published, and dated before
any optimization was written, so the later improvement numbers have an honest
baseline.

### Encryption at rest: the design that keeps keyword search working

**What.** COMPETITORS.md rec #1 made plaintext single-file storage a marketed
weakness. Landed `35e3241`: ChaCha20-Poly1305 over `memories.text`, andnon-obvious part, the FTS5 index built over HMAC-SHA256 token digests
(truncated to 128 bits) so keyword search still works with nothing reversible
on disk.

**Evidence.** Tests open the database file bytes after writes (unit + real
binary e2e, WAL included). A key-check blob makes a wrong key fail at open
instead of corrupting reads; legacy databases upgrade in place (re-encrypt
all plaintext rows in one transaction, rebuild FTS over digests, then
VACUUM).

**Consequence.** Scope stated plainly rather than hidden: text only;namespace names, tags, source, embeddings, and `export` output stay
plaintext; in-place-upgraded databases may retain remnants in freelist pages.
SQLCipher was rejected in D-014 (whole-file format change for every user,
OpenSSL-shaped dependency on windows-gnu, FTS content table still plaintext
unless separately encrypted).

### Update-in-place vs append-only, decided by consumers

**What.** D-015 (`d5c1d77`) needed update semantics. Append-only
(memory-log-style) was considered.

**Evidence.** Agents quote a memory id they just stored; a supersede-by-id
flow is one round trip. An append-style log needs consolidation reads, and
nothing currently consumes a consolidation read.

**Consequence.** In-place edit, with one subtle policy: a text change without
an explicit embedding **clears** the stored vector; a stale vector that no
longer matches the text is worse than no vector, and the server/CLI layers
re-embed automatically so callers keep embeddings fresh. The new FTS trigger
is guarded with `WHEN old.fts_tokens IS NOT new.fts_tokens` so access-time
`last_accessed_at` refreshes never churn the index. Same commit added the
ALL-of tag filter for recall (a correlated `json_each` count applied in *both*
scanning passes, so `k` applies after filtering).

### Smaller ones

- **OpenAPI drift guards.** Hand-written `openapi.json` (D-006, since utoipa's
  derive machinery buys nothing for six endpoints and pins macro versions)
  needs a drift guard: content tests assert the served document
  matches the committed one. `7689c5f` added guards for PATCH and the recall
  tag filter after those surfaces landed.
- **`base64` 0.23** lockfile refresh ((`9f7d0e5`), routine, recorded for
  completeness.


## 2026-09-07, v0.3: the measured re-engine, the operations surface

### D-016: re-engine the scan before swapping the engine

**What.** The measured 67 ms p95 tripped D-002's own revisit trigger. Before
adopting an ANN index (a quality and determinism regression), the exact scan
was re-engineered (`2e6d06a`, 00:14): slim the vector pass to
`(rowid, embedding, created_at, pinned)`, read BLOBs through a borrowed
`ValueRef` with zero Rust-side allocations, hoist the query norm, and keep an
exact top-k heap of vector-only totals, exact because outside the FTS
candidate set bm25 is 0, so vector-only total *is* final total.

**Evidence.** p95 67.2 → 33-35 ms (5 runs listed in EVALUATION.md); criterion
`hybrid_k5` 80.5 → 28.3 ms. Bit-identity of the BLOB cosine against the
reference is unit-tested, including the hoisted-norm variant; heap exactness
is tested against brute force at 300 memories.

**A rejected experiment worth recording.** Rewriting the FTS namespace filter
as `rowid IN (SELECT …)` made SQLite correlate the subquery per FTS match:
**1.6 s per query**. The JOIN formulation stays. Recorded so nobody "fixes"
it again.

**Residual.** The remaining ~20 ms was SQLite blob-read
throughput in the O(n) scan, which is exactly what D-019 then attacked.

### The v0.3 hardening pass, defaults that make it operable beyond loopback

`4d98d4f` (00:33) landed the operations surface in one slice, with ADRs
written in the same commit (`8759dcc`): constant-time API-key comparison
(`subtle::ConstantTimeEq`), 1 MiB body limit (413), per-key token-bucket rate
limiting (429 + Retry-After, off by default, shared `anonymous` bucket that
also throttles key-guessing), Prometheus `/metrics` (request counter, status
codes, per-route latency histogram, embedding timings via a timed wrapper
around the shared embedder, live store gauges), pagination with
`X-Total-Count`, capture ingest, graceful shutdown with WAL checkpoint,
`backup` (hot `VACUUM INTO`; refuses overwrite; keyed stores yield keyed
backups because ciphertext copies verbatim) and `vacuum`.

**Decisions inside the slice.** Rate limiting keys on the presented API key;it throttles per-key floods and key-guessing, not distributed key rotation
(threat model stated in EVALUATION limitations). tower-governor rejected: a
dependency for ~60 lines of bucket math. Unauthenticated `/metrics` rejected:
it would leak store sizes when a key is set. Config precedence is per knob;flag > env > file > default, so a file `rate_burst` applies even when the
rate comes from a flag; secrets are never file-configurable.

### D-019: the packed vector cache, the second halving, measured same-day

**What.** The D-016 residual was re-reads of ~10 MB of embedding BLOBs per
recall. The fix (`42eb68b`, 22:30) packs the scan columns into contiguous
memory on the first untagged vector pass; repeat recalls scan the packed
buffer.

**Evidence discipline worth copying.** The pre-change baseline was re-measured
**immediately before** the change (22:07 IST, same-day baseline 55.9 ms p95 on
the exact harness) so the comparison shares the load and thermal conditions.
Post-change: criterion `hybrid_k5` 31.8 → **16.16 ms** (−49.1%, CI −50.1 to
−48.2); exact-harness p95 26.2-27.2 ms across 3 runs. Cost-breakdown
diagnostic: the vector pass went from ≈ +23 ms over the keyword floor to −4
to +5 ms, within run noise of zero.

**Correctness perimeter, all unit-tested:** any insert/update/delete
invalidates the cache (total, not surgical, writes are rare next to reads);
the fill query preserves `scan_cap` LIMIT semantics row-for-row; NULL/differing
-dimension rows score cosine 0.0 exactly like the SQL path; the packed cosine
is bit-identical to the reference; tag-filtered recalls bypass the cache;
`last_accessed_at` refresh is deliberately not an invalidation. Golden set
re-run with the cache: 0.980 (49/50), unchanged, as expected, since the cache
is bit-identical to the SQL path.

**The headline.** The 25 ms PRD budget is **still breached at exactly
10k** on the exact harness (by 1.2-2.2 ms; the ±10-15% run-to-run variance spans the
line). The residual is the FTS keyword pass plus fetch/refresh (~15 ms floor)
: a query-plan problem with no documented one-line remedy. Reported as not
met, not massaged.

### Embedder selection completes D-013, and the failure mode was designed before the flag

`41e03fd` (22:48) added `--embedder onnx|hash|openai` (env, config-file,
precedence per knob). The design point: `onnx`/`openai` in builds without
their cargo features fail **closed**, with an error naming the required
feature, never a silent fallback to `hash`, which would silently corrupt
recall quality while every test stays green. `ServerConfig` carries the
resolved embedder so REST, MCP and capture embed identically (proven by a
router test with a fixed-vector embedder, cosine 1.0 end-to-end). A real
binary e2e asserts `--embedder onnx` without the feature refuses `remember`
and `serve`.


## 2026-09-08, the audit pass and the closing of D-018

### A self-audit found five real defects in the brand-new fast path

**What.** `6ea4025` (00:15) fixed the findings of a deliberate audit pass over
the v0.3 code:

1. **Packed-cache cap parity:** the cache over-scanned when a smaller
   `scan_cap` followed a larger incomplete fill; the scan is now bounded to
   the caller's cap so LIMIT semantics stay row-for-row (regression test
   added).
2. **Ciphertext-marker shape:** `is_encrypted` required only the `enc:v1:`
   prefix, a *plaintext* memory that merely starts with that marker failed
   the keyed upgrade and then broke later reads. Now validates ciphertext
   shape (canonical base64, ≥ nonce+tag) and the upgrade pass re-encrypts
   prefix-wearing literals (tests added).
3. **MCP 404 mapping:** unknown memory id mapped to internal error; it is
   caller-fixable and now maps to invalid params (test added).
4. **stdio shutdown skipped the WAL checkpoint** the HTTP path performed.
5. **`list_memories` limit clamped at 10k**: `usize::MAX` wrapped to
   SQLite's "negative = unlimited" convention; export no longer relies on
   that rule either.

Plus curated clippy pedantic opt-ins per the style guide: `unwrap`/`expect`
outside tests must carry a justified local allow.

**Why it is worth recording.** Every one of these was invisible to the suite
as it existed, because the tests and the bug shared the same wrong assumption. The
audit's value was asking "what would a hostile input do to this new code,"
not "do the tests pass."

### The MCP capture tool, closing D-018 by removing a duplication risk

`217678f` (00:50) exposed the capture pipeline as the sixth MCP tool. The
move that matters: the chunk/embed/commit pipeline was **lifted verbatim**
from `api.rs` into `recall-server::capture`, shared by `POST /v1/capture` and
the MCP tool, so the two surfaces cannot drift. Embedding happens in
`prepare()`, outside the store lock, on both paths. Conformance suites
extended to six tools with a full capture → recall round trip over
streamable-HTTP, and the stdio test asserts the six-tool list.

**State at close.** 126 tests green, clippy `-D warnings` clean, fmt clean.
Open items are stated in STATUS/EVALUATION: the 10k p95 budget line, coverage
unmeasurable on the windows-gnu toolchain and CI not yet executed (no remote), OpenAI embedder
never called live, encryption scope limits.

## 2026-09-11 (day), the adversarial review: 23 findings, closed in 18 commits

### The fix wave itself

The hostile review (docs/ADVERSARIAL-REVIEW.md, commit b3b4b85) found one P1
(the loopback server's browser attack surface), crypto-hygiene gaps, a
correctness pair in the OpenAI embedder, the missing mutation/property
testing layers, and doc drift. The fixes landed as c0df29f..8572f28, each a
conventional commit per finding: same-origin CORS default + Host validation
(AR-001), HKDF subkeys with one-transaction migration of pre-HKDF stores
(AR-002), embedding-inversion disclosure + a keyed-mode startup warning
(AR-003), the MCP/REST `MAX_PAGE_LIMIT` clamp with a 10 001-memory
end-to-end proof (AR-004), OpenAI dims validation (AR-005), atomic import
(AR-023), rate-limiter-wraps-auth (AR-012), secure_delete on keyed upgrades
with a forensic WAL canary (AR-013), plus the P3 wave and the STATUS/
ARCHITECTURE true-ups. No finding was marked done without a test or an
explicit doc statement behind it.

### The mutation run that silently never happened (AR-006)

EVALUATION.md shipped with `MUTANTS_PLACEHOLDER` standing in for the
mutation score: the first cargo-mutants invocation ran against the wrong
targets (the default workspace file set, so it spent its time mutating
`recall-cli/src/main.rs`), and the corrected invocation died at the
unmutated baseline build under the disk pressure that later forced the
`target/` deletion. The 8572f28 commit fixed the recipe documented in
`mutants.toml` (cargo-mutants 27.x selects files with `-f` globs, and
`--in-place` implies `-j 1`, an explicit `--jobs` flag is rejected), but
the run itself and the score recording were left for this session.

The completed run (21:05 IST, `--in-place`, warm build, 4 min): **51
mutants: 43 caught, 2 missed, 6 unviable.** The 2 survivors were both
`key_check_context` literal replacements (`""`, `"xyzzy"`), and they are
the interesting result: the key-check AAD is applied symmetrically (the
same function seals and opens the stored check value), so *no behavioral
test can ever distinguish a mutated literal*, because it is on-disk wire format,
in the same class as `ENC_PREFIX` and the HKDF vector, and it needed a
format-pinning test (`aad_context_literals_are_pinned_wire_format`), not a
behavioral one. Re-run: **45 caught, 0 missed, 6 unviable (45/45 viable).**

Two side effects recorded because they will recur on every mutant run:
proptest appends mutant-specific cases to `*.proptest-regressions` when a
mutant breaks a property (they pass against real code; revert after each
run), and `--in-place` mode mutates the working tree itself, so verify
`git status` is clean before and after.

### The count audit the mutation run triggered

Re-verifying the suite for the record surfaced that the test headline had
drifted: EVALUATION claimed 156, the table summed to 155, and the actual
count after the pin test is **158**: the cli-unit row had never picked up
the `--allow-origin` validation test from 743bcd5. Every count is now
taken from a labeled `cargo test --workspace` log, not carried forward.

**State at close.** 158 tests green (1 + 2 ignored diagnostics), clippy
`--all-targets` 0 warnings, fmt clean, mutation score recorded with the
survivor disposition. Remaining gaps unchanged: 10k p95 budget line,
coverage CI-gated but never executed (no remote), OpenAI embedder never
called live.


## 2026-09-12, publication pass: making the repo GitHub-public-ready

### The stale MCP spec claim a conformance gap was hiding

**What.** Re-running the documented quickstart against the built binary;not just the test suite, surfaced that an `initialize` naming
`protocolVersion: "2026-07-28"` is answered `2025-11-25` on BOTH transports.
Four docs (README, ARCHITECTURE, design, ALTERNATIVES) plus the D-004
amendment claimed the HTTP transport "honors" the client's `2026-07-28`
request. Neither conformance test pinned the returned value: one asserted
`is_some()`, the other did not look, so the rmcp 3.2 → 3.3 upgrade (AR-011)
had silently changed wire behavior and every doc had drifted with it.

**Evidence.** Live curl over streamable-HTTP and a stdio pipe probe of the
real binary, then the SDK source:
`rmcp-3.3.0/src/service/server.rs::negotiate_protocol_version` documents the
design: the 2026-07-28 revision *replaced the initialize handshake* with
per-request metadata, so a client naming that revision is answered with the
server's newest version that still has a handshake (`2025-11-25`). Behavior
change by design, not a bug, but a claim-vs-reality defect for the docs.

**Change.** The negotiated value is now pinned in both conformance tests
(`mcp_http.rs`, `mcp_stdio.rs`) the same way `ENC_PREFIX` and the AAD
literals are pinned: wire format asserted verbatim, so the next SDK bump
that changes it is caught, not absorbed. Docs corrected everywhere current
(README, ARCHITECTURE, design, ALTERNATIVES, EVALUATION, AGENTS, D-004
amended again, and a dated correction note inside the adversarial-review
record rather than a silent rewrite of history).

### Dependency freshness: one laggard, one ecosystem constraint

**What.** Live crates.io checks of all 29 direct dependencies: everything
already at its latest stable except `reqwest` (0.12.28 → 0.13.5 wanted). The
bump renamed `rustls-tls` → `rustls`; the blocking-client API we use is
unchanged. First `cargo update -p reqwest --precise 0.13.5` failed: fastembed
6 → `hf-hub 0.5` requires `reqwest ^0.12`. Plain `cargo update` resolved it
correctly; the lockfile now carries reqwest 0.12.28 (hf-hub's) *and* 0.13.5
(ours), semver-incompatible duplicates coexisting, so `--all-features` still
resolves and the onnx feature's dependency edges are untouched.

**Verification.** `cargo check -p recall-core --features openai` and the
focused openai tests compile/pass on 0.13.5 (rustls 0.23/aws-lc stack builds
clean on windows-gnu).

### Small hostile-review catches

- `ServerConfig`'s hand-written `Debug` printed the API key verbatim. A
  config Debug is exactly what ends up in logs; it now prints
  `[redacted]`, pinned by test.
- README claimed a "rustup override included in the repo"; overrides live live
  in rustup's settings, not repos; reworded to match D-005 instead of adding
  a `rust-toolchain.toml` that would override the directory override and
  flip local builds off the GNU toolchain.
- CI actions two majors behind: checkout v5 → v7 (v6/v7 breaks: fork-PR
  checkout under `pull_request_target`/`workflow_run`, creds moved out of
  local git config, neither used here), upload-artifact v4 → v7 (v7 adds an
  opt-in `archive` param; the `name`+`path` usage is unaffected).

### Quickstart re-execution (the whole README block, in a temp dir)

`remember`/`recall`/`export`/`import`/`backup`/`vacuum`: all green as
documented, including the export plaintext warning line. `serve` +
`/healthz` + `/openapi.json` (all six `/v1` paths listed) + `/metrics`
(Prometheus text) + `/v1/stats` + full MCP streamable-HTTP handshake
(initialize → tools/list → tools/call remember) with the required dual
`accept` header. One doc nuance found by doing this: `/mcp` rejects
`accept: application/json`-only requests with "Not Acceptable", which is correct
per the streamable-HTTP transport, worth knowing for hand-rolled clients.

**State at close.** 160 tests green (3 ignored diagnostics), clippy
`--workspace --all-targets -- -D warnings` passes, fmt clean, tree clean.
Coverage remains CI-gated and unmeasured until a remote exists (D-010,
unchanged); mutation score unchanged (crypto/scorer sources untouched by
this build except the new pin test, which only adds assertions).


## 2026-09-18, release readiness (fresh clone, windows-msvc toolchain)

### The coverage gate everyone recorded did not exist

**What.** The first release-readiness run on the new toolchain (windows-msvc,
not the earlier windows-gnu one) tried to execute the coverage gate exactly as
recorded everywhere: `cargo llvm-cov -p recall-core --fail-under-lines 90
--fail-under-branches 90`. cargo-llvm-cov 0.9.1 (the latest release) answered
`error: invalid option '--fail-under-branches'`. Every copy of that command;AGENTS.md, ci.yml, README, ARCHITECTURE, design, the D-010 decision, recorded
a flag the tool never had at this version. The gate had never actually run
anywhere (Actions disabled, windows-gnu unable to run llvm-cov), so the typo
survived every earlier verification pass: everyone verified the *tests*, and
the gate line itself was never executed by anyone.

**Evidence.** `cargo llvm-cov --help` lists `--fail-under-{functions,lines,
file-lines,regions}` only; branch-level instrumentation exists behind
`--branch`, which is nightly-only (`-Z coverage-options=branch`, rejected by
stable rustc 1.98.1 with "1 nightly option were parsed").

**What landed.** The gate was corrected to what stable tooling can measure: lines + regions >= 90. It executed for real, passing: recall-core 95.00%
lines / 94.03% regions / 94.69% functions; recall-server measured at 96.83% /
96.13%. Coverage went from "unmeasurable, CI-only" to a
locally measured number, because D-005's MSVC exclusion no longer applies
with the GNU toolchain.
The D-010 amendment records the tooling gap: true branch coverage
stays unmeasured, and the branch gate comes back when stable rustc can
express it.

### Dependencies: two majors moved, one needed code

`cargo update` pulled rmcp 3.3.0 -> 3.4.0. The release notes listed no
handshake changes, and the thing that actually proves it is already in the
tree: both MCP conformance tests pin the negotiated `2025-11-25` answer, and
both stayed green. fastembed 6 -> 7 (optional `onnx` feature) deprecated
`InitOptions` (now a `TextInitOptions` alias that warns), so the embedder
moved to `TextInitOptions`; the four `--features onnx` tests then ran for real
against onnxruntime-win-x64-1.28.0 via `ORT_DYLIB_PATH`; model downloaded,
L2-norm and paraphrase-ordering assertions passed, golden set with the ONNX
embedder passed. First time the onnx feature has been executed on this
machine.

### The rest of the pass, briefly

- License audit of all 396 locked crates via the crates.io API (cargo-deny
  not installed locally): all permissive. The three worth naming:
  option-ext (MPL-2.0, file-level copyleft, link-safe), webpki-roots
  (CDLA-Permissive-2.0 data), r-efi (MIT/Apache/LGPL tri-license, and the LGPL
  arm is one of three choices, never mandatory). No GPL/AGPL anywhere; MIT
  stands.
- web/ deps refreshed within existing carets (react 19.3, vite 8.3);
  `npm audit` 0 vulnerabilities; Playwright suite re-run against a rebuilt
  production server: 5 passed including the axe AA scans.
- Prereq note the docs had drifted on: README said "Node 24"; the e2e now
  runs on Node 26.

**State at close.** 160 tests green (3 ignored diagnostics), clippy `-D
warnings` clean, fmt clean, coverage gate exit 0. No retrieval-path code
changed.
