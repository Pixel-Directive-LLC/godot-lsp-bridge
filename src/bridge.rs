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
        outcome = async {
            loop {
                match framing::read_message(&mut stdin).await {
                    Ok(Some(msg)) => {
                        if let Err(e) = framing::write_message(&mut tcp_writer, &msg).await {
                            tracing::error!("stdin → TCP (write): {e}");
                            return RunOutcome::TcpClosed;
                        }
                    }
                    Ok(None) => return RunOutcome::StdinClosed,
                    Err(e) => {
                        tracing::error!("stdin → TCP (read): {e}");
                        return RunOutcome::StdinClosed;
                    }
                }
            }
        } => outcome,
        outcome = async {
            loop {
                match framing::read_message(&mut tcp_reader).await {
                    Ok(Some(msg)) => {
                        if let Err(e) = framing::write_message(&mut stdout, &msg).await {
                            tracing::error!("TCP → stdout (write): {e}");
                            return RunOutcome::StdinClosed;
                        }
                    }
                    Ok(None) => return RunOutcome::TcpClosed,
                    Err(e) => {
                        tracing::error!("TCP → stdout (read): {e}");
                        return RunOutcome::TcpClosed;
                    }
                }
            }
        } => outcome,
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
