//! Runtime-neutral envd service contracts.

mod error;
mod request;
mod rpc;
mod service;
mod types;

pub use error::{EnvdError, EnvdErrorCode, EnvdResult};
pub use request::{
    CommandRunRequest, CommandRunResult, EnvironmentContextRequest, EnvironmentContextResult,
    EnvironmentRequest, FileCopyRequest, FileCreateDirRequest, FileDeleteRequest, FileGlobMatch,
    FileGlobOptions, FileGlobRequest, FileGrepMatch, FileGrepOptions, FileGrepRequest,
    FileListOptions, FileListRequest, FileListResult, FileMoveRequest, FileReadRequest,
    FileReadResult, FileStat, FileStatRequest, FileWriteRequest, FileWriteResult,
    FileWriteTmpRequest, FileWriteTmpResult, InitializeEnvdRequest, InitializeEnvdResult,
    MutationResult, OpenEnvironmentRequest, ProcessInputRequest, ProcessKillRequest,
    ProcessListResult, ProcessSignalRequest, ProcessStartRequest, ProcessWaitRequest,
    ShellReviewContextRequest, ShellReviewContextResult,
};
pub use rpc::{
    error_response, parse_json_rpc_text, success_response, EnvdRpcError, JsonRpcRequest,
    INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR, SERVER_ERROR,
};
pub use service::EnvdService;
pub use types::{
    EffectRecord, EnvironmentCapabilities, EnvironmentCapability, EnvironmentDescriptor,
    EnvironmentStateSnapshot, EnvironmentStatus, FileReadMode, MountBackendDescriptor,
    MountDescriptor, MountMode, MountStatus, OperationRecord, ProcessSnapshot, ProcessStatus,
    ResourceRef,
};

/// Current envd protocol identity.
pub const ENVD_PROTOCOL: &str = "starweaver.envd";

/// Current envd protocol version.
pub const ENVD_PROTOCOL_VERSION: &str = "0.1.0";

/// Default environment id used by direct local mode.
pub const DEFAULT_ENVIRONMENT_ID: &str = "env_cli_default";
