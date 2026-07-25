//! Generated typed public errors.

use super::types::*;
use serde::{Deserialize, Serialize};

/// JSON-RPC code for the generated `AlreadyExists` public error.
pub const ERROR_CODE_ALREADY_EXISTS: i64 = -32011;
/// JSON-RPC code for the generated `AuthorizationDenied` public error.
pub const ERROR_CODE_AUTHORIZATION_DENIED: i64 = -32017;
/// JSON-RPC code for the generated `ConfigurationFailed` public error.
pub const ERROR_CODE_CONFIGURATION_FAILED: i64 = -32050;
/// JSON-RPC code for the generated `CursorInvalid` public error.
pub const ERROR_CODE_CURSOR_INVALID: i64 = -32016;
/// JSON-RPC code for the generated `EnvironmentUnavailable` public error.
pub const ERROR_CODE_ENVIRONMENT_UNAVAILABLE: i64 = -32031;
/// JSON-RPC code for the generated `IdempotencyConflict` public error.
pub const ERROR_CODE_IDEMPOTENCY_CONFLICT: i64 = -32012;
/// JSON-RPC code for the generated `InternalError` public error.
pub const ERROR_CODE_INTERNAL_ERROR: i64 = -32000;
/// JSON-RPC code for the generated `InvalidParams` public error.
pub const ERROR_CODE_INVALID_PARAMS: i64 = -32602;
/// JSON-RPC code for the generated `InvalidRequest` public error.
pub const ERROR_CODE_INVALID_REQUEST: i64 = -32600;
/// JSON-RPC code for the generated `MethodNotFound` public error.
pub const ERROR_CODE_METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC code for the generated `NotFound` public error.
pub const ERROR_CODE_NOT_FOUND: i64 = -32010;
/// JSON-RPC code for the generated `NotInitialized` public error.
pub const ERROR_CODE_NOT_INITIALIZED: i64 = -32001;
/// JSON-RPC code for the generated `ParseError` public error.
pub const ERROR_CODE_PARSE_ERROR: i64 = -32700;
/// JSON-RPC code for the generated `RunConflict` public error.
pub const ERROR_CODE_RUN_CONFLICT: i64 = -32013;
/// JSON-RPC code for the generated `SessionSearchUnavailable` public error.
pub const ERROR_CODE_SESSION_SEARCH_UNAVAILABLE: i64 = -32032;
/// JSON-RPC code for the generated `StaleFence` public error.
pub const ERROR_CODE_STALE_FENCE: i64 = -32014;
/// JSON-RPC code for the generated `StorageUnavailable` public error.
pub const ERROR_CODE_STORAGE_UNAVAILABLE: i64 = -32015;
/// JSON-RPC code for the generated `UnsupportedFeature` public error.
pub const ERROR_CODE_UNSUPPORTED_FEATURE: i64 = -32002;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum HostErrorData {
    AlreadyExists(AlreadyExistsData),
    AuthorizationDenied(AuthorizationDeniedData),
    ConfigurationFailed(ConfigurationFailedData),
    CursorInvalid(CursorInvalidData),
    EnvironmentUnavailable(EnvironmentUnavailableData),
    IdempotencyConflict(IdempotencyConflictData),
    InternalError(InternalErrorData),
    InvalidParams(InvalidParamsData),
    InvalidRequest(InvalidRequestData),
    MethodNotFound(MethodNotFoundData),
    NotFound(NotFoundData),
    NotInitialized(NotInitializedData),
    ParseError(ParseErrorData),
    RunConflict(RunConflictData),
    SessionSearchUnavailable(SessionSearchUnavailableData),
    StaleFence(StaleFenceData),
    StorageUnavailable(StorageUnavailableData),
    UnsupportedFeature(UnsupportedFeatureData),
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostError {
    pub code: i64,
    pub message: String,
    pub data: HostErrorData,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApprovalDecideError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    AlreadyExists {
        message: String,
        data: AlreadyExistsData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    StaleFence {
        message: String,
        data: StaleFenceData,
    },
}
impl From<ApprovalDecideError> for HostError {
    fn from(error: ApprovalDecideError) -> Self {
        match error {
            ApprovalDecideError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            ApprovalDecideError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            ApprovalDecideError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            ApprovalDecideError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            ApprovalDecideError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            ApprovalDecideError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            ApprovalDecideError::AlreadyExists { message, data } => Self {
                code: -32011,
                message,
                data: HostErrorData::AlreadyExists(data),
            },
            ApprovalDecideError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            ApprovalDecideError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            ApprovalDecideError::StaleFence { message, data } => Self {
                code: -32014,
                message,
                data: HostErrorData::StaleFence(data),
            },
        }
    }
}
impl From<HostError> for ApprovalDecideError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::AlreadyExists(data)) => Self::AlreadyExists { message, data },
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::StaleFence(data)) => Self::StaleFence { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApprovalListError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    SessionSearchUnavailable {
        message: String,
        data: SessionSearchUnavailableData,
    },
}
impl From<ApprovalListError> for HostError {
    fn from(error: ApprovalListError) -> Self {
        match error {
            ApprovalListError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            ApprovalListError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            ApprovalListError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            ApprovalListError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            ApprovalListError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            ApprovalListError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            ApprovalListError::SessionSearchUnavailable { message, data } => Self {
                code: -32032,
                message,
                data: HostErrorData::SessionSearchUnavailable(data),
            },
        }
    }
}
impl From<HostError> for ApprovalListError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::SessionSearchUnavailable(data)) => {
                Self::SessionSearchUnavailable { message, data }
            }
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApprovalShowError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    SessionSearchUnavailable {
        message: String,
        data: SessionSearchUnavailableData,
    },
}
impl From<ApprovalShowError> for HostError {
    fn from(error: ApprovalShowError) -> Self {
        match error {
            ApprovalShowError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            ApprovalShowError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            ApprovalShowError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            ApprovalShowError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            ApprovalShowError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            ApprovalShowError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            ApprovalShowError::SessionSearchUnavailable { message, data } => Self {
                code: -32032,
                message,
                data: HostErrorData::SessionSearchUnavailable(data),
            },
        }
    }
}
impl From<HostError> for ApprovalShowError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::SessionSearchUnavailable(data)) => {
                Self::SessionSearchUnavailable { message, data }
            }
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CatalogListError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
}
impl From<CatalogListError> for HostError {
    fn from(error: CatalogListError) -> Self {
        match error {
            CatalogListError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            CatalogListError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            CatalogListError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            CatalogListError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            CatalogListError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            CatalogListError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
        }
    }
}
impl From<HostError> for CatalogListError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClarificationListError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    SessionSearchUnavailable {
        message: String,
        data: SessionSearchUnavailableData,
    },
}
impl From<ClarificationListError> for HostError {
    fn from(error: ClarificationListError) -> Self {
        match error {
            ClarificationListError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            ClarificationListError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            ClarificationListError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            ClarificationListError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            ClarificationListError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            ClarificationListError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            ClarificationListError::SessionSearchUnavailable { message, data } => Self {
                code: -32032,
                message,
                data: HostErrorData::SessionSearchUnavailable(data),
            },
        }
    }
}
impl From<HostError> for ClarificationListError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::SessionSearchUnavailable(data)) => {
                Self::SessionSearchUnavailable { message, data }
            }
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClarificationResolveError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    AlreadyExists {
        message: String,
        data: AlreadyExistsData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    StaleFence {
        message: String,
        data: StaleFenceData,
    },
}
impl From<ClarificationResolveError> for HostError {
    fn from(error: ClarificationResolveError) -> Self {
        match error {
            ClarificationResolveError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            ClarificationResolveError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            ClarificationResolveError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            ClarificationResolveError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            ClarificationResolveError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            ClarificationResolveError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            ClarificationResolveError::AlreadyExists { message, data } => Self {
                code: -32011,
                message,
                data: HostErrorData::AlreadyExists(data),
            },
            ClarificationResolveError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            ClarificationResolveError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            ClarificationResolveError::StaleFence { message, data } => Self {
                code: -32014,
                message,
                data: HostErrorData::StaleFence(data),
            },
        }
    }
}
impl From<HostError> for ClarificationResolveError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::AlreadyExists(data)) => Self::AlreadyExists { message, data },
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::StaleFence(data)) => Self::StaleFence { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClarificationShowError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    SessionSearchUnavailable {
        message: String,
        data: SessionSearchUnavailableData,
    },
}
impl From<ClarificationShowError> for HostError {
    fn from(error: ClarificationShowError) -> Self {
        match error {
            ClarificationShowError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            ClarificationShowError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            ClarificationShowError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            ClarificationShowError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            ClarificationShowError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            ClarificationShowError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            ClarificationShowError::SessionSearchUnavailable { message, data } => Self {
                code: -32032,
                message,
                data: HostErrorData::SessionSearchUnavailable(data),
            },
        }
    }
}
impl From<HostError> for ClarificationShowError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::SessionSearchUnavailable(data)) => {
                Self::SessionSearchUnavailable { message, data }
            }
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigActivateError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    InvalidParams {
        message: String,
        data: InvalidParamsData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    ConfigurationFailed {
        message: String,
        data: ConfigurationFailedData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
}
impl From<ConfigActivateError> for HostError {
    fn from(error: ConfigActivateError) -> Self {
        match error {
            ConfigActivateError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            ConfigActivateError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            ConfigActivateError::InvalidParams { message, data } => Self {
                code: -32602,
                message,
                data: HostErrorData::InvalidParams(data),
            },
            ConfigActivateError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            ConfigActivateError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            ConfigActivateError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            ConfigActivateError::ConfigurationFailed { message, data } => Self {
                code: -32050,
                message,
                data: HostErrorData::ConfigurationFailed(data),
            },
            ConfigActivateError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            ConfigActivateError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            ConfigActivateError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
        }
    }
}
impl From<HostError> for ConfigActivateError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::InvalidParams(data)) => Self::InvalidParams { message, data },
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::ConfigurationFailed(data)) => {
                Self::ConfigurationFailed { message, data }
            }
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigDiscardError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    InvalidParams {
        message: String,
        data: InvalidParamsData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    ConfigurationFailed {
        message: String,
        data: ConfigurationFailedData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
}
impl From<ConfigDiscardError> for HostError {
    fn from(error: ConfigDiscardError) -> Self {
        match error {
            ConfigDiscardError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            ConfigDiscardError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            ConfigDiscardError::InvalidParams { message, data } => Self {
                code: -32602,
                message,
                data: HostErrorData::InvalidParams(data),
            },
            ConfigDiscardError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            ConfigDiscardError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            ConfigDiscardError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            ConfigDiscardError::ConfigurationFailed { message, data } => Self {
                code: -32050,
                message,
                data: HostErrorData::ConfigurationFailed(data),
            },
            ConfigDiscardError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            ConfigDiscardError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            ConfigDiscardError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
        }
    }
}
impl From<HostError> for ConfigDiscardError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::InvalidParams(data)) => Self::InvalidParams { message, data },
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::ConfigurationFailed(data)) => {
                Self::ConfigurationFailed { message, data }
            }
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigGetError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
}
impl From<ConfigGetError> for HostError {
    fn from(error: ConfigGetError) -> Self {
        match error {
            ConfigGetError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            ConfigGetError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            ConfigGetError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            ConfigGetError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            ConfigGetError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
        }
    }
}
impl From<HostError> for ConfigGetError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigReloadError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    InvalidParams {
        message: String,
        data: InvalidParamsData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    ConfigurationFailed {
        message: String,
        data: ConfigurationFailedData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
}
impl From<ConfigReloadError> for HostError {
    fn from(error: ConfigReloadError) -> Self {
        match error {
            ConfigReloadError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            ConfigReloadError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            ConfigReloadError::InvalidParams { message, data } => Self {
                code: -32602,
                message,
                data: HostErrorData::InvalidParams(data),
            },
            ConfigReloadError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            ConfigReloadError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            ConfigReloadError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            ConfigReloadError::ConfigurationFailed { message, data } => Self {
                code: -32050,
                message,
                data: HostErrorData::ConfigurationFailed(data),
            },
            ConfigReloadError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            ConfigReloadError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            ConfigReloadError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
        }
    }
}
impl From<HostError> for ConfigReloadError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::InvalidParams(data)) => Self::InvalidParams { message, data },
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::ConfigurationFailed(data)) => {
                Self::ConfigurationFailed { message, data }
            }
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigUpdateError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    InvalidParams {
        message: String,
        data: InvalidParamsData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    ConfigurationFailed {
        message: String,
        data: ConfigurationFailedData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
}
impl From<ConfigUpdateError> for HostError {
    fn from(error: ConfigUpdateError) -> Self {
        match error {
            ConfigUpdateError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            ConfigUpdateError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            ConfigUpdateError::InvalidParams { message, data } => Self {
                code: -32602,
                message,
                data: HostErrorData::InvalidParams(data),
            },
            ConfigUpdateError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            ConfigUpdateError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            ConfigUpdateError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            ConfigUpdateError::ConfigurationFailed { message, data } => Self {
                code: -32050,
                message,
                data: HostErrorData::ConfigurationFailed(data),
            },
            ConfigUpdateError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            ConfigUpdateError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            ConfigUpdateError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
        }
    }
}
impl From<HostError> for ConfigUpdateError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::InvalidParams(data)) => Self::InvalidParams { message, data },
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::ConfigurationFailed(data)) => {
                Self::ConfigurationFailed { message, data }
            }
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigValidateError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    InvalidParams {
        message: String,
        data: InvalidParamsData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    ConfigurationFailed {
        message: String,
        data: ConfigurationFailedData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
}
impl From<ConfigValidateError> for HostError {
    fn from(error: ConfigValidateError) -> Self {
        match error {
            ConfigValidateError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            ConfigValidateError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            ConfigValidateError::InvalidParams { message, data } => Self {
                code: -32602,
                message,
                data: HostErrorData::InvalidParams(data),
            },
            ConfigValidateError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            ConfigValidateError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            ConfigValidateError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            ConfigValidateError::ConfigurationFailed { message, data } => Self {
                code: -32050,
                message,
                data: HostErrorData::ConfigurationFailed(data),
            },
            ConfigValidateError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            ConfigValidateError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            ConfigValidateError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
        }
    }
}
impl From<HostError> for ConfigValidateError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::InvalidParams(data)) => Self::InvalidParams { message, data },
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::ConfigurationFailed(data)) => {
                Self::ConfigurationFailed { message, data }
            }
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeferredCompleteError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    AlreadyExists {
        message: String,
        data: AlreadyExistsData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    StaleFence {
        message: String,
        data: StaleFenceData,
    },
}
impl From<DeferredCompleteError> for HostError {
    fn from(error: DeferredCompleteError) -> Self {
        match error {
            DeferredCompleteError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            DeferredCompleteError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            DeferredCompleteError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            DeferredCompleteError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            DeferredCompleteError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            DeferredCompleteError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            DeferredCompleteError::AlreadyExists { message, data } => Self {
                code: -32011,
                message,
                data: HostErrorData::AlreadyExists(data),
            },
            DeferredCompleteError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            DeferredCompleteError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            DeferredCompleteError::StaleFence { message, data } => Self {
                code: -32014,
                message,
                data: HostErrorData::StaleFence(data),
            },
        }
    }
}
impl From<HostError> for DeferredCompleteError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::AlreadyExists(data)) => Self::AlreadyExists { message, data },
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::StaleFence(data)) => Self::StaleFence { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeferredFailError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    AlreadyExists {
        message: String,
        data: AlreadyExistsData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    StaleFence {
        message: String,
        data: StaleFenceData,
    },
}
impl From<DeferredFailError> for HostError {
    fn from(error: DeferredFailError) -> Self {
        match error {
            DeferredFailError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            DeferredFailError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            DeferredFailError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            DeferredFailError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            DeferredFailError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            DeferredFailError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            DeferredFailError::AlreadyExists { message, data } => Self {
                code: -32011,
                message,
                data: HostErrorData::AlreadyExists(data),
            },
            DeferredFailError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            DeferredFailError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            DeferredFailError::StaleFence { message, data } => Self {
                code: -32014,
                message,
                data: HostErrorData::StaleFence(data),
            },
        }
    }
}
impl From<HostError> for DeferredFailError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::AlreadyExists(data)) => Self::AlreadyExists { message, data },
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::StaleFence(data)) => Self::StaleFence { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeferredListError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    SessionSearchUnavailable {
        message: String,
        data: SessionSearchUnavailableData,
    },
}
impl From<DeferredListError> for HostError {
    fn from(error: DeferredListError) -> Self {
        match error {
            DeferredListError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            DeferredListError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            DeferredListError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            DeferredListError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            DeferredListError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            DeferredListError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            DeferredListError::SessionSearchUnavailable { message, data } => Self {
                code: -32032,
                message,
                data: HostErrorData::SessionSearchUnavailable(data),
            },
        }
    }
}
impl From<HostError> for DeferredListError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::SessionSearchUnavailable(data)) => {
                Self::SessionSearchUnavailable { message, data }
            }
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeferredShowError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    SessionSearchUnavailable {
        message: String,
        data: SessionSearchUnavailableData,
    },
}
impl From<DeferredShowError> for HostError {
    fn from(error: DeferredShowError) -> Self {
        match error {
            DeferredShowError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            DeferredShowError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            DeferredShowError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            DeferredShowError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            DeferredShowError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            DeferredShowError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            DeferredShowError::SessionSearchUnavailable { message, data } => Self {
                code: -32032,
                message,
                data: HostErrorData::SessionSearchUnavailable(data),
            },
        }
    }
}
impl From<HostError> for DeferredShowError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::SessionSearchUnavailable(data)) => {
                Self::SessionSearchUnavailable { message, data }
            }
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticsGetError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
}
impl From<DiagnosticsGetError> for HostError {
    fn from(error: DiagnosticsGetError) -> Self {
        match error {
            DiagnosticsGetError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            DiagnosticsGetError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            DiagnosticsGetError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            DiagnosticsGetError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            DiagnosticsGetError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            DiagnosticsGetError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
        }
    }
}
impl From<HostError> for DiagnosticsGetError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnvironmentAttachError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    AlreadyExists {
        message: String,
        data: AlreadyExistsData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    StaleFence {
        message: String,
        data: StaleFenceData,
    },
    EnvironmentUnavailable {
        message: String,
        data: EnvironmentUnavailableData,
    },
}
impl From<EnvironmentAttachError> for HostError {
    fn from(error: EnvironmentAttachError) -> Self {
        match error {
            EnvironmentAttachError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            EnvironmentAttachError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            EnvironmentAttachError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            EnvironmentAttachError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            EnvironmentAttachError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            EnvironmentAttachError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            EnvironmentAttachError::AlreadyExists { message, data } => Self {
                code: -32011,
                message,
                data: HostErrorData::AlreadyExists(data),
            },
            EnvironmentAttachError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            EnvironmentAttachError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            EnvironmentAttachError::StaleFence { message, data } => Self {
                code: -32014,
                message,
                data: HostErrorData::StaleFence(data),
            },
            EnvironmentAttachError::EnvironmentUnavailable { message, data } => Self {
                code: -32031,
                message,
                data: HostErrorData::EnvironmentUnavailable(data),
            },
        }
    }
}
impl From<HostError> for EnvironmentAttachError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::AlreadyExists(data)) => Self::AlreadyExists { message, data },
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::StaleFence(data)) => Self::StaleFence { message, data },
            (message, HostErrorData::EnvironmentUnavailable(data)) => {
                Self::EnvironmentUnavailable { message, data }
            }
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnvironmentDetachError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    AlreadyExists {
        message: String,
        data: AlreadyExistsData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    StaleFence {
        message: String,
        data: StaleFenceData,
    },
    EnvironmentUnavailable {
        message: String,
        data: EnvironmentUnavailableData,
    },
}
impl From<EnvironmentDetachError> for HostError {
    fn from(error: EnvironmentDetachError) -> Self {
        match error {
            EnvironmentDetachError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            EnvironmentDetachError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            EnvironmentDetachError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            EnvironmentDetachError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            EnvironmentDetachError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            EnvironmentDetachError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            EnvironmentDetachError::AlreadyExists { message, data } => Self {
                code: -32011,
                message,
                data: HostErrorData::AlreadyExists(data),
            },
            EnvironmentDetachError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            EnvironmentDetachError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            EnvironmentDetachError::StaleFence { message, data } => Self {
                code: -32014,
                message,
                data: HostErrorData::StaleFence(data),
            },
            EnvironmentDetachError::EnvironmentUnavailable { message, data } => Self {
                code: -32031,
                message,
                data: HostErrorData::EnvironmentUnavailable(data),
            },
        }
    }
}
impl From<HostError> for EnvironmentDetachError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::AlreadyExists(data)) => Self::AlreadyExists { message, data },
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::StaleFence(data)) => Self::StaleFence { message, data },
            (message, HostErrorData::EnvironmentUnavailable(data)) => {
                Self::EnvironmentUnavailable { message, data }
            }
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnvironmentHealthError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    SessionSearchUnavailable {
        message: String,
        data: SessionSearchUnavailableData,
    },
    EnvironmentUnavailable {
        message: String,
        data: EnvironmentUnavailableData,
    },
}
impl From<EnvironmentHealthError> for HostError {
    fn from(error: EnvironmentHealthError) -> Self {
        match error {
            EnvironmentHealthError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            EnvironmentHealthError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            EnvironmentHealthError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            EnvironmentHealthError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            EnvironmentHealthError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            EnvironmentHealthError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            EnvironmentHealthError::SessionSearchUnavailable { message, data } => Self {
                code: -32032,
                message,
                data: HostErrorData::SessionSearchUnavailable(data),
            },
            EnvironmentHealthError::EnvironmentUnavailable { message, data } => Self {
                code: -32031,
                message,
                data: HostErrorData::EnvironmentUnavailable(data),
            },
        }
    }
}
impl From<HostError> for EnvironmentHealthError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::SessionSearchUnavailable(data)) => {
                Self::SessionSearchUnavailable { message, data }
            }
            (message, HostErrorData::EnvironmentUnavailable(data)) => {
                Self::EnvironmentUnavailable { message, data }
            }
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnvironmentListError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    SessionSearchUnavailable {
        message: String,
        data: SessionSearchUnavailableData,
    },
    EnvironmentUnavailable {
        message: String,
        data: EnvironmentUnavailableData,
    },
}
impl From<EnvironmentListError> for HostError {
    fn from(error: EnvironmentListError) -> Self {
        match error {
            EnvironmentListError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            EnvironmentListError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            EnvironmentListError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            EnvironmentListError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            EnvironmentListError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            EnvironmentListError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            EnvironmentListError::SessionSearchUnavailable { message, data } => Self {
                code: -32032,
                message,
                data: HostErrorData::SessionSearchUnavailable(data),
            },
            EnvironmentListError::EnvironmentUnavailable { message, data } => Self {
                code: -32031,
                message,
                data: HostErrorData::EnvironmentUnavailable(data),
            },
        }
    }
}
impl From<HostError> for EnvironmentListError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::SessionSearchUnavailable(data)) => {
                Self::SessionSearchUnavailable { message, data }
            }
            (message, HostErrorData::EnvironmentUnavailable(data)) => {
                Self::EnvironmentUnavailable { message, data }
            }
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnvironmentMountError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    AlreadyExists {
        message: String,
        data: AlreadyExistsData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    StaleFence {
        message: String,
        data: StaleFenceData,
    },
    EnvironmentUnavailable {
        message: String,
        data: EnvironmentUnavailableData,
    },
}
impl From<EnvironmentMountError> for HostError {
    fn from(error: EnvironmentMountError) -> Self {
        match error {
            EnvironmentMountError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            EnvironmentMountError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            EnvironmentMountError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            EnvironmentMountError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            EnvironmentMountError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            EnvironmentMountError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            EnvironmentMountError::AlreadyExists { message, data } => Self {
                code: -32011,
                message,
                data: HostErrorData::AlreadyExists(data),
            },
            EnvironmentMountError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            EnvironmentMountError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            EnvironmentMountError::StaleFence { message, data } => Self {
                code: -32014,
                message,
                data: HostErrorData::StaleFence(data),
            },
            EnvironmentMountError::EnvironmentUnavailable { message, data } => Self {
                code: -32031,
                message,
                data: HostErrorData::EnvironmentUnavailable(data),
            },
        }
    }
}
impl From<HostError> for EnvironmentMountError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::AlreadyExists(data)) => Self::AlreadyExists { message, data },
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::StaleFence(data)) => Self::StaleFence { message, data },
            (message, HostErrorData::EnvironmentUnavailable(data)) => {
                Self::EnvironmentUnavailable { message, data }
            }
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnvironmentMountsListError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    SessionSearchUnavailable {
        message: String,
        data: SessionSearchUnavailableData,
    },
    EnvironmentUnavailable {
        message: String,
        data: EnvironmentUnavailableData,
    },
}
impl From<EnvironmentMountsListError> for HostError {
    fn from(error: EnvironmentMountsListError) -> Self {
        match error {
            EnvironmentMountsListError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            EnvironmentMountsListError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            EnvironmentMountsListError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            EnvironmentMountsListError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            EnvironmentMountsListError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            EnvironmentMountsListError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            EnvironmentMountsListError::SessionSearchUnavailable { message, data } => Self {
                code: -32032,
                message,
                data: HostErrorData::SessionSearchUnavailable(data),
            },
            EnvironmentMountsListError::EnvironmentUnavailable { message, data } => Self {
                code: -32031,
                message,
                data: HostErrorData::EnvironmentUnavailable(data),
            },
        }
    }
}
impl From<HostError> for EnvironmentMountsListError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::SessionSearchUnavailable(data)) => {
                Self::SessionSearchUnavailable { message, data }
            }
            (message, HostErrorData::EnvironmentUnavailable(data)) => {
                Self::EnvironmentUnavailable { message, data }
            }
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnvironmentUnmountError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    AlreadyExists {
        message: String,
        data: AlreadyExistsData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    StaleFence {
        message: String,
        data: StaleFenceData,
    },
    EnvironmentUnavailable {
        message: String,
        data: EnvironmentUnavailableData,
    },
}
impl From<EnvironmentUnmountError> for HostError {
    fn from(error: EnvironmentUnmountError) -> Self {
        match error {
            EnvironmentUnmountError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            EnvironmentUnmountError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            EnvironmentUnmountError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            EnvironmentUnmountError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            EnvironmentUnmountError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            EnvironmentUnmountError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            EnvironmentUnmountError::AlreadyExists { message, data } => Self {
                code: -32011,
                message,
                data: HostErrorData::AlreadyExists(data),
            },
            EnvironmentUnmountError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            EnvironmentUnmountError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            EnvironmentUnmountError::StaleFence { message, data } => Self {
                code: -32014,
                message,
                data: HostErrorData::StaleFence(data),
            },
            EnvironmentUnmountError::EnvironmentUnavailable { message, data } => Self {
                code: -32031,
                message,
                data: HostErrorData::EnvironmentUnavailable(data),
            },
        }
    }
}
impl From<HostError> for EnvironmentUnmountError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::AlreadyExists(data)) => Self::AlreadyExists { message, data },
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::StaleFence(data)) => Self::StaleFence { message, data },
            (message, HostErrorData::EnvironmentUnavailable(data)) => {
                Self::EnvironmentUnavailable { message, data }
            }
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventsReplayError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    CursorInvalid {
        message: String,
        data: CursorInvalidData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    SessionSearchUnavailable {
        message: String,
        data: SessionSearchUnavailableData,
    },
}
impl From<EventsReplayError> for HostError {
    fn from(error: EventsReplayError) -> Self {
        match error {
            EventsReplayError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            EventsReplayError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            EventsReplayError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            EventsReplayError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            EventsReplayError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            EventsReplayError::CursorInvalid { message, data } => Self {
                code: -32016,
                message,
                data: HostErrorData::CursorInvalid(data),
            },
            EventsReplayError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            EventsReplayError::SessionSearchUnavailable { message, data } => Self {
                code: -32032,
                message,
                data: HostErrorData::SessionSearchUnavailable(data),
            },
        }
    }
}
impl From<HostError> for EventsReplayError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::CursorInvalid(data)) => Self::CursorInvalid { message, data },
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::SessionSearchUnavailable(data)) => {
                Self::SessionSearchUnavailable { message, data }
            }
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventsSubscribeError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    CursorInvalid {
        message: String,
        data: CursorInvalidData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    SessionSearchUnavailable {
        message: String,
        data: SessionSearchUnavailableData,
    },
}
impl From<EventsSubscribeError> for HostError {
    fn from(error: EventsSubscribeError) -> Self {
        match error {
            EventsSubscribeError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            EventsSubscribeError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            EventsSubscribeError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            EventsSubscribeError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            EventsSubscribeError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            EventsSubscribeError::CursorInvalid { message, data } => Self {
                code: -32016,
                message,
                data: HostErrorData::CursorInvalid(data),
            },
            EventsSubscribeError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            EventsSubscribeError::SessionSearchUnavailable { message, data } => Self {
                code: -32032,
                message,
                data: HostErrorData::SessionSearchUnavailable(data),
            },
        }
    }
}
impl From<HostError> for EventsSubscribeError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::CursorInvalid(data)) => Self::CursorInvalid { message, data },
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::SessionSearchUnavailable(data)) => {
                Self::SessionSearchUnavailable { message, data }
            }
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventsUnsubscribeError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    SessionSearchUnavailable {
        message: String,
        data: SessionSearchUnavailableData,
    },
}
impl From<EventsUnsubscribeError> for HostError {
    fn from(error: EventsUnsubscribeError) -> Self {
        match error {
            EventsUnsubscribeError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            EventsUnsubscribeError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            EventsUnsubscribeError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            EventsUnsubscribeError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            EventsUnsubscribeError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            EventsUnsubscribeError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            EventsUnsubscribeError::SessionSearchUnavailable { message, data } => Self {
                code: -32032,
                message,
                data: HostErrorData::SessionSearchUnavailable(data),
            },
        }
    }
}
impl From<HostError> for EventsUnsubscribeError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::SessionSearchUnavailable(data)) => {
                Self::SessionSearchUnavailable { message, data }
            }
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InitializeError {
    InvalidParams {
        message: String,
        data: InvalidParamsData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
}
impl From<InitializeError> for HostError {
    fn from(error: InitializeError) -> Self {
        match error {
            InitializeError::InvalidParams { message, data } => Self {
                code: -32602,
                message,
                data: HostErrorData::InvalidParams(data),
            },
            InitializeError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            InitializeError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
        }
    }
}
impl From<HostError> for InitializeError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::InvalidParams(data)) => Self::InvalidParams { message, data },
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelSelectError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    AlreadyExists {
        message: String,
        data: AlreadyExistsData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    StaleFence {
        message: String,
        data: StaleFenceData,
    },
}
impl From<ModelSelectError> for HostError {
    fn from(error: ModelSelectError) -> Self {
        match error {
            ModelSelectError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            ModelSelectError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            ModelSelectError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            ModelSelectError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            ModelSelectError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            ModelSelectError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            ModelSelectError::AlreadyExists { message, data } => Self {
                code: -32011,
                message,
                data: HostErrorData::AlreadyExists(data),
            },
            ModelSelectError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            ModelSelectError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            ModelSelectError::StaleFence { message, data } => Self {
                code: -32014,
                message,
                data: HostErrorData::StaleFence(data),
            },
        }
    }
}
impl From<HostError> for ModelSelectError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::AlreadyExists(data)) => Self::AlreadyExists { message, data },
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::StaleFence(data)) => Self::StaleFence { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelSelectionGetError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
}
impl From<ModelSelectionGetError> for HostError {
    fn from(error: ModelSelectionGetError) -> Self {
        match error {
            ModelSelectionGetError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            ModelSelectionGetError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            ModelSelectionGetError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            ModelSelectionGetError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            ModelSelectionGetError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            ModelSelectionGetError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
        }
    }
}
impl From<HostError> for ModelSelectionGetError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileGetError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
}
impl From<ProfileGetError> for HostError {
    fn from(error: ProfileGetError) -> Self {
        match error {
            ProfileGetError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            ProfileGetError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            ProfileGetError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            ProfileGetError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            ProfileGetError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            ProfileGetError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
        }
    }
}
impl From<HostError> for ProfileGetError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunInterruptError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    AlreadyExists {
        message: String,
        data: AlreadyExistsData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    StaleFence {
        message: String,
        data: StaleFenceData,
    },
}
impl From<RunInterruptError> for HostError {
    fn from(error: RunInterruptError) -> Self {
        match error {
            RunInterruptError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            RunInterruptError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            RunInterruptError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            RunInterruptError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            RunInterruptError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            RunInterruptError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            RunInterruptError::AlreadyExists { message, data } => Self {
                code: -32011,
                message,
                data: HostErrorData::AlreadyExists(data),
            },
            RunInterruptError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            RunInterruptError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            RunInterruptError::StaleFence { message, data } => Self {
                code: -32014,
                message,
                data: HostErrorData::StaleFence(data),
            },
        }
    }
}
impl From<HostError> for RunInterruptError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::AlreadyExists(data)) => Self::AlreadyExists { message, data },
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::StaleFence(data)) => Self::StaleFence { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunListError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    CursorInvalid {
        message: String,
        data: CursorInvalidData,
    },
}
impl From<RunListError> for HostError {
    fn from(error: RunListError) -> Self {
        match error {
            RunListError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            RunListError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            RunListError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            RunListError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            RunListError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            RunListError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            RunListError::CursorInvalid { message, data } => Self {
                code: -32016,
                message,
                data: HostErrorData::CursorInvalid(data),
            },
        }
    }
}
impl From<HostError> for RunListError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::CursorInvalid(data)) => Self::CursorInvalid { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunResumeError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    AlreadyExists {
        message: String,
        data: AlreadyExistsData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    StaleFence {
        message: String,
        data: StaleFenceData,
    },
}
impl From<RunResumeError> for HostError {
    fn from(error: RunResumeError) -> Self {
        match error {
            RunResumeError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            RunResumeError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            RunResumeError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            RunResumeError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            RunResumeError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            RunResumeError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            RunResumeError::AlreadyExists { message, data } => Self {
                code: -32011,
                message,
                data: HostErrorData::AlreadyExists(data),
            },
            RunResumeError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            RunResumeError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            RunResumeError::StaleFence { message, data } => Self {
                code: -32014,
                message,
                data: HostErrorData::StaleFence(data),
            },
        }
    }
}
impl From<HostError> for RunResumeError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::AlreadyExists(data)) => Self::AlreadyExists { message, data },
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::StaleFence(data)) => Self::StaleFence { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunStartError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    AlreadyExists {
        message: String,
        data: AlreadyExistsData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    StaleFence {
        message: String,
        data: StaleFenceData,
    },
}
impl From<RunStartError> for HostError {
    fn from(error: RunStartError) -> Self {
        match error {
            RunStartError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            RunStartError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            RunStartError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            RunStartError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            RunStartError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            RunStartError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            RunStartError::AlreadyExists { message, data } => Self {
                code: -32011,
                message,
                data: HostErrorData::AlreadyExists(data),
            },
            RunStartError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            RunStartError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            RunStartError::StaleFence { message, data } => Self {
                code: -32014,
                message,
                data: HostErrorData::StaleFence(data),
            },
        }
    }
}
impl From<HostError> for RunStartError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::AlreadyExists(data)) => Self::AlreadyExists { message, data },
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::StaleFence(data)) => Self::StaleFence { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunStatusError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    SessionSearchUnavailable {
        message: String,
        data: SessionSearchUnavailableData,
    },
}
impl From<RunStatusError> for HostError {
    fn from(error: RunStatusError) -> Self {
        match error {
            RunStatusError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            RunStatusError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            RunStatusError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            RunStatusError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            RunStatusError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            RunStatusError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            RunStatusError::SessionSearchUnavailable { message, data } => Self {
                code: -32032,
                message,
                data: HostErrorData::SessionSearchUnavailable(data),
            },
        }
    }
}
impl From<HostError> for RunStatusError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::SessionSearchUnavailable(data)) => {
                Self::SessionSearchUnavailable { message, data }
            }
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunSteerError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    AlreadyExists {
        message: String,
        data: AlreadyExistsData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    StaleFence {
        message: String,
        data: StaleFenceData,
    },
}
impl From<RunSteerError> for HostError {
    fn from(error: RunSteerError) -> Self {
        match error {
            RunSteerError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            RunSteerError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            RunSteerError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            RunSteerError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            RunSteerError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            RunSteerError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            RunSteerError::AlreadyExists { message, data } => Self {
                code: -32011,
                message,
                data: HostErrorData::AlreadyExists(data),
            },
            RunSteerError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            RunSteerError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            RunSteerError::StaleFence { message, data } => Self {
                code: -32014,
                message,
                data: HostErrorData::StaleFence(data),
            },
        }
    }
}
impl From<HostError> for RunSteerError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::AlreadyExists(data)) => Self::AlreadyExists { message, data },
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::StaleFence(data)) => Self::StaleFence { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionCreateError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    AlreadyExists {
        message: String,
        data: AlreadyExistsData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    StaleFence {
        message: String,
        data: StaleFenceData,
    },
}
impl From<SessionCreateError> for HostError {
    fn from(error: SessionCreateError) -> Self {
        match error {
            SessionCreateError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            SessionCreateError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            SessionCreateError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            SessionCreateError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            SessionCreateError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            SessionCreateError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            SessionCreateError::AlreadyExists { message, data } => Self {
                code: -32011,
                message,
                data: HostErrorData::AlreadyExists(data),
            },
            SessionCreateError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            SessionCreateError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            SessionCreateError::StaleFence { message, data } => Self {
                code: -32014,
                message,
                data: HostErrorData::StaleFence(data),
            },
        }
    }
}
impl From<HostError> for SessionCreateError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::AlreadyExists(data)) => Self::AlreadyExists { message, data },
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::StaleFence(data)) => Self::StaleFence { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionDeleteError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    AlreadyExists {
        message: String,
        data: AlreadyExistsData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    StaleFence {
        message: String,
        data: StaleFenceData,
    },
}
impl From<SessionDeleteError> for HostError {
    fn from(error: SessionDeleteError) -> Self {
        match error {
            SessionDeleteError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            SessionDeleteError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            SessionDeleteError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            SessionDeleteError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            SessionDeleteError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            SessionDeleteError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            SessionDeleteError::AlreadyExists { message, data } => Self {
                code: -32011,
                message,
                data: HostErrorData::AlreadyExists(data),
            },
            SessionDeleteError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            SessionDeleteError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            SessionDeleteError::StaleFence { message, data } => Self {
                code: -32014,
                message,
                data: HostErrorData::StaleFence(data),
            },
        }
    }
}
impl From<HostError> for SessionDeleteError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::AlreadyExists(data)) => Self::AlreadyExists { message, data },
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::StaleFence(data)) => Self::StaleFence { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionForkError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    AlreadyExists {
        message: String,
        data: AlreadyExistsData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    StaleFence {
        message: String,
        data: StaleFenceData,
    },
}
impl From<SessionForkError> for HostError {
    fn from(error: SessionForkError) -> Self {
        match error {
            SessionForkError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            SessionForkError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            SessionForkError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            SessionForkError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            SessionForkError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            SessionForkError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            SessionForkError::AlreadyExists { message, data } => Self {
                code: -32011,
                message,
                data: HostErrorData::AlreadyExists(data),
            },
            SessionForkError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            SessionForkError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            SessionForkError::StaleFence { message, data } => Self {
                code: -32014,
                message,
                data: HostErrorData::StaleFence(data),
            },
        }
    }
}
impl From<HostError> for SessionForkError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::AlreadyExists(data)) => Self::AlreadyExists { message, data },
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::StaleFence(data)) => Self::StaleFence { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionGetError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    SessionSearchUnavailable {
        message: String,
        data: SessionSearchUnavailableData,
    },
}
impl From<SessionGetError> for HostError {
    fn from(error: SessionGetError) -> Self {
        match error {
            SessionGetError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            SessionGetError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            SessionGetError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            SessionGetError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            SessionGetError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            SessionGetError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            SessionGetError::SessionSearchUnavailable { message, data } => Self {
                code: -32032,
                message,
                data: HostErrorData::SessionSearchUnavailable(data),
            },
        }
    }
}
impl From<HostError> for SessionGetError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::SessionSearchUnavailable(data)) => {
                Self::SessionSearchUnavailable { message, data }
            }
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionListError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
}
impl From<SessionListError> for HostError {
    fn from(error: SessionListError) -> Self {
        match error {
            SessionListError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            SessionListError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            SessionListError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            SessionListError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            SessionListError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            SessionListError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
        }
    }
}
impl From<HostError> for SessionListError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionSearchError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    SessionSearchUnavailable {
        message: String,
        data: SessionSearchUnavailableData,
    },
}
impl From<SessionSearchError> for HostError {
    fn from(error: SessionSearchError) -> Self {
        match error {
            SessionSearchError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            SessionSearchError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            SessionSearchError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            SessionSearchError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            SessionSearchError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            SessionSearchError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            SessionSearchError::SessionSearchUnavailable { message, data } => Self {
                code: -32032,
                message,
                data: HostErrorData::SessionSearchUnavailable(data),
            },
        }
    }
}
impl From<HostError> for SessionSearchError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::SessionSearchUnavailable(data)) => {
                Self::SessionSearchUnavailable { message, data }
            }
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShutdownError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
    AlreadyExists {
        message: String,
        data: AlreadyExistsData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    StaleFence {
        message: String,
        data: StaleFenceData,
    },
}
impl From<ShutdownError> for HostError {
    fn from(error: ShutdownError) -> Self {
        match error {
            ShutdownError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            ShutdownError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            ShutdownError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            ShutdownError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            ShutdownError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            ShutdownError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
            ShutdownError::AlreadyExists { message, data } => Self {
                code: -32011,
                message,
                data: HostErrorData::AlreadyExists(data),
            },
            ShutdownError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            ShutdownError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            ShutdownError::StaleFence { message, data } => Self {
                code: -32014,
                message,
                data: HostErrorData::StaleFence(data),
            },
        }
    }
}
impl From<HostError> for ShutdownError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (message, HostErrorData::AlreadyExists(data)) => Self::AlreadyExists { message, data },
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::StaleFence(data)) => Self::StaleFence { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceListError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
}
impl From<WorkspaceListError> for HostError {
    fn from(error: WorkspaceListError) -> Self {
        match error {
            WorkspaceListError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            WorkspaceListError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            WorkspaceListError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            WorkspaceListError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            WorkspaceListError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
        }
    }
}
impl From<HostError> for WorkspaceListError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceRegisterError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    InvalidParams {
        message: String,
        data: InvalidParamsData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    ConfigurationFailed {
        message: String,
        data: ConfigurationFailedData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
}
impl From<WorkspaceRegisterError> for HostError {
    fn from(error: WorkspaceRegisterError) -> Self {
        match error {
            WorkspaceRegisterError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            WorkspaceRegisterError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            WorkspaceRegisterError::InvalidParams { message, data } => Self {
                code: -32602,
                message,
                data: HostErrorData::InvalidParams(data),
            },
            WorkspaceRegisterError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            WorkspaceRegisterError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            WorkspaceRegisterError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            WorkspaceRegisterError::ConfigurationFailed { message, data } => Self {
                code: -32050,
                message,
                data: HostErrorData::ConfigurationFailed(data),
            },
            WorkspaceRegisterError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            WorkspaceRegisterError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            WorkspaceRegisterError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
        }
    }
}
impl From<HostError> for WorkspaceRegisterError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::InvalidParams(data)) => Self::InvalidParams { message, data },
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::ConfigurationFailed(data)) => {
                Self::ConfigurationFailed { message, data }
            }
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceRemoveError {
    NotInitialized {
        message: String,
        data: NotInitializedData,
    },
    UnsupportedFeature {
        message: String,
        data: UnsupportedFeatureData,
    },
    InvalidParams {
        message: String,
        data: InvalidParamsData,
    },
    NotFound {
        message: String,
        data: NotFoundData,
    },
    StorageUnavailable {
        message: String,
        data: StorageUnavailableData,
    },
    AuthorizationDenied {
        message: String,
        data: AuthorizationDeniedData,
    },
    ConfigurationFailed {
        message: String,
        data: ConfigurationFailedData,
    },
    IdempotencyConflict {
        message: String,
        data: IdempotencyConflictData,
    },
    RunConflict {
        message: String,
        data: RunConflictData,
    },
    InternalError {
        message: String,
        data: InternalErrorData,
    },
}
impl From<WorkspaceRemoveError> for HostError {
    fn from(error: WorkspaceRemoveError) -> Self {
        match error {
            WorkspaceRemoveError::NotInitialized { message, data } => Self {
                code: -32001,
                message,
                data: HostErrorData::NotInitialized(data),
            },
            WorkspaceRemoveError::UnsupportedFeature { message, data } => Self {
                code: -32002,
                message,
                data: HostErrorData::UnsupportedFeature(data),
            },
            WorkspaceRemoveError::InvalidParams { message, data } => Self {
                code: -32602,
                message,
                data: HostErrorData::InvalidParams(data),
            },
            WorkspaceRemoveError::NotFound { message, data } => Self {
                code: -32010,
                message,
                data: HostErrorData::NotFound(data),
            },
            WorkspaceRemoveError::StorageUnavailable { message, data } => Self {
                code: -32015,
                message,
                data: HostErrorData::StorageUnavailable(data),
            },
            WorkspaceRemoveError::AuthorizationDenied { message, data } => Self {
                code: -32017,
                message,
                data: HostErrorData::AuthorizationDenied(data),
            },
            WorkspaceRemoveError::ConfigurationFailed { message, data } => Self {
                code: -32050,
                message,
                data: HostErrorData::ConfigurationFailed(data),
            },
            WorkspaceRemoveError::IdempotencyConflict { message, data } => Self {
                code: -32012,
                message,
                data: HostErrorData::IdempotencyConflict(data),
            },
            WorkspaceRemoveError::RunConflict { message, data } => Self {
                code: -32013,
                message,
                data: HostErrorData::RunConflict(data),
            },
            WorkspaceRemoveError::InternalError { message, data } => Self {
                code: -32000,
                message,
                data: HostErrorData::InternalError(data),
            },
        }
    }
}
impl From<HostError> for WorkspaceRemoveError {
    fn from(error: HostError) -> Self {
        match (error.message, error.data) {
            (message, HostErrorData::NotInitialized(data)) => {
                Self::NotInitialized { message, data }
            }
            (message, HostErrorData::UnsupportedFeature(data)) => {
                Self::UnsupportedFeature { message, data }
            }
            (message, HostErrorData::InvalidParams(data)) => Self::InvalidParams { message, data },
            (message, HostErrorData::NotFound(data)) => Self::NotFound { message, data },
            (message, HostErrorData::StorageUnavailable(data)) => {
                Self::StorageUnavailable { message, data }
            }
            (message, HostErrorData::AuthorizationDenied(data)) => {
                Self::AuthorizationDenied { message, data }
            }
            (message, HostErrorData::ConfigurationFailed(data)) => {
                Self::ConfigurationFailed { message, data }
            }
            (message, HostErrorData::IdempotencyConflict(data)) => {
                Self::IdempotencyConflict { message, data }
            }
            (message, HostErrorData::RunConflict(data)) => Self::RunConflict { message, data },
            (message, HostErrorData::InternalError(data)) => Self::InternalError { message, data },
            (_, _) => Self::InternalError {
                message: "internal error".to_string(),
                data: InternalErrorData {
                    kind: InternalErrorDataKind::Value,
                    retryable: false,
                    reconciliation_required: true,
                    diagnostic_ref: None,
                    resource_kind: None,
                },
            },
        }
    }
}
