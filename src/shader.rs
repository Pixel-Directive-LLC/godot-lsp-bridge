//! Synthesised diagnostics for `.gdshader` files.
//!
//! Godot's native LSP only serves diagnostics for GDScript.  This module fills the gap
//! by spawning Godot in headless mode with a small validation script, capturing stderr,
//! and converting shader compilation errors into `textDocument/publishDiagnostics`
//! notifications.
//!
//! The embedded GDScript ([`VALIDATE_SCRIPT`]) is written to a temp file at runtime,
//! executed via `godot --headless --script`, and cleaned up automatically.

use crate::synthesizer::make_notification;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

/// GDScript that loads a shader file to trigger compilation diagnostics on stderr.
///
/// Usage: `godot --headless --script <this_file> -- <shader_res_path>`
///
/// The script loads the shader via `load()`, which forces Godot's shader compiler to
/// run.  Any compilation errors are printed to stderr in Godot's standard format.
/// The script then exits immediately.
const VALIDATE_SCRIPT: &str = r#"@tool
extends SceneTree

func _init() -> void:
    var args := OS.get_cmdline_user_args()
    if args.size() == 0:
        push_error("validate_shader: no shader path argument")
        quit(1)
        return
    var shader_path := args[0]
    var shader := load(shader_path)
    if shader == null:
        push_error("validate_shader: failed to load " + shader_path)
        quit(1)
        return
    quit(0)
"#;

/// Tracks in-flight shader validation tasks so they can be cancelled on re-edit.
#[derive(Default)]
pub struct ShaderState {
    /// Map from shader URI to the running validation task handle.
    pending: HashMap<String, JoinHandle<()>>,
}

impl ShaderState {
    /// Cancel any in-flight validation for `uri`.
    pub fn cancel(&mut self, uri: &str) {
        if let Some(handle) = self.pending.remove(uri) {
            handle.abort();
        }
    }

    /// Register a new validation task for `uri`, cancelling any previous one.
    pub fn register(&mut self, uri: String, handle: JoinHandle<()>) {
        self.cancel(&uri);
        self.pending.insert(uri, handle);
    }

    /// Remove tracking for `uri` (called on `didClose`).
    pub fn remove(&mut self, uri: &str) {
        self.cancel(uri);
    }
}

// ── Diagnostics synthesis ────────────────────────────────────────────────────

/// Debounce delay before launching Godot for shader validation.
const DEBOUNCE_MS: u64 = 300;

/// Validate a shader file by running Godot headless and publish diagnostics.
///
/// This is spawned as a background task from the bridge.  It sleeps for the debounce
/// period first, giving the user time to finish typing before we spawn a subprocess.
pub async fn validate_shader(
    uri: String,
    timeout: Duration,
    godot_path: Option<String>,
    to_stdout: mpsc::UnboundedSender<Vec<u8>>,
) {
    // Debounce — if the task is aborted during this sleep, no subprocess is spawned.
    tokio::time::sleep(Duration::from_millis(DEBOUNCE_MS)).await;

    // If there is no configured/discovered Godot binary, publish a single hint
    // diagnostic so the AI client (or user) knows to set it up.
    let godot_bin = match godot_path {
        Some(p) => p,
        None => {
            let notification = make_notification(
                "textDocument/publishDiagnostics",
                json!({
                    "uri": uri,
                    "diagnostics": [{
                        "range": {
                            "start": { "line": 0, "character": 0 },
                            "end":   { "line": 0, "character": 0 },
                        },
                        "severity": 3,
                        "source": "gdshader",
                        "message": "Shader diagnostics unavailable: Godot binary not found. \
                            Run `godot-lsp-bridge config set godot-path /path/to/godot` \
                            or add Godot to your PATH.",
                    }],
                }),
            );
            let _ = to_stdout.send(notification);
            return;
        }
    };

    let file_path = match uri_to_path(&uri) {
        Some(p) => p,
        None => {
            warn!("shader: cannot convert URI to path: {uri}");
            return;
        }
    };

    let diagnostics = match run_godot_validation(&godot_bin, &file_path, timeout).await {
        Ok(diags) => diags,
        Err(e) => {
            warn!("shader: validation failed for {uri}: {e}");
            // Publish empty diagnostics to clear any stale errors.
            Vec::new()
        }
    };

    let notification = make_notification(
        "textDocument/publishDiagnostics",
        json!({
            "uri": uri,
            "diagnostics": diagnostics,
        }),
    );
    let _ = to_stdout.send(notification);
}

/// Build and send a `publishDiagnostics` notification with an empty diagnostics array.
pub fn clear_diagnostics(uri: &str, to_stdout: &mpsc::UnboundedSender<Vec<u8>>) {
    let notification = make_notification(
        "textDocument/publishDiagnostics",
        json!({
            "uri": uri,
            "diagnostics": [],
        }),
    );
    let _ = to_stdout.send(notification);
}

/// Returns `true` if `uri` points to a `.gdshader` file.
pub fn is_shader_uri(uri: &str) -> bool {
    uri.ends_with(".gdshader")
}

// ── Godot subprocess ─────────────────────────────────────────────────────────

/// Write the validation script to a temp file, run Godot headless, and parse stderr.
async fn run_godot_validation(
    godot_bin: &str,
    file_path: &Path,
    timeout: Duration,
) -> anyhow::Result<Vec<Value>> {
    use std::io::Write;
    use tokio::process::Command;

    // Write the embedded script to a temp file.
    let mut script_file = tempfile::Builder::new()
        .prefix("glb_shader_")
        .suffix(".gd")
        .tempfile()?;
    script_file.write_all(VALIDATE_SCRIPT.as_bytes())?;
    script_file.flush()?;
    let script_path = script_file.path().to_path_buf();

    // Resolve the res:// path relative to the project.
    // The shader file_path is an absolute OS path.  Godot's --script loads from the
    // filesystem, but the shader itself must be referenced as a res:// path so that
    // Godot's resource loader can find it within the project.
    //
    // We pass the absolute path and let the script use it directly — Godot's load()
    // accepts res:// paths, so we need to figure out the project root.
    // For simplicity, we pass the OS path and let the user's project.godot handle it.
    // Actually, the script receives the path via OS.get_cmdline_user_args(), and load()
    // requires a res:// path.  We'll pass the absolute path and have Godot try to
    // resolve it.  If the file is within a Godot project, load() with an absolute path
    // won't work — we need the res:// path.
    //
    // Strategy: find the nearest project.godot ancestor, compute the relative path,
    // and prefix with "res://".
    let res_path = to_res_path(file_path)?;

    // Find the project root (directory containing project.godot) for --path.
    let project_root = find_project_root(file_path)
        .ok_or_else(|| anyhow::anyhow!("no project.godot found above {}", file_path.display()))?;

    let mut cmd = Command::new(godot_bin);
    cmd.arg("--headless")
        .arg("--path")
        .arg(&project_root)
        .arg("--script")
        .arg(script_path.to_string_lossy().as_ref())
        .arg("--")
        .arg(&res_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());

    debug!(
        "shader: running {godot_bin} --headless --path {} --script {} -- {res_path}",
        project_root.display(),
        script_path.display()
    );

    let output = tokio::time::timeout(timeout, cmd.output()).await??;

    let stderr = String::from_utf8_lossy(&output.stderr);
    debug!("shader: godot stderr ({} bytes): {stderr}", stderr.len());

    Ok(parse_shader_errors(&stderr))
}

/// Walk up from `path` to find the directory containing `project.godot`.
fn find_project_root(path: &Path) -> Option<PathBuf> {
    let mut dir = if path.is_file() { path.parent()? } else { path };
    loop {
        if dir.join("project.godot").exists() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

/// Convert an absolute file path to a `res://` path relative to the project root.
fn to_res_path(file_path: &Path) -> anyhow::Result<String> {
    let project_root = find_project_root(file_path)
        .ok_or_else(|| anyhow::anyhow!("no project.godot found above {}", file_path.display()))?;
    let relative = file_path
        .strip_prefix(&project_root)
        .map_err(|_| anyhow::anyhow!("shader path not inside project root"))?;
    // Use forward slashes for res:// paths.
    let rel_str = relative.to_string_lossy().replace('\\', "/");
    Ok(format!("res://{rel_str}"))
}

// ── Stderr parsing ───────────────────────────────────────────────────────────

/// Parse Godot's stderr output into LSP `Diagnostic` objects.
///
/// Recognises two formats:
///
/// 1. **Shader compilation errors:**
///    `res://path.gdshader:LINE - MESSAGE. Shader compilation failed.`
///
/// 2. **Engine errors (with optional ANSI codes):**
///    `ERROR: MESSAGE at: FUNCTION (FILE:LINE)`
pub fn parse_shader_errors(stderr: &str) -> Vec<Value> {
    let mut diagnostics = Vec::new();

    for line in stderr.lines() {
        // Strip ANSI escape sequences for reliable matching.
        let clean = strip_ansi(line);

        // Format 1: res://path.gdshader:LINE - MESSAGE
        if let Some(diag) = parse_shader_compilation_line(&clean) {
            diagnostics.push(diag);
            continue;
        }

        // Format 2: ERROR: MESSAGE at: FUNCTION (FILE:LINE)
        if let Some(diag) = parse_engine_error_line(&clean) {
            diagnostics.push(diag);
        }
    }

    diagnostics
}

/// Parse a shader compilation error line.
///
/// Format: `res://path.gdshader:LINE - MESSAGE[. Shader compilation failed.]`
fn parse_shader_compilation_line(line: &str) -> Option<Value> {
    // Match: starts with res://, has :LINE, then " - " separator.
    let rest = line.strip_prefix("res://")?;
    let colon_pos = rest.find(':')?;
    let after_colon = &rest[colon_pos + 1..];

    // Extract line number — digits up to the next non-digit.
    let line_end = after_colon
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after_colon.len());
    if line_end == 0 {
        return None;
    }
    let line_num: u32 = after_colon[..line_end].parse().ok()?;

    // The message follows " - ".
    let msg_start = after_colon.find(" - ")?;
    let mut message = after_colon[msg_start + 3..].trim().to_owned();

    // Strip trailing "Shader compilation failed." if present.
    if let Some(stripped) = message.strip_suffix("Shader compilation failed.") {
        message = stripped.trim().trim_end_matches('.').to_owned();
    }

    if message.is_empty() {
        return None;
    }

    // LSP lines are 0-based; Godot reports 1-based.
    let lsp_line = line_num.saturating_sub(1);

    Some(json!({
        "range": {
            "start": { "line": lsp_line, "character": 0 },
            "end":   { "line": lsp_line, "character": 0 },
        },
        "severity": 1,
        "source": "gdshader",
        "message": message,
    }))
}

/// Parse an engine ERROR: line.
///
/// Format: `ERROR: MESSAGE at: FUNCTION (FILE:LINE)`
fn parse_engine_error_line(line: &str) -> Option<Value> {
    let rest = line.strip_prefix("ERROR: ")?.trim();

    // We only care about shader-related engine errors.
    // Look for " at: " separator.
    let at_pos = rest.find(" at: ")?;
    let message = rest[..at_pos].trim();

    // Skip generic engine errors that aren't shader-related.
    let lower = message.to_ascii_lowercase();
    if !lower.contains("shader")
        && !lower.contains("compile")
        && !lower.contains("parse")
        && !lower.contains("validate_shader")
    {
        return None;
    }

    // Try to extract line number from the " at: func (file:line)" suffix.
    let location = &rest[at_pos + 5..];
    let line_num = extract_line_from_location(location).unwrap_or(0);
    let lsp_line = if line_num > 0 { line_num - 1 } else { 0 };

    Some(json!({
        "range": {
            "start": { "line": lsp_line, "character": 0 },
            "end":   { "line": lsp_line, "character": 0 },
        },
        "severity": 1,
        "source": "gdshader",
        "message": message,
    }))
}

/// Extract a line number from a Godot error location string like `func_name (file.cpp:123)`.
fn extract_line_from_location(location: &str) -> Option<u32> {
    let paren_start = location.rfind('(')?;
    let paren_end = location.rfind(')')?;
    if paren_end <= paren_start {
        return None;
    }
    let inner = &location[paren_start + 1..paren_end];
    let colon = inner.rfind(':')?;
    inner[colon + 1..].parse().ok()
}

/// Strip ANSI escape sequences from a string.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip until we hit a letter (the terminator of the escape sequence).
            for esc_c in chars.by_ref() {
                if esc_c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

// ── URI / path conversion ────────────────────────────────────────────────────

/// Convert a `file://` URI to a local filesystem path.
///
/// Handles percent-decoding and the Windows `file:///C:/...` convention.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let path_str = uri.strip_prefix("file://")?;

    // Percent-decode.
    let decoded = percent_decode(path_str);

    // On Windows, file:///C:/foo → /C:/foo — strip leading slash before drive letter.
    #[cfg(windows)]
    {
        let trimmed = decoded.strip_prefix('/').unwrap_or(&decoded);
        if trimmed.len() >= 2 && trimmed.as_bytes()[1] == b':' {
            return Some(PathBuf::from(trimmed));
        }
        Some(PathBuf::from(&decoded))
    }
    #[cfg(not(windows))]
    {
        Some(PathBuf::from(&decoded))
    }
}

/// Minimal percent-decoding for file URIs.
fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_shader_errors ──────────────────────────────────────────────────

    #[test]
    fn parse_empty_stderr_returns_no_diagnostics() {
        assert!(parse_shader_errors("").is_empty());
    }

    #[test]
    fn parse_valid_shader_output_returns_no_diagnostics() {
        let stderr = "Godot Engine v4.4.stable - https://godotengine.org\n";
        assert!(parse_shader_errors(stderr).is_empty());
    }

    #[test]
    fn parse_shader_compilation_error() {
        let stderr = "res://test.gdshader:8 - Unknown identifier in expression: 'xx'. Shader compilation failed.";
        let diags = parse_shader_errors(stderr);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0]["range"]["start"]["line"], 7); // 0-based
        assert_eq!(diags[0]["severity"], 1);
        assert_eq!(
            diags[0]["message"],
            "Unknown identifier in expression: 'xx'"
        );
        assert_eq!(diags[0]["source"], "gdshader");
    }

    #[test]
    fn parse_multiple_shader_errors() {
        let stderr = "\
res://test.gdshader:3 - Expected valid type hint after ':'. Shader compilation failed.\n\
res://test.gdshader:8 - Unknown identifier in expression: 'xx'. Shader compilation failed.";
        let diags = parse_shader_errors(stderr);
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0]["range"]["start"]["line"], 2);
        assert_eq!(diags[1]["range"]["start"]["line"], 7);
    }

    #[test]
    fn parse_engine_error_with_shader_keyword() {
        let stderr =
            "ERROR: Shader compilation error at: compile_shader (servers/rendering.cpp:123)";
        let diags = parse_shader_errors(stderr);
        assert_eq!(diags.len(), 1);
        assert!(diags[0]["message"]
            .as_str()
            .unwrap()
            .contains("Shader compilation error"));
    }

    #[test]
    fn parse_engine_error_without_shader_keyword_ignored() {
        let stderr = "ERROR: Some random engine error at: do_thing (core/main.cpp:50)";
        assert!(parse_shader_errors(stderr).is_empty());
    }

    #[test]
    fn parse_ansi_coded_error() {
        let stderr =
            "\x1b[1;31mERROR: \x1b[0;91mShader parse failed\x1b[0;90m at: compile (render.cpp:42)";
        let diags = parse_shader_errors(stderr);
        assert_eq!(diags.len(), 1);
        assert!(diags[0]["message"]
            .as_str()
            .unwrap()
            .contains("Shader parse failed"));
    }

    #[test]
    fn parse_shader_error_line_1_maps_to_lsp_line_0() {
        let stderr = "res://shader.gdshader:1 - Unexpected token. Shader compilation failed.";
        let diags = parse_shader_errors(stderr);
        assert_eq!(diags[0]["range"]["start"]["line"], 0);
    }

    // ── strip_ansi ───────────────────────────────────────────────────────────

    #[test]
    fn strip_ansi_removes_codes() {
        assert_eq!(strip_ansi("\x1b[1;31mERROR:\x1b[0m hello"), "ERROR: hello");
    }

    #[test]
    fn strip_ansi_passthrough_clean_string() {
        assert_eq!(strip_ansi("hello world"), "hello world");
    }

    // ── uri_to_path ──────────────────────────────────────────────────────────

    #[test]
    fn uri_to_path_unix_style() {
        let p = uri_to_path("file:///home/user/project/test.gdshader");
        assert!(p.is_some());
        let path = p.unwrap();
        let s = path.to_string_lossy();
        assert!(s.contains("home") && s.contains("test.gdshader"));
    }

    #[test]
    fn uri_to_path_percent_encoded() {
        let p = uri_to_path("file:///path/my%20shader.gdshader");
        assert!(p.is_some());
        assert!(p.unwrap().to_string_lossy().contains("my shader"));
    }

    #[test]
    fn uri_to_path_not_file_uri_returns_none() {
        assert!(uri_to_path("https://example.com").is_none());
    }

    // ── percent_decode ───────────────────────────────────────────────────────

    #[test]
    fn percent_decode_spaces() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
    }

    #[test]
    fn percent_decode_no_encoding() {
        assert_eq!(percent_decode("hello"), "hello");
    }

    // ── extract_line_from_location ───────────────────────────────────────────

    #[test]
    fn extract_line_from_typical_location() {
        assert_eq!(
            extract_line_from_location("compile_shader (servers/rendering.cpp:123)"),
            Some(123)
        );
    }

    #[test]
    fn extract_line_from_location_no_parens() {
        assert_eq!(extract_line_from_location("no_parens"), None);
    }

    // ── ShaderState ──────────────────────────────────────────────────────────

    #[test]
    fn shader_state_cancel_noop_when_empty() {
        let mut state = ShaderState::default();
        state.cancel("file:///test.gdshader"); // should not panic
    }

    // ── to_res_path / find_project_root ──────────────────────────────────────

    #[test]
    fn find_project_root_returns_none_for_nonexistent() {
        assert!(find_project_root(Path::new("/nonexistent/path/shader.gdshader")).is_none());
    }
}
