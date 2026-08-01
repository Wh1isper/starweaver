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
    AgentContext, AgentRuntimeBuilder, CodeActConfig, CodeExecutionError, CodeExecutionLimits,
    CodeExecutionRequest, CodeExecutionResult, CodeExecutor, RecipeToolset, TestModel, ToolContext,
    ToolError, ToolResult, attach_environment, codeact_tools,
};
use starweaver_context::DependencyStore;
use starweaver_core::{ConversationId, Metadata, RunId};
use starweaver_environment::{EnvironmentProvider, VirtualEnvironmentProvider};
use starweaver_model::{ModelMessage, ModelRequestPart, ModelResponse, tool_call_response};
use starweaver_tools::{
    DynTool, DynToolset, FunctionTool, NestedToolInvoker, StaticToolset,
    TOOL_METADATA_DEPENDENCIES_KEY, ToolDependencyRequirements, Toolset,
};

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
