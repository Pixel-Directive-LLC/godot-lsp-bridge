//! Persistent configuration for godot-lsp-bridge.
//!
//! Stores host and port overrides in a JSON file under the platform's standard
//! config directory, providing a way to set persistent defaults without repeating
//! CLI flags on every invocation.
//!
//! Resolution order for proxy mode: CLI flag → config file → built-in default.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Persistent settings written to disk by the `config` subcommand.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    /// Persistent host override (equivalent to `--host`).
    pub host: Option<String>,
    /// Persistent port override (equivalent to `--port`).
    pub port: Option<u16>,
}

/// Returns the platform-appropriate config file path.
///
/// - Windows: `%APPDATA%\godot-lsp-bridge\config.json`
/// - macOS:   `~/Library/Application Support/godot-lsp-bridge/config.json`
/// - Linux:   `~/.config/godot-lsp-bridge/config.json`
pub fn config_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("could not determine the platform config directory")?
        .join("godot-lsp-bridge");
    Ok(dir.join("config.json"))
}

/// Loads config from disk, returning a default if the file does not exist.
pub fn load() -> Result<Config> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
}

/// Saves config to disk, creating parent directories as needed.
fn save(cfg: &Config) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(cfg)?;
    std::fs::write(&path, text).with_context(|| format!("failed to write {}", path.display()))
}

/// Prints the value of `key` from the config file, or `(not set)` if absent.
pub fn get(key: &str) -> Result<()> {
    let cfg = load()?;
    match key {
        "host" => match cfg.host {
            Some(v) => println!("{v}"),
            None => println!("(not set)"),
        },
        "port" => match cfg.port {
            Some(v) => println!("{v}"),
            None => println!("(not set)"),
        },
        _ => anyhow::bail!("unknown key {key:?}; valid keys: host, port"),
    }
    Ok(())
}

/// Writes `value` for `key` to the config file.
pub fn set(key: &str, value: &str) -> Result<()> {
    let mut cfg = load()?;
    match key {
        "host" => cfg.host = Some(value.to_owned()),
        "port" => {
            let port: u16 = value
                .parse()
                .with_context(|| format!("invalid port {value:?}: must be a number 1–65535"))?;
            cfg.port = Some(port);
        }
        _ => anyhow::bail!("unknown key {key:?}; valid keys: host, port"),
    }
    save(&cfg)?;
    let path = config_path()?;
    println!("Set {key} = {value}  ({})", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_path_is_non_empty() {
        let path = config_path().expect("config_path failed");
        assert!(path.ends_with("config.json"));
    }

    #[test]
    fn get_rejects_unknown_key() {
        let err = get("banana").unwrap_err();
        assert!(err.to_string().contains("unknown key"));
    }

    #[test]
    fn set_rejects_unknown_key() {
        let err = set("banana", "val").unwrap_err();
        assert!(err.to_string().contains("unknown key"));
    }

    #[test]
    fn set_rejects_invalid_port() {
        let err = set("port", "not-a-number").unwrap_err();
        assert!(err.to_string().contains("invalid port"));
    }
}
