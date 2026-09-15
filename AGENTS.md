# Reseam Bot

Discord bot for the Reseam team with a native AI agent. Rust 2024, stable 1.94, poise 0.7 on serenity 0.12.

## Rules

Code is reviewed line by line. Simple, flat, idiomatic Rust beats clever Rust.

### Structure
- Single binary crate. Modules are files: `src/foo.rs` and `src/foo/bar.rs`. Never `mod.rs`.
- One concern per module. Keep files under ~400 lines; split by concern, not by type kind (no `types.rs`, `utils.rs`, `helpers.rs`).
- No trait unless two real implementations exist today. No generic parameter used by a single type. No builders for internal structs. No "manager", "service", "handler factory" layers.
- Shared state is one `Arc<App>`-style struct built in `main.rs`. Pass concrete types. Avoid `Arc<Mutex<_>>` when ownership, channels, or `DashMap`-free designs (a `tokio::sync::Mutex<HashMap>` held briefly) work.
- Do not add dependencies beyond `Cargo.toml` without a concrete need. If you add one, say why in your final report.

### Types and errors
- Typed serde structs for every external payload (Discord, OpenAI-compatible API, GitHub, Forgejo, config). `serde_json::Value` is allowed only for data that is opaque by nature: JSON Schemas, MCP tool arguments/results, `llm.extra_body`.
- Application code returns `anyhow::Result` and adds `.context(...)`/`.with_context(...)` at I/O boundaries. Use `thiserror` only if a caller matches on variants.
- No `unwrap()`/`expect()` outside tests, except on proven invariants with an `expect("why this holds")`.
- No defensive checks for impossible states. Validate at the boundary (config load, tool arguments, Discord input), then trust the types.
- Newtypes/enums over stringly-typed flags. `let else`, `?`, early returns. Iterators over index loops.

### Async
- tokio. Cancellation with `tokio_util::sync::CancellationToken`. Child processes use `kill_on_drop(true)`.
- Blocking work (PDF parsing, image resizing, file walking) goes through `tokio::task::spawn_blocking`.
- Never hold a lock across an `.await`.
- Spawned tasks must not swallow errors silently: log them with `tracing::warn!`/`error!` including structured fields.

### Style
- `cargo fmt`. `cargo clippy --all-targets -- -D warnings` must pass. No `#[allow(...)]` to silence a lint unless the lint is wrong for that line.
- No comments that restate code. No section banners. No doc comments on self-explanatory items. A short comment only for a non-obvious why.
- No dead code, no commented-out code, no `todo!()`, no placeholder functions.
- `tracing` for logs, never `println!`.
- Limits and tunables are `const` in the module that uses them, or config fields when an operator would change them.
- User-facing Discord text: short, plain, no emoji spam, no em-dashes.

### API accuracy
- Never guess a crate API. Read the source under `~/.cargo/registry/src/*/<crate>-<version>/` (poise-0.7.0, serenity-0.12.5, rmcp-3.3.0, sqlx-0.9.0, reqwest-0.13.5, schemars-1.x) before using it.
- Discord limits: message content 2000 chars, embed description 4096, 100 messages per history fetch, message edits are rate limited (throttle streaming edits to at most one per ~1.5s per message).

### Tests and verification
- While iterating: `cargo check`. At the end of a task: `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test`. Report the real output.
- Test pure logic that is easy to get wrong (splitting, truncation, parsing, config interpolation, the agent loop against a mock server). No tests that only restate the implementation.
- Do not commit. Do not edit `.env`.
