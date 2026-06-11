#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use starweaver_model::{
    FunctionModel, ModelMessage, ModelRequestPart, ModelResponse, ModelSettings,
};
use starweaver_runtime::{
    resolve_capability_order, Agent, AgentCapability, CapabilityOrderError, CapabilityOrdering,
    CapabilityResult, CapabilitySpec, OutputValidationError, OutputValidationResult,
    OutputValidator, OutputValue, StaticCapabilityBundle, UsageLimits,
};
use starweaver_tools::{FunctionTool, ToolContext, ToolResult};

struct CompleteRecorder {
    completed: Arc<Mutex<bool>>,
}

struct OrderedRecorder {
    id: &'static str,
    ordering: CapabilityOrdering,
    calls: Arc<Mutex<Vec<&'static str>>>,
}

struct BundleAnswerValidator;

#[async_trait]
impl OutputValidator for BundleAnswerValidator {
    async fn validate(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        output: &OutputValue,
    ) -> OutputValidationResult<()> {
        if output.as_text().contains("bundle") {
            Ok(())
        } else {
            Err(OutputValidationError::failed("unexpected answer"))
        }
    }
}

#[async_trait]
impl AgentCapability for CompleteRecorder {
    async fn on_run_complete(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
    ) -> CapabilityResult<()> {
        *self.completed.lock().unwrap() = true;
        Ok(())
    }
}

#[async_trait]
impl AgentCapability for OrderedRecorder {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new(self.id).with_ordering(self.ordering.clone())
    }

    async fn on_run_start(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
    ) -> CapabilityResult<()> {
        self.calls.lock().unwrap().push(self.id);
        Ok(())
    }
}

#[tokio::test]
async fn capability_order_resolver_sorts_and_reports_diagnostics() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let first = Arc::new(OrderedRecorder {
        id: "first",
        ordering: CapabilityOrdering::default(),
        calls: calls.clone(),
    });
    let second = Arc::new(OrderedRecorder {
        id: "second",
        ordering: CapabilityOrdering::default().after("first"),
        calls: calls.clone(),
    });
    let ordered = resolve_capability_order(&[second, first.clone()]).unwrap();
    assert_eq!(ordered[0].spec().id.as_str(), "first");
    assert_eq!(ordered[1].spec().id.as_str(), "second");

    let duplicate = resolve_capability_order(&[first.clone(), first]);
    assert!(matches!(
        duplicate,
        Err(CapabilityOrderError::DuplicateId(_))
    ));

    let missing = Arc::new(OrderedRecorder {
        id: "missing",
        ordering: CapabilityOrdering::default().after("absent"),
        calls,
    });
    assert!(matches!(
        resolve_capability_order(&[missing]),
        Err(CapabilityOrderError::MissingDependency { .. })
    ));
}

#[tokio::test]
async fn agent_executes_capabilities_in_spec_order() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let first = Arc::new(OrderedRecorder {
        id: "first",
        ordering: CapabilityOrdering::default(),
        calls: calls.clone(),
    });
    let second = Arc::new(OrderedRecorder {
        id: "second",
        ordering: CapabilityOrdering::default().after("first"),
        calls: calls.clone(),
    });
    let model = Arc::new(FunctionModel::new(|_messages, _settings, _info| {
        Ok(ModelResponse::text("ok"))
    }));

    let result = Agent::new(model)
        .with_capability(second)
        .with_capability(first)
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
    assert_eq!(*calls.lock().unwrap(), vec!["first", "second"]);
}

#[tokio::test]
async fn capability_bundle_contributes_runtime_components() {
    let captured_messages = Arc::new(Mutex::new(Vec::<Vec<ModelMessage>>::new()));
    let captured_settings = Arc::new(Mutex::new(Vec::<Option<ModelSettings>>::new()));
    let messages = captured_messages.clone();
    let settings = captured_settings.clone();
    let model = FunctionModel::new(move |provider_messages, provider_settings, info| {
        messages.lock().unwrap().push(provider_messages);
        settings.lock().unwrap().push(provider_settings);
        assert_eq!(info.params.tools.len(), 1);
        assert_eq!(info.params.tools[0].name, "bundle_lookup");
        Ok(ModelResponse::text(r#"{"answer":"bundle"}"#))
    });
    let completed = Arc::new(Mutex::new(false));
    let tool = FunctionTool::new(
        "bundle_lookup",
        Some("Lookup from a capability bundle".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args| async move { Ok(ToolResult::new(args)) },
    );
    let bundle = StaticCapabilityBundle::new("bundle")
        .with_instruction("Use the bundle instruction.")
        .with_tool(Arc::new(tool))
        .with_model_settings(ModelSettings {
            temperature: Some(0.2),
            ..ModelSettings::default()
        })
        .with_output_validator(Arc::new(BundleAnswerValidator))
        .with_usage_limits(UsageLimits::new().with_request_limit(1))
        .with_hook(Arc::new(CompleteRecorder {
            completed: completed.clone(),
        }));

    let result = Agent::new(Arc::new(model))
        .with_capability_bundle(&bundle)
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, r#"{"answer":"bundle"}"#);
    assert!(*completed.lock().unwrap());
    let provider_messages = captured_messages.lock().unwrap()[0].clone();
    assert!(provider_messages.iter().any(|message| matches!(message, ModelMessage::Request(request) if request.parts.iter().any(|part| matches!(part, ModelRequestPart::SystemPrompt { text, .. } if text == "Use the bundle instruction.")))));
    assert_eq!(
        captured_settings.lock().unwrap()[0]
            .as_ref()
            .unwrap()
            .temperature,
        Some(0.2)
    );
}

#[tokio::test]
async fn override_can_apply_capability_bundle() {
    let bundle = StaticCapabilityBundle::new("override-bundle")
        .with_instruction("Override bundle instruction.");
    let model = Arc::new(FunctionModel::new(|messages, _settings, _info| {
        let has_instruction = messages.iter().any(|message| matches!(message, ModelMessage::Request(request) if request.parts.iter().any(|part| matches!(part, ModelRequestPart::SystemPrompt { text, .. } if text == "Override bundle instruction."))));
        assert!(has_instruction);
        Ok(ModelResponse::text("ok"))
    }));

    let result = Agent::new(model)
        .override_config()
        .capability_bundle(&bundle)
        .build()
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
}
