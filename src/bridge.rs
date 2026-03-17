//! Bidirectional TCP/stdio bridge with LSP method interception.
//!
//! [`run`] forwards LSP messages in both directions.  Before forwarding to Godot,
//! the bridge inspects each client message and either:
//!
//! - **passes it through** unchanged (the common path), or
//! - **intercepts it** to synthesise a response using Godot's available methods
//!   (see [`crate::synthesizer`] for the full list of intercepted methods).
//!
//! `textDocument/didOpen` and `textDocument/didClose` are additionally tracked so
//! the synthesiser knows which files are open (needed for `workspace/symbol`).
//!
//! [`run`] returns a [`RunOutcome`] so the caller can decide whether to reconnect
//! (TCP closed) or exit (stdin closed).

use crate::framing;
use crate::synthesizer::Synthesizer;
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;
use tokio::io::{self, BufReader, BufWriter};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};
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

    // Channel: messages destined for the Godot TCP socket.
    let (to_tcp_tx, to_tcp_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    // Channel: messages destined for the client (stdout).
    let (to_stdout_tx, to_stdout_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    let synth = Arc::new(Mutex::new(Synthesizer::new()));

    let outcome = tokio::select! {
        outcome = stdin_loop(
            BufReader::new(io::stdin()),
            Arc::clone(&synth),
            to_tcp_tx.clone(),
            to_stdout_tx.clone(),
        ) => outcome,

        outcome = tcp_reader_loop(
            BufReader::new(tcp_rx),
            Arc::clone(&synth),
            to_stdout_tx,
        ) => outcome,

        outcome = tcp_writer_loop(
            BufWriter::new(tcp_tx),
            to_tcp_rx,
        ) => outcome,

        outcome = stdout_writer_loop(
            BufWriter::new(io::stdout()),
            to_stdout_rx,
        ) => outcome,

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

// ── stdin → TCP (with intercept) ─────────────────────────────────────────────

/// Read LSP messages from stdin and either forward them to Godot or synthesise
/// a response locally.
async fn stdin_loop(
    mut stdin: BufReader<io::Stdin>,
    synth: Arc<Mutex<Synthesizer>>,
    to_tcp: mpsc::UnboundedSender<Vec<u8>>,
    to_stdout: mpsc::UnboundedSender<Vec<u8>>,
) -> RunOutcome {
    loop {
        match framing::read_message(&mut stdin).await {
            Ok(Some(msg)) => {
                handle_client_message(msg, Arc::clone(&synth), &to_tcp, &to_stdout).await;
            }
            Ok(None) => return RunOutcome::StdinClosed,
            Err(e) => {
                tracing::error!("stdin → TCP (read): {e}");
                return RunOutcome::StdinClosed;
            }
        }
    }
}

/// Inspect and route one client message.
///
/// - `textDocument/didOpen` / `didClose`: update open-file state, then pass through.
/// - `workspace/symbol`, `prepareCallHierarchy`, `callHierarchy/*`: intercept and
///   spawn a synthesis task.
/// - Everything else: pass through unchanged.
async fn handle_client_message(
    msg: Vec<u8>,
    synth: Arc<Mutex<Synthesizer>>,
    to_tcp: &mpsc::UnboundedSender<Vec<u8>>,
    to_stdout: &mpsc::UnboundedSender<Vec<u8>>,
) {
    let parsed: Value = match serde_json::from_slice(&msg) {
        Ok(v) => v,
        Err(_) => {
            // Unparseable — pass through and let Godot handle (or reject) it.
            let _ = to_tcp.send(msg);
            return;
        }
    };

    let method = parsed.get("method").and_then(Value::as_str);
    let id = parsed.get("id").cloned();
    let params = parsed.get("params").cloned().unwrap_or(Value::Null);

    match method {
        // Track open/closed files for workspace/symbol aggregation.
        Some("textDocument/didOpen") => {
            if let Some(uri) = params
                .get("textDocument")
                .and_then(|td| td.get("uri"))
                .and_then(Value::as_str)
            {
                synth.lock().await.on_did_open(uri.to_owned());
            }
            let _ = to_tcp.send(msg);
        }
        Some("textDocument/didClose") => {
            if let Some(uri) = params
                .get("textDocument")
                .and_then(|td| td.get("uri"))
                .and_then(Value::as_str)
            {
                synth.lock().await.on_did_close(uri);
            }
            let _ = to_tcp.send(msg);
        }

        // Intercepted: synthesise workspace/symbol locally.
        Some("workspace/symbol") => {
            if let Some(id) = id {
                let query = params
                    .get("query")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let to_tcp2 = to_tcp.clone();
                let to_stdout2 = to_stdout.clone();
                tokio::spawn(crate::synthesizer::workspace_symbol(
                    synth, to_tcp2, id, query, to_stdout2,
                ));
            }
            // Do not forward to Godot.
        }

        // Intercepted: prepareCallHierarchy.
        Some("textDocument/prepareCallHierarchy") => {
            if let Some(id) = id {
                let td = params.get("textDocument").cloned().unwrap_or(Value::Null);
                let pos = params.get("position").cloned().unwrap_or(Value::Null);
                let to_tcp2 = to_tcp.clone();
                let to_stdout2 = to_stdout.clone();
                tokio::spawn(crate::synthesizer::prepare_call_hierarchy(
                    synth, to_tcp2, id, td, pos, to_stdout2,
                ));
            }
        }

        // Intercepted: callHierarchy/incomingCalls.
        Some("callHierarchy/incomingCalls") => {
            if let Some(id) = id {
                let item = params.get("item").cloned().unwrap_or(Value::Null);
                let to_tcp2 = to_tcp.clone();
                let to_stdout2 = to_stdout.clone();
                tokio::spawn(crate::synthesizer::incoming_calls(
                    synth, to_tcp2, id, item, to_stdout2,
                ));
            }
        }

        // Intercepted: callHierarchy/outgoingCalls.
        Some("callHierarchy/outgoingCalls") => {
            if let Some(id) = id {
                let item = params.get("item").cloned().unwrap_or(Value::Null);
                let to_tcp2 = to_tcp.clone();
                let to_stdout2 = to_stdout.clone();
                tokio::spawn(crate::synthesizer::outgoing_calls(
                    synth, to_tcp2, id, item, to_stdout2,
                ));
            }
        }

        // All other messages pass through unchanged.
        _ => {
            let _ = to_tcp.send(msg);
        }
    }
}

// ── TCP → stdout (with sub-request routing) ───────────────────────────────────

/// Read messages from Godot and route them: sub-request responses go to the waiting
/// synthesiser; everything else (including `publishDiagnostics` notifications) is
/// forwarded to stdout.
async fn tcp_reader_loop(
    mut tcp_reader: BufReader<OwnedReadHalf>,
    synth: Arc<Mutex<Synthesizer>>,
    to_stdout: mpsc::UnboundedSender<Vec<u8>>,
) -> RunOutcome {
    loop {
        match framing::read_message(&mut tcp_reader).await {
            Ok(Some(msg)) => {
                route_godot_message(msg, Arc::clone(&synth), &to_stdout).await;
            }
            Ok(None) => return RunOutcome::TcpClosed,
            Err(e) => {
                tracing::error!("TCP → stdout (read): {e}");
                return RunOutcome::TcpClosed;
            }
        }
    }
}

/// Route one Godot message to either a pending synthesis sub-request or stdout.
async fn route_godot_message(
    msg: Vec<u8>,
    synth: Arc<Mutex<Synthesizer>>,
    to_stdout: &mpsc::UnboundedSender<Vec<u8>>,
) {
    let parsed: Value = match serde_json::from_slice(&msg) {
        Ok(v) => v,
        Err(_) => {
            let _ = to_stdout.send(msg);
            return;
        }
    };

    // Sub-request responses have string IDs prefixed with "__synth_".
    if let Some(id_str) = parsed.get("id").and_then(Value::as_str) {
        if id_str.starts_with("__synth_") {
            let result = parsed
                .get("result")
                .or_else(|| parsed.get("error"))
                .cloned()
                .unwrap_or(Value::Null);
            if synth.lock().await.try_complete(id_str, result) {
                return; // Consumed by synthesiser — do not forward to stdout.
            }
        }
    }

    // Notifications (no id), regular responses, and server-initiated requests all
    // go straight to stdout.
    let _ = to_stdout.send(msg);
}

// ── Writer loops ──────────────────────────────────────────────────────────────

/// Drain `rx` and write each message to the Godot TCP socket.
async fn tcp_writer_loop(
    mut tcp_writer: BufWriter<OwnedWriteHalf>,
    mut rx: mpsc::UnboundedReceiver<Vec<u8>>,
) -> RunOutcome {
    while let Some(msg) = rx.recv().await {
        if framing::write_message(&mut tcp_writer, &msg).await.is_err() {
            return RunOutcome::TcpClosed;
        }
    }
    RunOutcome::TcpClosed
}

/// Drain `rx` and write each message to stdout.
async fn stdout_writer_loop(
    mut stdout: BufWriter<io::Stdout>,
    mut rx: mpsc::UnboundedReceiver<Vec<u8>>,
) -> RunOutcome {
    while let Some(msg) = rx.recv().await {
        if framing::write_message(&mut stdout, &msg).await.is_err() {
            return RunOutcome::StdinClosed;
        }
    }
    RunOutcome::StdinClosed
}

// ── Shutdown signal ───────────────────────────────────────────────────────────

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
