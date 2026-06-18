//! Provider-neutral generation settings.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const OPENAI_PROMPT_CACHE_KEY_LIMIT: usize = 64;

/// Format a stable affinity identifier for `OpenAI` prompt-cache routing.
#[must_use]
pub fn format_openai_prompt_cache_key(affinity_id: &str) -> Option<String> {
    let mut key = String::from("sw_");
    for ch in affinity_id.trim().chars() {
        if key.len() >= OPENAI_PROMPT_CACHE_KEY_LIMIT {
            break;
        }
        key.push(
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
                ch
            } else {
                '_'
            },
        );
    }
    (key.len() > "sw_".len()).then_some(key)
}

/// Return whether Starweaver may auto-derive `OpenAI` prompt-cache keys for a model name.
#[must_use]
pub fn supports_automatic_openai_prompt_cache_key(model_name: &str) -> bool {
    let model = model_name.trim().to_ascii_lowercase();
    model.starts_with("gpt-")
        || model.starts_with("chatgpt-")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
}

/// Per-request generation configuration.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ModelSettings {
    /// Maximum generated tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Sampling temperature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Nucleus sampling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    /// Top-k sampling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Request timeout in milliseconds.
    ///
    /// This is the Rust transport-friendly representation of a cross-provider
    /// `timeout` model setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Allow multiple tool calls in one response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    /// Tool forcing or availability policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    /// Best-effort deterministic seed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Stop strings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
    /// Presence penalty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
    /// Frequency penalty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,
    /// Token-level logit bias.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub logit_bias: BTreeMap<String, i32>,
    /// Reasoning or thinking controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingSettings>,
    /// Latency/cost tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<ServiceTier>,
    /// Provider replay and server-side state controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_replay: Option<ProviderReplaySettings>,
    /// Typed provider-specific settings.
    #[serde(default, skip_serializing_if = "ProviderSettings::is_empty")]
    pub provider_settings: ProviderSettings,
    /// Provider-specific raw settings escape hatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<Value>,
    /// Request headers merged after adapter defaults and before request-level overrides.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra_headers: BTreeMap<String, String>,
    /// Extra JSON object merged into the top-level request body.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub extra_body: Map<String, Value>,
}

impl ModelSettings {
    /// Merge two settings values field by field, taking values from `overlay` when present.
    #[must_use]
    pub fn merge(&self, overlay: &Self) -> Self {
        Self {
            max_tokens: overlay.max_tokens.or(self.max_tokens),
            temperature: overlay.temperature.or(self.temperature),
            top_p: overlay.top_p.or(self.top_p),
            top_k: overlay.top_k.or(self.top_k),
            timeout_ms: overlay.timeout_ms.or(self.timeout_ms),
            parallel_tool_calls: overlay.parallel_tool_calls.or(self.parallel_tool_calls),
            tool_choice: overlay
                .tool_choice
                .clone()
                .or_else(|| self.tool_choice.clone()),
            seed: overlay.seed.or(self.seed),
            stop_sequences: if overlay.stop_sequences.is_empty() {
                self.stop_sequences.clone()
            } else {
                overlay.stop_sequences.clone()
            },
            presence_penalty: overlay.presence_penalty.or(self.presence_penalty),
            frequency_penalty: overlay.frequency_penalty.or(self.frequency_penalty),
            logit_bias: if overlay.logit_bias.is_empty() {
                self.logit_bias.clone()
            } else {
                let mut logit_bias = self.logit_bias.clone();
                logit_bias.extend(overlay.logit_bias.clone());
                logit_bias
            },
            thinking: overlay.thinking.clone().or_else(|| self.thinking.clone()),
            service_tier: overlay
                .service_tier
                .clone()
                .or_else(|| self.service_tier.clone()),
            provider_settings: self.provider_settings.merge(&overlay.provider_settings),
            provider_replay: overlay
                .provider_replay
                .clone()
                .or_else(|| self.provider_replay.clone()),
            provider_options: overlay
                .provider_options
                .clone()
                .or_else(|| self.provider_options.clone()),
            extra_headers: if overlay.extra_headers.is_empty() {
                self.extra_headers.clone()
            } else {
                overlay_headers_case_insensitive(&self.extra_headers, &overlay.extra_headers)
            },
            extra_body: if overlay.extra_body.is_empty() {
                self.extra_body.clone()
            } else {
                let mut body = self.extra_body.clone();
                body.extend(overlay.extra_body.clone());
                body
            },
        }
    }
}

/// Typed provider-specific settings.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderSettings {
    /// `OpenAI` Chat Completions settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_chat: Option<OpenAiChatSettings>,
    /// `OpenAI` Responses settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_responses: Option<OpenAiResponsesSettings>,
    /// Anthropic Messages settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic: Option<AnthropicSettings>,
    /// Gemini generateContent settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub google: Option<GoogleSettings>,
    /// Bedrock Converse settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bedrock: Option<BedrockSettings>,
    /// Codex OAuth routing settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<CodexSettings>,
    /// Gateway sticky-routing settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<GatewaySettings>,
}

impl ProviderSettings {
    /// Return whether no provider-specific settings are present.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.openai_chat.is_none()
            && self.openai_responses.is_none()
            && self.anthropic.is_none()
            && self.google.is_none()
            && self.bedrock.is_none()
            && self.codex.is_none()
            && self.gateway.is_none()
    }

    /// Merge provider settings field by field.
    #[must_use]
    pub fn merge(&self, overlay: &Self) -> Self {
        Self {
            openai_chat: merge_openai_chat(self.openai_chat.as_ref(), overlay.openai_chat.as_ref()),
            openai_responses: merge_openai_responses(
                self.openai_responses.as_ref(),
                overlay.openai_responses.as_ref(),
            ),
            anthropic: merge_anthropic(self.anthropic.as_ref(), overlay.anthropic.as_ref()),
            google: merge_google(self.google.as_ref(), overlay.google.as_ref()),
            bedrock: merge_bedrock(self.bedrock.as_ref(), overlay.bedrock.as_ref()),
            codex: merge_codex(self.codex.as_ref(), overlay.codex.as_ref()),
            gateway: merge_gateway(self.gateway.as_ref(), overlay.gateway.as_ref()),
        }
    }
}

fn merge_option<T, F>(base: Option<&T>, overlay: Option<&T>, merge: F) -> Option<T>
where
    T: Clone,
    F: FnOnce(&T, &T) -> T,
{
    match (base, overlay) {
        (Some(base), Some(overlay)) => Some(merge(base, overlay)),
        (Some(base), None) => Some(base.clone()),
        (None, Some(overlay)) => Some(overlay.clone()),
        (None, None) => None,
    }
}

fn overlay_vec<T: Clone>(base: &[T], overlay: &[T]) -> Vec<T> {
    if overlay.is_empty() {
        base.to_vec()
    } else {
        overlay.to_vec()
    }
}

fn overlay_headers_case_insensitive(
    base: &BTreeMap<String, String>,
    overlay: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut merged = base.clone();
    for (key, value) in overlay {
        merged.retain(|existing, _| !existing.eq_ignore_ascii_case(key));
        merged.insert(key.clone(), value.clone());
    }
    merged
}

fn merge_openai_chat(
    base: Option<&OpenAiChatSettings>,
    overlay: Option<&OpenAiChatSettings>,
) -> Option<OpenAiChatSettings> {
    merge_option(base, overlay, |base, overlay| OpenAiChatSettings {
        user: overlay.user.clone().or_else(|| base.user.clone()),
        store: overlay.store.or(base.store),
        logprobs: overlay.logprobs.or(base.logprobs),
        top_logprobs: overlay.top_logprobs.or(base.top_logprobs),
        prediction: overlay
            .prediction
            .clone()
            .or_else(|| base.prediction.clone()),
        prompt_cache_key: overlay
            .prompt_cache_key
            .clone()
            .or_else(|| base.prompt_cache_key.clone()),
        prompt_cache_retention: overlay
            .prompt_cache_retention
            .clone()
            .or_else(|| base.prompt_cache_retention.clone()),
    })
}

fn merge_openai_responses(
    base: Option<&OpenAiResponsesSettings>,
    overlay: Option<&OpenAiResponsesSettings>,
) -> Option<OpenAiResponsesSettings> {
    merge_option(base, overlay, |base, overlay| OpenAiResponsesSettings {
        store: overlay.store.or(base.store),
        user: overlay.user.clone().or_else(|| base.user.clone()),
        truncation: overlay
            .truncation
            .clone()
            .or_else(|| base.truncation.clone()),
        text_verbosity: overlay
            .text_verbosity
            .clone()
            .or_else(|| base.text_verbosity.clone()),
        context_management: overlay
            .context_management
            .clone()
            .or_else(|| base.context_management.clone()),
        include: overlay_vec(&base.include, &overlay.include),
        prompt_cache_key: overlay
            .prompt_cache_key
            .clone()
            .or_else(|| base.prompt_cache_key.clone()),
        prompt_cache_retention: overlay
            .prompt_cache_retention
            .clone()
            .or_else(|| base.prompt_cache_retention.clone()),
    })
}

fn merge_anthropic(
    base: Option<&AnthropicSettings>,
    overlay: Option<&AnthropicSettings>,
) -> Option<AnthropicSettings> {
    merge_option(base, overlay, |base, overlay| AnthropicSettings {
        metadata: overlay.metadata.clone().or_else(|| base.metadata.clone()),
        betas: overlay_vec(&base.betas, &overlay.betas),
        context_management: overlay
            .context_management
            .clone()
            .or_else(|| base.context_management.clone()),
        container: overlay.container.clone().or_else(|| base.container.clone()),
        service_tier: overlay
            .service_tier
            .clone()
            .or_else(|| base.service_tier.clone()),
    })
}

fn merge_google(
    base: Option<&GoogleSettings>,
    overlay: Option<&GoogleSettings>,
) -> Option<GoogleSettings> {
    merge_option(base, overlay, |base, overlay| GoogleSettings {
        safety_settings: overlay
            .safety_settings
            .clone()
            .or_else(|| base.safety_settings.clone()),
        cached_content: overlay
            .cached_content
            .clone()
            .or_else(|| base.cached_content.clone()),
        labels: overlay.labels.clone().or_else(|| base.labels.clone()),
        response_logprobs: overlay.response_logprobs.or(base.response_logprobs),
        logprobs: overlay.logprobs.or(base.logprobs),
        service_tier: overlay
            .service_tier
            .clone()
            .or_else(|| base.service_tier.clone()),
    })
}

fn merge_bedrock(
    base: Option<&BedrockSettings>,
    overlay: Option<&BedrockSettings>,
) -> Option<BedrockSettings> {
    merge_option(base, overlay, |base, overlay| BedrockSettings {
        guardrail_config: overlay
            .guardrail_config
            .clone()
            .or_else(|| base.guardrail_config.clone()),
        performance_config: overlay
            .performance_config
            .clone()
            .or_else(|| base.performance_config.clone()),
        request_metadata: overlay
            .request_metadata
            .clone()
            .or_else(|| base.request_metadata.clone()),
        additional_model_response_field_paths: overlay_vec(
            &base.additional_model_response_field_paths,
            &overlay.additional_model_response_field_paths,
        ),
        prompt_variables: overlay
            .prompt_variables
            .clone()
            .or_else(|| base.prompt_variables.clone()),
        additional_model_request_fields: overlay
            .additional_model_request_fields
            .clone()
            .or_else(|| base.additional_model_request_fields.clone()),
        inference_profile: overlay
            .inference_profile
            .clone()
            .or_else(|| base.inference_profile.clone()),
    })
}

fn merge_codex(
    base: Option<&CodexSettings>,
    overlay: Option<&CodexSettings>,
) -> Option<CodexSettings> {
    merge_option(base, overlay, |base, overlay| CodexSettings {
        session_id: overlay
            .session_id
            .clone()
            .or_else(|| base.session_id.clone()),
        thread_id: overlay.thread_id.clone().or_else(|| base.thread_id.clone()),
    })
}

fn merge_gateway(
    base: Option<&GatewaySettings>,
    overlay: Option<&GatewaySettings>,
) -> Option<GatewaySettings> {
    merge_option(base, overlay, |base, overlay| GatewaySettings {
        x_session_id: overlay
            .x_session_id
            .clone()
            .or_else(|| base.x_session_id.clone()),
        extra_headers: overlay_headers_case_insensitive(
            &base.extra_headers,
            &overlay.extra_headers,
        ),
    })
}

/// `OpenAI` Chat Completions typed settings.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct OpenAiChatSettings {
    /// End-user identifier forwarded to `OpenAI`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Store the completion for provider-side distillation/evals where supported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<bool>,
    /// Return log probabilities for output tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<bool>,
    /// Number of top log probabilities to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_logprobs: Option<u32>,
    /// Prediction hint payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prediction: Option<Value>,
    /// Prompt cache key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    /// Prompt cache retention setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache_retention: Option<String>,
}

/// `OpenAI` Responses typed settings.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct OpenAiResponsesSettings {
    /// Store the response for provider-side distillation/evals where supported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<bool>,
    /// End-user identifier forwarded to `OpenAI`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Response truncation strategy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncation: Option<String>,
    /// Text verbosity for supported models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_verbosity: Option<String>,
    /// Context management payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_management: Option<Value>,
    /// Additional include values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    /// Prompt cache key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    /// Prompt cache retention setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache_retention: Option<String>,
}

/// Anthropic Messages typed settings.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AnthropicSettings {
    /// Request metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    /// Beta feature names merged into the `anthropic-beta` request header.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub betas: Vec<String>,
    /// Context management payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_management: Option<Value>,
    /// Container identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// Service tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
}

/// Gemini generateContent typed settings.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct GoogleSettings {
    /// Safety settings array.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safety_settings: Option<Value>,
    /// Cached content resource name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_content: Option<String>,
    /// Backend-specific labels map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Value>,
    /// Return chosen-token log probabilities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_logprobs: Option<bool>,
    /// Number of top candidate log probabilities to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<u32>,
    /// Cloud service tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
}

/// Bedrock Converse typed settings.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BedrockSettings {
    /// Guardrail config payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guardrail_config: Option<Value>,
    /// Performance config payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub performance_config: Option<Value>,
    /// Request metadata payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_metadata: Option<Value>,
    /// Additional response field paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_model_response_field_paths: Vec<String>,
    /// Prompt variables payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_variables: Option<Value>,
    /// Additional model request fields merged after typed passthrough fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_model_request_fields: Option<Value>,
    /// Inference profile identifier used as the Bedrock `modelId` routing value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inference_profile: Option<String>,
}

/// Codex OAuth typed routing settings.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CodexSettings {
    /// Provider session ID for Codex headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Provider thread ID for Codex headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

/// Gateway typed sticky-routing settings.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct GatewaySettings {
    /// Gateway sticky session header value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x_session_id: Option<String>,
    /// Gateway-specific extra headers.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra_headers: BTreeMap<String, String>,
}

/// Provider replay and server-side state controls.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderReplaySettings {
    /// Same-provider response ID chaining policy, such as `auto` or a concrete provider response ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,
    /// Same-provider server-side conversation policy, such as `auto` or a concrete conversation ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// Whether to replay provider item IDs when the same provider can consume them safely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_item_ids: Option<bool>,
    /// Whether to request and replay encrypted reasoning payloads when supported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_encrypted_reasoning: Option<bool>,
}

/// Tool selection policy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ToolChoice {
    /// Provider decides whether to call a tool.
    Auto,
    /// Disable tools.
    None,
    /// Require any tool call.
    Required,
    /// Restrict the model to one or more named function tools.
    Tools {
        /// Function tool names.
        names: Vec<String>,
    },
    /// Restrict function tools while keeping structured output, text, and image output available.
    ToolOrOutput {
        /// Function tool names.
        function_tools: Vec<String>,
    },
    /// Force a named tool.
    Tool {
        /// Tool name.
        name: String,
    },
}

/// Reasoning or thinking controls.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ThinkingSettings {
    /// Effort level such as low, medium, high, xhigh, or max.
    pub effort: String,
    /// Optional token budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
    /// Provider-specific thinking mode, such as enabled, adaptive, or disabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Whether provider should include thought summaries or thinking traces when supported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_thoughts: Option<bool>,
    /// Optional provider-specific reasoning summary mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Provider service tier.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceTier {
    /// Let the provider decide, when it has an explicit auto tier.
    Auto,
    /// Explicit standard/default provider tier.
    Default,
    /// Low-latency tier.
    Flex,
    /// Priority tier.
    Priority,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_overlay_fields() {
        let base = ModelSettings {
            max_tokens: Some(128),
            temperature: Some(0.2),
            stop_sequences: vec!["base".to_string()],
            ..ModelSettings::default()
        };
        let overlay = ModelSettings {
            temperature: Some(0.7),
            stop_sequences: vec!["overlay".to_string()],
            ..ModelSettings::default()
        };

        let merged = base.merge(&overlay);

        assert_eq!(merged.max_tokens, Some(128));
        assert_eq!(merged.temperature, Some(0.7));
        assert_eq!(merged.stop_sequences, vec!["overlay"]);
    }
}
