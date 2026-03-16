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

### Option 1 — Pre-built binary (recommended)

```bash
# macOS / Linux
curl -L https://github.com/Pixel-Directive-LLC/godot-lsp-bridge/releases/latest/download/godot-lsp-bridge-x86_64-unknown-linux-gnu.tar.gz | tar xz
sudo mv godot-lsp-bridge /usr/local/bin/
```

```powershell
# Windows (PowerShell)
Invoke-WebRequest -Uri "https://github.com/Pixel-Directive-LLC/godot-lsp-bridge/releases/latest/download/godot-lsp-bridge-x86_64-pc-windows-msvc.zip" -OutFile bridge.zip
Expand-Archive bridge.zip .; Move-Item godot-lsp-bridge.exe $env:USERPROFILE\.local\bin\
```

See the [Releases page][releases] for all platform downloads and SHA-256 checksums.

### Option 2 — Build from source

```bash
cargo install --git https://github.com/Pixel-Directive-LLC/godot-lsp-bridge
```

---

## Claude Code Plugin Setup

1. Put `godot-lsp-bridge` on your `PATH` (see [Installation](#installation) above)
2. Open Godot with a project — GDScript LSP starts automatically on port 6005
3. Add the plugin to Claude Code:

```bash
claude mcp add --transport stdio godot-gdscript godot-lsp-bridge
```

4. Open a `.gd` file in your project — GDScript intelligence is live

**Manual configuration** — add to `~/.claude/settings.json`:

```json
{
  "lsp": {
    "godot-gdscript": {
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

## Verify Release Signatures

Every release artifact is signed with [Sigstore/cosign][cosign] keyless signing and includes a SHA-256 checksum file.

```bash
# Verify checksum
sha256sum --check godot-lsp-bridge-x86_64-unknown-linux-gnu.tar.gz.sha256

# Verify cosign signature (requires cosign installed)
cosign verify-blob \
  --certificate godot-lsp-bridge-x86_64-unknown-linux-gnu.tar.gz.pem \
  --signature godot-lsp-bridge-x86_64-unknown-linux-gnu.tar.gz.sig \
  --certificate-identity-regexp "https://github.com/Pixel-Directive-LLC/godot-lsp-bridge" \
  --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
  godot-lsp-bridge-x86_64-unknown-linux-gnu.tar.gz
```

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
[cosign]: https://docs.sigstore.dev/cosign/overview/
