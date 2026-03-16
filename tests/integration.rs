//! Integration tests against a live Godot LSP instance.
//!
//! All tests are marked `#[ignore]` — they are skipped in CI automatically and must be
//! run locally with Godot open.
//!
//! **Run locally (requires Godot editor open with a project):**
//! ```bash
//! cargo nextest run --ignored
//! # or
//! cargo test -- --ignored
//! ```

use godot_lsp_bridge::discovery::{enumerate_candidates, probe_port};
use godot_lsp_bridge::framing::{read_message, write_message};
use serde_json::json;
use tokio::io::BufReader;
use tokio::net::TcpStream;

// NOTE: requires Godot editor open with a project

/// Discovers a live Godot LSP port, panicking with a clear message if none is found.
///
/// Shared by all integration tests to avoid repeating the discovery + prerequisite-check
/// boilerplate.
async fn find_live_godot_port() -> u16 {
    let candidates = enumerate_candidates("127.0.0.1").await;
    assert!(
        !candidates.is_empty(),
        "No Godot LSP found on ports 6005\u{2013}6014. \
         Open a Godot project and re-run with `cargo nextest run --ignored`."
    );
    candidates[0]
}

/// Verifies that `enumerate_candidates` finds at least one open Godot LSP port.
///
/// Covers UC2 (Godot already running) — the most common developer workflow.
#[tokio::test]
#[ignore]
async fn connect_to_live_godot() {
    let port = find_live_godot_port().await;
    // Confirm we can actually connect to the discovered port.
    let stream = probe_port("127.0.0.1", port).await;
    assert!(
        stream.is_some(),
        "enumerate_candidates returned port {port} but probe_port failed to connect"
    );
}

/// Sends an LSP `initialize` request to a live Godot instance and validates the response.
///
/// Validates:
/// - The response is framed with a correct `Content-Length` header.
/// - The JSON body contains `jsonrpc`, `id`, and `result` fields.
#[tokio::test]
#[ignore]
async fn framing_round_trip_live() {
    let port = find_live_godot_port().await;

    let stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to Godot LSP");

    let (rx, tx) = stream.into_split();
    let mut reader = BufReader::new(rx);
    let mut writer = tx;

    // Build a minimal LSP `initialize` request.
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": null,
            "rootUri": null,
            "capabilities": {}
        }
    });
    let body = serde_json::to_vec(&request).expect("failed to serialise initialize request");

    write_message(&mut writer, &body)
        .await
        .expect("failed to send initialize request to Godot LSP");

    // Read the response — Godot should reply with an `initialize` result.
    let response_bytes = read_message(&mut reader)
        .await
        .expect("framing error reading Godot LSP response")
        .expect("unexpected EOF from Godot LSP");

    let response: serde_json::Value =
        serde_json::from_slice(&response_bytes).expect("response is not valid JSON");

    assert_eq!(
        response["jsonrpc"], "2.0",
        "expected jsonrpc field; got: {response}"
    );
    assert_eq!(response["id"], 1, "response id should match request id");
    assert!(
        response.get("result").is_some() || response.get("error").is_some(),
        "LSP response must contain 'result' or 'error'; got: {response}"
    );
}

/// Spawns `bridge::run()` against a live Godot port and confirms it exits cleanly.
///
/// Because test stdin has no LSP data, `bridge::run()` returns `RunOutcome::StdinClosed`
/// almost immediately.  The test asserts that no I/O error occurs during that path.
#[tokio::test]
#[ignore]
async fn bridge_runs_against_live_godot() {
    use godot_lsp_bridge::bridge::{run, RunOutcome};

    let port = find_live_godot_port().await;

    let stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to Godot LSP");

    // bridge::run reads from stdin; in the test runner stdin has no LSP payload so
    // read_message returns Ok(None) immediately → RunOutcome::StdinClosed.
    let outcome = run(stream).await.expect("bridge::run returned an error");
    assert_eq!(
        outcome,
        RunOutcome::StdinClosed,
        "expected StdinClosed on empty test stdin; got {outcome:?}"
    );
}
