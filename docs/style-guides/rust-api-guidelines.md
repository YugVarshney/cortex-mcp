# Style guide: Rust API Guidelines (primary)

Source: <https://rust-lang.github.io/api-guidelines/checklist.html> (Rust
project-maintained; the canonical API style reference: the 2026 ecosystem
still treats it as the authority). Checklist verified live on 2026-09-06.

## What it is

A C-*-prefixed checklist of conventions for public Rust APIs across nine
areas: naming, interoperability, macros, documentation, predictability,
flexibility, type safety, dependability, and future-proofing.

## Items we adopt (with the guideline text)

- **C-CASE**: "casing conforms to RFC 430 conventions" (types `UpperCamelCase`,
  methods/fields `snake_case`, consts `SCREAMING_SNAKE_CASE`).
- **C-CONV**: "ad-hoc conversions follow `as_`, `to_`, `into_` conventions"
  (cheap borrow → `as_`, mostly-free → `to_`, consuming → `into_`).
- **C-GETTER**: getter names match the field (`created_at()`, not `get_created_at`).
- **C-ITER / C-ITER-TY**: "methods named `iter`, `iter_mut`, `into_iter`";
  iterator types named after the producing method (`Memories`, `MemoriesMut`).
- **C-FEATURE**: feature names: noun phrases, no placeholder words
  (`openai`, not `use-openai-support-thing`).
- **C-COMMON-TRAITS**: eagerly derive `Debug, Clone, Copy, PartialEq, Eq,
  PartialOrd, Ord, Hash, Default` where they make sense.
- **C-CONV-TRAITS**: implement `From`, `AsRef`, `AsMut` rather than
  bespoke conversion methods.
- **C-SERDE**: all DTOs (`MemoryRecord`, `ScoreBreakdown`, export/import
  format) derive serde with `#[serde(rename_all = "snake_case")]` so the JSON
  API, MCP tool args, and export format stay byte-compatible.
- **C-SEND-SYNC**: "types are `Send` and `Sync` where possible". We deviate
  knowingly for `SqliteStore` (D-007: `Send`-only, shared via
  `Arc<Mutex<dyn Store>>`): document the deviation in the trait docs.
- **C-GOOD-ERR**: "errors are meaningful, not pessimistic" (library errors
  are concrete `enum`s implementing `std::error::Error`; see
  [error-handling guide](./rust-error-handling.md)).
- **C-CTOR**: "constructors are static, inherent `new` methods": no
  builder-only or trait-based construction for `HybridScorer`, stores.
- **C-NEWTYPE / C-CUSTOM-TYPE**: newtypes for `MemoryId`, `NamespaceId`;
  no bare `bool` parameters where a meaningful type reads better.
- **C-BUILDER**: `RecallParams` keeps its builder; complex values get builders.
- **C-VALIDATE**: public functions validate arguments and return typed
  errors (`RecallParams`: `k > 0`, non-negative weights).
- **C-DEBUG / C-DEBUG-NONEMPTY**: every public type `Debug`-derives with
  non-empty output.
- **C-SEALED**: the `Embedder` trait is sealed so new required methods don't
  break downstream impls.
- **C-STRUCT-PRIVATE**: public structs expose getters; fields stay private
  unless the type is a plain DTO.
- **C-STABLE**: release crates depend only on stable (we do: GNU stable
  toolchain, D-005).
- **C-METADATA / C-RELNOTES / C-CRATE-DOC / C-EXAMPLE / C-QUESTION-MARK**:
  full `Cargo.toml` metadata, crate-level docs with `?`-style examples, no
  `unwrap()`/`try!` in public docs.

## Where it does / doesn't apply

Applies fully to `recall-core` (the public library crate). `recall-server` and
`recall-cli` are applications: adopt the naming/doc items, skip API-surface
items that only matter for external consumers. `web/` is out of scope.
