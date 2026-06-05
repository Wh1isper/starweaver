//! Workspace binding and provider primitives.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ClawError, ClawResult, ClawSettings, WorkspaceBackend};

/// Read/write mode for a mounted workspace folder.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMountMode {
    /// Read and write access.
    #[default]
    Rw,
    /// Read-only access.
    Ro,
}

/// User-supplied workspace mount specification.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceMountSpec {
    /// Stable mount id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Host path to mount.
    pub host_path: PathBuf,
    /// Virtual path exposed to the agent.
    pub virtual_path: String,
    /// Mount access mode.
    #[serde(default)]
    pub mode: WorkspaceMountMode,
    /// Host path seen by a Docker service process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docker_host_path: Option<PathBuf>,
    /// Additional metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

/// User-supplied workspace binding request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceBindingSpec {
    /// Mounted folders.
    pub mounts: Vec<WorkspaceMountSpec>,
    /// Default mount id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_mount_id: Option<String>,
    /// Virtual current working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Binding metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

/// Normalized mount binding.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResolvedWorkspaceMount {
    /// Stable mount id.
    pub id: String,
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Host path to mount.
    pub host_path: PathBuf,
    /// Virtual path exposed to the agent.
    pub virtual_path: String,
    /// Mount access mode.
    pub mode: WorkspaceMountMode,
    /// Host path seen by Docker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docker_host_path: Option<PathBuf>,
    /// Additional metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

/// Fully resolved workspace binding.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResolvedWorkspaceBinding {
    /// Active backend.
    pub backend: WorkspaceBackend,
    /// Default host path.
    pub host_path: PathBuf,
    /// Default virtual path.
    pub virtual_path: String,
    /// Virtual current working directory.
    pub cwd: String,
    /// Readable virtual roots.
    pub readable_paths: Vec<String>,
    /// Writable virtual roots.
    pub writable_paths: Vec<String>,
    /// Mounts.
    pub mounts: Vec<ResolvedWorkspaceMount>,
    /// Stable fingerprint for sandbox reuse.
    pub fingerprint: String,
    /// Resolution time.
    pub resolved_at: DateTime<Utc>,
    /// Binding metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

/// Workspace runtime provider.
#[derive(Clone, Debug)]
pub struct WorkspaceProvider {
    settings: ClawSettings,
}

impl WorkspaceProvider {
    /// Build a provider from settings.
    #[must_use]
    pub const fn new(settings: ClawSettings) -> Self {
        Self { settings }
    }

    /// Resolve a workspace request, falling back to configured workspace dir.
    ///
    /// # Errors
    ///
    /// Returns validation errors for empty mounts, duplicated ids, invalid virtual
    /// paths, and out-of-mount `cwd` values.
    pub fn resolve(
        &self,
        request: Option<WorkspaceBindingSpec>,
    ) -> ClawResult<ResolvedWorkspaceBinding> {
        let spec = request.unwrap_or_else(|| WorkspaceBindingSpec {
            mounts: vec![WorkspaceMountSpec {
                id: Some("workspace".to_string()),
                name: Some("workspace".to_string()),
                host_path: self.settings.workspace_dir.clone(),
                virtual_path: "/workspace".to_string(),
                mode: WorkspaceMountMode::Rw,
                docker_host_path: self.settings.docker_host_workspace_dir.clone(),
                metadata: serde_json::Map::new(),
            }],
            default_mount_id: Some("workspace".to_string()),
            cwd: Some("/workspace".to_string()),
            metadata: serde_json::Map::new(),
        });
        normalize_binding(self.settings.workspace_backend, spec)
    }

    /// Return workspace runtime status payload.
    #[must_use]
    pub fn runtime_status(&self) -> WorkspaceRuntimeStatus {
        let sandbox_lifecycle = self.settings.workspace_backend == WorkspaceBackend::Docker;
        WorkspaceRuntimeStatus {
            backend: self.settings.workspace_backend,
            status: "ready".to_string(),
            execution_location: format!("{:?}", self.settings.workspace_backend)
                .to_ascii_lowercase(),
            workspace: serde_json::json!({
                "service_path": self.settings.workspace_dir,
                "docker_host_path": self.settings.docker_host_workspace_dir,
                "virtual_path": "/workspace",
                "exists": self.settings.workspace_dir.exists(),
                "writable": true,
            }),
            capabilities: serde_json::json!({
                "file_browse": true,
                "shell": true,
                "sandbox_prepare": sandbox_lifecycle,
                "sandbox_stop": sandbox_lifecycle,
            }),
            checks: vec![serde_json::json!({
                "id": "workspace",
                "status": "ready",
                "message": "workspace provider configured",
                "details": {},
            })],
            docker: (self.settings.workspace_backend == WorkspaceBackend::Docker).then(|| {
                serde_json::json!({
                    "daemon": { "status": "skipped" },
                    "image": {
                        "ref": self.settings.docker_image.clone().unwrap_or_default(),
                        "present": false,
                    },
                    "workspace_user": {},
                    "container_cache": { "enabled": false },
                })
            }),
            workspace_dir: self.settings.workspace_dir.clone(),
            docker_image: self.settings.docker_image.clone(),
            docker_host_workspace_dir: self.settings.docker_host_workspace_dir.clone(),
            sandbox_lifecycle,
            updated_at: Utc::now(),
        }
    }
}

/// Runtime workspace status.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceRuntimeStatus {
    /// Active backend.
    pub backend: WorkspaceBackend,
    /// Runtime status.
    pub status: String,
    /// Execution location label.
    pub execution_location: String,
    /// Console-compatible workspace path status.
    pub workspace: Value,
    /// Console-compatible capability flags.
    pub capabilities: Value,
    /// Runtime checks.
    pub checks: Vec<Value>,
    /// Docker runtime status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docker: Option<Value>,
    /// Default workspace path.
    pub workspace_dir: PathBuf,
    /// Docker image for sandbox execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docker_image: Option<String>,
    /// Host workspace mapping path for Docker service deployments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docker_host_workspace_dir: Option<PathBuf>,
    /// Whether explicit sandbox lifecycle endpoints can prepare/stop sandboxes.
    pub sandbox_lifecycle: bool,
    /// Update time.
    pub updated_at: DateTime<Utc>,
}

fn normalize_binding(
    backend: WorkspaceBackend,
    spec: WorkspaceBindingSpec,
) -> ClawResult<ResolvedWorkspaceBinding> {
    if spec.mounts.is_empty() {
        return Err(ClawError::InvalidRequest(
            "workspace must declare at least one mount".to_string(),
        ));
    }
    if spec.mounts.len() > 8 {
        return Err(ClawError::InvalidRequest(
            "workspace mounts exceed limit 8".to_string(),
        ));
    }

    let mut seen_ids = HashSet::new();
    let mut seen_virtual_paths = HashSet::new();
    let mut mounts = Vec::with_capacity(spec.mounts.len());
    for mount in spec.mounts {
        let virtual_path = normalize_virtual_path(&mount.virtual_path)?;
        let id = mount
            .id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| derive_mount_id(&virtual_path));
        if !seen_ids.insert(id.clone()) {
            return Err(ClawError::InvalidRequest(format!(
                "workspace mount id '{id}' is duplicated"
            )));
        }
        if !seen_virtual_paths.insert(virtual_path.clone()) {
            return Err(ClawError::InvalidRequest(format!(
                "workspace virtual path '{virtual_path}' is duplicated"
            )));
        }
        mounts.push(ResolvedWorkspaceMount {
            id,
            name: mount.name,
            host_path: mount.host_path,
            virtual_path,
            mode: mount.mode,
            docker_host_path: mount.docker_host_path,
            metadata: mount.metadata,
        });
    }

    let default_mount_id = spec
        .default_mount_id
        .or_else(|| (mounts.len() == 1).then(|| mounts[0].id.clone()))
        .ok_or_else(|| {
            ClawError::InvalidRequest(
                "workspace.default_mount_id is required when multiple mounts are declared"
                    .to_string(),
            )
        })?;
    let default_mount = mounts
        .iter()
        .find(|mount| mount.id == default_mount_id)
        .ok_or_else(|| {
            ClawError::InvalidRequest(format!(
                "workspace.default_mount_id '{default_mount_id}' does not match a declared mount"
            ))
        })?;
    let cwd = spec
        .cwd
        .as_deref()
        .map(normalize_virtual_path)
        .transpose()?
        .unwrap_or_else(|| default_mount.virtual_path.clone());
    if mounts
        .iter()
        .all(|mount| !virtual_path_contains(&mount.virtual_path, &cwd))
    {
        return Err(ClawError::InvalidRequest(
            "workspace.cwd must be within a declared virtual mount".to_string(),
        ));
    }

    let readable_paths = mounts
        .iter()
        .map(|mount| mount.virtual_path.clone())
        .collect::<Vec<_>>();
    let writable_paths = mounts
        .iter()
        .filter(|mount| mount.mode == WorkspaceMountMode::Rw)
        .map(|mount| mount.virtual_path.clone())
        .collect::<Vec<_>>();
    let fingerprint = compute_fingerprint(&mounts, &cwd, backend);
    Ok(ResolvedWorkspaceBinding {
        backend,
        host_path: default_mount.host_path.clone(),
        virtual_path: default_mount.virtual_path.clone(),
        cwd,
        readable_paths,
        writable_paths,
        mounts,
        fingerprint,
        resolved_at: Utc::now(),
        metadata: spec.metadata,
    })
}

fn normalize_virtual_path(value: &str) -> ClawResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || !trimmed.starts_with('/') {
        return Err(ClawError::InvalidRequest(
            "virtual path must be a non-empty absolute path".to_string(),
        ));
    }
    let mut parts = Vec::new();
    for part in trimmed.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                let _ = parts.pop();
            }
            value => parts.push(value),
        }
    }
    if parts.is_empty() {
        Ok("/".to_string())
    } else {
        Ok(format!("/{}", parts.join("/")))
    }
}

fn derive_mount_id(virtual_path: &str) -> String {
    let leaf = virtual_path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("workspace");
    let normalized = leaf
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches(['-', '.', '_'])
        .to_string();
    if normalized.is_empty() {
        "workspace".to_string()
    } else {
        normalized
    }
}

fn virtual_path_contains(root: &str, child: &str) -> bool {
    root == "/"
        || child == root
        || child
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn compute_fingerprint(
    mounts: &[ResolvedWorkspaceMount],
    cwd: &str,
    backend: WorkspaceBackend,
) -> String {
    let mut payload = format!("backend={backend:?};cwd={cwd};");
    for mount in mounts {
        let host = display_path(&mount.host_path);
        let docker = mount
            .docker_host_path
            .as_deref()
            .map_or_else(String::new, display_path);
        payload.push_str(&format!(
            "{}:{}:{:?}:{}:{};",
            mount.id, mount.virtual_path, mount.mode, host, docker
        ));
    }
    format!("workspace_{:016x}", fnv1a64(payload.as_bytes()))
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_single_mount_defaults() {
        let spec = WorkspaceBindingSpec {
            mounts: vec![WorkspaceMountSpec {
                id: None,
                name: None,
                host_path: PathBuf::from("/tmp/project"),
                virtual_path: "/workspace/project".to_string(),
                mode: WorkspaceMountMode::Rw,
                docker_host_path: None,
                metadata: serde_json::Map::new(),
            }],
            default_mount_id: None,
            cwd: None,
            metadata: serde_json::Map::new(),
        };
        let binding = normalize_binding(WorkspaceBackend::Local, spec).expect("workspace resolves");
        assert_eq!(binding.cwd, "/workspace/project");
        assert_eq!(binding.mounts[0].id, "project");
    }
}
