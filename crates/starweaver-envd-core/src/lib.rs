//! Runtime-neutral envd service contracts.

mod error;
mod request;
mod rpc;
mod service;
mod types;

pub use error::{EnvdError, EnvdErrorCode, EnvdResult};
pub use request::{
    CleanupIdleRequest, CommandRunRequest, CommandRunResult, EnvironmentContextRequest,
    EnvironmentContextResult, EnvironmentRequest, FileCopyRequest, FileCreateDirRequest,
    FileDeleteRequest, FileGlobMatch, FileGlobOptions, FileGlobRequest, FileGrepMatch,
    FileGrepOptions, FileGrepRequest, FileListOptions, FileListRequest, FileListResult,
    FileMoveRequest, FileReadRequest, FileReadResult, FileStat, FileStatRequest, FileWriteRequest,
    FileWriteResult, FileWriteTmpRequest, FileWriteTmpResult, InitializeEnvdRequest,
    InitializeEnvdResult, MutationResult, OpenEnvironmentRequest, ProcessInputRequest,
    ProcessKillRequest, ProcessListResult, ProcessSignalRequest, ProcessStartRequest,
    ProcessWaitRequest, ShellReviewContextRequest, ShellReviewContextResult,
};
pub use rpc::{
    EnvdRpcError, INVALID_PARAMS, INVALID_REQUEST, JsonRpcRequest, METHOD_NOT_FOUND, PARSE_ERROR,
    SERVER_ERROR, error_response, parse_json_rpc_text, success_response,
};
pub use service::EnvdService;
pub use types::{
    EffectRecord, EnvironmentCapabilities, EnvironmentCapability, EnvironmentDescriptor,
    EnvironmentStateSnapshot, EnvironmentStatus, FileReadMode, MountBackendDescriptor,
    MountDescriptor, MountMode, MountStatus, OperationRecord, ProcessSnapshot, ProcessStatus,
    ResourceRef,
};

/// Stable envd protocol family name.
pub const ENVD_PROTOCOL_NAME: &str = "starweaver.envd";

/// Supported breaking envd protocol generation.
pub const ENVD_PROTOCOL_MAJOR: u32 = 1;

/// Current envd protocol documentation and fixture revision.
pub const ENVD_PROTOCOL_REVISION: &str = "2026-07-11";

/// Implemented envd protocol features.
pub const ENVD_PROTOCOL_FEATURES: &[&str] =
    &["environment.lifecycle", "files", "commands", "processes"];

/// Return the current typed envd protocol identity.
#[must_use]
pub fn envd_protocol_identity() -> starweaver_core::ProtocolIdentity {
    starweaver_core::ProtocolIdentity::new(
        ENVD_PROTOCOL_NAME,
        ENVD_PROTOCOL_MAJOR,
        ENVD_PROTOCOL_REVISION,
    )
    .with_features(ENVD_PROTOCOL_FEATURES.iter().copied())
}

/// Validate an envd protocol identity against the supported family and major.
///
/// # Errors
///
/// Returns an invalid-request error for another protocol name or major.
pub fn validate_envd_protocol(protocol: &starweaver_core::ProtocolIdentity) -> EnvdResult<()> {
    protocol
        .validate(ENVD_PROTOCOL_NAME, ENVD_PROTOCOL_MAJOR)
        .map_err(|error| EnvdError::invalid_request(error.to_string()))
}

/// Validate an initialize request when it carries an explicit protocol identity.
///
/// Omission remains readable for pre-v1 local clients.
///
/// # Errors
///
/// Returns an invalid-request error for another protocol name or major.
pub fn validate_envd_initialize(request: &InitializeEnvdRequest) -> EnvdResult<()> {
    request
        .protocol
        .as_ref()
        .map_or(Ok(()), validate_envd_protocol)
}

/// Default environment id used by direct local mode.
pub const DEFAULT_ENVIRONMENT_ID: &str = "env_cli_default";
