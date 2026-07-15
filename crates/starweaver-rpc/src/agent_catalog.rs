//! RPC-owned agent profile catalog and production model materialization.

use std::{env, sync::Arc};

use serde::Serialize;
use starweaver_agent::{
    AgentRuntimeBuilder, AgentSpec, AgentSpecRegistry, ModelPreset, SubagentConfig,
    SubagentToolInheritancePolicy, agent_session_control_tools, agent_session_query_tools,
    core_toolsets,
};
use starweaver_model::{
    HttpModelConfig, ModelAdapter, ModelProfile, ProtocolFamily, ProtocolModelClient,
    ReqwestHttpClient, anthropic_http_config, gemini_http_config, get_model_config,
    openai_chat_http_config, openai_responses_http_config,
};

use crate::{RpcConfig, RpcHostError, RpcHostResult, RpcProfileConfig, RpcProviderConfig};

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
}

impl RpcAgentCatalog {
    /// Validate and create a catalog from standalone RPC configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when the default profile is missing or a profile contains an unsupported
    /// model id or toolset.
    pub fn new(config: RpcConfig) -> RpcHostResult<Self> {
        let catalog = Self { config };
        catalog.validate()?;
        Ok(catalog)
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
        let profile = self.profile(name)?;
        let model = self.materialize_model(profile)?;
        let spec = agent_spec(name, profile);
        let mut registry = Self::registry_with_model(profile, model);
        for subagent_name in &profile.subagents {
            let declaration = self.config.subagents.get(subagent_name).ok_or_else(|| {
                RpcHostError::Invalid(format!(
                    "RPC profile {name} references unknown subagent: {subagent_name}"
                ))
            })?;
            let child_profile = self.profile(&declaration.profile)?;
            let child_model = self.materialize_model(child_profile)?;
            let child_registry = Self::registry_with_model(child_profile, child_model);
            let mut child_spec = agent_spec(&declaration.profile, child_profile);
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
        profile: &RpcProfileConfig,
        model: Arc<dyn ModelAdapter>,
    ) -> AgentSpecRegistry {
        let mut registry = AgentSpecRegistry::new().with_model(&profile.model_id, model);
        for toolset in core_toolsets()
            .into_iter()
            .chain([agent_session_query_tools(), agent_session_control_tools()])
        {
            registry = registry.with_toolset(toolset);
        }
        registry
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
            if profile.test_response.is_none() {
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

fn agent_spec(name: &str, profile: &RpcProfileConfig) -> AgentSpec {
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
        toolsets: profile.toolsets.clone(),
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
