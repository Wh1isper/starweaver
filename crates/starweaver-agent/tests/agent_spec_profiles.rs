#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{
    AgentSpec, AgentSpecError, AgentSpecRegistry, HostAdapterSpec, McpServerSpec,
    RetryPolicyPreset, TestModel,
};
use starweaver_model::{tool_call_response, ModelResponse};

#[tokio::test]
async fn agent_spec_resolves_policy_host_and_mcp_profiles() {
    let spec = AgentSpec::from_yaml(
        r"
name: profile
model:
  model_id: test
preset:
  retry_preset: balanced
  retry:
    tool_retries: 3
output:
  retries: 2
host_adapters:
  - web
mcp_servers:
  - local
",
    )
    .unwrap();
    let registry = AgentSpecRegistry::new()
        .with_model(
            "test",
            Arc::new(TestModel::with_responses(vec![
                tool_call_response("call", "missing_tool", serde_json::json!({})),
                ModelResponse::text("ok"),
            ])),
        )
        .with_retry_preset(
            "balanced",
            RetryPolicyPreset {
                max_steps: Some(1),
                output_retries: Some(1),
                tool_retries: Some(1),
                timeout_ms: None,
            },
        )
        .with_host_adapter(
            "web",
            HostAdapterSpec {
                kind: "search".to_string(),
                name: "fake".to_string(),
                metadata: serde_json::Map::new(),
            },
        )
        .with_mcp_server(
            "local",
            McpServerSpec {
                name: "local".to_string(),
                transport: "stdio".to_string(),
                metadata: serde_json::Map::new(),
            },
        );

    let result = spec.builder(&registry).unwrap().build().run("hello").await;

    assert!(matches!(
        result.err().unwrap(),
        starweaver_agent::AgentError::ToolCallsRequireTools
    ));
}

#[test]
fn agent_spec_reports_unknown_policy_host_and_mcp_refs() {
    let registry =
        AgentSpecRegistry::new().with_model("test", Arc::new(TestModel::with_text("ok")));
    let missing_retry = AgentSpec::from_yaml(
        r"
name: missing-retry
model:
  model_id: test
preset:
  retry_preset: missing
",
    )
    .unwrap();
    let missing_host = AgentSpec::from_yaml(
        r"
name: missing-host
model:
  model_id: test
host_adapters:
  - missing
",
    )
    .unwrap();
    let missing_mcp = AgentSpec::from_yaml(
        r"
name: missing-mcp
model:
  model_id: test
mcp_servers:
  - missing
",
    )
    .unwrap();

    assert!(matches!(
        missing_retry.builder(&registry).err().unwrap(),
        AgentSpecError::UnknownPolicyPreset { kind: "retry", name } if name == "missing"
    ));
    assert!(matches!(
        missing_host.builder(&registry).err().unwrap(),
        AgentSpecError::UnknownHostAdapter(name) if name == "missing"
    ));
    assert!(matches!(
        missing_mcp.builder(&registry).err().unwrap(),
        AgentSpecError::UnknownMcpServer(name) if name == "missing"
    ));
}
