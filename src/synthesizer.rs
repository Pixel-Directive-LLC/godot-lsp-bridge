//! Bridge-side synthesis for LSP methods not implemented by Godot's language server.
//!
//! Godot's LSP does not implement `workspace/symbol`,
//! `textDocument/prepareCallHierarchy`, or the `callHierarchy/*` methods — all return
//! `Method not found`.  This module synthesises responses for those methods by composing
//! Godot's available primitives:
//!
//! | Synthesised method | Sub-requests used |
//! |---|---|
//! | `workspace/symbol` | `textDocument/documentSymbol` per open file |
//! | `textDocument/prepareCallHierarchy` | `textDocument/documentSymbol` on current file |
//! | `callHierarchy/incomingCalls` | `textDocument/references` at item position |
//! | `callHierarchy/outgoingCalls` | `textDocument/documentSymbol` on item file |
//!
//! `textDocument/publishDiagnostics` is a server-push notification with no request ID;
//! it passes through the bridge unchanged via the normal TCP → stdout path.

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, Mutex};

/// Timeout for each synthesis sub-request to Godot.
const SUB_REQ_TIMEOUT: Duration = Duration::from_secs(5);

// ── Shared state ─────────────────────────────────────────────────────────────

/// Shared synthesis state, accessed by both the stdin intercept path and the TCP
/// response router.
///
/// All fields are protected behind the [`Synthesizer`] struct's [`Mutex`]-wrapped
/// `Arc` — callers must lock before use.  Lock acquisition is always brief (no
/// `await` while holding the guard).
#[derive(Default)]
pub struct Synthesizer {
    /// URIs of files currently open in the editor.
    open_files: HashSet<String>,
    /// Map from sub-request ID string (`__synth_N`) to the oneshot completion sender.
    pending: HashMap<String, oneshot::Sender<Value>>,
    /// Monotonically increasing counter used to generate unique sub-request IDs.
    next_id: u64,
}

impl Synthesizer {
    /// Create a new, empty `Synthesizer`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `uri` was opened in the editor (`textDocument/didOpen`).
    pub fn on_did_open(&mut self, uri: String) {
        self.open_files.insert(uri);
    }

    /// Record that `uri` was closed in the editor (`textDocument/didClose`).
    pub fn on_did_close(&mut self, uri: &str) {
        self.open_files.remove(uri);
    }

    /// Returns the set of currently tracked open file URIs.
    pub fn open_files(&self) -> &HashSet<String> {
        &self.open_files
    }

    /// Allocate a unique sub-request ID and register a completion channel.
    ///
    /// The returned `id` string must be used as the `id` field of the sub-request
    /// sent to Godot.  The [`oneshot::Receiver`] resolves once [`try_complete`] is
    /// called with a matching response.
    ///
    /// [`try_complete`]: Synthesizer::try_complete
    pub fn alloc_sub(&mut self) -> (String, oneshot::Receiver<Value>) {
        let id = format!("__synth_{}", self.next_id);
        self.next_id += 1;
        let (tx, rx) = oneshot::channel();
        self.pending.insert(id.clone(), tx);
        (id, rx)
    }

    /// Attempt to deliver a Godot response to a waiting synthesis sub-request.
    ///
    /// Returns `true` if `id` matched a pending sub-request (consumed); `false` if
    /// the response should be forwarded to the client instead.
    pub fn try_complete(&mut self, id: &str, result: Value) -> bool {
        if let Some(tx) = self.pending.remove(id) {
            // Receiver may have been dropped (e.g. bridge exited) — ignore the error.
            let _ = tx.send(result);
            true
        } else {
            false
        }
    }
}

// ── JSON-RPC helpers ──────────────────────────────────────────────────────────

/// Serialise a JSON-RPC request body with `id`, `method`, and `params`.
pub fn make_request(id: &str, method: &str, params: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id":      id,
        "method":  method,
        "params":  params,
    }))
    .expect("infallible JSON serialisation")
}

/// Serialise a JSON-RPC success response body with `id` and `result`.
pub fn make_response(id: &Value, result: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id":     id,
        "result": result,
    }))
    .expect("infallible JSON serialisation")
}

/// Serialise a JSON-RPC error response body with `id`, `code`, and `message`.
pub fn make_error(id: &Value, code: i64, message: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id":    id,
        "error": { "code": code, "message": message },
    }))
    .expect("infallible JSON serialisation")
}

// ── LSP data helpers ──────────────────────────────────────────────────────────

/// Flatten a `DocumentSymbol[]` tree into a flat `SymbolInformation[]` list for `uri`.
///
/// Each symbol's `selectionRange` (or `range` if absent) becomes the
/// `SymbolInformation.location.range`.  Children are flattened recursively with
/// `containerName` set to the parent's `name`.
fn flatten_doc_symbols(symbols: &[Value], uri: &str, container: Option<&str>) -> Vec<Value> {
    let mut out = Vec::new();
    for sym in symbols {
        let name = sym["name"].as_str().unwrap_or("?");
        let kind = &sym["kind"];
        let range = sym
            .get("selectionRange")
            .or_else(|| sym.get("range"))
            .cloned()
            .unwrap_or(Value::Null);

        let mut info = json!({
            "name": name,
            "kind": kind,
            "location": { "uri": uri, "range": range },
        });
        if let Some(c) = container {
            info["containerName"] = Value::String(c.to_owned());
        }
        out.push(info);

        if let Some(Value::Array(children)) = sym.get("children") {
            out.extend(flatten_doc_symbols(children, uri, Some(name)));
        }
    }
    out
}

/// Return `true` if LSP `position` falls within LSP `range` (inclusive start, inclusive end).
fn in_range(position: &Value, range: &Value) -> bool {
    fn lc(v: &Value) -> (i64, i64) {
        (
            v.get("line").and_then(Value::as_i64).unwrap_or(0),
            v.get("character").and_then(Value::as_i64).unwrap_or(0),
        )
    }
    let (pl, pc) = lc(position);
    let (sl, sc) = lc(&range["start"]);
    let (el, ec) = lc(&range["end"]);
    if pl < sl || pl > el {
        return false;
    }
    if pl == sl && pc < sc {
        return false;
    }
    if pl == el && pc > ec {
        return false;
    }
    true
}

/// Find the innermost `DocumentSymbol` in a symbol tree whose `range` contains `position`.
fn find_symbol_at<'a>(symbols: &'a [Value], position: &Value) -> Option<&'a Value> {
    for sym in symbols {
        if let Some(range) = sym.get("range") {
            if in_range(position, range) {
                if let Some(Value::Array(children)) = sym.get("children") {
                    if let Some(inner) = find_symbol_at(children, position) {
                        return Some(inner);
                    }
                }
                return Some(sym);
            }
        }
    }
    None
}

/// Await a synthesis sub-request response, returning `None` on timeout or cancellation.
async fn await_sub(rx: oneshot::Receiver<Value>) -> Option<Value> {
    tokio::time::timeout(SUB_REQ_TIMEOUT, rx)
        .await
        .ok()
        .and_then(|r| r.ok())
}

// ── Synthesis functions ───────────────────────────────────────────────────────

/// Synthesise `workspace/symbol` by aggregating `textDocument/documentSymbol` across
/// all open files and filtering by `query`.
///
/// For each open file a sub-request is issued to Godot.  Responses are awaited
/// concurrently (bounded by [`SUB_REQ_TIMEOUT`]), results are flattened into a flat
/// `SymbolInformation[]`, and the aggregated list filtered by `query` (case-insensitive
/// substring match) is returned to the client.
pub async fn workspace_symbol(
    synth: Arc<Mutex<Synthesizer>>,
    to_tcp: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    original_id: Value,
    query: String,
    to_stdout: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
) {
    // Snapshot open files and allocate sub-request IDs in a single, brief lock.
    let uri_sub_pairs: Vec<(String, String, oneshot::Receiver<Value>)> = {
        let mut s = synth.lock().await;
        let uris: Vec<String> = s.open_files().iter().cloned().collect();
        let subs: Vec<(String, oneshot::Receiver<Value>)> =
            (0..uris.len()).map(|_| s.alloc_sub()).collect();
        uris.into_iter()
            .zip(subs)
            .map(|(uri, (id, rx))| (uri, id, rx))
            .collect()
    };

    if uri_sub_pairs.is_empty() {
        let _ = to_stdout.send(make_response(&original_id, json!([])));
        return;
    }

    // Send all documentSymbol sub-requests to Godot.
    for (uri, sub_id, _) in &uri_sub_pairs {
        let body = make_request(
            sub_id,
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri } }),
        );
        let _ = to_tcp.send(body);
    }

    // Await all responses and aggregate.
    let q_lower = query.to_ascii_lowercase();
    let mut all_symbols: Vec<Value> = Vec::new();

    for (uri, _, rx) in uri_sub_pairs {
        if let Some(result) = await_sub(rx).await {
            if let Some(symbols) = result.as_array() {
                let flat = flatten_doc_symbols(symbols, &uri, None);
                for sym in flat {
                    let name = sym["name"].as_str().unwrap_or("").to_ascii_lowercase();
                    if q_lower.is_empty() || name.contains(&q_lower) {
                        all_symbols.push(sym);
                    }
                }
            }
        }
    }

    let _ = to_stdout.send(make_response(&original_id, Value::Array(all_symbols)));
}

/// Synthesise `textDocument/prepareCallHierarchy` by resolving the symbol at the
/// cursor position via `textDocument/documentSymbol` on the current file.
///
/// Returns a single-element `CallHierarchyItem[]` for the innermost symbol containing
/// `position`, or an empty array if no symbol is found.
pub async fn prepare_call_hierarchy(
    synth: Arc<Mutex<Synthesizer>>,
    to_tcp: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    original_id: Value,
    text_document: Value,
    position: Value,
    to_stdout: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
) {
    let uri = text_document["uri"].as_str().unwrap_or("").to_owned();
    let (sub_id, rx) = synth.lock().await.alloc_sub();

    let _ = to_tcp.send(make_request(
        &sub_id,
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": &uri } }),
    ));

    let result = match await_sub(rx).await {
        Some(r) => r,
        None => {
            let _ = to_stdout.send(make_response(&original_id, json!([])));
            return;
        }
    };

    let symbols = match result.as_array() {
        Some(s) => s,
        None => {
            let _ = to_stdout.send(make_response(&original_id, json!([])));
            return;
        }
    };

    let item = match find_symbol_at(symbols, &position) {
        Some(sym) => {
            let name = sym["name"].as_str().unwrap_or("?");
            let kind = &sym["kind"];
            let range = sym.get("range").cloned().unwrap_or(Value::Null);
            let sel = sym
                .get("selectionRange")
                .or_else(|| sym.get("range"))
                .cloned()
                .unwrap_or(Value::Null);
            json!({
                "name":           name,
                "kind":           kind,
                "uri":            &uri,
                "range":          range,
                "selectionRange": sel,
            })
        }
        None => {
            let _ = to_stdout.send(make_response(&original_id, json!([])));
            return;
        }
    };

    let _ = to_stdout.send(make_response(&original_id, json!([item])));
}

/// Synthesise `callHierarchy/incomingCalls` by finding all references to the call
/// hierarchy item via `textDocument/references`.
///
/// Each reference `Location` is returned as a `CallHierarchyIncomingCall` whose `from`
/// field points back to the referenced item.
pub async fn incoming_calls(
    synth: Arc<Mutex<Synthesizer>>,
    to_tcp: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    original_id: Value,
    item: Value,
    to_stdout: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
) {
    let uri = item["uri"].as_str().unwrap_or("").to_owned();
    let position = item["selectionRange"]["start"].clone().pipe_or(
        json!({"line": 0, "character": 0}),
    );

    let (sub_id, rx) = synth.lock().await.alloc_sub();
    let _ = to_tcp.send(make_request(
        &sub_id,
        "textDocument/references",
        json!({
            "textDocument": { "uri": &uri },
            "position": position,
            "context": { "includeDeclaration": false },
        }),
    ));

    let result = match await_sub(rx).await {
        Some(r) => r,
        None => {
            let _ = to_stdout.send(make_response(&original_id, json!([])));
            return;
        }
    };

    let calls: Vec<Value> = result
        .as_array()
        .map(|locs| {
            locs.iter()
                .map(|loc| {
                    let ref_uri = loc["uri"].as_str().unwrap_or("").to_owned();
                    let ref_range = loc.get("range").cloned().unwrap_or(Value::Null);
                    json!({
                        "from": {
                            "name":           item["name"],
                            "kind":           item["kind"],
                            "uri":            ref_uri,
                            "range":          ref_range,
                            "selectionRange": ref_range,
                        },
                        "fromRanges": [ref_range],
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let _ = to_stdout.send(make_response(&original_id, Value::Array(calls)));
}

/// Synthesise `callHierarchy/outgoingCalls` by querying `textDocument/documentSymbol`
/// for the item's file and returning symbols whose range falls inside the item's range.
///
/// Each contained symbol is returned as a `CallHierarchyOutgoingCall`.  This is an
/// approximation — Godot does not expose call-graph information directly.
pub async fn outgoing_calls(
    synth: Arc<Mutex<Synthesizer>>,
    to_tcp: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    original_id: Value,
    item: Value,
    to_stdout: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
) {
    let uri = item["uri"].as_str().unwrap_or("").to_owned();
    let item_range = item.get("range").cloned().unwrap_or(Value::Null);

    let (sub_id, rx) = synth.lock().await.alloc_sub();
    let _ = to_tcp.send(make_request(
        &sub_id,
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": &uri } }),
    ));

    let result = match await_sub(rx).await {
        Some(r) => r,
        None => {
            let _ = to_stdout.send(make_response(&original_id, json!([])));
            return;
        }
    };

    let symbols = match result.as_array() {
        Some(s) => s,
        None => {
            let _ = to_stdout.send(make_response(&original_id, json!([])));
            return;
        }
    };

    let flat = flatten_doc_symbols(symbols, &uri, None);
    let calls: Vec<Value> = flat
        .into_iter()
        .filter(|sym| {
            sym.get("location")
                .and_then(|loc| loc.get("range"))
                .and_then(|r| r.get("start"))
                .map(|start| in_range(start, &item_range))
                .unwrap_or(false)
        })
        .map(|sym| {
            let name = sym["name"].as_str().unwrap_or("?");
            let kind = &sym["kind"];
            let range = sym["location"]["range"].clone();
            json!({
                "to": {
                    "name":           name,
                    "kind":           kind,
                    "uri":            &uri,
                    "range":          range,
                    "selectionRange": range,
                },
                "fromRanges": [range],
            })
        })
        .collect();

    let _ = to_stdout.send(make_response(&original_id, Value::Array(calls)));
}

// ── Helper trait for Option chaining ─────────────────────────────────────────

/// Extension trait for replacing a `Value::Null` with a fallback.
trait PipeOr {
    fn pipe_or(self, fallback: Value) -> Value;
}

impl PipeOr for Value {
    fn pipe_or(self, fallback: Value) -> Value {
        if self.is_null() {
            fallback
        } else {
            self
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pos(line: i64, character: i64) -> Value {
        json!({ "line": line, "character": character })
    }

    fn range(sl: i64, sc: i64, el: i64, ec: i64) -> Value {
        json!({
            "start": { "line": sl, "character": sc },
            "end":   { "line": el, "character": ec },
        })
    }

    // ── in_range ──────────────────────────────────────────────────────────────

    #[test]
    fn in_range_start_boundary() {
        assert!(in_range(&pos(1, 0), &range(1, 0, 3, 10)));
    }

    #[test]
    fn in_range_end_boundary() {
        assert!(in_range(&pos(3, 10), &range(1, 0, 3, 10)));
    }

    #[test]
    fn in_range_middle() {
        assert!(in_range(&pos(2, 5), &range(1, 0, 3, 10)));
    }

    #[test]
    fn in_range_before_start_line() {
        assert!(!in_range(&pos(0, 0), &range(1, 0, 3, 10)));
    }

    #[test]
    fn in_range_after_end_line() {
        assert!(!in_range(&pos(4, 0), &range(1, 0, 3, 10)));
    }

    #[test]
    fn in_range_same_line_before_start_char() {
        assert!(!in_range(&pos(1, 0), &range(1, 5, 1, 20)));
    }

    #[test]
    fn in_range_same_line_after_end_char() {
        assert!(!in_range(&pos(1, 21), &range(1, 5, 1, 20)));
    }

    // ── flatten_doc_symbols ───────────────────────────────────────────────────

    #[test]
    fn flatten_single_symbol() {
        let sym = json!([{
            "name": "foo",
            "kind": 12,
            "range":          range(0, 0, 5, 0),
            "selectionRange": range(0, 4, 0, 7),
        }]);
        let flat = flatten_doc_symbols(sym.as_array().unwrap(), "file://a.gd", None);
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0]["name"], "foo");
        assert_eq!(flat[0]["location"]["uri"], "file://a.gd");
    }

    #[test]
    fn flatten_nested_symbols() {
        let sym = json!([{
            "name": "MyClass",
            "kind": 5,
            "range":          range(0, 0, 20, 0),
            "selectionRange": range(0, 6, 0, 13),
            "children": [{
                "name": "my_func",
                "kind": 12,
                "range":          range(2, 0, 5, 0),
                "selectionRange": range(2, 4, 2, 11),
            }]
        }]);
        let flat = flatten_doc_symbols(sym.as_array().unwrap(), "file://b.gd", None);
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0]["name"], "MyClass");
        assert_eq!(flat[1]["name"], "my_func");
        assert_eq!(flat[1]["containerName"], "MyClass");
    }

    // ── find_symbol_at ────────────────────────────────────────────────────────

    #[test]
    fn find_symbol_at_outer() {
        let syms = json!([{
            "name": "MyClass",
            "kind": 5,
            "range": range(0, 0, 20, 0),
        }]);
        let found = find_symbol_at(syms.as_array().unwrap(), &pos(10, 0));
        assert!(found.is_some());
        assert_eq!(found.unwrap()["name"], "MyClass");
    }

    #[test]
    fn find_symbol_at_inner_child_preferred() {
        let syms = json!([{
            "name": "MyClass",
            "kind": 5,
            "range": range(0, 0, 20, 0),
            "children": [{
                "name": "inner_func",
                "kind": 12,
                "range": range(5, 0, 8, 0),
            }]
        }]);
        // Position inside inner_func — should return inner_func, not MyClass.
        let found = find_symbol_at(syms.as_array().unwrap(), &pos(6, 0));
        assert!(found.is_some());
        assert_eq!(found.unwrap()["name"], "inner_func");
    }

    #[test]
    fn find_symbol_at_no_match() {
        let syms = json!([{
            "name": "MyClass",
            "kind": 5,
            "range": range(0, 0, 5, 0),
        }]);
        let found = find_symbol_at(syms.as_array().unwrap(), &pos(10, 0));
        assert!(found.is_none());
    }

    // ── Synthesizer state ─────────────────────────────────────────────────────

    #[test]
    fn synthesizer_open_close() {
        let mut s = Synthesizer::new();
        s.on_did_open("file://a.gd".to_owned());
        s.on_did_open("file://b.gd".to_owned());
        assert_eq!(s.open_files().len(), 2);
        s.on_did_close("file://a.gd");
        assert_eq!(s.open_files().len(), 1);
        assert!(s.open_files().contains("file://b.gd"));
    }

    #[test]
    fn synthesizer_alloc_and_complete() {
        let mut s = Synthesizer::new();
        let (id, mut rx) = s.alloc_sub();
        assert!(id.starts_with("__synth_"));

        // try_complete with matching id delivers value and returns true.
        assert!(s.try_complete(&id, json!({"result": 42})));

        // Receiver should now have the value.
        let v = rx.try_recv().expect("value should be delivered");
        assert_eq!(v["result"], 42);
    }

    #[test]
    fn synthesizer_try_complete_unknown_id_returns_false() {
        let mut s = Synthesizer::new();
        assert!(!s.try_complete("__synth_999", json!(null)));
    }

    // ── JSON-RPC helpers ──────────────────────────────────────────────────────

    #[test]
    fn make_request_round_trip() {
        let body = make_request(
            "__synth_0",
            "textDocument/documentSymbol",
            json!({"textDocument": {"uri": "file://x.gd"}}),
        );
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], "__synth_0");
        assert_eq!(v["method"], "textDocument/documentSymbol");
    }

    #[test]
    fn make_response_round_trip() {
        let body = make_response(&json!(1), json!([{"name": "foo"}]));
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["id"], 1);
        assert!(v["result"].is_array());
    }

    #[test]
    fn make_error_round_trip() {
        let body = make_error(&json!(2), -32601, "Method not found");
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], -32601);
    }
}
