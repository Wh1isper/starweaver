//! `EnvD` shared state and descriptor types.

use serde::{Deserialize, Serialize};
use starweaver_core::Metadata;

/// Environment lifecycle status.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentStatus {
    /// Environment is open and ready to serve operations.
    Open,
    /// Environment has been closed.
    Closed,
}

/// Mount readiness status.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MountStatus {
    /// Mount can serve operations.
    Ready,
    /// Mount failed.
    Failed,
}

/// Mount access mode.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MountMode {
    /// Read-only access.
    ReadOnly,
    /// Read and write access.
    ReadWrite,
}

/// File read mode.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileReadMode {
    /// Decode bytes as UTF-8 text.
    Text,
    /// Return raw bytes.
    Bytes,
}

/// Background process status.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessStatus {
    /// Process is still running.
    #[default]
    Running,
    /// Process completed successfully or unsuccessfully.
    Completed,
    /// Process failed before a normal completion status was available.
    Failed,
    /// Process was killed.
    Killed,
}

/// One capability advertised by an envd environment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentCapability {
    /// File operations are supported.
    Files,
    /// Foreground command execution is supported.
    Shell,
    /// Background process operations are supported.
    Processes,
    /// Snapshot export is supported.
    Snapshots,
    /// Model-facing context summaries are supported.
    ContextSummary,
}

/// Capabilities advertised by an envd environment.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentCapabilities {
    /// Advertised capability list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<EnvironmentCapability>,
}

/// Open environment descriptor.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentDescriptor {
    /// Stable environment id within the envd service.
    pub environment_id: String,
    /// Environment implementation kind.
    pub kind: String,
    /// State store kind.
    pub store: String,
    /// Lifecycle status.
    pub status: EnvironmentStatus,
    /// Monotonic state version.
    pub state_version: u64,
    /// Policy revision.
    pub policy_revision: u64,
    /// Advertised capabilities.
    pub capabilities: EnvironmentCapabilities,
    /// Implementation metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Mount backend descriptor.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MountBackendDescriptor {
    /// Backend kind such as `memory`, `local`, or `provider`.
    pub kind: String,
    /// Backend metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Mount descriptor.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MountDescriptor {
    /// Stable mount id.
    pub mount_id: String,
    /// Environment-visible root path.
    pub root: String,
    /// Access mode.
    pub mode: MountMode,
    /// Backend descriptor.
    pub backend: MountBackendDescriptor,
    /// Mount generation.
    pub generation: u64,
    /// Mount status.
    pub status: MountStatus,
}

/// Stable resource reference.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceRef {
    /// Provider-specific resource id.
    pub id: String,
    /// Resource URI.
    pub uri: String,
    /// Resource metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Background process snapshot.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessSnapshot {
    /// Stable process id.
    pub process_id: String,
    /// Original command.
    pub command: String,
    /// Process status.
    pub status: ProcessStatus,
    /// Buffered stdout.
    pub stdout: String,
    /// Buffered stderr.
    pub stderr: String,
    /// Return code when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_code: Option<i32>,
    /// Process metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Operation record for envd mutations and commands.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperationRecord {
    /// Stable operation id.
    pub operation_id: String,
    /// Operation kind.
    pub kind: String,
    /// State version after the operation.
    pub state_version: u64,
    /// Operation metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Effect record produced by an envd operation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EffectRecord {
    /// Stable effect id.
    pub effect_id: String,
    /// Related operation id.
    pub operation_id: String,
    /// Effect kind.
    pub kind: String,
    /// Effect metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Full environment state snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentStateSnapshot {
    /// Environment descriptor.
    pub descriptor: EnvironmentDescriptor,
    /// Mounts available in the environment.
    pub mounts: Vec<MountDescriptor>,
    /// Resource references.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<ResourceRef>,
    /// Background process snapshots.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub processes: Vec<ProcessSnapshot>,
    /// Operation history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<OperationRecord>,
    /// Effect history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<EffectRecord>,
    /// Snapshot metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}
