#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{
    get_model_config, get_model_settings, model_runtime_preset, AgentSpec, AgentSpecRegistry,
    TestModel,
};

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
