#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{AgentSpec, AgentSpecRegistry, TestModel};

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
