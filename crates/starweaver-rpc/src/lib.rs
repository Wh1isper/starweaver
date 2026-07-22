#![allow(
    clippy::default_trait_access,
    clippy::manual_let_else,
    clippy::missing_const_for_fn,
    clippy::needless_pass_by_value,
    clippy::redundant_pub_crate,
    clippy::single_match_else,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::unnecessary_wraps,
    clippy::unused_async
)]

//! Standalone Starweaver JSON-RPC product.
//!
//! This crate owns RPC configuration, method dispatch, active-run coordination,
//! and transports. It intentionally does not depend on `starweaver-cli`.

mod agent_catalog;
mod auth;
mod config;
mod coordinator;
mod environment;
mod environment_contract;
mod environment_manager;
mod error;
mod host_cursor;
mod service;
pub(crate) mod session_management;
mod session_tools;
mod transport;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

pub use agent_catalog::{RpcAgentCatalog, RpcProfileSummary};
pub use auth::{RpcHttpAuthConfig, RpcHttpScope};
pub use config::{
    ResolvedRpcEnvironmentResource, ResolvedRpcEnvironmentSource, RpcClientCapabilitiesConfig,
    RpcConfig, RpcEnvironmentCatalogEntry, RpcEnvironmentConfig, RpcEnvironmentResourceConfig,
    RpcEnvironmentSourceConfig, RpcLaunchEvidence, RpcProfileConfig, RpcProviderConfig,
    RpcSessionSearchBackend, RpcSessionSearchConfig, RpcSubagentConfig,
};
pub use coordinator::{
    RpcHitlResumeRequest, RpcRunRequest, RpcRunStatus, RpcRuntimeCoordinator, RpcStartedRun,
};
pub use error::{RpcHostError, RpcHostResult};

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
