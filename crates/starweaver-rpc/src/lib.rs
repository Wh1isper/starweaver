//! Standalone Starweaver JSON-RPC product.
//!
//! This crate owns RPC configuration, method dispatch, active-run coordination,
//! and transports. It intentionally does not depend on `starweaver-cli`.

mod agent_catalog;
mod auth;
mod config;
mod coordinator;
mod environment;
mod environment_manager;
mod error;
mod service;
mod state;
mod transport;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

pub use agent_catalog::{RpcAgentCatalog, RpcProfileSummary};
pub use auth::{RpcHttpAuthConfig, RpcHttpScope};
pub use config::{
    RpcConfig, RpcProfileConfig, RpcProviderConfig, RpcSessionSearchBackend, RpcSessionSearchConfig,
};
pub use coordinator::{RpcRunRequest, RpcRunStatus, RpcRuntimeCoordinator, RpcStartedRun};
pub use error::{RpcHostError, RpcHostResult};
pub use service::{RpcNotificationMode, RpcService};

/// Standalone RPC transport.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum RpcTransport {
    /// Newline-delimited JSON-RPC over stdin/stdout.
    #[default]
    Stdio,
    /// Authenticated unary JSON-RPC over loopback HTTP.
    Http,
}

/// Start the selected standalone RPC transport.
///
/// # Errors
///
/// Returns configuration, storage, bind, or transport failures.
pub fn run(
    config: &RpcConfig,
    transport: RpcTransport,
    host: &str,
    port: u16,
) -> RpcHostResult<()> {
    match transport {
        RpcTransport::Stdio => transport::run_stdio(config),
        RpcTransport::Http => transport::run_http(config, host, port),
    }
}
