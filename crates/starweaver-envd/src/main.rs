//! Standalone envd process.

use std::{path::PathBuf, process::ExitCode, sync::Arc};

use clap::{Parser, ValueEnum};
use starweaver_envd::{run_http, run_stdio, LocalEnvd};
use starweaver_environment::{
    EnvironmentPolicy, FilePolicy, LocalEnvironmentProvider, ShellPolicy,
};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = EnvdCli::parse();
    let provider = LocalEnvironmentProvider::new(cli.root).with_policy(EnvironmentPolicy {
        files: if cli.read_only {
            FilePolicy::read_only()
        } else {
            FilePolicy::read_write()
        },
        shell: if cli.no_shell {
            ShellPolicy::default()
        } else {
            ShellPolicy::allow_all()
        },
    });
    let service = Arc::new(LocalEnvd::new(Arc::new(provider)));
    let result = match cli.transport {
        EnvdTransport::Stdio => run_stdio(service).await,
        EnvdTransport::Http => run_http(service, &cli.host, cli.port).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

/// Standalone envd command.
#[derive(Clone, Debug, Parser)]
#[command(name = "starweaver-envd", version, about = "Starweaver envd process")]
struct EnvdCli {
    /// Local root directory for the default `LocalEnvd` instance.
    #[arg(long, default_value = ".")]
    root: PathBuf,
    /// Restrict file operations to read-only mode.
    #[arg(long)]
    read_only: bool,
    /// Disable shell execution.
    #[arg(long)]
    no_shell: bool,
    /// Runtime transport.
    #[arg(default_value = "stdio")]
    transport: EnvdTransport,
    /// HTTP bind host.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// HTTP bind port.
    #[arg(long, default_value_t = 8766)]
    port: u16,
}

/// Envd transport profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum EnvdTransport {
    /// Line-delimited JSON-RPC over stdio.
    Stdio,
    /// JSON-RPC over HTTP POST /rpc.
    Http,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn parses_envd_args() {
        let parsed = EnvdCli::try_parse_from([
            "starweaver-envd",
            "--root",
            "/tmp/workspace",
            "--read-only",
            "http",
            "--port",
            "0",
        ])
        .unwrap();
        assert_eq!(parsed.root, PathBuf::from("/tmp/workspace"));
        assert!(parsed.read_only);
        assert_eq!(parsed.transport, EnvdTransport::Http);
        assert_eq!(parsed.port, 0);
    }
}
