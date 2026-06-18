#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use starweaver_agent::{
    agent_runtime, render_instruction_template, AgentBuilder, AgentContext, AgentInput,
    AgentSession, AgentStreamEvent, ContentPart, EnvironmentHandle,
    EnvironmentProviderFactoryRegistry, FunctionModel, InstructionTemplateError, ModelCapability,
    ModelConfig, ResourceRestoreFactory, ResourceRestoreFactoryRegistry, TraceContext,
    RESOURCE_REF_KIND_KEY,
};
use starweaver_environment::{EnvironmentResult, ResourceRef, VirtualEnvironmentProvider};
use starweaver_model::{ModelMessage, ModelRequestPart, ModelResponse};
use starweaver_usage::Usage;

fn reusable_model() -> FunctionModel {
    FunctionModel::new(|_messages, _settings, _info| {
        Ok(ModelResponse {
            usage: Usage {
                requests: 1,
                ..Usage::default()
            },
            ..ModelResponse::text("ok")
        })
    })
}

#[derive(Debug)]
struct RuntimeResourceRestoreFactory;

#[async_trait::async_trait]
impl ResourceRestoreFactory for RuntimeResourceRestoreFactory {
    fn kind(&self) -> &'static str {
        "media"
    }

    async fn restore(&self, resource: &ResourceRef) -> EnvironmentResult<ResourceRef> {
        let mut restored = resource.clone();
        restored.uri = format!("resource://runtime-env/restored/{}", restored.id);
        restored
            .metadata
            .insert("runtime_restored".to_string(), serde_json::json!(true));
        Ok(restored)
    }
}

#[tokio::test]
async fn builder_renders_static_instruction_template_variables() {
    let model = Arc::new(FunctionModel::new(|_messages, _settings, _info| {
        Ok(ModelResponse::text("ok"))
    }));
    let variables = serde_json::json!({
        "project": {"name": "starweaver"},
        "priority": 2,
        "strict": true
    });

    let rendered = render_instruction_template(
        "Work on {{ project.name }} with P{{priority}} strict={{strict}}.",
        &variables,
    )
    .unwrap();
    assert_eq!(rendered, "Work on starweaver with P2 strict=true.");

    AgentBuilder::new(model.clone())
        .try_instruction_template(
            "Work on {{ project.name }} with P{{priority}} strict={{strict}}.",
            &variables,
        )
        .unwrap()
        .build()
        .run("status")
        .await
        .unwrap();

    let messages = model.captured_messages();
    let instruction = messages[0]
        .iter()
        .flat_map(|message| match message {
            ModelMessage::Request(request) => request.parts.iter().collect::<Vec<_>>(),
            ModelMessage::Response(_) => Vec::new(),
        })
        .find_map(|part| match part {
            ModelRequestPart::Instruction { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .unwrap();
    assert_eq!(instruction, "Work on starweaver with P2 strict=true.");

    let missing = render_instruction_template("{{project.slug}}", &variables).unwrap_err();
    assert_eq!(
        missing,
        InstructionTemplateError::MissingVariable("project.slug".to_string())
    );
    let non_scalar = render_instruction_template("{{project}}", &variables).unwrap_err();
    assert_eq!(
        non_scalar,
        InstructionTemplateError::NonScalarVariable("project".to_string())
    );
}

#[tokio::test]
async fn app_and_session_facades_delegate_to_runtime_agent() {
    let app = AgentBuilder::new(Arc::new(reusable_model())).build_app();

    assert_eq!(app.agent().run("via agent").await.unwrap().output, "ok");

    let mut session = app.session();
    assert_eq!(
        session
            .agent()
            .run("via session agent")
            .await
            .unwrap()
            .output,
        "ok"
    );
    session.set_trace_context(TraceContext::from_trace_id("trace-app-session"));
    assert_eq!(
        session.context().trace_context.trace_id.as_deref(),
        Some("trace-app-session")
    );
}

#[tokio::test]
async fn app_run_helpers_cover_history_context_and_streaming() {
    let app = AgentBuilder::new(Arc::new(reusable_model())).build_app();
    let first = app.run("first").await.unwrap();

    let with_history = app
        .run_with_history("second", first.messages.clone())
        .await
        .unwrap();
    assert_eq!(with_history.output, "ok");
    assert_eq!(with_history.new_messages().len(), 2);

    let mut context = AgentContext::default();
    let with_context = app.run_with_context("ctx", &mut context).await.unwrap();
    assert_eq!(with_context.output, "ok");
    assert_eq!(context.message_history.len(), with_context.messages.len());

    let stream = app.run_stream("stream").await.unwrap();
    assert_eq!(stream.result().output, "ok");
    assert!(!stream.events().is_empty());

    let mut live = app.stream("live");
    let mut live_events = Vec::new();
    while let Some(record) = live.recv().await {
        live_events.push(record);
    }
    let live_result = live.join().await.unwrap();
    assert_eq!(live_result.result.output, "ok");
    assert_eq!(live_result.context.usage.requests, 1);
    assert!(!live_events.is_empty());

    let mut explicit_events = Vec::new();
    let explicit = app
        .run_with_context_and_stream_events("events", &mut context, &mut explicit_events)
        .await
        .unwrap();
    assert_eq!(explicit.output, "ok");
    assert!(!explicit_events.is_empty());
}

#[tokio::test]
async fn app_run_accepts_multimodal_agent_input() {
    let captured = Arc::new(Mutex::new(None::<Vec<ContentPart>>));
    let captured_model = Arc::clone(&captured);
    let model = FunctionModel::new(move |messages, _settings, _info| {
        let content = messages.iter().find_map(|message| match message {
            ModelMessage::Request(request) => request.parts.iter().find_map(|part| match part {
                ModelRequestPart::UserPrompt { content, .. }
                    if matches!(
                        content.first(),
                        Some(ContentPart::Text { text }) if text == "Describe these assets."
                    ) =>
                {
                    Some(content.clone())
                }
                ModelRequestPart::UserPrompt { .. }
                | ModelRequestPart::SystemPrompt { .. }
                | ModelRequestPart::ToolReturn(_)
                | ModelRequestPart::RetryPrompt { .. }
                | ModelRequestPart::Instruction { .. } => None,
            }),
            ModelMessage::Response(_) => None,
        });
        *captured_model.lock().unwrap() = content;
        Ok(ModelResponse::text("ok"))
    });
    let mut model_config = ModelConfig::default();
    model_config.capabilities.insert(ModelCapability::Vision);
    model_config
        .capabilities
        .insert(ModelCapability::DocumentUnderstanding);
    let app = AgentBuilder::new(Arc::new(model))
        .model_config(model_config)
        .build_app();

    let input = AgentInput::parts(vec![
        ContentPart::text("Describe these assets."),
        ContentPart::image_url("https://example.test/image.png"),
        ContentPart::file_url("https://example.test/spec.pdf", "application/pdf"),
        ContentPart::image_bytes([1_u8, 2, 3], "image/png"),
        ContentPart::resource_ref("resource://workspace/doc-1", "application/pdf", "document"),
    ]);

    let result = app.run(input).await.unwrap();

    assert_eq!(result.output, "ok");
    assert_eq!(
        captured.lock().unwrap().clone().unwrap(),
        vec![
            ContentPart::text("Describe these assets."),
            ContentPart::image_url("https://example.test/image.png"),
            ContentPart::file_url("https://example.test/spec.pdf", "application/pdf"),
            ContentPart::image_bytes([1_u8, 2, 3], "image/png"),
            ContentPart::resource_ref("resource://workspace/doc-1", "application/pdf", "document"),
        ]
    );
}

#[test]
fn app_builds_sessions_from_context_and_exported_state() {
    let app = AgentBuilder::new(Arc::new(reusable_model())).build_app();
    let mut context = AgentContext::default();
    context.state.set("k", serde_json::json!("v"));

    let session = app.session_with_context(context);
    assert_eq!(
        session.context().state.get("k"),
        Some(&serde_json::json!("v"))
    );

    let restored = app.session_from_state(session.export_full_state());
    assert_eq!(
        restored.context().state.get("k"),
        Some(&serde_json::json!("v"))
    );

    let direct = AgentSession::from_state(app.agent().clone(), restored.export_full_state());
    assert_eq!(
        direct.context().state.get("k"),
        Some(&serde_json::json!("v"))
    );
}

#[tokio::test]
async fn runtime_builder_owns_session_state_environment_and_streaming() {
    let resource = ResourceRef {
        id: "artifact-1".to_string(),
        uri: "s3://bucket/artifact-1".to_string(),
        metadata: starweaver_core::Metadata::from_iter([(
            RESOURCE_REF_KIND_KEY.to_string(),
            serde_json::json!("media"),
        )]),
    };
    let provider = Arc::new(
        VirtualEnvironmentProvider::new("runtime-env")
            .with_file("README.md", "hello")
            .with_resource(resource.clone()),
    );
    let mut runtime = agent_runtime(Arc::new(reusable_model()))
        .instruction("runtime instruction")
        .environment(provider)
        .build();

    assert!(runtime
        .session()
        .context()
        .dependency::<EnvironmentHandle>()
        .is_some());
    let environment_state = runtime.export_environment_state().await.unwrap().unwrap();
    assert_eq!(environment_state.provider_id, "runtime-env");
    assert_eq!(environment_state.files["README.md"], "hello");
    assert_eq!(environment_state.resources, vec![resource.clone()]);
    let factories = EnvironmentProviderFactoryRegistry::portable_defaults();
    let resource_factories =
        ResourceRestoreFactoryRegistry::new().with_factory(Arc::new(RuntimeResourceRestoreFactory));
    runtime
        .restore_environment_from_state_with_resources(
            &factories,
            &resource_factories,
            &environment_state,
        )
        .await
        .unwrap();
    let restored_environment_state = runtime.export_environment_state().await.unwrap().unwrap();
    assert_eq!(restored_environment_state.files["README.md"], "hello");
    assert_eq!(restored_environment_state.resources[0].id, resource.id);
    assert_eq!(
        restored_environment_state.resources[0].uri,
        "resource://runtime-env/restored/artifact-1"
    );
    assert_eq!(
        restored_environment_state.resources[0].metadata["runtime_restored"],
        serde_json::json!(true)
    );

    let first = runtime.run("hello").await.unwrap();
    assert_eq!(first.output, "ok");
    assert_eq!(runtime.session().context().usage.requests, 1);

    let state = runtime.export_full_state();
    let mut restored = agent_runtime(Arc::new(reusable_model()))
        .state(state)
        .build();
    let second = restored.run("again").await.unwrap();
    assert_eq!(second.output, "ok");
    assert_eq!(restored.session().context().usage.requests, 2);

    let mut handle = restored.stream("live");
    let event = handle.recv().await.unwrap();
    assert!(matches!(event.event, AgentStreamEvent::RunStart { .. }));
    let live = handle
        .finish_into_session(restored.session_mut())
        .await
        .unwrap();
    assert_eq!(live.result.output, "ok");
    assert_eq!(restored.session().context().usage.requests, 3);
}
