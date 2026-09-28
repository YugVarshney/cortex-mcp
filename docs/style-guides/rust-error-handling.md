# Style guide: Rust error handling (2026 consensus)

Sources (all live 2026-09-06):
- <https://docs.rs/thiserror> (v2.x: derive `std::error::Error` for library
  enums; v2 supports `core::error::Error` / no_std)
- <https://docs.rs/anyhow> (type-erased application errors + `.context()`)
- <https://blog.rust-lang.org/2024/02/08/Rust-1.76.0.html>: stabilized
  `core::error::Error` (the `Error` trait usable in `no_std`); current notes at
  <https://blog.rust-lang.org/category/releases/>
- <https://rust-lang.github.io/api-guidelines/checklist.html>: **C-GOOD-ERR**

## The 2026 consensus (unchanged, restated)

1. **Libraries return concrete, typed errors.** `recall-core` defines
   `#[derive(Debug, Error)] pub enum StoreError { ... }` /
   `EmbedderError` / `ScorerError` with `#[error("...")]` messages and
   `#[from]` for wrapping (`#[from] rusqlite::Error`, `#[from] serde_json::Error`).
   Callers must be able to `match` on variants: never leak `anyhow::Error`
   from a library crate. (API Guidelines C-GOOD-ERR: "errors are meaningful".)
2. **Applications use type-erased errors with context.** `recall-cli` and
   `recall-server` handlers convert core errors via `?` into an app-level
   error (axum handlers map to typed HTTP statuses; the CLI prints a
   human message and exits non-zero). `anyhow::Context` is allowed in the
   two app crates only, never in `recall-core`.
3. **Errors implement the standard trait.** Everything bottoms out in
   `std::error::Error` (stabilized in `core` since 1.76), so `?`, `Box<dyn
   Error + Send + Sync>`, and ecosystem tooling work. No custom `Error`
   trait re-inventions.
4. **Source chains are preserved.** Wrap, don't flatten: `#[source]` /
   `#[from]` keep the underlying rusqlite/HTTP error inspectable; the
   top-level `Display` message answers "what was the program trying to do".
5. **No panics across boundaries.** `unwrap()`/`expect()` only in tests and
   for provably-infallible cases with an `expect("reason")` message. Poison
   recovery for the store mutex stays in `AppState::with_store` (D-007).
6. **HTTP/JSON mapping is explicit.** Each `StoreError` variant maps to one
   status/JSON error shape in `recall-server` (404 unknown namespace, 400
   invalid params, 409 duplicate, 500 with no info leak: see D-008's
   no-info-leak rule); MCP tool errors map to JSON-RPC invalid-params vs
   internal-error. Keep the mapping table in one module.

## Rejected / optional

- `eyre`/`color-eyre`: nicer reports for apps; skip: two app crates don't
  justify the extra dependency.
- `error-stack`: typed context stacks; overkill, niche adoption.
- Snafu: viable thiserror alternative, but thiserror is the community
  default and already familiar here.
