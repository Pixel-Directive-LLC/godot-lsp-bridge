//! Godot LSP auto-discovery: port probing, candidate enumeration, and connection backoff.
//!
//! Handles four real-world launch sequences (UC1–UC3):
//!
//! - **UC1** (Claude first): [`connect_with_backoff`] retries on exponential schedule.
//! - **UC2** (Godot first): [`probe_port`] succeeds on the first attempt.
//! - **UC3** (Multiple instances): [`enumerate_candidates`] scans the candidate range and
//!   returns all open ports so callers can report ambiguity to the user.

use anyhow::{Context, Result};
use std::ops::RangeInclusive;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::time::sleep;

/// Ports scanned during auto-discovery (Godot LSP default is 6005).
pub const CANDIDATE_PORTS: RangeInclusive<u16> = 6005..=6014;

/// Default total duration for connection retry before giving up.
pub const DEFAULT_RETRY_TIMEOUT: Duration = Duration::from_secs(300);

const INITIAL_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Per-port probe timeout to keep enumeration fast.
const PROBE_TIMEOUT: Duration = Duration::from_millis(200);

/// Probe a single `host:port`, returning the connected stream if the port is open.
///
/// Uses a short timeout so enumeration across the candidate range stays fast.
pub async fn probe_port(host: &str, port: u16) -> Option<TcpStream> {
    let addr = format!("{host}:{port}");
    match tokio::time::timeout(PROBE_TIMEOUT, TcpStream::connect(&addr)).await {
        Ok(Ok(stream)) => Some(stream),
        _ => None,
    }
}

/// Probe all ports in [`CANDIDATE_PORTS`] concurrently and return those that are open.
///
/// Ports are returned in ascending order.  The associated TCP connections are dropped
/// immediately; callers should call [`connect_with_backoff`] or [`probe_port`] again to
/// obtain a live [`TcpStream`] for the chosen port.
pub async fn enumerate_candidates(host: &str) -> Vec<u16> {
    let futures: Vec<_> = CANDIDATE_PORTS
        .map(|port| async move {
            if probe_port(host, port).await.is_some() {
                Some(port)
            } else {
                None
            }
        })
        .collect();

    let results = futures::future::join_all(futures).await;
    results.into_iter().flatten().collect()
}

/// Connect to `host:port` with exponential backoff, retrying until `timeout` elapses.
///
/// The first attempt is immediate (UC2).  On failure the delay starts at 500 ms and
/// doubles each retry, capped at 30 s (UC1).  Returns an error once `timeout` is reached
/// without a successful connection.
///
/// Progress is logged to `tracing` at `warn` level so users can observe retries on stderr.
pub async fn connect_with_backoff(host: &str, port: u16, timeout: Duration) -> Result<TcpStream> {
    let deadline = Instant::now() + timeout;
    let mut delay = INITIAL_BACKOFF;
    let addr = format!("{host}:{port}");

    loop {
        match TcpStream::connect(&addr).await {
            Ok(stream) => {
                tracing::info!("Connected to Godot LSP at {addr}");
                return Ok(stream);
            }
            Err(e) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(e).with_context(|| {
                        format!("Godot LSP at {addr} unreachable after {timeout:?}")
                    });
                }
                let actual_delay = delay.min(remaining);
                tracing::warn!(
                    "Godot LSP at {addr} not available ({e}); retrying in {actual_delay:.1?}"
                );
                sleep(actual_delay).await;
                delay = (delay * 2).min(MAX_BACKOFF);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_ports_range() {
        let ports: Vec<u16> = CANDIDATE_PORTS.collect();
        assert_eq!(ports.first(), Some(&6005));
        assert_eq!(ports.last(), Some(&6014));
        assert_eq!(ports.len(), 10);
    }

    #[tokio::test]
    async fn probe_closed_port_returns_none() {
        // Port 1 is privileged and will always be refused or timeout — good enough for a
        // "definitely closed" probe in unit tests.
        let result = probe_port("127.0.0.1", 1).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn backoff_times_out_on_unreachable_host() {
        let timeout = Duration::from_millis(600);
        let err = connect_with_backoff("127.0.0.1", 1, timeout)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("unreachable"),
            "unexpected error: {err}"
        );
    }
}
