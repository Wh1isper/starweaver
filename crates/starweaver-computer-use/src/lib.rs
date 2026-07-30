#![allow(missing_docs)]

//! Provider-neutral Computer Use service for the current active desktop.
//!
//! The crate owns one process-local typed service, canonical function-tool
//! catalog/router, deterministic fake, and target-gated native backends. It
//! intentionally has no runtime, model, CLI, RPC, environment, or storage
//! dependency.

mod backend;
mod error;
mod fake;
#[cfg(feature = "mcp-server")]
mod mcp_server;
#[cfg(feature = "mcp-server")]
mod mcp_transport;
mod platform;
mod router;
mod service;
mod types;

pub use backend::{
    BackendProbe, DynNativeDesktopBackend, NativeActionFailure, NativeActionReceipt,
    NativeDesktopBackend, NativeObservation,
};
pub use error::{ComputerUseError, ComputerUseErrorCode, ComputerUseFailure, RetryClassification};
pub use fake::{FakeComputerUseConfig, FakeComputerUseService, FakeNativeDesktopBackend};
#[cfg(feature = "mcp-server")]
pub use mcp_server::{ComputerUseMcpServer, McpResourceLimits};
#[cfg(feature = "mcp-server")]
pub use mcp_transport::{BoundedStdioTransport, MAX_MCP_INPUT_FRAME_BYTES, MAX_MCP_JSON_DEPTH};
pub use platform::{current_desktop_backend, current_desktop_service, current_desktop_tool_grant};
pub use router::*;
pub use service::{
    ComputerSession, ComputerUseService, DynComputerSession, DynComputerUseService,
    LocalComputerSession, LocalComputerUseService,
};
pub use types::*;

/// Stable typed service contract family.
pub const COMPUTER_USE_CONTRACT_NAME: &str = "starweaver.computer_use";

/// Stable first-party toolset identifier.
pub const COMPUTER_USE_TOOLSET_ID: &str = "starweaver.computer_use.tools.v1";
