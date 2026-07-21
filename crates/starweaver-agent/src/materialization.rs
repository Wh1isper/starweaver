//! Product-neutral resolved agent materialization evidence.
//!
//! Products own profile discovery and authorization. Durable evidence records allowlisted
//! identities and one-way digests rather than provider credentials, routing affinity, raw
//! extension payloads, or local root strings.

use std::{collections::BTreeSet, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{AgentSpec, AgentSpecError, AgentSpecRegistry};

/// Durable metadata key containing [`ResolvedAgentMaterialization`].
pub const AGENT_MATERIALIZATION_METADATA_KEY: &str = "starweaver.agent.materialization";
/// Durable metadata key containing [`ContinuationMaterialization`].
pub const AGENT_CONTINUATION_METADATA_KEY: &str = "starweaver.agent.continuation";
/// Current materialization evidence schema version.
pub const AGENT_MATERIALIZATION_VERSION: u32 = 2;
/// Shared first-party host policy identity for semantically equivalent CLI/RPC bundles.
pub const STARWEAVER_AGENT_POLICY_VERSION: &str = "starweaver-agent-policy-v1";

/// Safe, product-neutral evidence for one resolved agent runtime.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedAgentMaterialization {
    /// Evidence schema version.
    pub version: u32,
    /// SHA-256 digest of the allowlisted semantic `AgentSpec` projection.
    pub agent_spec_digest: String,
    /// Stable model profile or logical model id selected by the host.
    pub model_profile_id: String,
    /// Effective stable toolset ids in sorted order.
    pub toolset_ids: Vec<String>,
    /// Version of the host policy bundle applied during materialization.
    pub policy_version: String,
    /// Credential-free environment binding category, not a path, endpoint, or lease id.
    pub environment_binding_class: String,
    /// Domain-separated digest of host-resolved provider and runtime behavior.
    pub runtime_binding_digest: String,
    /// Domain-separated digest of the host workspace root identity.
    pub workspace_root_digest: String,
    /// SHA-256 fingerprint of the fields above.
    pub fingerprint: String,
}

/// The original v1 durable materialization shape.
///
/// This stays private so newly persisted evidence always uses v2. It is decoded only to verify
/// existing durable records before they are projected into the v2 in-memory representation.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyResolvedAgentMaterialization {
    version: u32,
    agent_spec_digest: String,
    model_profile_id: String,
    toolset_ids: Vec<String>,
    policy_version: String,
    environment_binding_class: String,
    fingerprint: String,
}

impl LegacyResolvedAgentMaterialization {
    fn fingerprint_is_valid(&self) -> bool {
        self.version == 1
            && self.fingerprint
                == calculate_legacy_fingerprint(
                    self.version,
                    &self.agent_spec_digest,
                    &self.model_profile_id,
                    &self.toolset_ids,
                    &self.policy_version,
                    &self.environment_binding_class,
                )
    }

    fn into_unbound_projection(self) -> ResolvedAgentMaterialization {
        ResolvedAgentMaterialization {
            version: self.version,
            agent_spec_digest: self.agent_spec_digest,
            model_profile_id: self.model_profile_id,
            toolset_ids: self.toolset_ids,
            policy_version: self.policy_version,
            environment_binding_class: self.environment_binding_class,
            runtime_binding_digest: "sha256:unbound".to_string(),
            workspace_root_digest: "sha256:unbound".to_string(),
            fingerprint: self.fingerprint,
        }
    }
}

impl AgentSpec {
    /// Resolve this spec into safe materialization evidence using host-provided policy and
    /// environment identities.
    ///
    /// Product configuration discovery and credential-backed model construction remain outside
    /// this contract; callers pass only the resulting non-secret identity classes.
    ///
    /// # Errors
    ///
    /// Returns an error when the spec has no model, references an unknown toolset, or cannot be
    /// canonically serialized.
    pub fn resolved_materialization(
        &self,
        registry: &AgentSpecRegistry,
        policy_version: impl Into<String>,
        environment_binding_class: impl Into<String>,
    ) -> Result<ResolvedAgentMaterialization, AgentSpecError> {
        let model_profile_id = self
            .model
            .as_ref()
            .or(self.preset.model.as_ref())
            .map(|model| model.model_id.clone())
            .ok_or_else(|| AgentSpecError::UnknownModel("<missing>".to_string()))?;
        let digest = safe_agent_spec_digest(self)
            .map_err(|error| AgentSpecError::Invalid(error.to_string()))?;
        let toolset_ids = registry.resolved_toolset_ids(self)?;
        Ok(ResolvedAgentMaterialization::new(
            digest,
            model_profile_id,
            toolset_ids,
            policy_version,
            environment_binding_class,
        ))
    }
}

impl ResolvedAgentMaterialization {
    /// Construct evidence and calculate its safe fingerprint.
    #[must_use]
    pub fn new(
        agent_spec_digest: impl Into<String>,
        model_profile_id: impl Into<String>,
        toolset_ids: impl IntoIterator<Item = String>,
        policy_version: impl Into<String>,
        environment_binding_class: impl Into<String>,
    ) -> Self {
        let mut toolset_ids = toolset_ids.into_iter().collect::<Vec<_>>();
        toolset_ids.sort();
        toolset_ids.dedup();
        let mut evidence = Self {
            version: AGENT_MATERIALIZATION_VERSION,
            agent_spec_digest: agent_spec_digest.into(),
            model_profile_id: model_profile_id.into(),
            toolset_ids,
            policy_version: policy_version.into(),
            environment_binding_class: environment_binding_class.into(),
            runtime_binding_digest: "sha256:unbound".to_string(),
            workspace_root_digest: "sha256:unbound".to_string(),
            fingerprint: String::new(),
        };
        evidence.fingerprint = evidence.calculate_fingerprint();
        evidence
    }

    /// Add one host-materialized toolset identity and refresh the fingerprint.
    #[must_use]
    pub fn with_additional_toolset_identity(mut self, identity: impl Into<String>) -> Self {
        self.toolset_ids.push(identity.into());
        self.toolset_ids.sort();
        self.toolset_ids.dedup();
        self.fingerprint = self.calculate_fingerprint();
        self
    }

    /// Bind credential-free host runtime and workspace identities before persisting evidence.
    #[must_use]
    pub fn with_host_bindings(
        mut self,
        runtime_binding_digest: impl Into<String>,
        workspace_root_digest: impl Into<String>,
    ) -> Self {
        self.runtime_binding_digest = runtime_binding_digest.into();
        self.workspace_root_digest = workspace_root_digest.into();
        self.fingerprint = self.calculate_fingerprint();
        self
    }

    /// Recalculate and verify the fingerprint carried by decoded durable evidence.
    #[must_use]
    pub fn fingerprint_is_valid(&self) -> bool {
        match self.version {
            AGENT_MATERIALIZATION_VERSION => self.fingerprint == self.calculate_fingerprint(),
            1 => {
                self.runtime_binding_digest == "sha256:unbound"
                    && self.workspace_root_digest == "sha256:unbound"
                    && self.fingerprint
                        == calculate_legacy_fingerprint(
                            self.version,
                            &self.agent_spec_digest,
                            &self.model_profile_id,
                            &self.toolset_ids,
                            &self.policy_version,
                            &self.environment_binding_class,
                        )
            }
            _ => false,
        }
    }

    const fn has_unbound_host_bindings(&self) -> bool {
        self.version == 1
    }

    /// Insert this evidence into durable run or context metadata.
    ///
    /// # Errors
    ///
    /// Returns a serialization error if the evidence cannot be represented as JSON.
    pub fn insert_into(&self, metadata: &mut Map<String, Value>) -> Result<(), serde_json::Error> {
        // A verified v1 projection must retain its original wire shape and fingerprint if a host
        // copies it forward. Emitting v2-only binding fields under `version: 1` would turn valid
        // historical evidence into malformed evidence on its next read.
        let value = if self.version == 1 {
            #[derive(Serialize)]
            #[serde(rename_all = "camelCase")]
            struct LegacyFingerprintInput<'a> {
                version: u32,
                agent_spec_digest: &'a str,
                model_profile_id: &'a str,
                toolset_ids: &'a [String],
                policy_version: &'a str,
                environment_binding_class: &'a str,
                fingerprint: &'a str,
            }
            serde_json::to_value(LegacyFingerprintInput {
                version: self.version,
                agent_spec_digest: &self.agent_spec_digest,
                model_profile_id: &self.model_profile_id,
                toolset_ids: &self.toolset_ids,
                policy_version: &self.policy_version,
                environment_binding_class: &self.environment_binding_class,
                fingerprint: &self.fingerprint,
            })?
        } else {
            serde_json::to_value(self)?
        };
        metadata.insert(AGENT_MATERIALIZATION_METADATA_KEY.to_string(), value);
        Ok(())
    }

    /// Decode and authenticate materialization evidence from durable metadata.
    ///
    /// Missing evidence returns `Ok(None)` for legacy runs. Invalid or tampered evidence fails
    /// closed instead of being used for continuation decisions.
    ///
    /// # Errors
    ///
    /// Returns an error when present evidence is malformed or its fingerprint does not verify.
    pub fn from_metadata(
        metadata: &Map<String, Value>,
    ) -> Result<Option<Self>, MaterializationEvidenceError> {
        let Some(value) = metadata.get(AGENT_MATERIALIZATION_METADATA_KEY) else {
            return Ok(None);
        };
        if value.get("version").and_then(Value::as_u64) == Some(1) {
            let legacy =
                serde_json::from_value::<LegacyResolvedAgentMaterialization>(value.clone())
                    .map_err(MaterializationEvidenceError::Malformed)?;
            if !legacy.fingerprint_is_valid() {
                return Err(MaterializationEvidenceError::InvalidFingerprint);
            }
            return Ok(Some(legacy.into_unbound_projection()));
        }

        let evidence = serde_json::from_value::<Self>(value.clone())
            .map_err(MaterializationEvidenceError::Malformed)?;
        if !evidence.fingerprint_is_valid() {
            return Err(MaterializationEvidenceError::InvalidFingerprint);
        }
        Ok(Some(evidence))
    }

    fn calculate_fingerprint(&self) -> String {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct FingerprintInput<'a> {
            version: u32,
            agent_spec_digest: &'a str,
            model_profile_id: &'a str,
            toolset_ids: &'a [String],
            policy_version: &'a str,
            environment_binding_class: &'a str,
            runtime_binding_digest: &'a str,
            workspace_root_digest: &'a str,
        }
        let input = FingerprintInput {
            version: self.version,
            agent_spec_digest: &self.agent_spec_digest,
            model_profile_id: &self.model_profile_id,
            toolset_ids: &self.toolset_ids,
            policy_version: &self.policy_version,
            environment_binding_class: &self.environment_binding_class,
            runtime_binding_digest: &self.runtime_binding_digest,
            workspace_root_digest: &self.workspace_root_digest,
        };
        let encoded = serde_json::to_vec(&input).unwrap_or_default();
        format!("sha256:{:x}", Sha256::digest(encoded))
    }
}

fn calculate_legacy_fingerprint(
    version: u32,
    agent_spec_digest: &str,
    model_profile_id: &str,
    toolset_ids: &[String],
    policy_version: &str,
    environment_binding_class: &str,
) -> String {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct LegacyFingerprintInput<'a> {
        version: u32,
        agent_spec_digest: &'a str,
        model_profile_id: &'a str,
        toolset_ids: &'a [String],
        policy_version: &'a str,
        environment_binding_class: &'a str,
    }

    let input = LegacyFingerprintInput {
        version,
        agent_spec_digest,
        model_profile_id,
        toolset_ids,
        policy_version,
        environment_binding_class,
    };
    let encoded = serde_json::to_vec(&input).unwrap_or_default();
    format!("sha256:{:x}", Sha256::digest(encoded))
}

/// Explicit semantics selected for a continuation across materialization boundaries.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationMaterializationMode {
    /// Require the exact source fingerprint.
    #[default]
    Preserve,
    /// Permit an `AgentSpec` revision while requiring model, tools, policy, and environment parity.
    Compatible,
    /// Deliberately accept all reported drift.
    Switch,
}

impl ContinuationMaterializationMode {
    /// Stable wire and CLI value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Preserve => "preserve",
            Self::Compatible => "compatible",
            Self::Switch => "switch",
        }
    }
}

/// One materialization field that differs between a source and continuation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaterializationDrift {
    /// Stable field name.
    pub field: String,
    /// Previous safe value. `None` means the source had no verifiable evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Value>,
    /// Requested safe value.
    pub target: Value,
}

/// Durable decision explaining continuation materialization and drift.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContinuationMaterialization {
    /// Explicit continuation mode.
    pub mode: ContinuationMaterializationMode,
    /// Source fingerprint when verified evidence exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_fingerprint: Option<String>,
    /// Target fingerprint.
    pub target_fingerprint: String,
    /// Ordered field-level drift safe to display to a user.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drift: Vec<MaterializationDrift>,
    /// Whether the selected mode admits this continuation.
    pub allowed: bool,
}

impl ContinuationMaterialization {
    /// Compare source and target evidence under an explicit mode.
    #[must_use]
    pub fn assess(
        source: Option<&ResolvedAgentMaterialization>,
        target: &ResolvedAgentMaterialization,
        mode: ContinuationMaterializationMode,
    ) -> Self {
        let mut drift = Vec::new();
        if let Some(source) = source {
            push_drift(
                &mut drift,
                "agentSpecDigest",
                &source.agent_spec_digest,
                &target.agent_spec_digest,
            );
            push_drift(
                &mut drift,
                "modelProfileId",
                &source.model_profile_id,
                &target.model_profile_id,
            );
            push_drift(
                &mut drift,
                "toolsetIds",
                &source.toolset_ids,
                &target.toolset_ids,
            );
            push_drift(
                &mut drift,
                "policyVersion",
                &source.policy_version,
                &target.policy_version,
            );
            push_drift(
                &mut drift,
                "environmentBindingClass",
                &source.environment_binding_class,
                &target.environment_binding_class,
            );
            if source.has_unbound_host_bindings() {
                // A verified v1 record predates the host-bound digests. Treat its unbound
                // projection as drift even if a caller supplied an unbound v2 target: v1 cannot
                // establish the binding parity required by preserve or compatible continuation.
                drift.push(MaterializationDrift {
                    field: "runtimeBindingDigest".to_string(),
                    source: Some(Value::String(source.runtime_binding_digest.clone())),
                    target: Value::String(target.runtime_binding_digest.clone()),
                });
                drift.push(MaterializationDrift {
                    field: "workspaceRootDigest".to_string(),
                    source: Some(Value::String(source.workspace_root_digest.clone())),
                    target: Value::String(target.workspace_root_digest.clone()),
                });
            } else {
                push_drift(
                    &mut drift,
                    "runtimeBindingDigest",
                    &source.runtime_binding_digest,
                    &target.runtime_binding_digest,
                );
                push_drift(
                    &mut drift,
                    "workspaceRootDigest",
                    &source.workspace_root_digest,
                    &target.workspace_root_digest,
                );
            }
        } else {
            drift.push(MaterializationDrift {
                field: "sourceEvidence".to_string(),
                source: None,
                target: Value::String("verified".to_string()),
            });
        }
        let allowed = match mode {
            ContinuationMaterializationMode::Preserve => source.is_some() && drift.is_empty(),
            ContinuationMaterializationMode::Compatible => {
                source.is_some() && drift.iter().all(|item| item.field == "agentSpecDigest")
            }
            ContinuationMaterializationMode::Switch => true,
        };
        Self {
            mode,
            source_fingerprint: source.map(|source| source.fingerprint.clone()),
            target_fingerprint: target.fingerprint.clone(),
            drift,
            allowed,
        }
    }

    /// Insert this decision into durable metadata.
    ///
    /// # Errors
    ///
    /// Returns a serialization error if the decision cannot be represented as JSON.
    pub fn insert_into(&self, metadata: &mut Map<String, Value>) -> Result<(), serde_json::Error> {
        metadata.insert(
            AGENT_CONTINUATION_METADATA_KEY.to_string(),
            serde_json::to_value(self)?,
        );
        Ok(())
    }

    /// Decode a persisted continuation decision from durable metadata.
    ///
    /// # Errors
    ///
    /// Returns an error when present evidence is malformed.
    pub fn from_metadata(
        metadata: &Map<String, Value>,
    ) -> Result<Option<Self>, MaterializationEvidenceError> {
        let Some(value) = metadata.get(AGENT_CONTINUATION_METADATA_KEY) else {
            return Ok(None);
        };
        serde_json::from_value(value.clone())
            .map(Some)
            .map_err(MaterializationEvidenceError::Malformed)
    }

    /// Verify this persisted decision against authenticated source and target evidence.
    ///
    /// # Errors
    ///
    /// Returns an error when the stored decision is denied or differs from a fresh assessment.
    pub fn validate(
        &self,
        source: Option<&ResolvedAgentMaterialization>,
        target: &ResolvedAgentMaterialization,
    ) -> Result<(), MaterializationEvidenceError> {
        let expected = Self::assess(source, target, self.mode);
        if !self.allowed || *self != expected {
            return Err(MaterializationEvidenceError::InconsistentContinuation);
        }
        Ok(())
    }

    /// Render a concise credential-free drift summary for a host UI or error.
    #[must_use]
    pub fn drift_summary(&self) -> String {
        if self.drift.is_empty() {
            return "none".to_string();
        }
        self.drift
            .iter()
            .map(|item| item.field.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Error decoding durable materialization evidence.
#[derive(Debug)]
pub enum MaterializationEvidenceError {
    /// Present evidence is malformed.
    Malformed(serde_json::Error),
    /// Present evidence fingerprint does not match its fields.
    InvalidFingerprint,
    /// Persisted continuation evidence is denied or inconsistent with its source and target.
    InconsistentContinuation,
}

impl fmt::Display for MaterializationEvidenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(error) => write!(
                formatter,
                "malformed agent materialization evidence: {error}"
            ),
            Self::InvalidFingerprint => {
                formatter.write_str("agent materialization fingerprint is invalid")
            }
            Self::InconsistentContinuation => {
                formatter.write_str("agent continuation materialization evidence is inconsistent")
            }
        }
    }
}

impl std::error::Error for MaterializationEvidenceError {}

/// Hash the versioned, allowlisted semantic projection of an agent spec.
///
/// Known provider credentials and routing affinity, raw extension payloads, arbitrary metadata,
/// and unknown wrapper parameters are excluded by construction. Skill and workspace root strings
/// are represented only by domain-separated root-set digests. Those digests avoid persisting raw
/// paths, but are not a confidentiality boundary for paths an attacker can guess.
///
/// # Errors
///
/// Returns a serialization error if the semantic projection cannot be encoded.
pub fn safe_agent_spec_digest(spec: &AgentSpec) -> Result<String, serde_json::Error> {
    let canonical = canonical_value(agent_spec_semantic_projection(spec));
    Ok(format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&canonical)?)
    ))
}

fn agent_spec_semantic_projection(spec: &AgentSpec) -> Value {
    let mut capabilities = spec
        .capabilities
        .iter()
        .map(|capability| {
            json!({
                "id": capability.id,
                "ordering": capability.ordering,
                "onDemand": capability.on_demand,
            })
        })
        .collect::<Vec<_>>();
    capabilities.sort_by_key(Value::to_string);
    let mut host_policies = spec
        .host_policies
        .iter()
        .map(|policy| {
            json!({
                "kind": policy.kind,
                "trust": policy.trust,
                "sanitizers": sorted_strings(&policy.sanitizers),
            })
        })
        .collect::<Vec<_>>();
    host_policies.sort_by_key(Value::to_string);
    json!({
        "version": 2,
        "name": spec.name,
        "description": spec.description,
        "templates": spec.templates,
        "instructions": spec.instructions,
        "model": spec.model.as_ref().map(safe_model_preset),
        "preset": {
            "model": spec.preset.model.as_ref().map(safe_model_preset),
            "runtime": spec.preset.runtime,
            "usageLimits": spec.preset.usage_limits,
            "approvalPreset": spec.preset.approval_preset,
            "approval": spec.preset.approval,
            "retryPreset": spec.preset.retry_preset,
            "retry": spec.preset.retry,
            "streamingPreset": spec.preset.streaming_preset,
            "streaming": spec.preset.streaming,
            "observabilityPreset": spec.preset.observability_preset,
            "observability": spec.preset.observability,
            "environmentPreset": spec.preset.environment_preset,
            "environment": spec.preset.environment,
            "durabilityPreset": spec.preset.durability_preset,
            "durability": spec.preset.durability,
        },
        "output": spec.output,
        "skills": spec.skills.as_ref().map(|skills| json!({
            "enabled": skills.enabled,
            "rootsDigest": root_set_digest("skills", &skills.roots),
            "skillsDirName": skills.skills_dir_name,
            "extraDirNames": sorted_strings(&skills.extra_dir_names),
            "hotReload": skills.hot_reload,
            "preScanHook": skills.pre_scan_hook,
        })),
        "capabilities": capabilities,
        "capabilityRefs": sorted_strings(&spec.capability_refs),
        "toolsetWrappers": spec.toolset_wrappers.iter().map(safe_wrapper).collect::<Vec<_>>(),
        "hostPolicies": host_policies,
        "workspace": spec.workspace.as_ref().map(|workspace| json!({
            "provider": workspace.provider,
            "rootsDigest": root_set_digest("workspace", &workspace.roots),
            "shell": workspace.shell,
            "sandbox": workspace.sandbox,
        })),
        "hostAdapters": sorted_strings(&spec.host_adapters),
        "mcpServers": sorted_strings(&spec.mcp_servers),
        "allToolsets": spec.all_toolsets,
        "toolsets": sorted_strings(&spec.toolsets),
        "allSubagents": spec.all_subagents,
        "subagents": sorted_strings(&spec.subagents),
    })
}

fn safe_model_preset(model: &crate::ModelPreset) -> Value {
    json!({
        "modelId": model.model_id,
        "settingsPreset": model.settings_preset,
        "configPreset": model.config_preset,
        "settings": model.settings.as_ref().map(safe_model_settings),
    })
}

fn safe_model_settings(settings: &starweaver_model::ModelSettings) -> Value {
    // Provider replay IDs are server-side routing/state affinity and remain excluded. The typed
    // replay booleans are credential-free request behavior and therefore participate in preserve.
    json!({
        "maxTokens": settings.max_tokens,
        "temperature": settings.temperature,
        "topP": settings.top_p,
        "topK": settings.top_k,
        "timeoutMs": settings.timeout_ms,
        "parallelToolCalls": settings.parallel_tool_calls,
        "toolChoice": settings.tool_choice,
        "seed": settings.seed,
        "stopSequences": settings.stop_sequences,
        "presencePenalty": settings.presence_penalty,
        "frequencyPenalty": settings.frequency_penalty,
        "logitBias": settings.logit_bias,
        "thinking": settings.thinking,
        "serviceTier": settings.service_tier,
        "providerSettings": safe_provider_settings(&settings.provider_settings),
        "providerReplay": settings.provider_replay.as_ref().and_then(|replay| {
            (replay.send_item_ids.is_some() || replay.include_encrypted_reasoning.is_some())
                .then(|| json!({
                    "sendItemIds": replay.send_item_ids,
                    "includeEncryptedReasoning": replay.include_encrypted_reasoning,
                }))
        }),
    })
}

// Keep this projection as a field-by-field allowlist. In particular, do not serialize a provider
// settings struct wholesale: several typed containers also carry user IDs, routing affinity,
// arbitrary metadata, headers, or raw request payloads.
fn safe_provider_settings(settings: &starweaver_model::ProviderSettings) -> Value {
    json!({
        "openaiChat": settings.openai_chat.as_ref().and_then(|settings| {
            (settings.store.is_some()
                || settings.logprobs.is_some()
                || settings.top_logprobs.is_some()
                || settings.prompt_cache_retention.is_some()
                || settings.prompt_cache_options.is_some())
            .then(|| json!({
                "store": settings.store,
                "logprobs": settings.logprobs,
                "topLogprobs": settings.top_logprobs,
                "promptCacheRetention": settings.prompt_cache_retention,
                "promptCacheOptions": settings.prompt_cache_options,
            }))
        }),
        "openaiResponses": settings.openai_responses.as_ref().and_then(|settings| {
            (settings.store.is_some()
                || settings.truncation.is_some()
                || settings.text_verbosity.is_some()
                || !settings.include.is_empty()
                || settings.prompt_cache_retention.is_some()
                || settings.prompt_cache_options.is_some()
                || settings.stream_transport.is_some())
            .then(|| json!({
                "store": settings.store,
                "truncation": settings.truncation,
                "textVerbosity": settings.text_verbosity,
                "include": sorted_strings(&settings.include),
                "promptCacheRetention": settings.prompt_cache_retention,
                "promptCacheOptions": settings.prompt_cache_options,
                "streamTransport": settings.stream_transport,
            }))
        }),
        "anthropic": settings.anthropic.as_ref().and_then(|settings| {
            (!settings.betas.is_empty() || settings.service_tier.is_some()).then(|| json!({
                "betas": sorted_strings(&settings.betas),
                "serviceTier": settings.service_tier,
            }))
        }),
        "google": settings.google.as_ref().and_then(|settings| {
            (settings.response_logprobs.is_some()
                || settings.logprobs.is_some()
                || settings.service_tier.is_some()
                || settings.cloud_service_tier.is_some())
            .then(|| json!({
                "responseLogprobs": settings.response_logprobs,
                "logprobs": settings.logprobs,
                "serviceTier": settings.service_tier,
                "cloudServiceTier": settings.cloud_service_tier,
            }))
        }),
        "bedrock": settings.bedrock.as_ref().and_then(|settings| {
            (!settings.additional_model_response_field_paths.is_empty()).then(|| json!({
                "additionalModelResponseFieldPaths":
                    sorted_strings(&settings.additional_model_response_field_paths),
            }))
        }),
    })
}

fn root_set_digest(scope: &str, roots: &[String]) -> String {
    const DOMAIN: &[u8] = b"starweaver.agent.materialization.root-set/v1";

    let roots = roots.iter().map(String::as_bytes).collect::<BTreeSet<_>>();
    let mut hasher = Sha256::new();
    update_len_prefixed(&mut hasher, DOMAIN);
    update_len_prefixed(&mut hasher, scope.as_bytes());
    update_len_prefixed(&mut hasher, roots.len().to_string().as_bytes());
    for root in roots {
        update_len_prefixed(&mut hasher, root);
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn update_len_prefixed(hasher: &mut Sha256, value: &[u8]) {
    hasher.update(value.len().to_string().as_bytes());
    hasher.update(b":");
    hasher.update(value);
}

fn safe_wrapper(wrapper: &crate::ToolsetWrapperSpec) -> Value {
    let params = match wrapper.kind.as_str() {
        "approval_required" => json!({
            "tools": normalized_string_set(
                wrapper.params.get("tools")
                    .or_else(|| wrapper.params.get("tool"))
                    .or_else(|| wrapper.params.get("approval_required_tools")),
                true,
            ),
        }),
        "deferred" | "deferred_call" | "deferred_tools" => json!({
            "tools": normalized_string_set(
                wrapper.params.get("tools")
                    .or_else(|| wrapper.params.get("tool"))
                    .or_else(|| wrapper.params.get("deferred_tools")),
                true,
            ),
        }),
        "filtered" => json!({
            "includeTools": normalized_string_set(
                wrapper.params.get("include_tools").or_else(|| wrapper.params.get("tools")),
                false,
            ),
            "excludeTools": normalized_string_set(wrapper.params.get("exclude_tools"), false),
        }),
        "renamed" => json!({
            "mappings": wrapper.params.get("mappings").cloned().unwrap_or(Value::Null),
        }),
        "dynamic" | "tool_proxy" | "dynamic_tool_proxy" => json!({
            "prefix": wrapper.params.get("prefix").cloned().unwrap_or(Value::Null),
            "maxResults": wrapper.params.get("max_results").cloned().unwrap_or(Value::Null),
        }),
        // Dynamic-search wrappers have no semantic params. Unknown/custom wrapper params are
        // intentionally excluded; hosts must roll `policy_version` when their behavior changes.
        _ => Value::Object(Map::new()),
    };
    json!({
        "kind": wrapper.kind,
        "toolset": wrapper.toolset,
        "params": params,
    })
}

fn normalized_string_set(value: Option<&Value>, wildcard_default: bool) -> Value {
    let Some(value) = value else {
        return if wildcard_default {
            json!(["*"])
        } else {
            Value::Null
        };
    };
    let mut values = match value {
        Value::String(value) => vec![value.trim().to_string()],
        Value::Array(values) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .map(ToString::to_string)
            .collect(),
        _ => return Value::Null,
    };
    values.sort();
    values.dedup();
    json!(values)
}

fn sorted_strings(values: &[String]) -> Vec<&str> {
    let mut values = values.iter().map(String::as_str).collect::<Vec<_>>();
    values.sort_unstable();
    values.dedup();
    values
}

/// Build a stable environment category from credential-free `(kind, access mode)` pairs.
#[must_use]
pub fn environment_binding_class<I, K, M>(bindings: I) -> String
where
    I: IntoIterator<Item = (K, M)>,
    K: Into<String>,
    M: Into<String>,
{
    let bindings = bindings
        .into_iter()
        .map(|(kind, mode)| format!("{}:{}", kind.into(), mode.into()))
        .collect::<BTreeSet<_>>();
    if bindings.is_empty() {
        "none".to_string()
    } else if bindings.len() == 1 {
        bindings.into_iter().next().unwrap_or_default()
    } else {
        format!(
            "composite[{}]",
            bindings.into_iter().collect::<Vec<_>>().join(",")
        )
    }
}

fn push_drift<T: Serialize + PartialEq>(
    drift: &mut Vec<MaterializationDrift>,
    field: &str,
    source: &T,
    target: &T,
) {
    if source == target {
        return;
    }
    drift.push(MaterializationDrift {
        field: field.to_string(),
        source: serde_json::to_value(source).ok(),
        target: serde_json::to_value(target).unwrap_or(Value::Null),
    });
}

fn canonical_value(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries = object.into_iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            let mut canonical = Map::new();
            for (key, value) in entries {
                canonical.insert(key, canonical_value(value));
            }
            Value::Object(canonical)
        }
        Value::Array(values) => Value::Array(values.into_iter().map(canonical_value).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use serde_json::json;

    use super::*;

    fn evidence(spec: &str, model: &str) -> ResolvedAgentMaterialization {
        ResolvedAgentMaterialization::new(
            spec,
            model,
            ["shell".to_string(), "filesystem".to_string()],
            "policy-v1",
            "local:read_write",
        )
    }

    fn spec_with_settings(settings: starweaver_model::ModelSettings) -> AgentSpec {
        AgentSpec {
            name: "agent".to_string(),
            model: Some(crate::ModelPreset {
                model_id: "model-a".to_string(),
                settings: Some(settings),
                ..crate::ModelPreset::default()
            }),
            ..AgentSpec::default()
        }
    }

    #[test]
    fn fingerprint_is_stable_and_authenticated() {
        let left = ResolvedAgentMaterialization::new(
            "spec-a",
            "model-a",
            ["shell".to_string(), "filesystem".to_string()],
            "policy-v1",
            "local:read_write",
        );
        let right = ResolvedAgentMaterialization::new(
            "spec-a",
            "model-a",
            ["filesystem".to_string(), "shell".to_string()],
            "policy-v1",
            "local:read_write",
        );
        assert_eq!(left, right);
        let mut metadata = Map::new();
        left.insert_into(&mut metadata).unwrap();
        assert_eq!(
            ResolvedAgentMaterialization::from_metadata(&metadata).unwrap(),
            Some(left.clone())
        );
        metadata[AGENT_MATERIALIZATION_METADATA_KEY]["policyVersion"] = json!("tampered");
        assert!(matches!(
            ResolvedAgentMaterialization::from_metadata(&metadata),
            Err(MaterializationEvidenceError::InvalidFingerprint)
        ));
    }

    #[test]
    fn verified_v1_evidence_projects_as_unbound_and_requires_switch() {
        let toolset_ids = vec!["filesystem".to_string(), "shell".to_string()];
        let fingerprint = calculate_legacy_fingerprint(
            1,
            "spec-a",
            "model-a",
            &toolset_ids,
            "policy-v1",
            "local:read_write",
        );
        let mut metadata = Map::from_iter([(
            AGENT_MATERIALIZATION_METADATA_KEY.to_string(),
            json!({
                "version": 1,
                "agentSpecDigest": "spec-a",
                "modelProfileId": "model-a",
                "toolsetIds": toolset_ids,
                "policyVersion": "policy-v1",
                "environmentBindingClass": "local:read_write",
                "fingerprint": fingerprint,
            }),
        )]);

        let Some(source) = ResolvedAgentMaterialization::from_metadata(&metadata).unwrap() else {
            panic!("v1 durable evidence must decode");
        };
        assert_eq!(source.version, 1);
        assert_eq!(source.runtime_binding_digest, "sha256:unbound");
        assert_eq!(source.workspace_root_digest, "sha256:unbound");
        assert!(source.fingerprint_is_valid());
        let mut forwarded = Map::new();
        source.insert_into(&mut forwarded).unwrap();
        assert_eq!(
            ResolvedAgentMaterialization::from_metadata(&forwarded).unwrap(),
            Some(source.clone()),
            "a verified v1 receipt must remain readable after a host copies its evidence"
        );

        let target = evidence("spec-a", "model-a")
            .with_host_bindings("sha256:runtime-bound", "sha256:workspace-bound");
        let preserve = ContinuationMaterialization::assess(
            Some(&source),
            &target,
            ContinuationMaterializationMode::Preserve,
        );
        assert!(!preserve.allowed);
        let compatible = ContinuationMaterialization::assess(
            Some(&source),
            &target,
            ContinuationMaterializationMode::Compatible,
        );
        assert!(!compatible.allowed);
        assert_eq!(
            compatible.drift_summary(),
            "runtimeBindingDigest, workspaceRootDigest"
        );
        assert!(
            ContinuationMaterialization::assess(
                Some(&source),
                &target,
                ContinuationMaterializationMode::Switch,
            )
            .allowed
        );

        metadata[AGENT_MATERIALIZATION_METADATA_KEY]["policyVersion"] = json!("tampered");
        assert!(matches!(
            ResolvedAgentMaterialization::from_metadata(&metadata),
            Err(MaterializationEvidenceError::InvalidFingerprint)
        ));
    }

    #[test]
    fn spec_digest_uses_an_allowlisted_semantic_projection() {
        let mut first = AgentSpec {
            name: "agent".to_string(),
            instructions: vec!["Be useful".to_string()],
            model: Some(crate::ModelPreset {
                model_id: "model-a".to_string(),
                settings: Some(starweaver_model::ModelSettings {
                    temperature: Some(0.2),
                    ..starweaver_model::ModelSettings::default()
                }),
                ..crate::ModelPreset::default()
            }),
            toolset_wrappers: vec![crate::ToolsetWrapperSpec {
                kind: "custom".to_string(),
                toolset: Some("tools".to_string()),
                params: Map::from_iter([("connectionString".to_string(), json!("first-secret"))]),
            }],
            ..AgentSpec::default()
        };
        first
            .metadata
            .insert("cookie".to_string(), json!("first-secret"));
        first
            .model
            .as_mut()
            .unwrap()
            .settings
            .as_mut()
            .unwrap()
            .extra_headers
            .insert("X-Custom-Auth".to_string(), "first-secret".to_string());

        let mut second = first.clone();
        second
            .metadata
            .insert("cookie".to_string(), json!("second-secret"));
        second.toolset_wrappers[0]
            .params
            .insert("connectionString".to_string(), json!("second-secret"));
        second
            .model
            .as_mut()
            .unwrap()
            .settings
            .as_mut()
            .unwrap()
            .extra_headers
            .insert("X-Custom-Auth".to_string(), "second-secret".to_string());
        assert_eq!(
            safe_agent_spec_digest(&first).unwrap(),
            safe_agent_spec_digest(&second).unwrap()
        );

        second.instructions.push("Use tools".to_string());
        assert_ne!(
            safe_agent_spec_digest(&first).unwrap(),
            safe_agent_spec_digest(&second).unwrap()
        );
    }

    #[test]
    fn safe_typed_provider_behavior_changes_the_spec_digest() {
        let responses = |truncation: &str, text_verbosity: &str| {
            spec_with_settings(starweaver_model::ModelSettings {
                provider_settings: starweaver_model::ProviderSettings {
                    openai_responses: Some(starweaver_model::OpenAiResponsesSettings {
                        truncation: Some(truncation.to_string()),
                        text_verbosity: Some(text_verbosity.to_string()),
                        ..starweaver_model::OpenAiResponsesSettings::default()
                    }),
                    ..starweaver_model::ProviderSettings::default()
                },
                ..starweaver_model::ModelSettings::default()
            })
        };
        let baseline = responses("auto", "low");
        assert_ne!(
            safe_agent_spec_digest(&baseline).unwrap(),
            safe_agent_spec_digest(&responses("disabled", "low")).unwrap()
        );
        assert_ne!(
            safe_agent_spec_digest(&baseline).unwrap(),
            safe_agent_spec_digest(&responses("auto", "high")).unwrap()
        );

        let chat_logprobs = |enabled| {
            spec_with_settings(starweaver_model::ModelSettings {
                provider_settings: starweaver_model::ProviderSettings {
                    openai_chat: Some(starweaver_model::OpenAiChatSettings {
                        logprobs: Some(enabled),
                        ..starweaver_model::OpenAiChatSettings::default()
                    }),
                    ..starweaver_model::ProviderSettings::default()
                },
                ..starweaver_model::ModelSettings::default()
            })
        };
        assert_ne!(
            safe_agent_spec_digest(&chat_logprobs(false)).unwrap(),
            safe_agent_spec_digest(&chat_logprobs(true)).unwrap()
        );

        let anthropic_beta = |beta: &str| {
            spec_with_settings(starweaver_model::ModelSettings {
                provider_settings: starweaver_model::ProviderSettings {
                    anthropic: Some(starweaver_model::AnthropicSettings {
                        betas: vec![beta.to_string()],
                        ..starweaver_model::AnthropicSettings::default()
                    }),
                    ..starweaver_model::ProviderSettings::default()
                },
                ..starweaver_model::ModelSettings::default()
            })
        };
        assert_ne!(
            safe_agent_spec_digest(&anthropic_beta("feature-a")).unwrap(),
            safe_agent_spec_digest(&anthropic_beta("feature-b")).unwrap()
        );

        let replay_policy = |send_item_ids, include_encrypted_reasoning| {
            spec_with_settings(starweaver_model::ModelSettings {
                provider_replay: Some(starweaver_model::ProviderReplaySettings {
                    previous_response_id: Some("provider-response-routing-id".to_string()),
                    conversation_id: Some("provider-conversation-routing-id".to_string()),
                    send_item_ids: Some(send_item_ids),
                    include_encrypted_reasoning: Some(include_encrypted_reasoning),
                }),
                ..starweaver_model::ModelSettings::default()
            })
        };
        let replay_baseline = replay_policy(false, false);
        assert_ne!(
            safe_agent_spec_digest(&replay_baseline).unwrap(),
            safe_agent_spec_digest(&replay_policy(true, false)).unwrap()
        );
        assert_ne!(
            safe_agent_spec_digest(&replay_baseline).unwrap(),
            safe_agent_spec_digest(&replay_policy(false, true)).unwrap()
        );
    }

    #[test]
    fn skill_and_workspace_roots_use_stable_one_way_set_identities() {
        let skill_root_a = "/private/skill-root-a";
        let skill_root_b = "/private/skill-root-b";
        let workspace_root_a = "/private/workspace-root-a";
        let workspace_root_b = "/private/workspace-root-b";
        let spec = |skill_roots: Vec<String>, workspace_roots: Vec<String>| AgentSpec {
            name: "agent".to_string(),
            skills: Some(crate::SkillBundleSpec {
                roots: skill_roots,
                ..crate::SkillBundleSpec::default()
            }),
            workspace: Some(crate::WorkspacePolicySpec {
                roots: workspace_roots,
                ..crate::WorkspacePolicySpec::default()
            }),
            ..AgentSpec::default()
        };

        let first = spec(
            vec![skill_root_a.to_string()],
            vec![workspace_root_a.to_string()],
        );
        let changed_skill = spec(
            vec![skill_root_b.to_string()],
            vec![workspace_root_a.to_string()],
        );
        let changed_workspace = spec(
            vec![skill_root_a.to_string()],
            vec![workspace_root_b.to_string()],
        );
        assert_ne!(
            safe_agent_spec_digest(&first).unwrap(),
            safe_agent_spec_digest(&changed_skill).unwrap()
        );
        assert_ne!(
            safe_agent_spec_digest(&first).unwrap(),
            safe_agent_spec_digest(&changed_workspace).unwrap()
        );

        let reordered = spec(
            vec![skill_root_b.to_string(), skill_root_a.to_string()],
            vec![workspace_root_b.to_string(), workspace_root_a.to_string()],
        );
        let reordered_with_duplicates = spec(
            vec![
                skill_root_a.to_string(),
                skill_root_b.to_string(),
                skill_root_a.to_string(),
            ],
            vec![
                workspace_root_a.to_string(),
                workspace_root_b.to_string(),
                workspace_root_a.to_string(),
            ],
        );
        assert_eq!(
            safe_agent_spec_digest(&reordered).unwrap(),
            safe_agent_spec_digest(&reordered_with_duplicates).unwrap()
        );

        let projection = serde_json::to_string(&agent_spec_semantic_projection(&first)).unwrap();
        assert!(projection.contains("rootsDigest"));
        for root in [
            skill_root_a,
            skill_root_b,
            workspace_root_a,
            workspace_root_b,
        ] {
            assert!(!projection.contains(root));
        }
    }

    #[test]
    fn routing_raw_extensions_and_metadata_do_not_affect_or_leak_into_projection() {
        let excluded_settings = |secret: &str| starweaver_model::ModelSettings {
            provider_replay: Some(starweaver_model::ProviderReplaySettings {
                previous_response_id: Some(format!("response-{secret}")),
                conversation_id: Some(format!("conversation-{secret}")),
                send_item_ids: Some(false),
                include_encrypted_reasoning: Some(false),
            }),
            provider_options: Some(json!({"credentials": secret})),
            extra_headers: std::collections::BTreeMap::from([(
                "Authorization".to_string(),
                secret.to_string(),
            )]),
            extra_body: Map::from_iter([("rawSecret".to_string(), json!(secret))]),
            provider_settings: starweaver_model::ProviderSettings {
                openai_chat: Some(starweaver_model::OpenAiChatSettings {
                    user: Some(format!("user-{secret}")),
                    prediction: Some(json!({"secret": secret})),
                    prompt_cache_key: Some(format!("chat-affinity-{secret}")),
                    ..starweaver_model::OpenAiChatSettings::default()
                }),
                openai_responses: Some(starweaver_model::OpenAiResponsesSettings {
                    user: Some(format!("user-{secret}")),
                    context_management: Some(json!({"secret": secret})),
                    prompt_cache_key: Some(format!("responses-affinity-{secret}")),
                    ..starweaver_model::OpenAiResponsesSettings::default()
                }),
                anthropic: Some(starweaver_model::AnthropicSettings {
                    metadata: Some(json!({"secret": secret})),
                    context_management: Some(json!({"secret": secret})),
                    container: Some(format!("container-{secret}")),
                    ..starweaver_model::AnthropicSettings::default()
                }),
                google: Some(starweaver_model::GoogleSettings {
                    safety_settings: Some(json!({"secret": secret})),
                    cached_content: Some(format!("cached-{secret}")),
                    labels: Some(json!({"secret": secret})),
                    ..starweaver_model::GoogleSettings::default()
                }),
                bedrock: Some(starweaver_model::BedrockSettings {
                    guardrail_config: Some(json!({"secret": secret})),
                    performance_config: Some(json!({"secret": secret})),
                    request_metadata: Some(json!({"secret": secret})),
                    prompt_variables: Some(json!({"secret": secret})),
                    additional_model_request_fields: Some(json!({"secret": secret})),
                    inference_profile: Some(format!("route-{secret}")),
                    ..starweaver_model::BedrockSettings::default()
                }),
                codex: Some(starweaver_model::CodexSettings {
                    session_id: Some(format!("session-{secret}")),
                    thread_id: Some(format!("thread-{secret}")),
                }),
                gateway: Some(starweaver_model::GatewaySettings {
                    x_session_id: Some(format!("gateway-session-{secret}")),
                    extra_headers: std::collections::BTreeMap::from([(
                        "X-Gateway-Auth".to_string(),
                        secret.to_string(),
                    )]),
                }),
            },
            ..starweaver_model::ModelSettings::default()
        };
        let spec = |secret: &str| {
            let mut spec = spec_with_settings(excluded_settings(secret));
            spec.metadata.insert("secret".to_string(), json!(secret));
            spec.host_policies.push(crate::HostPolicySpec {
                kind: "cli".to_string(),
                metadata: Map::from_iter([("secret".to_string(), json!(secret))]),
                ..crate::HostPolicySpec::default()
            });
            spec.workspace = Some(crate::WorkspacePolicySpec {
                metadata: Map::from_iter([("secret".to_string(), json!(secret))]),
                ..crate::WorkspacePolicySpec::default()
            });
            spec.toolset_wrappers.push(crate::ToolsetWrapperSpec {
                kind: "custom".to_string(),
                toolset: Some("tools".to_string()),
                params: Map::from_iter([("secret".to_string(), json!(secret))]),
            });
            spec
        };

        let first_secret = "first-sensitive-marker";
        let second_secret = "second-sensitive-marker";
        let first = spec(first_secret);
        let second = spec(second_secret);
        assert_eq!(
            safe_agent_spec_digest(&first).unwrap(),
            safe_agent_spec_digest(&second).unwrap()
        );

        let first_projection =
            serde_json::to_string(&agent_spec_semantic_projection(&first)).unwrap();
        let second_projection =
            serde_json::to_string(&agent_spec_semantic_projection(&second)).unwrap();
        for secret in [first_secret, second_secret] {
            assert!(!first_projection.contains(secret));
            assert!(!second_projection.contains(secret));
        }
    }

    #[test]
    fn known_wrapper_semantics_are_canonicalized_without_hashing_unknown_params() {
        let wrapper = |tools: Value, cookie: &str| crate::ToolsetWrapperSpec {
            kind: "approval_required".to_string(),
            toolset: Some("tools".to_string()),
            params: Map::from_iter([
                ("tools".to_string(), tools),
                ("cookie".to_string(), json!(cookie)),
            ]),
        };
        let spec = |wrapper| AgentSpec {
            name: "agent".to_string(),
            toolset_wrappers: vec![wrapper],
            ..AgentSpec::default()
        };
        let first = spec(wrapper(json!(["write", "read"]), "first-secret"));
        let equivalent = spec(wrapper(json!(["read", "write"]), "second-secret"));
        assert_eq!(
            safe_agent_spec_digest(&first).unwrap(),
            safe_agent_spec_digest(&equivalent).unwrap()
        );
        let changed = spec(wrapper(json!(["read"]), "second-secret"));
        assert_ne!(
            safe_agent_spec_digest(&first).unwrap(),
            safe_agent_spec_digest(&changed).unwrap()
        );
    }

    #[test]
    fn continuation_modes_report_and_enforce_drift() {
        let source = evidence("spec-a", "model-a");
        let exact = ContinuationMaterialization::assess(
            Some(&source),
            &source,
            ContinuationMaterializationMode::Preserve,
        );
        assert!(exact.allowed);
        assert!(exact.drift.is_empty());

        let revised = evidence("spec-b", "model-a");
        let compatible = ContinuationMaterialization::assess(
            Some(&source),
            &revised,
            ContinuationMaterializationMode::Compatible,
        );
        assert!(compatible.allowed);
        assert_eq!(compatible.drift_summary(), "agentSpecDigest");

        let switched = evidence("spec-b", "model-b");
        let rejected = ContinuationMaterialization::assess(
            Some(&source),
            &switched,
            ContinuationMaterializationMode::Compatible,
        );
        assert!(!rejected.allowed);
        let allowed = ContinuationMaterialization::assess(
            Some(&source),
            &switched,
            ContinuationMaterializationMode::Switch,
        );
        assert!(allowed.allowed);
        assert_eq!(allowed.drift.len(), 2);
    }

    #[test]
    fn continuation_validation_rejects_semantically_tampered_evidence() {
        let source = evidence("spec-a", "model-a");
        let target = evidence("spec-b", "model-b");
        let valid = ContinuationMaterialization::assess(
            Some(&source),
            &target,
            ContinuationMaterializationMode::Switch,
        );
        assert!(valid.validate(Some(&source), &target).is_ok());

        let mut denied = valid.clone();
        denied.allowed = false;
        assert!(matches!(
            denied.validate(Some(&source), &target),
            Err(MaterializationEvidenceError::InconsistentContinuation)
        ));

        let mut target_tamper = valid.clone();
        target_tamper.target_fingerprint = source.fingerprint.clone();
        assert!(matches!(
            target_tamper.validate(Some(&source), &target),
            Err(MaterializationEvidenceError::InconsistentContinuation)
        ));

        let mut source_tamper = valid.clone();
        source_tamper.source_fingerprint = Some(target.fingerprint.clone());
        assert!(matches!(
            source_tamper.validate(Some(&source), &target),
            Err(MaterializationEvidenceError::InconsistentContinuation)
        ));

        let mut drift_tamper = valid;
        drift_tamper.drift.clear();
        assert!(matches!(
            drift_tamper.validate(Some(&source), &target),
            Err(MaterializationEvidenceError::InconsistentContinuation)
        ));
    }

    #[test]
    fn continuation_metadata_roundtrips_and_rejects_malformed_evidence() {
        let source = evidence("spec-a", "model-a");
        let target = evidence("spec-b", "model-a");
        let continuation = ContinuationMaterialization::assess(
            Some(&source),
            &target,
            ContinuationMaterializationMode::Compatible,
        );
        let mut metadata = Map::new();
        continuation.insert_into(&mut metadata).unwrap();
        assert_eq!(
            ContinuationMaterialization::from_metadata(&metadata).unwrap(),
            Some(continuation)
        );
        assert_eq!(
            ContinuationMaterialization::from_metadata(&Map::new()).unwrap(),
            None
        );
        metadata.insert(
            AGENT_CONTINUATION_METADATA_KEY.to_string(),
            json!({"mode": "invalid"}),
        );
        assert!(matches!(
            ContinuationMaterialization::from_metadata(&metadata),
            Err(MaterializationEvidenceError::Malformed(_))
        ));
    }

    #[test]
    fn legacy_source_requires_explicit_switch() {
        let target = evidence("spec-a", "model-a");
        let preserve = ContinuationMaterialization::assess(
            None,
            &target,
            ContinuationMaterializationMode::Preserve,
        );
        assert!(!preserve.allowed);
        assert_eq!(preserve.drift_summary(), "sourceEvidence");
        assert!(
            ContinuationMaterialization::assess(
                None,
                &target,
                ContinuationMaterializationMode::Switch
            )
            .allowed
        );
    }

    #[test]
    fn environment_class_omits_binding_identity_and_credentials() {
        let class = environment_binding_class([
            ("envd", "read_only"),
            ("local", "read_write"),
            ("envd", "read_only"),
        ]);
        assert_eq!(class, "composite[envd:read_only,local:read_write]");
    }
}
