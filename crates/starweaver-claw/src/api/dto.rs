//! Shared HTTP DTOs for Claw service handlers.

use serde::Serialize;
use serde_json::Value;

/// Health response payload.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    /// Service status.
    pub status: &'static str,
    /// Storage backend kind.
    pub storage: &'static str,
    /// Runtime backend kind.
    pub runtime: &'static str,
    /// Database status.
    pub database: &'static str,
    /// Runtime state status.
    pub runtime_state: &'static str,
}

/// Claw info response payload.
#[derive(Debug, Serialize)]
pub struct ClawInfoResponse {
    /// Service name.
    pub name: String,
    /// Application name.
    pub app_name: String,
    /// Crate version.
    pub version: &'static str,
    /// Service version.
    pub service_version: String,
    /// Source commit.
    pub service_commit: Option<String>,
    /// Service revision.
    pub service_revision: String,
    /// Build label.
    pub service_build: Option<String>,
    /// Image label.
    pub service_image: Option<String>,
    /// Deployment environment.
    pub environment: String,
    /// Public base URL.
    pub public_base_url: String,
    /// Runtime instance identifier.
    pub instance_id: String,
    /// Auth policy label.
    pub auth: &'static str,
    /// Available surfaces.
    pub surfaces: Vec<&'static str>,
    /// Workspace provider backend.
    pub workspace_provider_backend: String,
    /// Storage model.
    pub storage_model: &'static str,
    /// Feature flags.
    pub features: Value,
    /// Capability flags.
    pub capabilities: Value,
}
