#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{
    AgentSpec, AgentSpecError, AgentSpecRegistry, ApprovalPolicyPreset, CapabilitySpec,
    DurabilityPolicyPreset, EnvironmentPolicyPreset, ObservabilityPolicyPreset,
    StreamingPolicyPreset, TestModel,
};
use starweaver_environment::VirtualEnvironmentProvider;

#[test]
#[allow(clippy::too_many_lines)]
fn agent_spec_v2_projects_host_materialized_policies() {
    let spec = AgentSpec::from_yaml(
        r#"
name: v2-agent
description: Host policy rich agent
dependency_schema:
  type: object
  properties:
    project:
      type: object
      properties:
        name:
          type: string
templates:
  - name: project-instruction
    template: "Work on {{project.name}}"
    target: instruction
capability_refs:
  - memory
capabilities:
  - id: inline-capability
    description: Inline capability
toolset_wrappers:
  - kind: approval_required
    toolset: filesystem
    params:
      tools: [edit]
host_policies:
  - kind: agui
    trust: untrusted_client
    sanitizers: [drop_system_prompts, reject_dangling_tool_results]
workspace:
  provider: local
  roots: [workspace]
  shell: review
metadata:
  owner: sdk-test
model:
  model_id: test
preset:
  approval_preset: approval-default
  streaming_preset: stream-default
  observability_preset: trace-default
  environment_preset: env-default
  durability_preset: durable-default
"#,
    )
    .unwrap();
    let registry = AgentSpecRegistry::new()
        .with_model("test", Arc::new(TestModel::with_text("ok")))
        .with_capability("memory", CapabilitySpec::new("memory"))
        .with_approval_preset(
            "approval-default",
            ApprovalPolicyPreset {
                approval_required_tools: vec!["edit".to_string()],
                deferred_tools: Vec::new(),
                network_requires_approval: false,
            },
        )
        .with_streaming_preset(
            "stream-default",
            StreamingPolicyPreset {
                collect_events: true,
                adapter: Some("display-jsonl".to_string()),
                replay: true,
            },
        )
        .with_observability_preset(
            "trace-default",
            ObservabilityPolicyPreset {
                trace_enabled: true,
                exporter: Some("otlp".to_string()),
                redaction_keys: vec!["api_key".to_string()],
                sampling_ratio: Some(1.0),
            },
        )
        .with_environment_preset(
            "env-default",
            EnvironmentPolicyPreset {
                provider: Some("local".to_string()),
                roots: vec!["workspace".to_string()],
                process_capable: true,
                sandbox: false,
            },
        )
        .with_durability_preset(
            "durable-default",
            DurabilityPolicyPreset {
                session_store: Some("sqlite".to_string()),
                checkpoint_every_steps: Some(1),
                persist_streams: true,
                resume_enabled: true,
            },
        );

    let policies = spec.host_policies(&registry).unwrap();

    assert_eq!(policies.templates[0].name, "project-instruction");
    assert_eq!(policies.capabilities.len(), 2);
    assert_eq!(policies.capability_refs, vec!["memory"]);
    assert_eq!(policies.host_policies[0].kind, "agui");
    assert_eq!(policies.workspace.unwrap().shell.as_deref(), Some("review"));
    assert_eq!(
        policies.streaming.unwrap().adapter.as_deref(),
        Some("display-jsonl")
    );
    assert_eq!(
        policies.approval.unwrap().approval_required_tools,
        vec!["edit".to_string()]
    );
    assert_eq!(
        policies.environment.unwrap().provider.as_deref(),
        Some("local")
    );
    assert!(policies.observability.unwrap().trace_enabled);
    assert_eq!(
        policies.durability.unwrap().session_store.as_deref(),
        Some("sqlite")
    );
    assert_eq!(policies.metadata["owner"], "sdk-test");

    let schema = AgentSpec::json_schema();
    assert_eq!(schema["title"], "Starweaver AgentSpec v2");
    assert!(schema["properties"].get("dependency_schema").is_some());
    assert!(schema["properties"].get("host_policies").is_some());
}

#[test]
fn agent_spec_v2_reports_unknown_capability_and_template_variable() {
    let registry =
        AgentSpecRegistry::new().with_model("test", Arc::new(TestModel::with_text("ok")));
    let missing_capability = AgentSpec::from_yaml(
        r"
name: missing-capability
model:
  model_id: test
capability_refs:
  - missing
",
    )
    .unwrap();
    assert!(matches!(
        missing_capability.host_policies(&registry).err().unwrap(),
        AgentSpecError::UnknownCapability(name) if name == "missing"
    ));

    let missing_template_var = AgentSpec::from_yaml(
        r#"
name: missing-template-var
model:
  model_id: test
dependency_schema:
  type: object
  properties:
    project:
      type: object
      properties:
        name:
          type: string
templates:
  - name: bad-template
    template: "Work on {{project.slug}}"
"#,
    )
    .unwrap();
    assert!(matches!(
        missing_template_var.host_policies(&registry).err().unwrap(),
        AgentSpecError::UnknownTemplateVariable { template, variable }
            if template == "bad-template" && variable == "project.slug"
    ));
}

#[tokio::test]
async fn agent_spec_v2_builds_owned_runtime() {
    let spec = AgentSpec::from_yaml(
        r"
name: runtime-agent
model:
  model_id: test
instructions:
  - Stay concise.
preset:
  environment:
    provider: local
",
    )
    .unwrap();
    let registry = AgentSpecRegistry::new()
        .with_model("test", Arc::new(TestModel::with_text("ok")))
        .with_environment_provider(
            "local",
            Arc::new(VirtualEnvironmentProvider::new("runtime-env")),
        );

    let mut runtime = spec.runtime_builder(&registry).unwrap().build();
    let result = runtime.run("hello").await.unwrap();

    assert_eq!(result.output, "ok");
    assert_eq!(runtime.session().context().usage.requests, 0);
    assert_eq!(
        runtime
            .export_environment_state()
            .await
            .unwrap()
            .unwrap()
            .provider_id,
        "runtime-env"
    );
}
