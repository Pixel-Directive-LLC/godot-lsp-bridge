# Setting Up godot-lsp-bridge in Claude Code

`godot-lsp-bridge` connects Godot's built-in GDScript Language Server to Claude Code,
giving you completion, hover docs, go-to-definition, and diagnostics for `.gd` files.

---

## Prerequisites

- Godot 4.x with the built-in Language Server enabled (see below)
- A pre-built `godot-lsp-bridge` binary on your `PATH`

---

## Step 1 — Enable Godot's Language Server

In Godot:

1. Open **Editor → Editor Settings**
2. Navigate to **Network → Language Server**
3. Ensure **Use Built-in Words Only** is unchecked
4. Note the port (default **6005**); leave it at the default unless you have a conflict

Godot starts its LSP automatically when the editor opens.

---

## Step 2 — Install the binary

### Option A — Download a pre-built release

Download the binary for your platform from the [releases page][releases] and place it
somewhere on your `PATH` (e.g. `~/.local/bin` on Linux/macOS or `C:\Tools` on Windows).

### Option B — Build from source

```bash
git clone https://github.com/Pixel-Directive-LLC/godot-lsp-bridge
cd godot-lsp-bridge
cargo build --release
# Binary is at target/release/godot-lsp-bridge (or .exe on Windows)
```

---

## Step 3 — Register the plugin in Claude Code

Claude Code reads LSP configuration from `.claude-plugin/plugin.json` in your project
root.  This repository ships one at [`.claude-plugin/plugin.json`][plugin-json].

Copy `.claude-plugin/plugin.json` into the root of your Godot project:

```
my-godot-project/
  .claude-plugin/
    plugin.json   ← copy this file here
  project.godot
  src/
    Player.gd
```

The plugin tells Claude Code to launch `godot-lsp-bridge` over stdio whenever a `.gd`
file is opened.

---

## Step 4 — Open your project in Claude Code

```bash
cd my-godot-project
claude
```

Claude Code will detect `.claude-plugin/plugin.json`, launch `godot-lsp-bridge`, and
connect it to Godot's Language Server automatically.

---

## Launch sequence behaviour

`godot-lsp-bridge` handles the four common ways you might open your tools:

| Sequence | What happens |
|---|---|
| **Claude first, Godot later** | Bridge waits for Godot's LSP to come up, retrying with exponential backoff (default timeout: 5 min) |
| **Godot first, Claude later** | Bridge connects immediately on startup — no manual steps |
| **Multiple Godot instances** | Bridge lists all detected ports on stderr and connects to the lowest one; use `--port` to select a specific instance |
| **In-session project switch** | Bridge detects the dropped connection and reconnects to the new project's LSP automatically |

---

## Configuration flags

All flags are optional.  Run `godot-lsp-bridge --help` for the full list.

| Flag | Default | Purpose |
|---|---|---|
| `--port <N>` | auto-detect | Force a specific port; skips discovery |
| `--host <addr>` | `127.0.0.1` | Godot host address |
| `--connect-timeout <secs>` | `300` | Seconds to retry before giving up (UC1) |
| `--log-level <level>` | `info` | Logging verbosity (`error`/`warn`/`info`/`debug`/`trace`) |

Logs are always written to **stderr** so they do not corrupt the stdio LSP stream.

---

## Troubleshooting

**"No Godot LSP found on ports 6005–6014"**
Godot is not running, or its LSP is on a non-standard port.  Start Godot first, or
use `--port` to specify the exact port.

**"Multiple Godot LSP instances detected"**
More than one Godot editor is open.  Re-run with `--port <N>` where `N` is the port
for the project you want.

**LSP features not appearing in Claude Code**
Check that `.claude-plugin/plugin.json` is in the project root (not a subdirectory) and
that `godot-lsp-bridge` is on your `PATH`.

---

[releases]: https://github.com/Pixel-Directive-LLC/godot-lsp-bridge/releases
[plugin-json]: ../.claude-plugin/plugin.json
