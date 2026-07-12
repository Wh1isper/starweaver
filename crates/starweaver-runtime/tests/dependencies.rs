#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use starweaver_context::{
    AgentContext, AgentContextHandle, CONTEXT_TASKS_CAPABILITY, HostCapabilities,
    ShellEnvironmentSnapshot, TaskContextHandle, ToolCapabilityGrant, ToolRuntimeSnapshot,
    ToolSearchContextHandle,
};
use starweaver_core::Metadata;
use starweaver_model::{
    ModelResponse, ModelResponsePart, TestModel, ToolCallPart, tool_call_response,
};
use starweaver_runtime::{
    Agent, AgentCapability, AgentRunState, CapabilityOrdering, CapabilityResult, CapabilitySpec,
};
use starweaver_tools::{
    FunctionTool, StaticToolset, TOOL_METADATA_DEPENDENCIES_KEY, ToolContext,
    ToolDependencyProfile, ToolDependencyRequirements, ToolRegistry, ToolResult,
    dynamic_tool_search,
};

#[derive(Debug, Eq, PartialEq)]
struct WeatherService {
    city: String,
}

struct ReplaceWeatherService;

#[async_trait]
impl AgentCapability for ReplaceWeatherService {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new("replace-weather")
    }

    async fn before_tool_execution_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        _tool_context: &mut ToolContext,
        _call: &ToolCallPart,
    ) -> CapabilityResult<()> {
        context.insert_named_dependency(
            "weather",
            WeatherService {
                city: "Berlin".to_string(),
            },
        );
        context
            .state
            .set("weather", serde_json::json!({"city": "Berlin"}));
        Ok(())
    }
}

struct ObserveWeatherService {
    observed: Arc<Mutex<Option<String>>>,
    observed_context: Arc<Mutex<Option<String>>>,
}

struct RecordFilteredContextTransitions;

#[async_trait]
impl AgentCapability for RecordFilteredContextTransitions {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new("record-filtered-context-transitions")
    }

    async fn before_tool_execution_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        _tool_context: &mut ToolContext,
        call: &ToolCallPart,
    ) -> CapabilityResult<()> {
        context
            .state
            .set(format!("prepared.{}", call.name), serde_json::json!(true));
        Ok(())
    }

    async fn after_tool_result_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        call: &ToolCallPart,
        _tool_return: &mut starweaver_model::ToolReturnPart,
    ) -> CapabilityResult<()> {
        context
            .state
            .set(format!("completed.{}", call.name), serde_json::json!(true));
        Ok(())
    }
}

#[async_trait]
impl AgentCapability for ObserveWeatherService {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new("observe-weather")
            .with_ordering(CapabilityOrdering::default().after("replace-weather"))
    }

    async fn before_tool_execution(
        &self,
        _state: &mut AgentRunState,
        tool_context: &mut ToolContext,
        _call: &ToolCallPart,
    ) -> CapabilityResult<()> {
        let city = tool_context
            .dependency::<HostCapabilities>()
            .and_then(|capabilities| capabilities.get::<WeatherService>())
            .map(|service| service.city.clone());
        *self.observed.lock().unwrap() = city;
        let context_city = tool_context
            .dependency::<AgentContextHandle>()
            .and_then(|handle| {
                handle
                    .snapshot()
                    .state
                    .get("weather")
                    .and_then(|value| value.get("city"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            });
        *self.observed_context.lock().unwrap() = context_city;
        Ok(())
    }
}

#[tokio::test]
async fn runtime_passes_typed_dependencies_to_tools() {
    let model = TestModel::with_responses(vec![
        tool_call_response("call_1", "weather", serde_json::json!({})),
        ModelResponse::text("done"),
    ]);
    let weather_tool = FunctionTool::new(
        "weather",
        Some("Return weather city".to_string()),
        serde_json::json!({"type": "object"}),
        |ctx: ToolContext, _args| async move {
            let service = ctx.dependency::<WeatherService>().unwrap();
            Ok(ToolResult::new(serde_json::json!({"city": service.city})))
        },
    );
    let tools = ToolRegistry::new().with_tool(Arc::new(weather_tool));
    let mut context = AgentContext::default();
    context.insert_dependency(WeatherService {
        city: "Paris".to_string(),
    });

    let result = Agent::new(Arc::new(model))
        .with_tools(tools)
        .run_with_context("weather", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    assert!(format!("{:?}", result.all_messages()).contains("Paris"));
}

#[tokio::test]
async fn runtime_refreshes_host_capabilities_after_before_tool_hooks() {
    let model = TestModel::with_responses(vec![
        tool_call_response("call_1", "weather", serde_json::json!({})),
        ModelResponse::text("done"),
    ]);
    let weather_tool = FunctionTool::new(
        "weather",
        Some("Return refreshed weather dependency".to_string()),
        serde_json::json!({"type": "object"}),
        |ctx: ToolContext, _args| async move {
            let capabilities = ctx.dependency::<HostCapabilities>().unwrap();
            let refreshed = capabilities.get::<WeatherService>().unwrap();
            let initial = ctx.dependency::<WeatherService>().unwrap();
            Ok(ToolResult::new(serde_json::json!({
                "initial": initial.city,
                "refreshed": refreshed.city,
            })))
        },
    );
    let mut context = AgentContext::default();
    context.insert_dependency(WeatherService {
        city: "Paris".to_string(),
    });
    let observed = Arc::new(Mutex::new(None));
    let observed_context = Arc::new(Mutex::new(None));

    let result = Agent::new(Arc::new(model))
        .with_tools(ToolRegistry::new().with_tool(Arc::new(weather_tool)))
        .with_capability(Arc::new(ReplaceWeatherService))
        .with_capability(Arc::new(ObserveWeatherService {
            observed: Arc::clone(&observed),
            observed_context: Arc::clone(&observed_context),
        }))
        .run_with_context("weather", &mut context)
        .await
        .unwrap();

    let messages = format!("{:?}", result.all_messages());
    assert!(!messages.contains("Paris"));
    assert!(messages.contains("Berlin"));
    assert_eq!(observed.lock().unwrap().as_deref(), Some("Berlin"));
    assert_eq!(observed_context.lock().unwrap().as_deref(), Some("Berlin"));
}

#[tokio::test]
async fn runtime_passes_named_dependencies_to_tools() {
    let model = TestModel::with_responses(vec![
        tool_call_response("call_1", "answer", serde_json::json!({})),
        ModelResponse::text("done"),
    ]);
    let tool = FunctionTool::new(
        "answer",
        Some("Return named dependency".to_string()),
        serde_json::json!({"type": "object"}),
        |ctx: ToolContext, _args| async move {
            let answer = ctx.named_dependency::<u32>("answer").unwrap();
            Ok(ToolResult::new(serde_json::json!({"answer": *answer})))
        },
    );
    let mut context = AgentContext::default();
    context.insert_named_dependency("answer", 42_u32);

    let result = Agent::new(Arc::new(model))
        .with_tools(ToolRegistry::new().with_tool(Arc::new(tool)))
        .run_with_context("answer", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    assert!(format!("{:?}", result.all_messages()).contains("42"));
}

#[tokio::test]
async fn runtime_passes_context_handle_and_narrow_tool_snapshot() {
    let model = TestModel::with_responses(vec![
        tool_call_response("call_1", "context_snapshot", serde_json::json!({})),
        ModelResponse::text("done"),
    ]);
    let full_context_injected = Arc::new(AtomicBool::new(true));
    let observed_full_context = Arc::clone(&full_context_injected);
    let tool = FunctionTool::new(
        "context_snapshot",
        Some("Return context state and note snapshots".to_string()),
        serde_json::json!({"type": "object"}),
        move |ctx: ToolContext, _args| {
            let observed_full_context = Arc::clone(&observed_full_context);
            async move {
                observed_full_context
                    .store(ctx.dependency::<AgentContext>().is_some(), Ordering::SeqCst);
                let context_handle = ctx.dependency::<AgentContextHandle>().unwrap();
                let agent_context = context_handle.snapshot();
                let runtime = ctx.dependency::<ToolRuntimeSnapshot>().unwrap();
                Ok(ToolResult::new(serde_json::json!({
                    "workspace": agent_context.state.get("workspace").unwrap()["root"],
                    "language": agent_context.notes.get("language").unwrap(),
                    "fetch_stream_chunk_size": runtime.tool_config().fetch_stream_chunk_size,
                })))
            }
        },
    );
    let mut context = AgentContext::default();
    context
        .state
        .set("workspace", serde_json::json!({"root": "/repo"}));
    context.notes.set("language", "Chinese");
    context.tool_config.fetch_stream_chunk_size = 12_345;

    let result = Agent::new(Arc::new(model))
        .with_tools(ToolRegistry::new().with_tool(Arc::new(tool)))
        .run_with_context("context", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    let messages = format!("{:?}", result.all_messages());
    assert!(messages.contains("/repo"));
    assert!(messages.contains("Chinese"));
    assert!(messages.contains("12345"));
    assert!(!full_context_injected.load(Ordering::SeqCst));
}

#[derive(Debug, Eq, PartialEq)]
struct UnrelatedService {
    value: String,
}

#[tokio::test]
async fn filtered_tool_retains_ambient_dependencies_and_filters_generated_projections() {
    let model = TestModel::with_responses(vec![
        tool_call_response("call_1", "filtered_weather", serde_json::json!({})),
        ModelResponse::text("done"),
    ]);
    let requirements = ToolDependencyRequirements::filtered(["weather"], false);
    assert_eq!(requirements.profile, ToolDependencyProfile::Filtered);
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        TOOL_METADATA_DEPENDENCIES_KEY.to_string(),
        requirements.to_metadata_value(),
    );
    let tool = FunctionTool::new(
        "filtered_weather",
        Some("Return filtered dependency evidence".to_string()),
        serde_json::json!({"type": "object"}),
        |ctx: ToolContext, _args| async move {
            assert!(ctx.dependency::<AgentContextHandle>().is_none());
            let runtime = ctx.dependency::<ToolRuntimeSnapshot>().unwrap();
            assert!(runtime.shell_environment().is_empty());
            assert!(ctx.dependency::<ShellEnvironmentSnapshot>().is_none());
            assert_eq!(ctx.dependency::<WeatherService>().unwrap().city, "Berlin");
            assert_eq!(
                ctx.dependency::<UnrelatedService>().unwrap().value,
                "ambient-compatible"
            );
            let host = ctx.dependency::<HostCapabilities>().unwrap();
            let refreshed_weather = host.get::<WeatherService>().unwrap();
            assert!(host.get::<UnrelatedService>().is_none());
            assert_eq!(host.keys(), vec!["weather".to_string()]);
            Ok(ToolResult::new(serde_json::json!({
                "refreshed_weather": refreshed_weather.city,
            })))
        },
    )
    .with_metadata(metadata);
    let mut context = AgentContext::default();
    context.insert_named_dependency(
        "weather",
        WeatherService {
            city: "Paris".to_string(),
        },
    );
    context.insert_dependency(UnrelatedService {
        value: "ambient-compatible".to_string(),
    });
    context.tools.shell_environment.insert(
        "STARWEAVER_SECRET".to_string(),
        "STARWEAVER_SECRET_SENTINEL_7f9c".to_string(),
    );

    let result = Agent::new(Arc::new(model))
        .with_tools(ToolRegistry::new().with_tool(Arc::new(tool)))
        .with_capability(Arc::new(ReplaceWeatherService))
        .run_with_context("weather", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    let messages = format!("{:?}", result.all_messages());
    assert!(!messages.contains("Paris"));
    assert!(messages.contains("Berlin"));
    assert!(!messages.contains("ambient-compatible"));
    assert!(!messages.contains("STARWEAVER_SECRET_SENTINEL_7f9c"));
}

#[tokio::test]
async fn direct_tool_search_uses_only_its_narrow_mutation_handle() {
    let model = TestModel::with_responses(vec![
        tool_call_response(
            "call_search",
            "tool_search",
            serde_json::json!({"query": "docs"}),
        ),
        tool_call_response(
            "call_lookup",
            "lookup_docs",
            serde_json::json!({"topic": "agents"}),
        ),
        ModelResponse::text("done"),
    ]);
    let lookup = FunctionTool::new(
        "lookup_docs",
        Some("Look up documentation".to_string()),
        serde_json::json!({"type": "object"}),
        |ctx: ToolContext, _args| async move {
            assert!(ctx.dependency::<AgentContextHandle>().is_none());
            assert!(ctx.dependency::<ToolSearchContextHandle>().is_some());
            Ok(ToolResult::new(serde_json::json!({"found": true})))
        },
    );
    let search = dynamic_tool_search(vec![Arc::new(
        StaticToolset::new("docs")
            .with_id("docs")
            .with_tool(Arc::new(lookup)),
    )]);
    let mut registry = ToolRegistry::new();
    registry.insert_toolset(&search);
    let mut context = AgentContext::default();
    context.insert_dependency(UnrelatedService {
        value: "must-not-reach-search-tools".to_string(),
    });

    let result = Agent::new(Arc::new(model))
        .with_tools(registry)
        .run_with_context("find docs", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    assert_eq!(context.tools.loaded_tool_namespaces, vec!["docs"]);
    assert!(
        context
            .events
            .events()
            .iter()
            .any(|event| event.kind == "tool_search_loaded")
    );
    assert!(!format!("{:?}", result.all_messages()).contains("must-not-reach-search-tools"));
}

#[tokio::test]
async fn strict_tool_receives_only_authorized_host_and_context_grants() {
    let model = TestModel::with_responses(vec![
        tool_call_response("call_1", "strict_weather", serde_json::json!({})),
        ModelResponse::text("done"),
    ]);
    let requirements =
        ToolDependencyRequirements::strict(["weather"], [CONTEXT_TASKS_CAPABILITY], false);
    let metadata = serde_json::Map::from_iter([(
        TOOL_METADATA_DEPENDENCIES_KEY.to_string(),
        requirements.to_metadata_value(),
    )]);
    let tool = FunctionTool::new(
        "strict_weather",
        Some("Inspect strict grants".to_string()),
        serde_json::json!({"type": "object"}),
        |ctx: ToolContext, _args| async move {
            assert!(ctx.dependency::<AgentContextHandle>().is_none());
            assert!(ctx.dependency::<WeatherService>().is_none());
            assert!(ctx.dependency::<UnrelatedService>().is_none());
            let host = ctx.dependency::<HostCapabilities>().unwrap();
            assert_eq!(host.keys(), vec!["weather".to_string()]);
            assert_eq!(host.get::<WeatherService>().unwrap().city, "Paris");
            let tasks = ctx.dependency::<TaskContextHandle>().unwrap();
            tasks.update(|manager| {
                manager.create(
                    "strict task",
                    "created through a capability-specific grant",
                    None,
                    Metadata::default(),
                )
            });
            Ok(ToolResult::new(serde_json::json!({"strict": true})))
        },
    )
    .with_metadata(metadata);
    let mut context = AgentContext::default();
    context.insert_named_dependency(
        "weather",
        WeatherService {
            city: "Paris".to_string(),
        },
    );
    context.insert_dependency(UnrelatedService {
        value: "must-not-leak".to_string(),
    });
    context.grant_tool_capabilities(
        "strict_weather",
        ToolCapabilityGrant::new()
            .with_host_capabilities(["weather"])
            .with_context_capabilities([CONTEXT_TASKS_CAPABILITY]),
    );

    let result = Agent::new(Arc::new(model))
        .with_tools(ToolRegistry::new().with_tool(Arc::new(tool)))
        .run_with_context("strict", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    assert_eq!(context.tasks().len(), 1);
    assert_eq!(context.tasks()[0].subject, "strict task");
}

#[tokio::test]
async fn strict_tool_requests_are_denied_without_host_grants() {
    let model = TestModel::with_responses(vec![
        tool_call_response("call_1", "strict_denied", serde_json::json!({})),
        ModelResponse::text("done"),
    ]);
    let requirements =
        ToolDependencyRequirements::strict(["weather"], [CONTEXT_TASKS_CAPABILITY], true);
    let metadata = serde_json::Map::from_iter([(
        TOOL_METADATA_DEPENDENCIES_KEY.to_string(),
        requirements.to_metadata_value(),
    )]);
    let tool = FunctionTool::new(
        "strict_denied",
        Some("Verify denied strict grants".to_string()),
        serde_json::json!({"type": "object"}),
        |ctx: ToolContext, _args| async move {
            let host = ctx.dependency::<HostCapabilities>().unwrap();
            assert!(host.get::<WeatherService>().is_none());
            assert!(ctx.dependency::<TaskContextHandle>().is_none());
            assert!(ctx.dependency::<ShellEnvironmentSnapshot>().is_none());
            Ok(ToolResult::new(serde_json::json!({"denied": true})))
        },
    )
    .with_metadata(metadata);
    let mut context = AgentContext::default();
    context.insert_named_dependency(
        "weather",
        WeatherService {
            city: "Paris".to_string(),
        },
    );
    context
        .tools
        .shell_environment
        .insert("SECRET".to_string(), "must-not-leak".to_string());

    let result = Agent::new(Arc::new(model))
        .with_tools(ToolRegistry::new().with_tool(Arc::new(tool)))
        .run_with_context("strict", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    assert!(context.tasks().is_empty());
}

async fn run_mixed_profile_batch(filtered_first: bool) -> AgentContext {
    let calls = if filtered_first {
        [("call_filtered", "filtered"), ("call_legacy", "legacy")]
    } else {
        [("call_legacy", "legacy"), ("call_filtered", "filtered")]
    };
    let model = TestModel::with_responses(vec![
        ModelResponse {
            parts: calls
                .into_iter()
                .map(|(id, name)| {
                    ModelResponsePart::ToolCall(ToolCallPart {
                        id: id.to_string(),
                        name: name.to_string(),
                        arguments: serde_json::json!({}).into(),
                    })
                })
                .collect(),
            ..ModelResponse::text("")
        },
        ModelResponse::text("done"),
    ]);
    let metadata = serde_json::Map::from_iter([(
        TOOL_METADATA_DEPENDENCIES_KEY.to_string(),
        ToolDependencyRequirements::filtered(std::iter::empty::<String>(), false)
            .to_metadata_value(),
    )]);
    let filtered = FunctionTool::new(
        "filtered",
        Some("Filtered tool".to_string()),
        serde_json::json!({"type": "object"}),
        |ctx: ToolContext, _args| async move {
            assert!(ctx.dependency::<AgentContextHandle>().is_none());
            Ok(ToolResult::new(serde_json::json!({"done": "filtered"})))
        },
    )
    .with_metadata(metadata);
    let legacy = FunctionTool::new(
        "legacy",
        Some("Legacy tool".to_string()),
        serde_json::json!({"type": "object"}),
        |ctx: ToolContext, _args| async move {
            assert!(ctx.dependency::<AgentContextHandle>().is_some());
            Ok(ToolResult::new(serde_json::json!({"done": "legacy"})))
        },
    );
    let mut context = AgentContext::default();

    let result = Agent::new(Arc::new(model))
        .with_tools(
            ToolRegistry::new()
                .with_tool(Arc::new(filtered))
                .with_tool(Arc::new(legacy)),
        )
        .with_capability(Arc::new(RecordFilteredContextTransitions))
        .run_with_context("run both", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    context
}

fn assert_mixed_profile_context_transitions(context: &AgentContext) {
    for key in [
        "prepared.filtered",
        "prepared.legacy",
        "completed.filtered",
        "completed.legacy",
    ] {
        assert_eq!(context.state.get(key), Some(&serde_json::json!(true)));
    }
}

#[tokio::test]
async fn distinct_legacy_tools_run_sequentially_without_snapshot_rollback() {
    let model = TestModel::with_responses(vec![
        ModelResponse {
            parts: vec![
                ModelResponsePart::ToolCall(ToolCallPart {
                    id: "call_alpha".to_string(),
                    name: "legacy_alpha".to_string(),
                    arguments: serde_json::json!({}).into(),
                }),
                ModelResponsePart::ToolCall(ToolCallPart {
                    id: "call_beta".to_string(),
                    name: "legacy_beta".to_string(),
                    arguments: serde_json::json!({}).into(),
                }),
            ],
            ..ModelResponse::text("")
        },
        ModelResponse::text("done"),
    ]);
    let legacy_tool = |name: &'static str, key: &'static str| {
        Arc::new(FunctionTool::new(
            name,
            Some("Mutate one legacy context key".to_string()),
            serde_json::json!({"type": "object"}),
            move |ctx: ToolContext, _args| async move {
                ctx.dependency::<AgentContextHandle>()
                    .unwrap()
                    .update(|context| context.state.set(key, serde_json::json!(true)));
                Ok(ToolResult::new(serde_json::json!({"done": true})))
            },
        ))
    };
    let mut context = AgentContext::default();

    let result = Agent::new(Arc::new(model))
        .with_tools(
            ToolRegistry::new()
                .with_tool(legacy_tool("legacy_alpha", "legacy.alpha"))
                .with_tool(legacy_tool("legacy_beta", "legacy.beta")),
        )
        .run_with_context("run legacy", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    assert_eq!(
        context.state.get("legacy.alpha"),
        Some(&serde_json::json!(true))
    );
    assert_eq!(
        context.state.get("legacy.beta"),
        Some(&serde_json::json!(true))
    );
}

#[tokio::test]
async fn filtered_then_legacy_batch_runs_sequentially_without_context_rollback() {
    let context = run_mixed_profile_batch(true).await;
    assert_mixed_profile_context_transitions(&context);
}

#[tokio::test]
async fn legacy_then_filtered_batch_runs_sequentially_without_context_rollback() {
    let context = run_mixed_profile_batch(false).await;
    assert_mixed_profile_context_transitions(&context);
}

#[tokio::test]
async fn parallel_filtered_tools_do_not_absorb_stale_internal_context_snapshots() {
    let model = TestModel::with_responses(vec![
        ModelResponse {
            parts: vec![
                ModelResponsePart::ToolCall(ToolCallPart {
                    id: "call_alpha".to_string(),
                    name: "filtered_alpha".to_string(),
                    arguments: serde_json::json!({}).into(),
                }),
                ModelResponsePart::ToolCall(ToolCallPart {
                    id: "call_beta".to_string(),
                    name: "filtered_beta".to_string(),
                    arguments: serde_json::json!({}).into(),
                }),
            ],
            ..ModelResponse::text("")
        },
        ModelResponse::text("done"),
    ]);
    let requirements = ToolDependencyRequirements::filtered(std::iter::empty::<String>(), false);
    let metadata = serde_json::Map::from_iter([(
        TOOL_METADATA_DEPENDENCIES_KEY.to_string(),
        requirements.to_metadata_value(),
    )]);
    let alpha = FunctionTool::new(
        "filtered_alpha",
        Some("Complete alpha".to_string()),
        serde_json::json!({"type": "object"}),
        |ctx: ToolContext, _args| async move {
            assert!(ctx.dependency::<AgentContextHandle>().is_none());
            Ok(ToolResult::new(serde_json::json!({"done": "alpha"})))
        },
    )
    .with_metadata(metadata.clone());
    let beta = FunctionTool::new(
        "filtered_beta",
        Some("Complete beta".to_string()),
        serde_json::json!({"type": "object"}),
        |ctx: ToolContext, _args| async move {
            assert!(ctx.dependency::<AgentContextHandle>().is_none());
            Ok(ToolResult::new(serde_json::json!({"done": "beta"})))
        },
    )
    .with_metadata(metadata);
    let mut context = AgentContext::default();

    let result = Agent::new(Arc::new(model))
        .with_tools(
            ToolRegistry::new()
                .with_tool(Arc::new(alpha))
                .with_tool(Arc::new(beta)),
        )
        .with_capability(Arc::new(RecordFilteredContextTransitions))
        .run_with_context("run both", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    for key in [
        "prepared.filtered_alpha",
        "prepared.filtered_beta",
        "completed.filtered_alpha",
        "completed.filtered_beta",
    ] {
        assert_eq!(context.state.get(key), Some(&serde_json::json!(true)));
    }
}

#[tokio::test]
async fn filtered_tool_receives_shell_values_only_through_dedicated_projection() {
    let model = TestModel::with_responses(vec![
        tool_call_response("call_1", "filtered_shell", serde_json::json!({})),
        ModelResponse::text("done"),
    ]);
    let requirements = ToolDependencyRequirements::filtered(std::iter::empty::<String>(), true);
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        TOOL_METADATA_DEPENDENCIES_KEY.to_string(),
        requirements.to_metadata_value(),
    );
    let tool = FunctionTool::new(
        "filtered_shell",
        Some("Inspect dedicated shell projection".to_string()),
        serde_json::json!({"type": "object"}),
        |ctx: ToolContext, _args| async move {
            assert!(ctx.dependency::<AgentContextHandle>().is_none());
            let runtime = ctx.dependency::<ToolRuntimeSnapshot>().unwrap();
            assert!(runtime.shell_environment().is_empty());
            let shell = ctx.dependency::<ShellEnvironmentSnapshot>().unwrap();
            assert_eq!(
                shell.environment()["STARWEAVER_SECRET"],
                "STARWEAVER_SECRET_SENTINEL_7f9c"
            );
            let debug = format!("{shell:?}");
            assert!(debug.contains("STARWEAVER_SECRET"));
            assert!(!debug.contains("STARWEAVER_SECRET_SENTINEL_7f9c"));
            Ok(ToolResult::new(serde_json::json!({"available": true})))
        },
    )
    .with_metadata(metadata);
    let mut context = AgentContext::default();
    context.tools.shell_environment.insert(
        "STARWEAVER_SECRET".to_string(),
        "STARWEAVER_SECRET_SENTINEL_7f9c".to_string(),
    );

    let result = Agent::new(Arc::new(model))
        .with_tools(ToolRegistry::new().with_tool(Arc::new(tool)))
        .run_with_context("shell", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    assert!(!format!("{:?}", result.all_messages()).contains("STARWEAVER_SECRET_SENTINEL_7f9c"));
}
