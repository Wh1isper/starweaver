//! Standalone JSON-RPC host process for Starweaver.

use std::{path::PathBuf, process::ExitCode};

use clap::Parser;
use starweaver_rpc::{RpcConfig, RpcTransport, run};

fn main() -> ExitCode {
    let cli = StandaloneRpcCli::parse();
    let result = match cli.launch_envelope.as_deref() {
        Some(_) if cli.transport != RpcTransport::Stdio => {
            Err(starweaver_rpc::RpcHostError::Invalid(
                "supervised launch envelopes require the stdio transport".to_string(),
            ))
        }
        Some(path) => RpcConfig::from_launch_envelope(path),
        None => RpcConfig::resolve(cli.store),
    }
    .and_then(|config| run(&config, cli.transport, &cli.host, cli.port));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

/// Standalone RPC host command.
#[derive(Clone, Debug, Parser)]
#[command(
    name = "starweaver-rpc",
    version,
    about = "Starweaver JSON-RPC host process"
)]
struct StandaloneRpcCli {
    /// Override local store database path for standalone mode.
    #[arg(long, conflicts_with = "launch_envelope")]
    store: Option<String>,
    /// Exact public supervised-process launch envelope.
    #[arg(long, value_name = "ABSOLUTE_PATH")]
    launch_envelope: Option<PathBuf>,
    /// Runtime transport.
    #[arg(default_value = "stdio")]
    transport: RpcTransport,
    /// HTTP bind host.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// HTTP bind port.
    #[arg(long, default_value_t = 8765)]
    port: u16,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn parses_standalone_rpc_args() {
        let parsed = StandaloneRpcCli::try_parse_from([
            "starweaver-rpc".to_string(),
            "--store".to_string(),
            "/tmp/starweaver.db".to_string(),
            "http".to_string(),
            "--port".to_string(),
            "0".to_string(),
        ])
        .unwrap();
        assert_eq!(parsed.store.as_deref(), Some("/tmp/starweaver.db"));
        assert!(parsed.launch_envelope.is_none());
        assert_eq!(parsed.transport, RpcTransport::Http);
        assert_eq!(parsed.port, 0);
    }

    #[test]
    fn supervised_launch_is_explicit_and_conflicts_with_store_override() {
        let parsed = StandaloneRpcCli::try_parse_from([
            "starweaver-rpc",
            "--launch-envelope",
            "/tmp/launch.json",
            "stdio",
        ]);
        assert!(matches!(
            parsed,
            Ok(StandaloneRpcCli {
                launch_envelope: Some(path),
                transport: RpcTransport::Stdio,
                ..
            }) if path == std::path::Path::new("/tmp/launch.json")
        ));
        assert!(
            StandaloneRpcCli::try_parse_from([
                "starweaver-rpc",
                "--launch-envelope",
                "/tmp/launch.json",
                "--store",
                "/tmp/store.sqlite",
                "stdio",
            ])
            .is_err()
        );
    }
}
