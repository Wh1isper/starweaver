//! Standalone JSON-RPC host process for Starweaver.

use std::process::ExitCode;

use clap::Parser;
use starweaver_cli::{run_rpc_server, RpcCommand, RpcTransport};

fn main() -> ExitCode {
    let cli = StandaloneRpcCli::parse();
    let command = RpcCommand {
        transport: cli.transport,
        host: cli.host,
        port: cli.port,
    };
    match run_rpc_server(&command, cli.store) {
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
    /// Override local store database path.
    #[arg(long)]
    store: Option<String>,
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
        assert_eq!(parsed.transport, RpcTransport::Http);
        assert_eq!(parsed.port, 0);
    }
}
