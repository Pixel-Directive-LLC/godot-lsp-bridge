//! Bidirectional TCP/stdio bridge with disconnect detection.
//!
//! [`run`] forwards LSP messages in both directions and returns a [`RunOutcome`] so the
//! caller can decide whether to reconnect (TCP closed) or exit (stdin closed).

use crate::framing;
use anyhow::Result;
use tokio::io::{self, BufReader, BufWriter};
use tokio::net::TcpStream;
use tracing::info;

/// Outcome of a single bridge session.
#[derive(Debug, PartialEq, Eq)]
pub enum RunOutcome {
    /// stdin reached EOF — the host editor (Claude Code) has exited.  Do not reconnect.
    StdinClosed,
    /// The TCP connection to Godot's LSP dropped.  Reconnecting is appropriate (UC4).
    TcpClosed,
}

/// Bridge `stream` bidirectionally with stdin/stdout using LSP message framing.
///
/// Returns [`RunOutcome::StdinClosed`] when stdin reaches EOF (editor exited) or
/// [`RunOutcome::TcpClosed`] when the Godot TCP connection drops (project switched).
///
/// A graceful OS shutdown signal also terminates the loop and returns
/// [`RunOutcome::StdinClosed`] (treated as a clean exit).
pub async fn run(stream: TcpStream) -> Result<RunOutcome> {
    let (tcp_rx, tcp_tx) = stream.into_split();
    let mut tcp_reader = BufReader::new(tcp_rx);
    let mut tcp_writer = BufWriter::new(tcp_tx);
    let mut stdin = BufReader::new(io::stdin());
    let mut stdout = BufWriter::new(io::stdout());

    let outcome = tokio::select! {
        result = async {
            while let Some(msg) = framing::read_message(&mut stdin).await? {
                framing::write_message(&mut tcp_writer, &msg).await?;
            }
            Ok::<_, anyhow::Error>(RunOutcome::StdinClosed)
        } => {
            match result {
                Ok(outcome) => outcome,
                Err(e) => {
                    tracing::error!("stdin → TCP: {e}");
                    RunOutcome::StdinClosed
                }
            }
        }
        result = async {
            while let Some(msg) = framing::read_message(&mut tcp_reader).await? {
                framing::write_message(&mut stdout, &msg).await?;
            }
            Ok::<_, anyhow::Error>(RunOutcome::TcpClosed)
        } => {
            match result {
                Ok(outcome) => outcome,
                Err(e) => {
                    tracing::error!("TCP → stdout: {e}");
                    RunOutcome::TcpClosed
                }
            }
        }
        res = shutdown_signal() => {
            match res {
                Ok(()) => info!("Shutdown signal received, exiting"),
                Err(e) => tracing::error!("Shutdown signal handler failed: {e}"),
            }
            RunOutcome::StdinClosed
        }
    };

    Ok(outcome)
}

/// Resolves when a shutdown signal is received (Ctrl-C on all platforms, SIGTERM on Unix).
async fn shutdown_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate())?;
        tokio::select! {
            res = tokio::signal::ctrl_c() => { res?; }
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await?;
    }
    Ok(())
}
