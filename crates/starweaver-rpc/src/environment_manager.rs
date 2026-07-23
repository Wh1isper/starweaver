use std::{collections::BTreeSet, time::Duration};

use starweaver_envd_client::EnvdRpcClient;
use starweaver_envd_core::{
    DEFAULT_ENVIRONMENT_ID, EnvdService, EnvironmentRequest, InitializeEnvdRequest,
    OpenEnvironmentRequest, envd_protocol_identity,
};
use tokio::time::timeout;

use crate::{
    environment_contract::{
        EnvironmentAttachmentRef, LOCAL_ENVIRONMENT_ATTACHMENT_ID,
        LOCAL_ENVIRONMENT_ATTACHMENT_KIND, is_valid_environment_attachment_id,
        normalize_environment_attachment_refs,
    },
    error::{ENVIRONMENT_UNAVAILABLE, INVALID_PARAMS, RpcError, UNSUPPORTED_FEATURE},
};

/// Maximum provider readiness probe duration used by transport drain budgeting.
pub const MAX_READINESS_TIMEOUT_MS: u64 = 5_000;
const READINESS_TIMEOUT: Duration = Duration::from_millis(MAX_READINESS_TIMEOUT_MS);

/// Stateless materializer for configured durable environment attachments.
///
/// Attachment identity, scope, idempotency, and lifecycle are owned by the durable storage
/// aggregate. This type only validates provider-private material and proves readiness before a run
/// receives the corresponding SDK provider.
#[derive(Clone, Copy, Default)]
pub struct EnvironmentAttachmentManager;

impl EnvironmentAttachmentManager {
    #[must_use]
    pub(super) const fn new() -> Self {
        Self
    }

    pub(super) async fn materialize_run_attachments(
        &self,
        refs: Vec<EnvironmentAttachmentRef>,
        _run_session_id: Option<&str>,
        _connection_id: Option<&str>,
    ) -> Result<Vec<EnvironmentAttachmentRef>, RpcError> {
        validate_attachment_set(&refs)?;
        let refs = normalize_environment_attachment_refs(&refs);
        for attachment in &refs {
            validate_attachment_source(attachment)?;
            prove_readiness(attachment).await?;
        }
        Ok(refs)
    }
}

fn validate_attachment_set(refs: &[EnvironmentAttachmentRef]) -> Result<(), RpcError> {
    let mut ids = BTreeSet::new();
    let default_count = refs
        .iter()
        .filter(|attachment| attachment.is_default)
        .count();
    let shell_default_count = refs
        .iter()
        .filter(|attachment| attachment.is_default_for_shell)
        .count();
    for attachment in refs {
        if !is_valid_environment_attachment_id(&attachment.id) {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!(
                    "invalid environment attachment id: {}; expected an ASCII slug",
                    attachment.id
                ),
            ));
        }
        if !ids.insert(attachment.id.as_str()) {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!("duplicate environment attachment id: {}", attachment.id),
            ));
        }
    }
    if refs.len() > 1 && default_count != 1 {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "multiple environment attachments require exactly one default attachment",
        ));
    }
    if shell_default_count > 1 {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "environment attachments allow at most one defaultForShell attachment",
        ));
    }
    Ok(())
}

fn validate_attachment_source(attachment: &EnvironmentAttachmentRef) -> Result<(), RpcError> {
    if attachment.id == LOCAL_ENVIRONMENT_ATTACHMENT_ID
        && attachment.kind != LOCAL_ENVIRONMENT_ATTACHMENT_KIND
    {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "reserved environment attachment id local requires kind local",
        ));
    }
    match attachment.kind.as_str() {
        LOCAL_ENVIRONMENT_ATTACHMENT_KIND => Ok(()),
        "envd" => {
            let endpoint = attachment.requested_endpoint_ref().ok_or_else(|| {
                RpcError::new(
                    INVALID_PARAMS,
                    "envd environment attachment requires endpointRef",
                )
            })?;
            starweaver_envd_client::validate_local_endpoint_ref(
                endpoint,
                attachment.requested_auth_token(),
            )
            .map_err(|error| RpcError::new(INVALID_PARAMS, error.to_string()))
        }
        other => Err(RpcError::new(
            UNSUPPORTED_FEATURE,
            format!("unsupported environment attachment kind: {other}"),
        )),
    }
}

async fn prove_readiness(attachment: &EnvironmentAttachmentRef) -> Result<(), RpcError> {
    if attachment.kind == LOCAL_ENVIRONMENT_ATTACHMENT_KIND {
        return Ok(());
    }
    let endpoint = attachment.requested_endpoint_ref().ok_or_else(|| {
        RpcError::new(
            INVALID_PARAMS,
            "envd environment attachment requires endpointRef",
        )
    })?;
    let client =
        EnvdRpcClient::from_local_endpoint_ref(endpoint, attachment.requested_auth_token())
            .map_err(|error| RpcError::new(INVALID_PARAMS, error.to_string()))?;
    let environment_id = attachment
        .requested_environment_id()
        .unwrap_or(DEFAULT_ENVIRONMENT_ID)
        .to_string();
    let probe = async {
        client
            .initialize(InitializeEnvdRequest {
                protocol: Some(envd_protocol_identity()),
                client_name: Some("starweaver-rpc".to_string()),
                metadata: starweaver_core::Metadata::default(),
            })
            .await?;
        client
            .open_environment(OpenEnvironmentRequest {
                environment_id: Some(environment_id.clone()),
                metadata: starweaver_core::Metadata::default(),
            })
            .await?;
        client
            .environment_state(EnvironmentRequest { environment_id })
            .await
    };
    timeout(READINESS_TIMEOUT, probe)
        .await
        .map_err(|_| {
            RpcError::new(
                ENVIRONMENT_UNAVAILABLE,
                "envd readiness probe timed out after 5000ms",
            )
        })?
        .map_err(|error| RpcError::new(ENVIRONMENT_UNAVAILABLE, error.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::environment_contract::EnvironmentAttachmentAccessMode;

    fn local(id: &str, is_default: bool) -> EnvironmentAttachmentRef {
        EnvironmentAttachmentRef {
            id: id.to_string(),
            kind: "local".to_string(),
            mode: None,
            is_default,
            is_default_for_shell: false,
            endpoint_ref: None,
            environment_id: None,
            auth_token: None,
            metadata: serde_json::Map::new(),
        }
    }

    #[tokio::test]
    async fn materialization_normalizes_configured_local_attachments() {
        let refs = EnvironmentAttachmentManager::new()
            .materialize_run_attachments(vec![local("workspace", false)], None, None)
            .await
            .unwrap();
        assert!(refs[0].is_default);
        assert!(refs[0].is_default_for_shell);
        assert_eq!(
            refs[0].resolved_mode(),
            EnvironmentAttachmentAccessMode::ReadWrite
        );
    }

    #[tokio::test]
    async fn materialization_rejects_duplicate_mount_ids() {
        let error = EnvironmentAttachmentManager::new()
            .materialize_run_attachments(
                vec![local("workspace", true), local("workspace", false)],
                None,
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(error.message.contains("duplicate"));
    }

    #[tokio::test]
    async fn materialization_rejects_unconfigured_provider_kinds() {
        let mut attachment = local("workspace", false);
        attachment.kind = "custom".to_string();
        let error = EnvironmentAttachmentManager::new()
            .materialize_run_attachments(vec![attachment], None, None)
            .await
            .unwrap_err();
        assert_eq!(error.code, UNSUPPORTED_FEATURE);
    }
}
