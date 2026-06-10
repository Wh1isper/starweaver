#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{
    get_model_config, get_model_settings, model_runtime_preset, AgentSpec, AgentSpecRegistry,
    FunctionModel, FunctionModelInfo, ModelSettings, TestModel,
};
use starweaver_core::Usage;
use starweaver_model::{ModelMessage, ModelResponse};

#[tokio::test]
async fn agent_spec_loads_yaml_and_builds_agent() {
    let spec = AgentSpec::from_yaml(
        r"
name: helper
instructions:
  - Be concise
model:
  model_id: test-model
preset:
  runtime:
    max_steps: 4
    output_retries: 1
",
    )
    .unwrap();
    let registry = AgentSpecRegistry::new()
        .with_model("test-model", Arc::new(TestModel::with_text("from spec")));

    let agent = spec.builder(&registry).unwrap().build();
    let result = agent.run("hello").await.unwrap();

    assert_eq!(spec.name, "helper");
    assert_eq!(result.output, "from spec");
}

#[test]
fn model_presets_resolve_settings_configs_and_runtime_aliases() {
    let settings = get_model_settings("openai_responses_high_fast").unwrap();
    assert_eq!(settings.max_tokens, Some(32 * 1024));
    assert_eq!(settings.thinking.as_ref().unwrap().effort, "high");
    assert!(settings.service_tier.is_some());

    let config = get_model_config("claude").unwrap();
    assert_eq!(config.context_window, 1_000_000);
    assert!(config.profile.supports_document_input);

    let preset = model_runtime_preset(
        "gpt-main",
        "openai",
        "gpt-5.1",
        "openai_responses_high",
        "gpt5_270k",
    )
    .unwrap();
    assert_eq!(preset.model_id, "gpt-main");
    assert_eq!(
        preset.protocol,
        starweaver_agent::ProtocolFamily::OpenAiResponses
    );
    assert_eq!(preset.config.context_window, 270_000);
}

#[tokio::test]
async fn agent_spec_resolves_built_in_model_settings_preset() {
    let spec = AgentSpec::from_yaml(
        r"
name: preset-helper
model:
  model_id: test-model
  settings_preset: anthropic_low
  settings:
    max_tokens: 99
",
    )
    .unwrap();
    let registry =
        AgentSpecRegistry::new().with_model("test-model", Arc::new(TestModel::with_text("preset")));

    let agent = spec.builder(&registry).unwrap().build();
    let result = agent.run("hello").await.unwrap();

    assert_eq!(result.output, "preset");
}

#[test]
fn agent_spec_infers_oauth_model_ids_like_ya_agent_sdk() {
    let spec = AgentSpec::from_yaml(
        r"
name: oauth-helper
model:
  model_id: oauth@codex:gpt-5.5
",
    )
    .unwrap();
    let _agent = spec.builder(&AgentSpecRegistry::new()).unwrap().build();
}

#[test]
fn agent_spec_rejects_invalid_oauth_model_ids() {
    let spec = AgentSpec::from_yaml(
        r"
name: invalid-oauth-helper
model:
  model_id: oauth@codex
",
    )
    .unwrap();
    let Err(error) = spec.builder(&AgentSpecRegistry::new()) else {
        panic!("invalid oauth model id should fail");
    };

    assert!(matches!(
        error,
        starweaver_agent::AgentSpecError::InvalidOAuthModel { model_id }
            if model_id == "oauth@codex"
    ));
}

#[tokio::test]
async fn agent_spec_registered_model_takes_precedence_over_oauth_inference() {
    let spec = AgentSpec::from_yaml(
        r"
name: registered-oauth-helper
model:
  model_id: oauth@codex:gpt-5.5
",
    )
    .unwrap();
    let registry = AgentSpecRegistry::new().with_model(
        "oauth@codex:gpt-5.5",
        Arc::new(TestModel::with_text("registered")),
    );

    let result = spec
        .builder(&registry)
        .unwrap()
        .build()
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "registered");
}

#[tokio::test]
async fn agent_spec_model_settings_preset_reaches_runtime_requests() {
    let spec = AgentSpec::from_yaml(
        r"
name: settings-helper
model:
  model_id: settings-model
  settings_preset: openai_responses_high
  settings:
    max_tokens: 99
",
    )
    .unwrap();
    let model = FunctionModel::new(
        |_messages: Vec<ModelMessage>,
         settings: Option<ModelSettings>,
         _info: FunctionModelInfo| {
            let Some(settings) = settings else {
                panic!("settings preset should reach model request");
            };
            assert_eq!(settings.max_tokens, Some(99));
            let Some(thinking) = settings.thinking.as_ref() else {
                panic!("thinking preset should reach model request");
            };
            assert_eq!(thinking.effort, "high");
            Ok(ModelResponse::text("settings ok"))
        },
    );
    let registry = AgentSpecRegistry::new().with_model("settings-model", Arc::new(model));

    let result = spec
        .builder(&registry)
        .unwrap()
        .build()
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "settings ok");
}

#[tokio::test]
async fn agent_spec_context_window_does_not_default_to_accumulated_usage_limit() {
    let spec = AgentSpec::from_yaml(
        r"
name: context-window-helper
model:
  model_id: usage-model
  config_preset: gpt5_270k
",
    )
    .unwrap();
    let model = TestModel::with_responses(vec![ModelResponse {
        usage: Usage {
            requests: 1,
            input_tokens: 200_000,
            output_tokens: 100_001,
            total_tokens: 300_001,
            tool_calls: 0,
        },
        ..ModelResponse::text("usage ok")
    }]);
    let registry = AgentSpecRegistry::new().with_model("usage-model", Arc::new(model));

    let result = spec
        .builder(&registry)
        .unwrap()
        .build()
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "usage ok");
    assert_eq!(result.state.usage.total_tokens, 300_001);
}
