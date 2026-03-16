//! `godot-lsp-bridge` — bidirectional TCP/stdio proxy for Godot's Language Server.
//!
//! Connects to Godot's TCP Language Server and bridges stdin/stdout with the TCP socket
//! using JSON-RPC message framing.  Supports auto-discovery of running Godot instances,
//! exponential-backoff retry when Godot is not yet open, and hot-reconnect on project
//! switch — covering the four real-world launch sequences (UC1–UC4) described in the
//! Phase 4 design.

mod bridge;
mod discovery;
mod framing;

use anyhow::Result;
use bridge::RunOutcome;
use clap::Parser;
use discovery::{connect_with_backoff, enumerate_candidates, DEFAULT_RETRY_TIMEOUT};
use std::time::Duration;
use tracing::info;
use tracing_subscriber::EnvFilter;

/// Command-line interface for `godot-lsp-bridge`.
#[derive(Parser, Debug)]
#[command(
    name = "godot-lsp-bridge",
    about = "Proxy Godot's TCP Language Server to stdio for Claude Code"
)]
struct Args {
    /// Godot LSP host address.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Godot LSP port.  When omitted, the bridge auto-discovers running Godot instances
    /// by probing ports 6005–6014.  Set explicitly to bypass auto-discovery.
    #[arg(long)]
    port: Option<u16>,

    /// Total seconds to keep retrying the connection before giving up (UC1 backoff).
    #[arg(long, default_value_t = DEFAULT_RETRY_TIMEOUT.as_secs())]
    connect_timeout: u64,

    /// Tracing log level (error, warn, info, debug, or trace).
    #[arg(long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let filter = EnvFilter::try_new(&args.log_level).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    let timeout = Duration::from_secs(args.connect_timeout);

    // Determine which port to use.
    let port = match args.port {
        // Explicit --port: skip discovery, use it directly.
        Some(p) => {
            info!("Using explicit port {p}; skipping auto-discovery");
            p
        }
        // Auto-discover: probe the candidate range.
        None => resolve_port(&args.host, timeout).await?,
    };

    // Outer reconnect loop (UC4): on TCP drop, re-probe and reconnect.
    loop {
        let stream = connect_with_backoff(&args.host, port, timeout).await?;
        info!("Bridging stdio ↔ {}:{port}", args.host);

        match bridge::run(stream).await? {
            RunOutcome::StdinClosed => {
                info!("Editor closed stdin — exiting");
                break;
            }
            RunOutcome::TcpClosed => {
                tracing::warn!("Godot LSP connection dropped — reconnecting (UC4) …");
                // Brief pause before re-probe so Godot has time to restart its LSP.
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }

    Ok(())
}

/// Scan candidate ports and return the port to connect to.
///
/// - 0 open: log that Godot is not yet running and return the default port so the
///   caller's backoff loop handles UC1.
/// - 1 open: connect immediately (UC2).
/// - N open: report ambiguity to the user on stderr (UC3) and return the first candidate.
///   The user can re-run with `--port` to select a specific instance.
async fn resolve_port(host: &str, timeout: Duration) -> Result<u16> {
    info!("Auto-discovering Godot LSP on {host} (ports 6005–6014) …");
    let candidates = enumerate_candidates(host).await;

    match candidates.as_slice() {
        [] => {
            // UC1: Godot not yet running.  Return the default port; the backoff loop
            // will keep retrying until Godot's LSP becomes available.
            tracing::warn!(
                "No Godot LSP found on ports 6005–6014. \
                 Waiting for Godot to start (timeout {timeout:.0?}) …"
            );
            Ok(6005)
        }
        [port] => {
            info!("Found Godot LSP on port {port}");
            Ok(*port)
        }
        ports => {
            // UC3: multiple instances detected.  Connect to the first but surface the
            // ambiguity so the user can use --port to be explicit.
            let port_list: Vec<String> = ports.iter().map(|p| p.to_string()).collect();
            eprintln!(
                "godot-lsp-bridge: multiple Godot LSP instances detected on ports [{}].\n\
                 Connecting to port {} (the lowest-numbered instance).\n\
                 Run with --port <N> to connect to a specific instance.",
                port_list.join(", "),
                ports[0]
            );
            Ok(ports[0])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_args_parse() {
        let args = Args::parse_from(["godot-lsp-bridge"]);
        assert_eq!(args.host, "127.0.0.1");
        assert!(args.port.is_none());
        assert_eq!(args.connect_timeout, DEFAULT_RETRY_TIMEOUT.as_secs());
        assert_eq!(args.log_level, "info");
    }

    #[test]
    fn explicit_port_parsed() {
        let args = Args::parse_from(["godot-lsp-bridge", "--port", "6006"]);
        assert_eq!(args.port, Some(6006));
    }

    #[test]
    fn custom_args_parse() {
        let args = Args::parse_from([
            "godot-lsp-bridge",
            "--host",
            "0.0.0.0",
            "--port",
            "6006",
            "--log-level",
            "debug",
            "--connect-timeout",
            "60",
        ]);
        assert_eq!(args.host, "0.0.0.0");
        assert_eq!(args.port, Some(6006));
        assert_eq!(args.log_level, "debug");
        assert_eq!(args.connect_timeout, 60);
    }
}
