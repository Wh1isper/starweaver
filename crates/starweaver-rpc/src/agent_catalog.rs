//! RPC-owned agent profile catalog and production model materialization.

use std::{env, path::Path, sync::Arc};

use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use starweaver_agent::materialization::STARWEAVER_AGENT_POLICY_VERSION;
use starweaver_agent::{
    AgentRuntimeBuilder, AgentSpec, AgentSpecRegistry, ModelPreset, ResolvedAgentMaterialization,
    SubagentConfig, SubagentToolInheritancePolicy, agent_session_control_tools,
    agent_session_query_tools, core_toolsets, user_input_tools,
};
use starweaver_model::{
    HttpModelConfig, ModelAdapter, ModelProfile, ProtocolFamily, ProtocolModelClient,
    ReqwestHttpClient, anthropic_http_config, gemini_http_config, get_model_config,
    openai_chat_http_config, openai_responses_http_config,
};

use crate::{RpcConfig, RpcHostError, RpcHostResult, RpcProfileConfig, RpcProviderConfig};

#[cfg(test)]
pub type TestRuntimeFactory =
    dyn Fn(&str) -> RpcHostResult<AgentRuntimeBuilder> + Send + Sync + 'static;

/// Stable RPC profile projection returned by management methods.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcProfileSummary {
    /// RPC-owned profile name.
    pub name: String,
    /// Optional human-readable label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Logical model id materialized by the RPC host.
    pub model_id: String,
    /// Profile source.
    pub source: &'static str,
}

/// RPC-owned catalog that projects profile config into public Agent SDK abstractions.
#[derive(Clone)]
pub struct RpcAgentCatalog {
    config: RpcConfig,
    #[cfg(test)]
    test_runtime_factory: Option<Arc<TestRuntimeFactory>>,
}

impl RpcAgentCatalog {
    /// Validate and create a catalog from standalone RPC configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when the default profile is missing or a profile contains an unsupported
    /// model id or toolset.
    pub fn new(config: RpcConfig) -> RpcHostResult<Self> {
        let catalog = Self {
            config,
            #[cfg(test)]
            test_runtime_factory: None,
        };
        catalog.validate()?;
        Ok(catalog)
    }

    #[cfg(test)]
    pub(crate) fn with_test_runtime_factory(mut self, factory: Arc<TestRuntimeFactory>) -> Self {
        self.test_runtime_factory = Some(factory);
        self
    }

    /// Return the configured default profile.
    #[must_use]
    pub fn default_profile(&self) -> &str {
        &self.config.default_profile
    }

    /// Return stable profile summaries in name order.
    #[must_use]
    pub fn profiles(&self) -> Vec<RpcProfileSummary> {
        self.config
            .profiles
            .iter()
            .map(|(name, profile)| RpcProfileSummary {
                name: name.clone(),
                label: profile.label.clone(),
                model_id: profile.model_id.clone(),
                source: if profile.test_response.is_some() {
                    "rpc_test"
                } else {
                    "rpc_config"
                },
            })
            .collect()
    }

    /// Return one configured profile.
    ///
    /// # Errors
    ///
    /// Returns an error when the profile does not exist.
    pub fn profile(&self, name: &str) -> RpcHostResult<&RpcProfileConfig> {
        self.config
            .profiles
            .get(name)
            .ok_or_else(|| RpcHostError::Invalid(format!("unknown RPC profile: {name}")))
    }

    /// Materialize an owned runtime builder from the selected RPC profile.
    ///
    /// This is deliberately RPC-owned. It consumes only public SDK/model abstractions and does not
    /// import CLI configuration, coordinators, or handlers.
    ///
    /// # Errors
    ///
    /// Returns an error when credentials are absent or the model/spec cannot be materialized.
    pub fn runtime_builder(&self, name: &str) -> RpcHostResult<AgentRuntimeBuilder> {
        #[cfg(test)]
        if let Some(factory) = self.test_runtime_factory.as_ref() {
            return factory(name);
        }
        let profile = self.profile(name)?;
        let model = self.materialize_model(profile)?;
        let spec = agent_spec(name, profile, self.clarifying_questions_enabled());
        let mut registry = self.registry_with_model(profile, model);
        for subagent_name in &profile.subagents {
            let declaration = self.config.subagents.get(subagent_name).ok_or_else(|| {
                RpcHostError::Invalid(format!(
                    "RPC profile {name} references unknown subagent: {subagent_name}"
                ))
            })?;
            let child_profile = self.profile(&declaration.profile)?;
            let child_model = self.materialize_model(child_profile)?;
            let child_registry = self.registry_with_model(child_profile, child_model);
            let mut child_spec = agent_spec(
                &declaration.profile,
                child_profile,
                self.clarifying_questions_enabled(),
            );
            child_spec.subagents.clear();
            child_spec.all_subagents = false;
            let child = child_spec
                .builder(&child_registry)
                .map_err(|error| RpcHostError::Invalid(error.to_string()))?
                .build();
            let mut configured = SubagentConfig::new(subagent_name, Arc::new(child))
                .with_tool_inheritance(SubagentToolInheritancePolicy::new(
                    declaration.required_tools.clone(),
                    declaration.optional_tools.clone(),
                ));
            if let Some(description) = declaration.description.as_deref() {
                configured = configured.with_description(description);
            }
            registry = registry.with_subagent(configured);
        }
        spec.runtime_builder(&registry)
            .map_err(|error| RpcHostError::Invalid(error.to_string()))
    }

    fn registry_with_model(
        &self,
        profile: &RpcProfileConfig,
        model: Arc<dyn ModelAdapter>,
    ) -> AgentSpecRegistry {
        let mut registry = AgentSpecRegistry::new().with_model(&profile.model_id, model);
        let mut toolsets = core_toolsets()
            .into_iter()
            .chain([agent_session_query_tools(), agent_session_control_tools()])
            .collect::<Vec<_>>();
        if self.clarifying_questions_enabled() {
            toolsets.push(user_input_tools());
        }
        for toolset in toolsets {
            registry = registry.with_toolset(toolset);
        }
        registry
    }

    const fn clarifying_questions_enabled(&self) -> bool {
        self.config.client_capabilities.hitl && self.config.client_capabilities.clarifying_questions
    }

    /// Resolve safe, credential-free materialization evidence for one profile.
    ///
    /// # Errors
    ///
    /// Returns an error when the profile or its `AgentSpec` references cannot be resolved.
    pub fn materialization(
        &self,
        name: &str,
        environment_binding_class: impl Into<String>,
    ) -> RpcHostResult<ResolvedAgentMaterialization> {
        let profile = self.profile(name)?;
        let spec = agent_spec(name, profile, self.clarifying_questions_enabled());
        let registry = self.registry_with_model(
            profile,
            Arc::new(starweaver_model::TestModel::with_text("identity-only")),
        );
        spec.resolved_materialization(
            &registry,
            STARWEAVER_AGENT_POLICY_VERSION,
            environment_binding_class,
        )
        .map(|materialization| {
            materialization.with_host_bindings(
                rpc_runtime_binding_digest(profile, &self.config.providers),
                workspace_root_digest(&self.config.workspace_root),
            )
        })
        .map_err(|error| RpcHostError::Invalid(error.to_string()))
    }

    /// Return whether a profile explicitly grants one toolset.
    #[must_use]
    pub(crate) fn grants_toolset(&self, profile: &str, toolset: &str) -> bool {
        self.config
            .profiles
            .get(profile)
            .is_some_and(|profile| profile.toolsets.iter().any(|name| name == toolset))
    }

    fn validate(&self) -> RpcHostResult<()> {
        if !self
            .config
            .profiles
            .contains_key(&self.config.default_profile)
        {
            return Err(RpcHostError::Invalid(format!(
                "RPC default profile is not configured: {}",
                self.config.default_profile
            )));
        }
        let available_toolsets = core_toolsets()
            .into_iter()
            .chain([agent_session_query_tools(), agent_session_control_tools()])
            .chain(self.clarifying_questions_enabled().then(user_input_tools))
            .flat_map(|toolset| {
                let mut keys = vec![toolset.name().to_string()];
                if let Some(id) = toolset.id() {
                    keys.push(id.to_string());
                }
                keys
            })
            .collect::<std::collections::BTreeSet<_>>();
        for (name, profile) in &self.config.profiles {
            if profile.model_id.trim().is_empty() {
                return Err(RpcHostError::Invalid(format!(
                    "RPC profile {name} has an empty model_id"
                )));
            }
            if profile.test_response.is_none() && profile.model_id != "local_echo" {
                let parsed = RpcModelId::parse(&profile.model_id)?;
                self.provider_config(&parsed)?;
            }
            for toolset in &profile.toolsets {
                if !available_toolsets.contains(toolset) {
                    return Err(RpcHostError::Invalid(format!(
                        "RPC profile {name} references unknown toolset: {toolset}"
                    )));
                }
            }
            for subagent in &profile.subagents {
                let declaration = self.config.subagents.get(subagent).ok_or_else(|| {
                    RpcHostError::Invalid(format!(
                        "RPC profile {name} references unknown subagent: {subagent}"
                    ))
                })?;
                if !self.config.profiles.contains_key(&declaration.profile) {
                    return Err(RpcHostError::Invalid(format!(
                        "RPC subagent {subagent} references unknown profile: {}",
                        declaration.profile
                    )));
                }
            }
            if let Some(preset) = profile.model_config.as_deref() {
                get_model_config(preset)
                    .map_err(|error| RpcHostError::Invalid(error.to_string()))?;
            }
        }
        Ok(())
    }

    fn materialize_model(
        &self,
        profile: &RpcProfileConfig,
    ) -> RpcHostResult<Arc<dyn ModelAdapter>> {
        #[cfg(test)]
        if let Some(response) = profile.test_response.as_ref() {
            return Ok(Arc::new(starweaver_model::TestModel::with_text(response)));
        }
        #[cfg(not(test))]
        if profile.test_response.is_some() {
            return Err(RpcHostError::Invalid(
                "deterministic RPC test models are not available in production builds".to_string(),
            ));
        }
        if profile.model_id == "local_echo" {
            return Ok(Arc::new(
                starweaver_model::TestModel::with_text("rpc local echo")
                    .with_model_name("local_echo"),
            ));
        }

        let parsed = RpcModelId::parse(&profile.model_id)?;
        let provider = self.provider_config(&parsed)?;
        if !provider.enabled {
            return Err(RpcHostError::Invalid(format!(
                "RPC provider {} is disabled for model {}",
                parsed.provider_key, profile.model_id
            )));
        }
        let key_env = provider
            .api_key_env
            .as_deref()
            .unwrap_or_else(|| parsed.default_api_key_env())
            .trim();
        if key_env.is_empty() {
            return Err(RpcHostError::Invalid(format!(
                "RPC provider {} has an empty api_key_env",
                parsed.provider_key
            )));
        }
        let api_key = env::var(key_env)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                RpcHostError::Invalid(format!(
                    "missing {key_env} for RPC model {}",
                    profile.model_id
                ))
            })?;
        let mut http_config = match parsed.protocol {
            ProtocolFamily::OpenAiResponses => openai_responses_http_config(api_key),
            ProtocolFamily::OpenAiChatCompletions => openai_chat_http_config(api_key),
            ProtocolFamily::AnthropicMessages => anthropic_http_config(api_key),
            ProtocolFamily::GeminiGenerateContent => {
                gemini_http_config(api_key, parsed.model_name.clone())
            }
            ProtocolFamily::BedrockConverse => {
                return Err(RpcHostError::Invalid(
                    "Bedrock model materialization is not configured by the standalone RPC host"
                        .to_string(),
                ));
            }
        };
        apply_http_overrides(&mut http_config, provider);
        let model_profile = match profile.model_config.as_deref() {
            Some(preset) => {
                get_model_config(preset)
                    .map_err(|error| RpcHostError::Invalid(error.to_string()))?
                    .profile
            }
            None => ModelProfile::for_protocol(parsed.protocol),
        };
        let client = ProtocolModelClient::new(
            parsed.provider_name,
            parsed.model_name,
            model_profile,
            http_config,
            Arc::new(
                ReqwestHttpClient::new()
                    .map_err(|error| RpcHostError::Invalid(error.to_string()))?,
            ),
        );
        Ok(Arc::new(client))
    }

    fn provider_config(&self, parsed: &RpcModelId) -> RpcHostResult<&RpcProviderConfig> {
        self.config
            .providers
            .get(&parsed.provider_key)
            .ok_or_else(|| {
                RpcHostError::Invalid(format!(
                    "RPC provider {} is not configured for model protocol {}",
                    parsed.provider_key, parsed.protocol_name
                ))
            })
    }
}

fn agent_spec(
    name: &str,
    profile: &RpcProfileConfig,
    enable_clarifying_questions: bool,
) -> AgentSpec {
    let mut toolsets = profile.toolsets.clone();
    if enable_clarifying_questions && !toolsets.iter().any(|name| name == "user_input") {
        toolsets.push("user_input".to_string());
    }
    AgentSpec {
        name: name.to_string(),
        description: profile.label.clone(),
        instructions: profile.instructions.clone(),
        model: Some(ModelPreset {
            model_id: profile.model_id.clone(),
            settings_preset: profile.model_settings.clone(),
            config_preset: profile.model_config.clone(),
            settings: None,
        }),
        toolsets,
        subagents: profile.subagents.clone(),
        ..AgentSpec::default()
    }
}

fn apply_http_overrides(config: &mut HttpModelConfig, provider: &RpcProviderConfig) {
    if let Some(base_url) = provider.base_url.as_deref() {
        config.set_base_url(base_url);
    }
    if let Some(endpoint_path) = provider.endpoint_path.as_deref() {
        config.set_endpoint_path(endpoint_path);
    }
}

fn rpc_runtime_binding_digest(
    profile: &RpcProfileConfig,
    providers: &std::collections::BTreeMap<String, RpcProviderConfig>,
) -> String {
    let provider = RpcModelId::parse(&profile.model_id).ok().and_then(|model| {
        providers.get(&model.provider_key).map(|provider| {
            json!({
                "key": model.provider_key,
                "protocol": model.protocol_name,
                "provider": model.provider_name,
                "enabled": provider.enabled,
                "baseUrl": provider.base_url,
                "endpointPath": provider.endpoint_path,
            })
        })
    });
    digest_binding(
        b"starweaver.rpc.materialization.runtime-binding/v1",
        &json!({
            "modelId": profile.model_id,
            "modelSettings": profile.model_settings,
            "modelConfig": profile.model_config,
            "testModel": profile.test_response.is_some(),
            "provider": provider,
        }),
    )
}

fn workspace_root_digest(root: &Path) -> String {
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    digest_binding(
        b"starweaver.rpc.materialization.workspace-root/v1",
        &json!({"root": canonical.to_string_lossy()}),
    )
}

fn digest_binding(domain: &[u8], value: &serde_json::Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update([0]);
    hasher.update(serde_json::to_vec(&value).unwrap_or_default());
    format!("sha256:{:x}", hasher.finalize())
}

struct RpcModelId {
    provider_key: String,
    provider_name: String,
    protocol_name: String,
    model_name: String,
    protocol: ProtocolFamily,
}

impl RpcModelId {
    fn parse(model_id: &str) -> RpcHostResult<Self> {
        let (prefix, model_name) = model_id.split_once(':').ok_or_else(|| {
            RpcHostError::Invalid(format!(
                "invalid RPC model id {model_id}; expected protocol:model"
            ))
        })?;
        if model_name.trim().is_empty() {
            return Err(RpcHostError::Invalid(format!(
                "invalid RPC model id {model_id}; model name is empty"
            )));
        }
        let (provider_key, protocol_name) = prefix
            .split_once('@')
            .map_or((None, prefix), |(provider, protocol)| {
                (Some(provider), protocol)
            });
        let (provider_name, protocol) = match protocol_name {
            "openai" | "openai-responses" => ("openai", ProtocolFamily::OpenAiResponses),
            "openai-chat" => ("openai", ProtocolFamily::OpenAiChatCompletions),
            "anthropic" | "claude" => ("anthropic", ProtocolFamily::AnthropicMessages),
            "gemini" | "google" => ("gemini", ProtocolFamily::GeminiGenerateContent),
            other => {
                return Err(RpcHostError::Invalid(format!(
                    "unsupported RPC model protocol in {model_id}: {other}"
                )));
            }
        };
        Ok(Self {
            provider_key: provider_key.unwrap_or(provider_name).to_string(),
            provider_name: provider_name.to_string(),
            protocol_name: protocol_name.to_string(),
            model_name: model_name.to_string(),
            protocol,
        })
    }

    const fn default_api_key_env(&self) -> &'static str {
        match self.protocol {
            ProtocolFamily::OpenAiResponses | ProtocolFamily::OpenAiChatCompletions => {
                "OPENAI_API_KEY"
            }
            ProtocolFamily::AnthropicMessages => "ANTHROPIC_API_KEY",
            ProtocolFamily::GeminiGenerateContent => "GEMINI_API_KEY",
            ProtocolFamily::BedrockConverse => "AWS_ACCESS_KEY_ID",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn production_profile_requires_configured_provider_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = RpcConfig::for_tests(temp.path());
        config.profiles.insert(
            "production".to_string(),
            RpcProfileConfig {
                model_id: "test-gateway@openai-responses:gpt-5".to_string(),
                test_response: None,
                ..RpcProfileConfig::default()
            },
        );
        config.providers.insert(
            "test-gateway".to_string(),
            RpcProviderConfig {
                api_key_env: Some("STARWEAVER_RPC_TEST_CREDENTIAL_THAT_MUST_NOT_EXIST".to_string()),
                ..RpcProviderConfig::default()
            },
        );
        let catalog = RpcAgentCatalog::new(config).unwrap();
        let Err(error) = catalog.runtime_builder("production") else {
            panic!("production model materialization must require credentials");
        };
        assert!(
            error
                .to_string()
                .contains("STARWEAVER_RPC_TEST_CREDENTIAL_THAT_MUST_NOT_EXIST")
        );
    }

    #[test]
    fn deterministic_fixture_is_private_to_test_profile() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        let catalog = RpcAgentCatalog::new(config).unwrap();
        assert_eq!(catalog.profiles()[0].source, "rpc_test");
        assert!(catalog.runtime_builder("default").is_ok());
    }

    #[test]
    fn materialization_binds_provider_endpoint_behavior_and_workspace_without_leaking_them() {
        let first_root = tempfile::tempdir().unwrap();
        let second_root = tempfile::tempdir().unwrap();
        let mut first = RpcConfig::for_tests(first_root.path());
        first.profiles.insert(
            "production".to_string(),
            RpcProfileConfig {
                model_id: "gateway@openai-responses:gpt-5".to_string(),
                test_response: None,
                ..RpcProfileConfig::default()
            },
        );
        first.providers.insert(
            "gateway".to_string(),
            RpcProviderConfig {
                base_url: Some("https://first.example.invalid/v1".to_string()),
                endpoint_path: Some("private-route-marker".to_string()),
                ..RpcProviderConfig::default()
            },
        );
        let first_evidence = RpcAgentCatalog::new(first.clone())
            .unwrap()
            .materialization("production", "local:read_write")
            .unwrap();

        let mut endpoint_changed = first.clone();
        endpoint_changed
            .providers
            .get_mut("gateway")
            .unwrap()
            .endpoint_path = Some("chat/completions".to_string());
        let endpoint_evidence = RpcAgentCatalog::new(endpoint_changed)
            .unwrap()
            .materialization("production", "local:read_write")
            .unwrap();
        assert_ne!(
            first_evidence.runtime_binding_digest,
            endpoint_evidence.runtime_binding_digest
        );
        assert_ne!(first_evidence.fingerprint, endpoint_evidence.fingerprint);

        let mut workspace_changed = first;
        workspace_changed.workspace_root = second_root.path().join("workspace");
        let workspace_evidence = RpcAgentCatalog::new(workspace_changed)
            .unwrap()
            .materialization("production", "local:read_write")
            .unwrap();
        assert_ne!(
            first_evidence.workspace_root_digest,
            workspace_evidence.workspace_root_digest
        );
        assert_ne!(first_evidence.fingerprint, workspace_evidence.fingerprint);
        let rendered = serde_json::to_string(&first_evidence).unwrap();
        assert!(!rendered.contains("first.example.invalid"));
        assert!(!rendered.contains("private-route-marker"));
        assert!(!rendered.contains(first_root.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn clarifying_question_tool_requires_explicit_rpc_client_capabilities() {
        let temp = tempfile::tempdir().unwrap();
        let config = RpcConfig::for_tests(temp.path());
        let disabled = RpcAgentCatalog::new(config.clone())
            .unwrap()
            .runtime_builder("default")
            .unwrap()
            .build();
        assert!(
            !disabled
                .app()
                .agent()
                .tools()
                .contains(starweaver_agent::ASK_USER_QUESTION_TOOL_NAME)
        );

        let mut enabled_config = config;
        enabled_config.client_capabilities.hitl = true;
        enabled_config.client_capabilities.clarifying_questions = true;
        let enabled = RpcAgentCatalog::new(enabled_config)
            .unwrap()
            .runtime_builder("default")
            .unwrap()
            .build();
        assert!(
            enabled
                .app()
                .agent()
                .tools()
                .contains(starweaver_agent::ASK_USER_QUESTION_TOOL_NAME)
        );
    }

    #[test]
    fn configured_rpc_subagents_use_async_only_tool_topology() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = RpcConfig::for_tests(temp.path());
        config.profiles.insert(
            "child".to_string(),
            RpcProfileConfig {
                model_id: "test:child".to_string(),
                test_response: Some("child result".to_string()),
                ..RpcProfileConfig::default()
            },
        );
        config.profiles.get_mut("default").unwrap().subagents = vec!["researcher".to_string()];
        config.subagents.insert(
            "researcher".to_string(),
            crate::RpcSubagentConfig {
                profile: "child".to_string(),
                description: Some("Research specialist".to_string()),
                required_tools: Vec::new(),
                optional_tools: Vec::new(),
            },
        );
        let catalog = RpcAgentCatalog::new(config).unwrap();
        let runtime = catalog
            .runtime_builder("default")
            .unwrap()
            .subagent_delegation_mode(starweaver_agent::SubagentDelegationMode::Async)
            .background_subagent_supervisor(Arc::new(
                starweaver_agent::BackgroundSubagentSupervisor::new(),
            ))
            .build();
        let tools = runtime.app().agent().tools();
        assert!(tools.contains("delegate"));
        assert!(tools.contains("steer_subagent"));
        assert!(tools.contains("cancel_subagent"));
        assert!(tools.contains("wait_subagent"));
        assert!(tools.contains("subagent_info"));
        assert!(!tools.contains("spawn_delegate"));
        assert!(!tools.contains("__delegate_backend"));
    }
}
