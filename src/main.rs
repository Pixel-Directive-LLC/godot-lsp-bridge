//! `godot-lsp-bridge` — bidirectional TCP/stdio proxy for Godot's Language Server.
//!
//! Connects to Godot's TCP Language Server and bridges stdin/stdout with the TCP socket
//! using JSON-RPC message framing.  Supports auto-discovery of running Godot instances,
//! exponential-backoff retry when Godot is not yet open, and hot-reconnect on project
//! switch — covering the four real-world launch sequences (UC1–UC4) described in the
//! Phase 4 design.
//!
//! Additional subcommands:
//! - `update`          — download and replace the binary from the latest GitHub release.
//! - `doctor`          — diagnose PATH and LSP connectivity issues.
//! - `config get <k>`  — read a persistent setting.
//! - `config set <k> <v>` — write a persistent setting.

mod config;
mod doctor;
mod update;

use anyhow::Result;
use clap::{Parser, Subcommand};
use godot_lsp_bridge::bridge::{self, RunOutcome};
use godot_lsp_bridge::discovery::{
    connect_with_backoff, enumerate_candidates, DEFAULT_RETRY_TIMEOUT,
};
use std::time::Duration;
use tracing::info;
use tracing_subscriber::EnvFilter;

/// Proxy Godot's TCP Language Server to stdio for Claude Code.
#[derive(Parser, Debug)]
#[command(name = "godot-lsp-bridge", version, about)]
struct Cli {
    /// Subcommand to run. When omitted the bridge starts in proxy mode.
    #[command(subcommand)]
    command: Option<Commands>,

    /// Godot LSP host address.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Godot LSP port. When omitted the bridge auto-discovers running Godot instances
    /// by probing ports 6005–6014. Set explicitly to bypass auto-discovery.
    #[arg(long)]
    port: Option<u16>,

    /// Total seconds to keep retrying the connection before giving up (proxy mode only).
    #[arg(long, default_value_t = DEFAULT_RETRY_TIMEOUT.as_secs())]
    connect_timeout: u64,

    /// Tracing log level (error, warn, info, debug, or trace).
    #[arg(long, default_value = "info")]
    log_level: String,
}

/// Available subcommands.
#[derive(Subcommand, Debug)]
enum Commands {
    /// Download and install the latest release, then ensure the install directory is on PATH.
    Update,

    /// Check the environment and report diagnostics for PATH and Godot LSP connectivity.
    Doctor,

    /// Read or write persistent host/port configuration.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

/// `config` sub-subcommands.
#[derive(Subcommand, Debug)]
enum ConfigAction {
    /// Print the stored value for KEY (host or port).
    Get {
        /// Configuration key to read.
        key: String,
    },
    /// Store VALUE for KEY (host or port), creating the config file if needed.
    Set {
        /// Configuration key to write.
        key: String,
        /// Value to assign.
        value: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Dispatch subcommands before initialising the logger (they manage their own output).
    match cli.command {
        Some(Commands::Update) => return update::run().await,
        Some(Commands::Doctor) => {
            // Resolve host/port: CLI flag → config file → built-in default.
            let cfg = config::load().unwrap_or_default();
            let host = cli.host_or_config(cfg.host.as_deref());
            let port = cli.port_or_config(cfg.port);
            return doctor::run(&host, port).await;
        }
        Some(Commands::Config { action }) => {
            return match action {
                ConfigAction::Get { key } => config::get(&key),
                ConfigAction::Set { key, value } => config::set(&key, &value),
            };
        }
        None => {} // fall through to proxy mode
    }

    // ── Proxy mode ──────────────────────────────────────────────────────────

    let filter = EnvFilter::try_new(&cli.log_level).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    // Resolve host/port: CLI flag → config file → built-in default.
    let cfg = config::load().unwrap_or_default();
    let host = cli.host_or_config(cfg.host.as_deref());
    let port_override = cli.port.or(cfg.port);
    let timeout = Duration::from_secs(cli.connect_timeout);

    let port = match port_override {
        Some(p) => {
            info!("Using explicit port {p}; skipping auto-discovery");
            p
        }
        None => resolve_port(&host, timeout).await?,
    };

    // Outer reconnect loop (UC4): on TCP drop, re-probe and reconnect.
    loop {
        let stream = connect_with_backoff(&host, port, timeout).await?;
        info!("Bridging stdio <=> {host}:{port}");

        match bridge::run(stream).await? {
            RunOutcome::StdinClosed => {
                info!("Editor closed stdin — exiting");
                break;
            }
            RunOutcome::TcpClosed => {
                tracing::warn!("Godot LSP connection dropped — reconnecting (UC4) ...");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }

    Ok(())
}

impl Cli {
    /// Returns the host from the CLI flag; falls back to the config value then the flag default.
    fn host_or_config<'a>(&'a self, config_host: Option<&'a str>) -> String {
        // `--host` has a default value so `self.host` is always populated.
        // Prefer the CLI value when it differs from the hard-coded default, meaning the
        // user explicitly passed `--host`; otherwise fall back to the config file.
        if self.host != "127.0.0.1" {
            self.host.clone()
        } else {
            config_host.unwrap_or(&self.host).to_owned()
        }
    }

    /// Returns the port from the CLI flag, falling back to the config value.
    fn port_or_config(&self, config_port: Option<u16>) -> u16 {
        self.port.or(config_port).unwrap_or(6005)
    }
}

/// Scan candidate ports and return the port to connect to.
///
/// - 0 open: log that Godot is not yet running and return the default port so the
///   caller's backoff loop handles UC1.
/// - 1 open: connect immediately (UC2).
/// - N open: report ambiguity to the user on stderr (UC3) and return the first candidate.
///   The user can re-run with `--port` to select a specific instance.
async fn resolve_port(host: &str, timeout: Duration) -> Result<u16> {
    info!("Auto-discovering Godot LSP on {host} (ports 6005-6014) ...");
    let candidates = enumerate_candidates(host).await;

    match candidates.as_slice() {
        [] => {
            tracing::warn!(
                "No Godot LSP found on ports 6005-6014. \
                 Waiting for Godot to start (timeout {timeout:.0?}) ..."
            );
            Ok(6005)
        }
        [port] => {
            info!("Found Godot LSP on port {port}");
            Ok(*port)
        }
        ports => {
            let port_list: Vec<String> = ports.iter().map(|p| p.to_string()).collect();
            eprintln!(
                "godot-lsp-bridge: multiple Godot LSP instances detected on ports [{}].\n\
                 Connecting to port {} (the lowest-numbered instance).\n\
                 Run with --port <N> to connect to a specific instance.",
                port_list.join(", "),
                ports[0]
            );
            Ok(ports[0])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_args_parse() {
        let cli = Cli::parse_from(["godot-lsp-bridge"]);
        assert_eq!(cli.host, "127.0.0.1");
        assert!(cli.port.is_none());
        assert_eq!(cli.connect_timeout, DEFAULT_RETRY_TIMEOUT.as_secs());
        assert_eq!(cli.log_level, "info");
        assert!(cli.command.is_none());
    }

    #[test]
    fn explicit_port_parsed() {
        let cli = Cli::parse_from(["godot-lsp-bridge", "--port", "6006"]);
        assert_eq!(cli.port, Some(6006));
    }

    #[test]
    fn custom_args_parse() {
        let cli = Cli::parse_from([
            "godot-lsp-bridge",
            "--host",
            "0.0.0.0",
            "--port",
            "6006",
            "--log-level",
            "debug",
            "--connect-timeout",
            "60",
        ]);
        assert_eq!(cli.host, "0.0.0.0");
        assert_eq!(cli.port, Some(6006));
        assert_eq!(cli.log_level, "debug");
        assert_eq!(cli.connect_timeout, 60);
    }

    #[test]
    fn update_subcommand_parsed() {
        let cli = Cli::parse_from(["godot-lsp-bridge", "update"]);
        assert!(matches!(cli.command, Some(Commands::Update)));
    }

    #[test]
    fn doctor_subcommand_parsed() {
        let cli = Cli::parse_from(["godot-lsp-bridge", "doctor"]);
        assert!(matches!(cli.command, Some(Commands::Doctor)));
    }

    #[test]
    fn config_get_subcommand_parsed() {
        let cli = Cli::parse_from(["godot-lsp-bridge", "config", "get", "host"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Config {
                action: ConfigAction::Get { .. }
            })
        ));
    }

    #[test]
    fn config_set_subcommand_parsed() {
        let cli = Cli::parse_from(["godot-lsp-bridge", "config", "set", "port", "6005"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Config {
                action: ConfigAction::Set { .. }
            })
        ));
    }

    #[test]
    fn host_or_config_prefers_cli_when_explicit() {
        let cli = Cli::parse_from(["godot-lsp-bridge", "--host", "10.0.0.1"]);
        assert_eq!(cli.host_or_config(Some("192.168.1.1")), "10.0.0.1");
    }

    #[test]
    fn host_or_config_falls_back_to_config() {
        let cli = Cli::parse_from(["godot-lsp-bridge"]);
        assert_eq!(cli.host_or_config(Some("192.168.1.1")), "192.168.1.1");
    }

    #[test]
    fn port_or_config_prefers_cli() {
        let cli = Cli::parse_from(["godot-lsp-bridge", "--port", "6007"]);
        assert_eq!(cli.port_or_config(Some(6005)), 6007);
    }

    #[test]
    fn port_or_config_falls_back_to_config() {
        let cli = Cli::parse_from(["godot-lsp-bridge"]);
        assert_eq!(cli.port_or_config(Some(6008)), 6008);
    }

    #[test]
    fn port_or_config_uses_default_when_neither_set() {
        let cli = Cli::parse_from(["godot-lsp-bridge"]);
        assert_eq!(cli.port_or_config(None), 6005);
    }
}
