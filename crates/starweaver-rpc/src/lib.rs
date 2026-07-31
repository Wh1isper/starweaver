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
mod computer_use;
mod config;
mod config_authorization;
mod coordinator;
mod environment;
mod environment_contract;
mod environment_manager;
mod error;
mod execution_domain_lock;
mod host_cursor;
mod private_fs;
mod runtime_config;
mod service;
pub(crate) mod session_management;
mod session_tools;
mod transport;
mod workspace_registry;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

pub use agent_catalog::{RpcAgentCatalog, RpcProfileSummary};
pub use auth::{RpcHttpAuthConfig, RpcHttpScope};
pub use config::{
    ResolvedRpcEnvironmentResource, ResolvedRpcEnvironmentSource, RpcClientCapabilitiesConfig,
    RpcComputerUseConfig, RpcComputerUseDesktopScope, RpcConfig, RpcEnvironmentCatalogEntry,
    RpcEnvironmentConfig, RpcEnvironmentResourceConfig, RpcEnvironmentSourceConfig,
    RpcLaunchEvidence, RpcProfileConfig, RpcProviderConfig, RpcSessionSearchBackend,
    RpcSessionSearchConfig, RpcSubagentConfig,
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

/// Start standalone mode and explicitly register its resolved initial workspace before serving.
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
    run_product(config, transport, host, port, true)
}

/// Start a supervised domain host without any launch-time workspace registration.
///
/// # Errors
///
/// Returns configuration, storage, bind, or transport failures.
pub fn run_supervised(
    config: &RpcConfig,
    transport: RpcTransport,
    host: &str,
    port: u16,
) -> RpcHostResult<()> {
    run_product(config, transport, host, port, false)
}

fn run_product(
    config: &RpcConfig,
    transport: RpcTransport,
    host: &str,
    port: u16,
    register_standalone_workspace: bool,
) -> RpcHostResult<()> {
    match transport {
        RpcTransport::Stdio => transport::run_stdio(config, register_standalone_workspace),
        RpcTransport::Http => {
            transport::run_http(config, host, port, register_standalone_workspace)
        }
    }
}
