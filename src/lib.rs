//! `godot-lsp-bridge` library — public API for integration tests and downstream use.
//!
//! Exposes the four core modules so that `tests/` and external crates can access
//! the bridge, discovery, framing, and synthesiser primitives without going through
//! the binary.

pub mod bridge;
pub mod discovery;
pub mod framing;
pub mod synthesizer;
