#![allow(missing_docs, clippy::unwrap_used)]

use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use starweaver_agent::{
    AgentBuilder, AgentContext, AgentRuntimeBuilder, CodeActConfig, CodeExecutionError,
    CodeExecutionLimits, CodeExecutionRequest, CodeExecutionResult, CodeExecutor,
    ComputerUseToolsetPolicy, InputApprovalPolicy, RecipeToolset, TestModel, ToolContext,
    ToolError, ToolResult, attach_computer_use, attach_environment, codeact_tools,
    computer_use_tools,
};
use starweaver_computer_use::{
    ComputerCapabilityGrant, ComputerSessionBinding, ComputerToolGrant, ComputerToolRouter,
    ComputerUseError, ComputerUseErrorCode, ComputerUsePolicy, EffectStatus, FakeComputerUseConfig,
    FakeComputerUseService, InputCleanupStatus, NativeActionFailure, RetryClassification,
};
use starweaver_context::{DependencyStore, ModelCapability};
use starweaver_core::{ConversationId, Metadata, RunId};
use starweaver_environment::{EnvironmentProvider, VirtualEnvironmentProvider};
use starweaver_model::{ModelMessage, ModelRequestPart, ModelResponse, tool_call_response};
use starweaver_tools::{
    DynTool, DynToolset, FunctionTool, NestedToolError, NestedToolInvoker, StaticToolset,
    TOOL_METADATA_DEPENDENCIES_KEY, TOOL_RESULT_NESTED_NON_RESUMABLE_KEY,
    ToolDependencyRequirements, Toolset,
};

struct TerminalIgnoringExecutor;

#[async_trait]
impl CodeExecutor for TerminalIgnoringExecutor {
    async fn execute(
        &self,
        _request: CodeExecutionRequest,
        tools: NestedToolInvoker,
    ) -> Result<CodeExecutionResult, CodeExecutionError> {
        let first = tools
            .invoke("dangerous_effect", serde_json::json!({}))
            .await;
        if !matches!(first, Err(NestedToolError::NonResumableControlFlow { .. })) {
            return Err(CodeExecutionError::Worker(format!(
                "expected first effect to be non-resumable, got {first:?}"
            )));
        }
        let second = tools
            .invoke("dangerous_effect", serde_json::json!({}))
            .await;
        if !matches!(second, Err(NestedToolError::NonResumableControlFlow { .. })) {
            return Err(CodeExecutionError::Worker(format!(
                "runtime broker accepted a call after terminal effect: {second:?}"
            )));
        }
        Err(CodeExecutionError::ToolBridge(
            "effect-bearing child failure is terminal".to_string(),
        ))
    }
}

#[tokio::test]
async fn run_code_composes_strict_target_through_agent_runtime() {
    let effects = Arc::new(AtomicUsize::new(0));
    let effects_for_tool = effects.clone();
    let mut metadata = Metadata::new();
    metadata.insert(
        TOOL_METADATA_DEPENDENCIES_KEY.to_string(),
        ToolDependencyRequirements::strict(Vec::<String>::new(), Vec::<String>::new(), false)
            .to_metadata_value(),
    );
    let target = Arc::new(
        FunctionTool::new(
            "increment",
            Some("Increment one test counter.".to_string()),
            serde_json::json!({
                "type": "object",
                "properties": {"amount": {"type": "integer"}},
                "required": ["amount"],
                "additionalProperties": false
            }),
            move |_context: ToolContext, arguments: serde_json::Value| {
                let effects = effects_for_tool.clone();
                async move {
                    effects.fetch_add(1, Ordering::SeqCst);
                    Ok(ToolResult::new(serde_json::json!({
                        "amount": arguments["amount"]
                    })))
                }
            },
        )
        .with_metadata(metadata),
    ) as DynTool;
    let targets = Arc::new(StaticToolset::new("targets").with_tool(target)) as DynToolset;
    let model = TestModel::with_responses(vec![
        tool_call_response(
            "code-call",
            "run_code",
            serde_json::json!({
                "source": "function main(input) { return tools.call(\"increment\", { amount: input.amount }); }",
                "input": {"amount": 4}
            }),
        ),
        ModelResponse::text("completed"),
    ]);
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(model))
        .toolset(&targets)
        .toolset(&codeact_tools(CodeActConfig::default()))
        .build();

    let result = runtime.run("compose the strict target").await.unwrap();

    assert_eq!(result.output, "completed");
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    let tool_return_names = result
        .state
        .message_history
        .iter()
        .filter_map(|message| match message {
            ModelMessage::Request(request) => Some(&request.parts),
            ModelMessage::Response(_) => None,
        })
        .flatten()
        .filter_map(|part| match part {
            ModelRequestPart::ToolReturn(tool_return) => Some(tool_return.name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(tool_return_names, vec!["run_code"]);
}

#[tokio::test]
async fn runtime_broker_rejects_custom_executor_calls_after_terminal_child_effect() {
    let effects = Arc::new(AtomicUsize::new(0));
    let effects_for_tool = effects.clone();
    let mut tool_metadata = Metadata::new();
    tool_metadata.insert(
        TOOL_METADATA_DEPENDENCIES_KEY.to_string(),
        ToolDependencyRequirements::strict(Vec::<String>::new(), Vec::<String>::new(), false)
            .to_metadata_value(),
    );
    let target = FunctionTool::new(
        "dangerous_effect",
        Some("Inject one effect-bearing structured failure".to_string()),
        serde_json::json!({"type": "object"}),
        move |_context: ToolContext, _arguments: serde_json::Value| {
            let effects = effects_for_tool.clone();
            async move {
                effects.fetch_add(1, Ordering::SeqCst);
                Ok(ToolResult::new(serde_json::json!({
                    "success": false,
                    "receipt": {"effect_status": "delivery_uncertain"},
                    "padding": "x".repeat(10 * 1024)
                }))
                .with_error()
                .with_metadata(Metadata::from_iter([(
                    TOOL_RESULT_NESTED_NON_RESUMABLE_KEY.to_string(),
                    serde_json::json!(true),
                )])))
            }
        },
    )
    .with_metadata(tool_metadata);
    let targets =
        Arc::new(StaticToolset::new("dangerous-target").with_tool(Arc::new(target))) as DynToolset;
    let model = TestModel::with_responses(vec![
        tool_call_response(
            "custom-terminal-executor",
            "run_code",
            serde_json::json!({"source": "function main() {}", "input": null}),
        ),
        ModelResponse::text("terminal effect reported"),
    ]);
    let limits = CodeExecutionLimits {
        max_output_bytes: 1024,
        ..CodeExecutionLimits::default()
    };
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(model))
        .toolset(&targets)
        .toolset(&codeact_tools(
            CodeActConfig::new(Arc::new(TerminalIgnoringExecutor)).with_limits(limits),
        ))
        .build();

    let result = runtime
        .run("exercise runtime terminal latch")
        .await
        .unwrap();

    assert_eq!(result.output, "terminal effect reported");
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    let run_code_return = result
        .state
        .message_history
        .iter()
        .filter_map(|message| match message {
            ModelMessage::Request(request) => Some(&request.parts),
            ModelMessage::Response(_) => None,
        })
        .flatten()
        .find_map(|part| match part {
            ModelRequestPart::ToolReturn(tool_return) if tool_return.name == "run_code" => {
                Some(tool_return)
            }
            _ => None,
        })
        .unwrap();
    assert!(run_code_return.is_error);
    assert_eq!(
        run_code_return
            .content
            .pointer("/nested_tool_result/result_omitted"),
        Some(&serde_json::json!(true))
    );
    assert!(serde_json::to_vec(&run_code_return.content).unwrap().len() <= 1024);
    assert!(run_code_return.app_value.is_none());
}

#[tokio::test]
async fn runtime_broker_rejects_children_when_terminal_evidence_budget_is_too_small() {
    let effects = Arc::new(AtomicUsize::new(0));
    let effects_for_tool = effects.clone();
    let mut tool_metadata = Metadata::new();
    tool_metadata.insert(
        TOOL_METADATA_DEPENDENCIES_KEY.to_string(),
        ToolDependencyRequirements::strict(Vec::<String>::new(), Vec::<String>::new(), false)
            .to_metadata_value(),
    );
    let target = FunctionTool::new(
        "dangerous_effect",
        Some("Effect that must not run without terminal evidence capacity".to_string()),
        serde_json::json!({"type": "object"}),
        move |_context: ToolContext, _arguments: serde_json::Value| {
            let effects = effects_for_tool.clone();
            async move {
                effects.fetch_add(1, Ordering::SeqCst);
                Ok(ToolResult::new(serde_json::json!({"success": true})))
            }
        },
    )
    .with_metadata(tool_metadata);
    let targets = Arc::new(StaticToolset::new("tiny-budget-target").with_tool(Arc::new(target)))
        as DynToolset;
    let model = TestModel::with_responses(vec![
        tool_call_response(
            "tiny-terminal-budget",
            "run_code",
            serde_json::json!({"source": "function main() {}", "input": null}),
        ),
        ModelResponse::text("tiny budget rejected"),
    ]);
    let limits = CodeExecutionLimits {
        max_output_bytes: 1,
        ..CodeExecutionLimits::default()
    };
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(model))
        .toolset(&targets)
        .toolset(&codeact_tools(
            CodeActConfig::new(Arc::new(TerminalIgnoringExecutor)).with_limits(limits),
        ))
        .build();

    let result = runtime
        .run("reject unsafe tiny output budget")
        .await
        .unwrap();

    assert_eq!(result.output, "tiny budget rejected");
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn run_code_computer_observe_then_click_keeps_structured_value_and_latest_image() {
    let grant = ComputerToolGrant::full();
    let policy = ComputerUsePolicy {
        allowed_capabilities: ComputerCapabilityGrant {
            observe: true,
            pointer: true,
            keyboard: true,
            accessibility_snapshot: false,
        },
        post_action_settle: Duration::ZERO,
        ..ComputerUsePolicy::default()
    };
    let service = Arc::new(FakeComputerUseService::new(
        policy,
        FakeComputerUseConfig::default(),
    ));
    let router = Arc::new(ComputerToolRouter::new(
        service.clone(),
        ComputerSessionBinding::ServiceOwnedLazy,
        grant,
    ));
    let mut context = AgentContext::default();
    context
        .model_config
        .capabilities
        .insert(ModelCapability::Vision);
    attach_computer_use(&mut context, router, grant).unwrap();

    let model = TestModel::with_responses(vec![
        tool_call_response(
            "code-computer-use",
            "run_code",
            serde_json::json!({
                "source": r#"function main() {
                    const observed = tools.call("computer_observe", { include_accessibility: false });
                    if (typeof observed !== "object" || Array.isArray(observed)) {
                        throw new TypeError("computer_observe must return an object");
                    }
                    return tools.call("computer_click", {
                        observation_id: observed.observation.observation_id,
                        x: 10,
                        y: 20
                    });
                }"#,
                "input": null
            }),
        ),
        ModelResponse::text("computer use completed"),
    ]);
    let runtime = AgentBuilder::new(Arc::new(model))
        .toolset(&computer_use_tools(
            grant,
            ComputerUseToolsetPolicy {
                input_approval: InputApprovalPolicy::Never,
                ..ComputerUseToolsetPolicy::default()
            },
        ))
        .toolset(&codeact_tools(CodeActConfig::default()))
        .build();

    let result = runtime
        .run_with_context("observe and click", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "computer use completed");
    let run_code_return = result
        .state
        .message_history
        .iter()
        .filter_map(|message| match message {
            ModelMessage::Request(request) => Some(&request.parts),
            ModelMessage::Response(_) => None,
        })
        .flatten()
        .find_map(|part| match part {
            ModelRequestPart::ToolReturn(tool_return) if tool_return.name == "run_code" => {
                Some(tool_return)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(
        run_code_return
            .content
            .pointer("/value/receipt/effect_status"),
        Some(&serde_json::json!("executed")),
        "unexpected run_code return: {}",
        run_code_return.content
    );
    assert!(
        run_code_return
            .content
            .pointer("/value/observation/observation_id")
            .and_then(serde_json::Value::as_str)
            .is_some()
    );
    assert!(!run_code_return.content.to_string().contains("data:image/"));
    assert_eq!(
        run_code_return
            .private_metadata
            .get("starweaver_geometry_bound_immutable_media"),
        Some(&serde_json::json!(true))
    );
    assert!(
        run_code_return
            .private_metadata
            .get("starweaver_tool_return_content_parts")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|parts| parts.len() == 1)
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn run_code_effect_bearing_failure_is_terminal_and_preserves_receipt() {
    let grant = ComputerToolGrant::full();
    let policy = ComputerUsePolicy {
        allowed_capabilities: ComputerCapabilityGrant {
            observe: true,
            pointer: true,
            keyboard: true,
            accessibility_snapshot: false,
        },
        post_action_settle: Duration::ZERO,
        ..ComputerUsePolicy::default()
    };
    let service = Arc::new(FakeComputerUseService::new(
        policy,
        FakeComputerUseConfig::default(),
    ));
    service
        .backend()
        .fail_next_action(NativeActionFailure {
            error: ComputerUseError::new(
                ComputerUseErrorCode::InputDeliveryUncertain,
                "injected effect-bearing failure",
                RetryClassification::EffectStatusDependent,
            ),
            effect_status: EffectStatus::Executed,
            receipt: None,
            cleanup: InputCleanupStatus::Complete,
        })
        .await;
    let router = Arc::new(ComputerToolRouter::new(
        service.clone(),
        ComputerSessionBinding::ServiceOwnedLazy,
        grant,
    ));
    let mut context = AgentContext::default();
    context
        .model_config
        .capabilities
        .insert(ModelCapability::Vision);
    attach_computer_use(&mut context, router, grant).unwrap();

    let model = TestModel::with_responses(vec![
        tool_call_response(
            "code-computer-effect-failure",
            "run_code",
            serde_json::json!({
                "source": r#"function main() {
                    const observed = tools.call("computer_observe", { include_accessibility: false });
                    try {
                        tools.call("computer_click", {
                            observation_id: observed.observation.observation_id,
                            x: 10,
                            y: 20
                        });
                    } catch (_) {
                        const fresh = tools.call("computer_observe", { include_accessibility: false });
                        return tools.call("computer_click", {
                            observation_id: fresh.observation.observation_id,
                            x: 10,
                            y: 20
                        });
                    }
                    return "unexpected success";
                }"#,
                "input": null
            }),
        ),
        ModelResponse::text("effect failure reported"),
    ]);
    let runtime = AgentBuilder::new(Arc::new(model))
        .toolset(&computer_use_tools(
            grant,
            ComputerUseToolsetPolicy {
                input_approval: InputApprovalPolicy::Never,
                ..ComputerUseToolsetPolicy::default()
            },
        ))
        .toolset(&codeact_tools(CodeActConfig::default()))
        .build();

    let result = runtime
        .run_with_context("do not repeat an uncertain effect", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "effect failure reported");
    assert!(service.backend().recorded_actions().await.is_empty());
    let run_code_return = result
        .state
        .message_history
        .iter()
        .filter_map(|message| match message {
            ModelMessage::Request(request) => Some(&request.parts),
            ModelMessage::Response(_) => None,
        })
        .flatten()
        .find_map(|part| match part {
            ModelRequestPart::ToolReturn(tool_return) if tool_return.name == "run_code" => {
                Some(tool_return)
            }
            _ => None,
        })
        .unwrap();
    assert!(run_code_return.is_error);
    assert_eq!(
        run_code_return
            .content
            .pointer("/nested_tool_result/result/receipt/effect_status"),
        Some(&serde_json::json!("executed")),
        "unexpected terminal run_code return: {}",
        run_code_return.content
    );
    assert!(run_code_return.app_value.is_none());
    assert!(
        !run_code_return
            .private_metadata
            .contains_key("starweaver_geometry_bound_immutable_media")
    );
    assert!(
        !run_code_return
            .private_metadata
            .contains_key("starweaver_tool_return_content_parts")
    );
}

#[tokio::test]
async fn run_code_timeout_abandons_an_in_flight_nested_call_without_waiting_for_it() {
    let effects = Arc::new(AtomicUsize::new(0));
    let effects_for_tool = effects.clone();
    let mut metadata = Metadata::new();
    metadata.insert(
        TOOL_METADATA_DEPENDENCIES_KEY.to_string(),
        ToolDependencyRequirements::strict(Vec::<String>::new(), Vec::<String>::new(), false)
            .to_metadata_value(),
    );
    let target = Arc::new(
        FunctionTool::new(
            "slow_target",
            Some("Slow test target".to_string()),
            serde_json::json!({"type": "object"}),
            move |_context: ToolContext, _arguments: serde_json::Value| {
                let effects = effects_for_tool.clone();
                async move {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    effects.fetch_add(1, Ordering::SeqCst);
                    Ok(ToolResult::new(serde_json::json!({"completed": true})))
                }
            },
        )
        .with_metadata(metadata),
    ) as DynTool;
    let targets = Arc::new(StaticToolset::new("slow-targets").with_tool(target)) as DynToolset;
    let model = TestModel::with_responses(vec![
        tool_call_response(
            "code-timeout",
            "run_code",
            serde_json::json!({
                "source": "function main() { return tools.call(\"slow_target\", {}); }",
                "input": null
            }),
        ),
        ModelResponse::text("timeout observed"),
    ]);
    let codeact = CodeActConfig::default().with_limits(CodeExecutionLimits {
        timeout_ms: 20,
        ..CodeExecutionLimits::default()
    });
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(model))
        .toolset(&targets)
        .toolset(&codeact_tools(codeact))
        .build();

    let started_at = Instant::now();
    let result = runtime.run("time out the composition").await.unwrap();

    assert_eq!(result.output, "timeout observed");
    assert!(started_at.elapsed() < Duration::from_millis(500));
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    let run_code_return = result
        .state
        .message_history
        .iter()
        .filter_map(|message| match message {
            ModelMessage::Request(request) => Some(&request.parts),
            ModelMessage::Response(_) => None,
        })
        .flatten()
        .find_map(|part| match part {
            ModelRequestPart::ToolReturn(tool_return) if tool_return.name == "run_code" => {
                Some(tool_return)
            }
            _ => None,
        })
        .unwrap();
    assert!(run_code_return.is_error);
}

#[derive(Clone, Default)]
struct CapturingExecutor {
    sources: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl CodeExecutor for CapturingExecutor {
    async fn execute(
        &self,
        request: CodeExecutionRequest,
        _tools: NestedToolInvoker,
    ) -> Result<CodeExecutionResult, CodeExecutionError> {
        self.sources
            .lock()
            .unwrap()
            .push(request.source.to_string());
        Ok(CodeExecutionResult {
            value: request.input,
            source_digest: request.source_digest,
            duration_ms: 0,
        })
    }
}

#[tokio::test]
async fn recipe_preparation_pins_source_allowlist_and_validated_input_schema() {
    let original_source = "function main(input) { return input; }";
    let provider = Arc::new(
        VirtualEnvironmentProvider::new("recipes")
            .with_file(
                ".starweaver/recipes/demo/recipe.toml",
                r#"
version = 1
name = "demo_recipe"
description = "Pinned recipe"
source = "main.js"
tools = ["increment"]
input_schema = "input.schema.json"
"#,
            )
            .with_file(".starweaver/recipes/demo/main.js", original_source)
            .with_file(
                ".starweaver/recipes/demo/input.schema.json",
                r#"{
                    "type": "object",
                    "properties": {"amount": {"type": "integer"}},
                    "required": ["amount"],
                    "additionalProperties": false
                }"#,
            ),
    );
    let executor = CapturingExecutor::default();
    let mut context = AgentContext::default();
    attach_environment(&mut context, provider.clone());
    let toolset = RecipeToolset::new(Arc::new(executor.clone()));

    let preparation = toolset.prepare_with_context(&context).await.unwrap();
    assert_eq!(preparation.tools.len(), 1);
    let recipe = preparation.tools[0].clone();
    assert_eq!(recipe.name(), "demo_recipe");
    provider
        .write_text(
            ".starweaver/recipes/demo/main.js",
            "function main() { return 'changed'; }",
        )
        .await
        .unwrap();

    let (invoker, _receiver) =
        NestedToolInvoker::channel(BTreeSet::from(["increment".to_string()]), 1);
    let mut dependencies = DependencyStore::new();
    dependencies.insert(invoker);
    let tool_context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies);
    let result = recipe
        .call(tool_context.clone(), serde_json::json!({"amount": 2}))
        .await
        .unwrap();
    assert_eq!(result.content["value"], serde_json::json!({"amount": 2}));
    assert_eq!(
        executor.sources.lock().unwrap().as_slice(),
        &[original_source.to_string()]
    );

    let error = recipe
        .call(tool_context, serde_json::json!({"amount": "two"}))
        .await
        .unwrap_err();
    assert!(matches!(error, ToolError::InvalidArguments { .. }));
    assert_eq!(executor.sources.lock().unwrap().len(), 1);
}
