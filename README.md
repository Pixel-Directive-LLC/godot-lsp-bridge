# godot-lsp-bridge

A high-performance Rust proxy that bridges Godot's TCP Language Server (port 6005) to Stdio, enabling full GDScript code intelligence inside **Claude Code**.

---

## Features

- **Full GDScript intelligence** — go-to-definition, hover docs, completions, and diagnostics powered by Godot's native LSP
- **Auto-discovery** — scans ports 6005–6014 and connects to the running Godot instance automatically
- **Retry on startup** — exponential-backoff probe when Godot hasn't launched yet; no manual restarts needed
- **Hot-reconnect** — detects in-session project switches and reconnects without restarting Claude Code
- **Reliable framing** — `Content-Length` JSON-RPC framing prevents message truncation on high-latency connections
- **Zero dependencies at runtime** — single statically-linked binary; no Node, Python, or JVM required
- **Cross-platform** — native binaries for Windows, macOS, and Linux

---

## Installation

Install directly from the Claude Code marketplace:

```shell
/plugin marketplace add Pixel-Directive-LLC/godot-lsp-bridge
/plugin install godot-lsp-bridge@godot-lsp-bridge
```

Open Godot with a project — GDScript LSP starts automatically on port 6005. Open any `.gd` file — GDScript intelligence is live.

**Build from source** (contributors or platforms without pre-built binaries):

```bash
cargo install --git https://github.com/Pixel-Directive-LLC/godot-lsp-bridge
```

Then add to Claude Code manually:

```bash
claude mcp add --transport stdio godot-lsp-bridge godot-lsp-bridge
```

Or add directly to `~/.claude/settings.json`:

```json
{
  "lsp": {
    "godot-lsp-bridge": {
      "transport": "stdio",
      "command": "godot-lsp-bridge",
      "args": []
    }
  }
}
```

---

## Quick Start

```bash
# Auto-discover Godot on ports 6005–6014 (Godot editor must be open)
godot-lsp-bridge

# Connect to an explicit port (skips auto-discovery)
godot-lsp-bridge --port 6005

# Wait up to 10 minutes for Godot to start
godot-lsp-bridge --connect-timeout 600
```

---

## CLI Reference

All flags below are **stable** as of v1.0.

| Flag | Default | Description |
|---|---|---|
| `--host <ADDR>` | `127.0.0.1` | Godot LSP host |
| `--port <N>` | *(auto-detect)* | Skip discovery; connect to explicit port |
| `--connect-timeout <SECS>` | `300` | Max wait time for Godot to appear |
| `--log-level <LEVEL>` | `info` | Tracing level (`error`/`warn`/`info`/`debug`/`trace`) |

The `RUST_LOG` environment variable is also honoured (tracing-subscriber env-filter).

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

- [x] **Phase 3 — Cross-platform CI/CD (GitHub Actions)**
  - Matrix: `ubuntu-latest`, `macos-latest`, `windows-latest`
  - Gates: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo nextest run`
  - Release artifacts: pre-built binaries uploaded per tag

- [x] **Phase 4 — Claude Code Plugin Manifest**
  - `.claude-plugin/plugin.json` with stdin transport declaration
  - Auto-discovery of running Godot instances via concurrent port probe (ports 6005–6014)
  - Exponential-backoff retry when Godot has not yet started (UC1)
  - Immediate connection when Godot is already running (UC2)
  - Ambiguity report with port list when multiple instances are open (UC3)
  - Hot-reconnect on in-session Godot project switch (UC4)

- [x] **Phase 5 — Release Preparation**
  - Versioned release tags with pre-built binaries for all platforms
  - Automated release notes generated from PR labels and conventional commits
  - SHA-256 checksum and Sigstore/cosign keyless signature verification for all release artifacts

- [x] **Phase 6 — v1.0**
  - Stable public API and CLI contract (all flags frozen)
  - Full integration test suite against a live Godot LSP instance (`#[ignore]` in CI)
  - Binary installation commands and Claude Code plugin setup in README

- [x] **Phase 7 — Marketplace Distribution**
  - `.lsp.json` at repo root with `extensionToLanguage` mapping
  - `.claude-plugin/plugin.json` updated with full marketplace metadata
  - `.claude-plugin/marketplace.json` — repo self-hosts the marketplace
  - Single-command install via `/plugin marketplace add` + `/plugin install`

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

# Test (CI suite — skips #[ignore] tests)
cargo nextest run

# Integration tests (requires Godot editor open with a project)
cargo nextest run --ignored

# Build release
cargo build --release
```

---

## License

MPL-2.0 — see [LICENSE](./LICENSE).

---

> **AI-Coded Disclaimer**
> This repository is **100% AI-coded** using Claude Code (Anthropic) and is **human-reviewed** by a Senior Developer at Pixel Directive, LLC before any release or merge to `main`.

---

*Pixel Directive, LLC — [pixeldirective.com](https://pixeldirective.com)*

[releases]: https://github.com/Pixel-Directive-LLC/godot-lsp-bridge/releases
