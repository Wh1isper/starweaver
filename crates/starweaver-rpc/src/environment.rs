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
    LOCAL_ENVIRONMENT_ATTACHMENT_KIND, normalize_environment_attachment_refs,
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
    let effective = effective_rpc_environment_attachments(attachments);
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

/// Normalize the credential-free attachment identities that define an RPC run binding.
pub fn effective_rpc_environment_attachments(
    attachments: &[EnvironmentAttachmentRef],
) -> Vec<EnvironmentAttachmentRef> {
    let effective = if attachments.is_empty() {
        vec![default_local_attachment()]
    } else {
        attachments.to_vec()
    };
    normalize_environment_attachment_refs(&effective)
}

/// Project provider-private attachments into credential-free durable and host-visible evidence.
pub fn safe_rpc_environment_attachments(
    attachments: &[EnvironmentAttachmentRef],
) -> Vec<EnvironmentAttachmentRef> {
    attachments
        .iter()
        .cloned()
        .map(|mut attachment| {
            attachment.auth_token = None;
            attachment.endpoint_ref = attachment
                .requested_endpoint_ref()
                .and_then(starweaver_envd_client::redacted_endpoint_ref);
            // Attachment metadata is an extension map with no reviewed safe keys. Keep it only in
            // the process-private provider binding until a typed allowlist exists.
            attachment.metadata.clear();
            attachment
        })
        .collect()
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

    #[test]
    fn safe_attachment_projection_redacts_provider_private_values() {
        let source = EnvironmentAttachmentRef {
            id: "workspace".to_string(),
            kind: "envd".to_string(),
            mode: Some(EnvironmentAttachmentAccessMode::ReadWrite),
            is_default: true,
            is_default_for_shell: true,
            attachment_lease_id: Some("lease-safe-id".to_string()),
            endpoint_ref: Some("stdio:///private/envd?arg=--token&arg=private-value".to_string()),
            environment_id: Some("environment-safe-id".to_string()),
            auth_token: Some("private-bearer".to_string()),
            metadata: serde_json::Map::from_iter([(
                "private".to_string(),
                serde_json::json!("metadata-value"),
            )]),
        };

        let projected = safe_rpc_environment_attachments(&[source]);
        assert_eq!(
            projected[0].endpoint_ref.as_deref(),
            Some("stdio://<redacted>")
        );
        assert_eq!(
            projected[0].attachment_lease_id.as_deref(),
            Some("lease-safe-id")
        );
        assert_eq!(
            projected[0].environment_id.as_deref(),
            Some("environment-safe-id")
        );
        assert!(projected[0].auth_token.is_none());
        assert!(projected[0].metadata.is_empty());
    }
}
