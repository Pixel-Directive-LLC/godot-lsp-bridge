//! Doctor subcommand — diagnose common environment issues.
//!
//! Checks:
//! 1. Whether the `godot-lsp-bridge` binary is reachable on `PATH`.
//! 2. Whether Godot's Language Server is reachable on the configured host and port.
//!
//! Exits with code 1 if any check fails so the result is machine-readable.

use anyhow::Result;
use std::path::Path;
use std::time::Duration;

/// Timeout for each LSP connectivity probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Run all diagnostic checks and print a pass/fail report to stdout.
///
/// Returns `Ok(())` on success or exits the process with code 1 when any check fails.
pub async fn run(host: &str, port: u16) -> Result<()> {
    let binary_ok = check_binary_in_path();
    let lsp_ok = check_lsp_reachable(host, port).await;

    println!("godot-lsp-bridge doctor");
    println!("=======================");
    print_status("binary in PATH", binary_ok);
    print_status(&format!("Godot LSP reachable ({host}:{port})"), lsp_ok);

    if !binary_ok || !lsp_ok {
        println!();
        if !binary_ok {
            println!(
                "  [PATH] Add the directory containing godot-lsp-bridge to your PATH.\n\n  \
                 Run `godot-lsp-bridge update` to reinstall and fix PATH automatically,\n  \
                 or add the install directory manually and restart your terminal."
            );
        }
        if !lsp_ok {
            println!(
                "  [LSP]  Godot is not reachable on {host}:{port}.\n\n  \
                 Open a project in the Godot editor and ensure the Language Server is\n  \
                 enabled: Editor -> Editor Settings -> Network -> Language Server -> Enable."
            );
        }
        std::process::exit(1);
    }

    println!();
    println!("All checks passed.");
    Ok(())
}

fn print_status(label: &str, ok: bool) {
    let tag = if ok { "PASS" } else { "FAIL" };
    println!("  [{tag}]  {label}");
}

/// Returns `true` if `godot-lsp-bridge` is found on `PATH`.
fn check_binary_in_path() -> bool {
    let exe = if cfg!(windows) {
        "godot-lsp-bridge.exe"
    } else {
        "godot-lsp-bridge"
    };

    let sep = if cfg!(windows) { ';' } else { ':' };

    std::env::var("PATH")
        .unwrap_or_default()
        .split(sep)
        .any(|dir| Path::new(dir).join(exe).is_file())
}

/// Returns `true` if a TCP connection to `host:port` succeeds within [`PROBE_TIMEOUT`].
async fn check_lsp_reachable(host: &str, port: u16) -> bool {
    tokio::time::timeout(PROBE_TIMEOUT, tokio::net::TcpStream::connect((host, port)))
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn closed_port_not_reachable() {
        // Port 1 is privileged / always refused — safe "definitely closed" port.
        assert!(!check_lsp_reachable("127.0.0.1", 1).await);
    }
}
