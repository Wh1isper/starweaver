//! RPC-owned environment provider resolution.
//!
//! This module consumes host-protocol attachment refs but shares only lower SDK and envd client
//! abstractions with other products.

use std::{path::Path, sync::Arc};

use starweaver_envd_client::EnvdRpcClient;
use starweaver_envd_core::DEFAULT_ENVIRONMENT_ID;
use starweaver_environment::{
    CompositeEnvironmentProvider, DynEnvironmentProvider, EnvdEnvironmentProvider,
    EnvironmentMount, EnvironmentMountMode, LocalEnvironmentProvider,
    SwitchableEnvironmentProvider, SwitchableEnvironmentTarget,
};
use starweaver_rpc_core::{
    EnvironmentAttachmentAccessMode, EnvironmentAttachmentRef, LOCAL_ENVIRONMENT_ATTACHMENT_ID,
    LOCAL_ENVIRONMENT_ATTACHMENT_KIND,
};

use crate::{RpcHostError, RpcHostResult};

/// Environment binding retained by one RPC-owned active run.
#[derive(Clone)]
pub struct ResolvedRpcEnvironment {
    /// Stable SDK provider handle installed in the runtime.
    pub provider: DynEnvironmentProvider,
    /// Mutable target retained for active environment operations.
    pub switchable: Arc<SwitchableEnvironmentProvider>,
    /// Effective attachments after default selection.
    pub attachments: Vec<EnvironmentAttachmentRef>,
}

/// Resolve run attachment refs into an SDK provider owned by the standalone RPC product.
pub fn resolve_rpc_environment(
    workspace_root: &Path,
    session_id: &str,
    attachments: &[EnvironmentAttachmentRef],
) -> RpcHostResult<ResolvedRpcEnvironment> {
    let target = resolve_rpc_environment_target(workspace_root, session_id, attachments)?;
    let switchable = Arc::new(SwitchableEnvironmentProvider::new(
        "rpc-active-environment",
        SwitchableEnvironmentTarget::new(
            target.provider.clone(),
            target.provider.clone().process_shell_provider(),
        ),
    ));
    let provider: DynEnvironmentProvider = switchable.clone();
    Ok(ResolvedRpcEnvironment {
        provider,
        switchable,
        attachments: target.attachments,
    })
}

/// Resolve attachments into a replacement target for an active RPC run.
pub fn resolve_rpc_environment_target(
    workspace_root: &Path,
    session_id: &str,
    attachments: &[EnvironmentAttachmentRef],
) -> RpcHostResult<ResolvedRpcEnvironmentTarget> {
    let mut effective = if attachments.is_empty() {
        vec![default_local_attachment()]
    } else {
        attachments.to_vec()
    };
    let default_id = default_attachment_id(&effective).map(ToString::to_string);
    let default_shell_id =
        default_shell_attachment_id(&effective, default_id.as_deref()).map(ToString::to_string);
    for attachment in &mut effective {
        attachment.is_default = default_id.as_deref() == Some(attachment.id.as_str());
        attachment.is_default_for_shell =
            default_shell_id.as_deref() == Some(attachment.id.as_str());
    }

    let mut mounts = Vec::with_capacity(effective.len());
    for attachment in &effective {
        let provider = resolve_attachment(workspace_root, session_id, attachment)?;
        let mount = EnvironmentMount::new(&attachment.id, provider)
            .map_err(|error| RpcHostError::Invalid(error.to_string()))?
            .with_mode(environment_mount_mode(attachment.resolved_mode()))
            .with_default(attachment.is_default)
            .with_default_for_shell(attachment.is_default_for_shell);
        mounts.push(mount);
    }
    let provider: DynEnvironmentProvider = Arc::new(
        CompositeEnvironmentProvider::with_id("rpc-composite", mounts)
            .map_err(|error| RpcHostError::Invalid(error.to_string()))?,
    );
    Ok(ResolvedRpcEnvironmentTarget {
        provider,
        attachments: effective,
    })
}

/// Non-switchable environment target prepared for an active binding update.
pub struct ResolvedRpcEnvironmentTarget {
    /// Replacement SDK provider.
    pub provider: DynEnvironmentProvider,
    /// Effective attachments after default selection.
    pub attachments: Vec<EnvironmentAttachmentRef>,
}

fn resolve_attachment(
    workspace_root: &Path,
    session_id: &str,
    attachment: &EnvironmentAttachmentRef,
) -> RpcHostResult<DynEnvironmentProvider> {
    if attachment.id == LOCAL_ENVIRONMENT_ATTACHMENT_ID
        && attachment.kind != LOCAL_ENVIRONMENT_ATTACHMENT_KIND
    {
        return Err(RpcHostError::Invalid(
            "reserved environment attachment id local requires kind local".to_string(),
        ));
    }
    match attachment.kind.as_str() {
        LOCAL_ENVIRONMENT_ATTACHMENT_KIND => Ok(Arc::new(
            LocalEnvironmentProvider::new(workspace_root.to_path_buf())
                .with_id(format!("rpc-local-{}", attachment.id))
                .with_tmp_namespace(session_id),
        )),
        "envd" => {
            let endpoint = attachment.requested_endpoint_ref().ok_or_else(|| {
                RpcHostError::Invalid("envd attachment requires endpointRef".to_string())
            })?;
            let client =
                EnvdRpcClient::from_local_endpoint_ref(endpoint, attachment.requested_auth_token())
                    .map_err(|error| RpcHostError::Invalid(error.to_string()))?;
            let environment_id = attachment
                .requested_environment_id()
                .unwrap_or(DEFAULT_ENVIRONMENT_ID)
                .to_string();
            Ok(Arc::new(
                EnvdEnvironmentProvider::new(Arc::new(client), environment_id)
                    .with_id(&attachment.id),
            ))
        }
        other => Err(RpcHostError::Invalid(format!(
            "unsupported environment attachment kind: {other}"
        ))),
    }
}

fn default_attachment_id(attachments: &[EnvironmentAttachmentRef]) -> Option<&str> {
    if attachments.len() == 1 {
        return Some(attachments[0].id.as_str());
    }
    attachments
        .iter()
        .find(|attachment| attachment.is_default)
        .map(|attachment| attachment.id.as_str())
}

fn default_shell_attachment_id<'a>(
    attachments: &'a [EnvironmentAttachmentRef],
    default_id: Option<&'a str>,
) -> Option<&'a str> {
    if let Some(explicit) = attachments
        .iter()
        .find(|attachment| attachment.is_default_for_shell)
        .map(|attachment| attachment.id.as_str())
    {
        return Some(explicit);
    }
    let default_id = default_id?;
    attachments
        .iter()
        .find(|attachment| attachment.id == default_id && attachment_supports_shell(attachment))
        .map(|attachment| attachment.id.as_str())
}

fn default_local_attachment() -> EnvironmentAttachmentRef {
    EnvironmentAttachmentRef {
        id: LOCAL_ENVIRONMENT_ATTACHMENT_ID.to_string(),
        kind: LOCAL_ENVIRONMENT_ATTACHMENT_KIND.to_string(),
        mode: Some(EnvironmentAttachmentAccessMode::ReadWrite),
        is_default: true,
        is_default_for_shell: true,
        attachment_lease_id: None,
        endpoint_ref: None,
        environment_id: None,
        auth_token: None,
        metadata: serde_json::Map::new(),
    }
}

const fn environment_mount_mode(mode: EnvironmentAttachmentAccessMode) -> EnvironmentMountMode {
    match mode {
        EnvironmentAttachmentAccessMode::ReadOnly => EnvironmentMountMode::ReadOnly,
        EnvironmentAttachmentAccessMode::ReadWrite => EnvironmentMountMode::ReadWrite,
    }
}

fn attachment_supports_shell(attachment: &EnvironmentAttachmentRef) -> bool {
    matches!(
        attachment.resolved_mode(),
        EnvironmentAttachmentAccessMode::ReadWrite
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn resolver_builds_rpc_owned_composite_without_cli_types() {
        let temp = tempfile::tempdir().unwrap();
        let resolved = resolve_rpc_environment(temp.path(), "session-test", &[]).unwrap();
        assert_eq!(resolved.attachments.len(), 1);
        assert_eq!(resolved.attachments[0].id, LOCAL_ENVIRONMENT_ATTACHMENT_ID);
        assert_eq!(resolved.provider.id(), "rpc-active-environment");
    }
}
