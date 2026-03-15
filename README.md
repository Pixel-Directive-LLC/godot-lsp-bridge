# godot-lsp-bridge

A high-performance Rust proxy that bridges Godot's TCP Language Server (port 6005) to Stdio, enabling full GDScript code intelligence inside **Claude Code**.

---

> **AI-Coded Disclaimer**
> This repository is **100% AI-coded** using Claude Code (Anthropic) and is **human-reviewed** by a Senior Developer at Pixel Directive, LLC before any release or merge to `main`.

---

## Why Rust?

| Concern | Rust advantage |
|---|---|
| **Latency** | Zero-cost async via Tokio; no GC pauses on the hot path |
| **Throughput** | Lock-free I/O piping saturates the TCP socket without copying |
| **Reliability** | Ownership model eliminates data races on shared buffer state |
| **Binary size** | Single statically-linked executable — no runtime to install |
| **Cross-platform** | First-class support for Windows, macOS, and Linux from one codebase |

---

## Roadmap

- [x] **Phase 1 — Basic Async TCP/Stdio Proxy (Tokio)**
  - Clap CLI (`--port`, `--host`, `--log-level`)
  - Bidirectional `tokio::io::copy` loop between `TcpStream` and `stdin`/`stdout`
  - Graceful shutdown on SIGTERM / Ctrl-C

- [x] **Phase 2 — JSON-RPC Message Framing**
  - `Content-Length` header parser to prevent packet clipping on LSP boundaries
  - Buffered reader that reconstructs complete JSON-RPC messages before forwarding
  - Unit tests covering fragmented / batched TCP payloads

- [ ] **Phase 3 — Cross-platform CI/CD (GitHub Actions)**
  - Matrix: `ubuntu-latest`, `macos-latest`, `windows-latest`
  - Gates: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo nextest run`
  - Release artifacts: pre-built binaries uploaded per tag

- [ ] **Phase 4 — Claude Code Plugin Manifest**
  - `.claude-plugin/plugin.json` with stdin transport declaration
  - Auto-discovery of running Godot editor instance via port probe
  - Documentation for one-click setup inside Claude Code settings

---

## Quick Start (Phase 1, once implemented)

```bash
# Build
cargo build --release

# Run (Godot editor must be open with LSP enabled on port 6005)
./target/release/godot-lsp-bridge --port 6005
```

---

## Development

### Prerequisites

- Rust stable toolchain (`rustup`)
- [`cargo-nextest`](https://nexte.st/) — `cargo install cargo-nextest`
- [`bacon`](https://dystroy.org/bacon/) — `cargo install bacon` (continuous background checker)

### Commands

```bash
# Continuous check (recommended during development)
bacon

# Format
cargo fmt

# Lint (zero-warning policy)
cargo clippy -- -D warnings

# Test
cargo nextest run

# Build release
cargo build --release
```

---

## License

MPL-2.0 — see [LICENSE](./LICENSE).

---

*Pixel Directive, LLC — [pixeldirective.com](https://pixeldirective.com)*
