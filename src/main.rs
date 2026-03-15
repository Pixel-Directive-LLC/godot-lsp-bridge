//! `godot-lsp-bridge` — bidirectional TCP/stdio proxy for Godot's Language Server.
//!
//! Connects to Godot's TCP Language Server (default `127.0.0.1:6005`) and forwards
//! raw bytes between stdin/stdout and the TCP socket, enabling editors that consume
//! stdio-based LSPs (e.g. Claude Code) to speak with Godot's LSP.

use anyhow::{Context, Result};
use clap::Parser;
use tokio::io;
use tokio::net::TcpStream;
use tracing::info;
use tracing_subscriber::EnvFilter;

/// Command-line interface for `godot-lsp-bridge`.
#[derive(Parser, Debug)]
#[command(
    name = "godot-lsp-bridge",
    about = "Proxy Godot's TCP Language Server to stdio"
)]
struct Args {
    /// Godot LSP host address.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Godot LSP port.
    #[arg(long, default_value_t = 6005)]
    port: u16,

    /// Tracing log level (error, warn, info, debug, or trace).
    #[arg(long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let filter = EnvFilter::try_new(&args.log_level)
        .with_context(|| format!("invalid log filter: '{}'", &args.log_level))?;
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    let addr = format!("{}:{}", args.host, args.port);
    info!("Connecting to Godot LSP at {addr}");

    let stream = TcpStream::connect(&addr)
        .await
        .with_context(|| format!("failed to connect to {addr}"))?;

    info!("Connected \u{2014} bridging stdio \u{2194} {addr}");

    run(stream).await
}

/// Bridges `stream` bidirectionally with stdin/stdout until EOF or a shutdown signal.
async fn run(stream: TcpStream) -> Result<()> {
    let (mut tcp_rx, mut tcp_tx) = stream.into_split();
    let mut stdin = io::stdin();
    let mut stdout = io::stdout();

    tokio::select! {
        result = io::copy(&mut stdin, &mut tcp_tx) => {
            if let Err(e) = result {
                tracing::error!("stdin \u{2192} TCP: {e}");
            }
        }
        result = io::copy(&mut tcp_rx, &mut stdout) => {
            if let Err(e) = result {
                tracing::error!("TCP \u{2192} stdout: {e}");
            }
        }
        res = shutdown_signal() => {
            if let Err(e) = res {
                tracing::error!("Shutdown signal handler failed: {e}");
            } else {
                info!("Shutdown signal received, exiting");
            }
        }
    }

    Ok(())
}

/// Resolves when a shutdown signal is received (Ctrl-C, or SIGTERM on Unix).
async fn shutdown_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate())?;
        tokio::select! {
            res = tokio::signal::ctrl_c() => {
                res?;
            }
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_args_parse() {
        let args = Args::parse_from(["godot-lsp-bridge"]);
        assert_eq!(args.host, "127.0.0.1");
        assert_eq!(args.port, 6005);
        assert_eq!(args.log_level, "info");
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
        ]);
        assert_eq!(args.host, "0.0.0.0");
        assert_eq!(args.port, 6006);
        assert_eq!(args.log_level, "debug");
    }
}
