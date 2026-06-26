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
    let http_token = match required_http_token(&cli) {
        Ok(token) => token,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
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
        EnvdTransport::Http => {
            let Some(token) = http_token else {
                eprintln!("error: envd HTTP transport requires --token");
                return ExitCode::from(2);
            };
            run_http(service, &cli.host, cli.port, token).await
        }
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
    /// Bearer token required for HTTP access.
    #[arg(long)]
    token: Option<String>,
}

/// Envd transport profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum EnvdTransport {
    /// Line-delimited JSON-RPC over stdio.
    Stdio,
    /// JSON-RPC over HTTP POST /rpc.
    Http,
}

fn required_http_token(cli: &EnvdCli) -> Result<Option<String>, String> {
    if cli.transport != EnvdTransport::Http {
        return Ok(None);
    }
    let token = cli
        .token
        .as_deref()
        .ok_or_else(|| "envd HTTP transport requires --token".to_string())?;
    if token.trim().is_empty() {
        return Err("envd HTTP --token cannot be empty".to_string());
    }
    if token.bytes().any(|byte| matches!(byte, b'\r' | b'\n')) {
        return Err("envd HTTP --token cannot contain newlines".to_string());
    }
    Ok(Some(token.to_string()))
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
            "--token",
            "secret",
        ])
        .unwrap();
        assert_eq!(parsed.root, PathBuf::from("/tmp/workspace"));
        assert!(parsed.read_only);
        assert_eq!(parsed.transport, EnvdTransport::Http);
        assert_eq!(parsed.port, 0);
        assert_eq!(
            required_http_token(&parsed).unwrap(),
            Some("secret".to_string())
        );
    }

    #[test]
    fn rejects_http_without_token() {
        let parsed = EnvdCli::try_parse_from(["starweaver-envd", "http"]).unwrap();
        let error = required_http_token(&parsed).unwrap_err();
        assert!(error.contains("--token"));
    }
}
