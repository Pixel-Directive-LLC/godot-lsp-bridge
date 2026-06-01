# Review Rules — godot-lsp-bridge

Read by the user-level `pr-code-reviewer` skill (its Step 2) and applied on top of its generic correctness + cleanup review. These are this repo's own emphases — flag any regression hard.

## Load-bearing invariants — flag any regression as likely-blocking

- **LSP framing correctness** (`src/framing.rs`): messages are delimited by HTTP-style `Content-Length` headers followed by `\r\n\r\n` then exactly N body bytes. Any change to header parsing, the `\r\n` handling, byte counting, or clean-EOF (`Ok(None)`) semantics is high-risk — a framing bug silently corrupts every JSON-RPC message crossing the bridge.
- **`MAX_MESSAGE_SIZE` allocation guard** (`src/framing.rs`, 64 MiB): the declared `Content-Length` must be bounded before any allocation. Flag any read path that allocates from an attacker/peer-controlled length without enforcing this cap (runaway-allocation DoS).
- **Stdio ↔ TCP proxy contract**: this binary proxies Godot's TCP Language Server (port 6005) to stdio for the Claude Code LSP plugin. It transports JSON-RPC; it must not interpret, rewrite, or drop LSP payloads beyond framing. Flag changes that mutate message bodies or assume a non-Godot peer.
- **Tokio-only async**: `tokio` is the only runtime. Flag any introduction of `async-std`, `smol`, or blocking I/O on the async path.

## Conventions — flag violations

- **Zero-warning lint gate**: `cargo clippy -- -D warnings` must pass. Flag any new `#[allow(...)]` lacking a documented justification and operator approval.
- **Public items need doc comments**: every public item (lib re-exports the core modules) requires a `///` doc comment. Flag undocumented public additions.
- **Error handling**: `anyhow` for binary error propagation; typed `thiserror` errors at library boundaries. Flag a binary that swallows errors or a lib boundary that leaks `anyhow` where a typed error is expected.
- **Edition Rust 2021.**
- **File-safety**: nothing under `.claude/` may be modified or deleted; `.vscode/`, `.idea/`, `target/` must never be committed (all gitignored). Flag any diff touching these.
- **Public repo**: this is a public repository. Per the autopilot/audit rules, review text destined for GitHub goes through the operator approval gate — keep review output free of internal codenames, people, or secrets.
- **LSP-verified refactors**: renames/structural changes should be confirmed via rust-analyzer (`findReferences`, `goToDefinition`), not text search alone — trait impls, macro expansions, and re-exports are easy to miss. Be skeptical of a refactor that only updates grep-visible call sites.

## Hot files (extra scrutiny)

- `src/framing.rs` — JSON-RPC message framing; the correctness + allocation-guard core (see invariants above).
- `src/bridge.rs` — the stdio↔TCP proxy loop; concurrency/shutdown/EOF handling across the two directions.
- `src/discovery.rs` — Godot LSP endpoint discovery (port 6005); connection/retry assumptions.
- `src/synthesizer.rs` / `src/shader.rs` — LSP response synthesis; verify these don't malform or fabricate JSON-RPC the editor will choke on.
- `src/lib.rs` — public surface re-exporting the core modules; doc-comment + API-stability watch.
