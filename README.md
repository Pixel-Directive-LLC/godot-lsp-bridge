# Godot LSP Bridge

```
┌──────────────────────────────────────────────┐
│   G O D O T   L S P   B R I D G E           │
│   GDScript intelligence for Claude Code      │
└──────────────────────────────────────────────┘
```

Full GDScript code intelligence — go-to-definition, hover docs, completions, and diagnostics — inside **Claude Code**, powered by Godot's native Language Server.

---

## Supported Godot Versions

Godot 4.x (4.0 and later). The LSP server runs on port 6005 automatically when the Godot editor is open with a project loaded.

---

## Quick Start

### Step 1 — Install the binary

**Linux / macOS (recommended):**

```bash
curl -fsSL https://raw.githubusercontent.com/Pixel-Directive-LLC/godot-lsp-bridge/main/install.sh | bash
```

**Windows (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/Pixel-Directive-LLC/godot-lsp-bridge/main/install.ps1 | iex
```

**Rust developers — install via cargo-binstall (downloads pre-built binary, no compile):**

```bash
cargo binstall godot-lsp-bridge
```

### Step 2 — Register the Claude Code plugin

```shell
/plugin marketplace add https://github.com/Pixel-Directive-LLC/godot-lsp-bridge.git
/plugin install godot-lsp-bridge@godot-lsp-bridge
```

That's it. Open Godot with a project, open any `.gd` file in Claude Code, and GDScript intelligence is live.

**Manual registration (fallback):** if the marketplace commands are unavailable, add the entry directly to `~/.claude/settings.json`:

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

## Features

- **Full GDScript intelligence** — go-to-definition, hover docs, completions, and diagnostics powered by Godot's native LSP
- **Auto-discovery** — scans ports 6005–6014 and connects to the running Godot instance automatically
- **Retry on startup** — exponential-backoff probe when Godot hasn't launched yet; no manual restarts needed
- **Hot-reconnect** — detects in-session project switches and reconnects without restarting Claude Code
- **Reliable framing** — `Content-Length` JSON-RPC framing prevents message truncation on high-latency connections
- **Zero dependencies at runtime** — single statically-linked binary; no Node, Python, or JVM required
- **Cross-platform** — native binaries for Windows, macOS, and Linux

---

## CLI Reference

### Proxy flags

All flags below are **stable** as of v1.0.

| Flag | Default | Description |
|---|---|---|
| `--version` | — | Print version and exit |
| `--host <ADDR>` | `127.0.0.1` | Godot LSP host |
| `--port <N>` | *(auto-detect)* | Skip discovery; connect to explicit port |
| `--connect-timeout <SECS>` | `300` | Max wait time for Godot to appear |
| `--log-level <LEVEL>` | `info` | Tracing level (`error`/`warn`/`info`/`debug`/`trace`) |

Resolution order for `--host` and `--port`: CLI flag → config file → built-in default.

The `RUST_LOG` environment variable is also honoured (tracing-subscriber env-filter).

```bash
# Print the installed version
godot-lsp-bridge --version

# Auto-discover Godot on ports 6005–6014 (Godot editor must be open)
godot-lsp-bridge

# Connect to an explicit port (skips auto-discovery)
godot-lsp-bridge --port 6005

# Wait up to 10 minutes for Godot to start
godot-lsp-bridge --connect-timeout 600
```

### Subcommands

#### `update`

Download and install the latest release, then ensure the install directory is on `PATH`.

```bash
godot-lsp-bridge update
```

- Fetches release metadata from GitHub, downloads the platform-appropriate archive, and atomically replaces the running binary.
- If the install directory is not already on `PATH`, it is added automatically (shell rc file on Linux/macOS; User PATH via PowerShell on Windows).

#### `doctor`

Check the environment and report diagnostics. Exits with code 1 if any check fails.

```bash
godot-lsp-bridge doctor
```

Checks:

1. **Binary in PATH** — confirms `godot-lsp-bridge` is reachable from the shell.
2. **Godot LSP reachable** — probes the configured `host:port` with a 2-second TCP timeout.

Host and port are resolved using the same priority as proxy mode (CLI flag → config file → built-in default). Pass `--host` or `--port` to test a non-default target:

```bash
godot-lsp-bridge --port 6007 doctor
```

#### `config`

Read or write persistent host/port defaults. Settings are stored in a JSON file under the platform config directory:

| Platform | Path |
|---|---|
| Windows | `%APPDATA%\godot-lsp-bridge\config.json` |
| macOS | `~/Library/Application Support/godot-lsp-bridge/config.json` |
| Linux | `~/.config/godot-lsp-bridge/config.json` |

Supported keys: `host`, `port`.

```bash
# Read a value (prints "(not set)" if absent)
godot-lsp-bridge config get host
godot-lsp-bridge config get port

# Write a value
godot-lsp-bridge config set host 192.168.1.10
godot-lsp-bridge config set port 6007
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

## Development

<details>
<summary>Build from source (contributors)</summary>

### Prerequisites

- Rust stable toolchain (`rustup`)
- [`cargo-nextest`][nextest] — `cargo install cargo-nextest`
- [`bacon`][bacon] — `cargo install bacon` (continuous background checker)

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

Or install the HEAD revision directly without cloning:

```bash
cargo install --git https://github.com/Pixel-Directive-LLC/godot-lsp-bridge
```

</details>

---

## License

MPL-2.0 — see [LICENSE](./LICENSE).

---

*Pixel Directive, LLC — [pixeldirective.com](https://pixeldirective.com)*

[nextest]: https://nexte.st/
[bacon]: https://dystroy.org/bacon/
[releases]: https://github.com/Pixel-Directive-LLC/godot-lsp-bridge/releases
