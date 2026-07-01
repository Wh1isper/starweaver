#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Map, json};
use starweaver_context::{AgentContext, ToolAvailabilityPolicy, ToolConfig};
use starweaver_core::{ConversationId, RunId};
use starweaver_model::{
    CONTEXT_ORIGIN_METADATA, CONTEXT_ORIGIN_TOOL_RETURN_MEDIA, ContentPart, FunctionModel,
    INSTRUCTION_DYNAMIC_METADATA, INSTRUCTION_ORIGIN_AGENT, INSTRUCTION_ORIGIN_METADATA,
    INSTRUCTION_ORIGIN_TOOLSET, ModelMessage, ModelRequest, ModelRequestPart, ModelResponse,
    TestModel, ToolDefinition,
};
use starweaver_runtime::{
    Agent, AgentCapability, AgentError, AgentRunState, FunctionDynamicInstruction,
    FunctionOutputFunction, OutputFunctionContext, OutputFunctionDefinition, OutputValue,
};
use starweaver_tools::{
    DynTool, DynToolset, FunctionTool, StaticToolset, ToolContext, ToolInstruction, ToolKind,
    ToolRegistry, ToolResult, set_tool_metadata_kind,
};

struct ReorderToolsCapability;

#[async_trait]
impl AgentCapability for ReorderToolsCapability {
    async fn prepare_tools(
        &self,
        _state: &AgentRunState,
        mut tools: Vec<ToolDefinition>,
    ) -> starweaver_runtime::CapabilityResult<Vec<ToolDefinition>> {
        tools.reverse();
        Ok(tools)
    }
}

struct AddToolCapability;

#[async_trait]
impl AgentCapability for AddToolCapability {
    async fn prepare_tools(
        &self,
        _state: &AgentRunState,
        mut tools: Vec<ToolDefinition>,
    ) -> starweaver_runtime::CapabilityResult<Vec<ToolDefinition>> {
        tools.push(ToolDefinition {
            name: "added".to_string(),
            description: None,
            parameters: json!({"type": "object"}),
            return_schema: None,
            strict: None,
            sequential: None,
            metadata: Map::new(),
        });
        Ok(tools)
    }
}

struct RenameToolCapability;

#[async_trait]
impl AgentCapability for RenameToolCapability {
    async fn prepare_tools(
        &self,
        _state: &AgentRunState,
        mut tools: Vec<ToolDefinition>,
    ) -> starweaver_runtime::CapabilityResult<Vec<ToolDefinition>> {
        if let Some(tool) = tools.first_mut() {
            tool.name = "renamed".to_string();
        }
        Ok(tools)
    }
}

struct DropOutputToolCapability;

#[async_trait]
impl AgentCapability for DropOutputToolCapability {
    async fn prepare_tools(
        &self,
        _state: &AgentRunState,
        tools: Vec<ToolDefinition>,
    ) -> starweaver_runtime::CapabilityResult<Vec<ToolDefinition>> {
        Ok(tools
            .into_iter()
            .filter(|tool| tool.name != "final_answer")
            .collect())
    }
}

struct ChangeToolKindCapability;

#[async_trait]
impl AgentCapability for ChangeToolKindCapability {
    async fn prepare_tools(
        &self,
        _state: &AgentRunState,
        mut tools: Vec<ToolDefinition>,
    ) -> starweaver_runtime::CapabilityResult<Vec<ToolDefinition>> {
        if let Some(tool) = tools.first_mut() {
            set_tool_metadata_kind(&mut tool.metadata, ToolKind::Output);
        }
        Ok(tools)
    }
}

fn tool(name: &'static str) -> DynTool {
    Arc::new(FunctionTool::new(
        name,
        Some(format!("{name} tool")),
        json!({"type": "object"}),
        |_ctx: ToolContext, args| std::future::ready(Ok(ToolResult::new(args))),
    ))
}

fn media_tool(name: &'static str) -> DynTool {
    Arc::new(FunctionTool::new(
        name,
        Some(format!("{name} tool")),
        json!({"type": "object"}),
        |_ctx: ToolContext, args| {
            let mut private_metadata = Map::new();
            private_metadata.insert(
                "starweaver_tool_return_content_parts".to_string(),
                json!([{ "kind": "image_url", "url": "https://example.test/image.png" }]),
            );
            std::future::ready(Ok(
                ToolResult::new(args).with_private_metadata(private_metadata)
            ))
        },
    ))
}

fn final_answer_function() -> FunctionOutputFunction<
    impl Send
    + Sync
    + Fn(
        OutputFunctionContext,
        serde_json::Value,
    ) -> std::future::Ready<Result<OutputValue, starweaver_runtime::OutputValidationError>>,
> {
    FunctionOutputFunction::new(
        OutputFunctionDefinition::new("final_answer", json!({"type": "object"})),
        |_ctx, args| std::future::ready(Ok(OutputValue::Json(args))),
    )
}

#[tokio::test]
async fn prepare_tools_reordering_is_normalized_to_original_stable_order() {
    let model = FunctionModel::new(|_messages, _settings, info| {
        let names = info
            .params
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["alpha", "beta"]);
        Ok(ModelResponse::text("ok"))
    });
    let tools = ToolRegistry::new()
        .with_tool(tool("alpha"))
        .with_tool(tool("beta"));

    let result = Agent::new(Arc::new(model))
        .with_tools(tools)
        .with_capability(Arc::new(ReorderToolsCapability))
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
}

#[tokio::test]
async fn unavailable_tools_are_filtered_before_model_request() {
    let model = FunctionModel::new(|_messages, _settings, info| {
        let names = info
            .params
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["always"]);
        Ok(ModelResponse::text("ok"))
    });
    let gated = FunctionTool::new(
        "gated",
        Some("Context-gated tool".to_string()),
        json!({"type": "object"}),
        |_ctx: ToolContext, args| std::future::ready(Ok(ToolResult::new(args))),
    )
    .with_availability(|context| {
        context
            .metadata
            .get("enable_gated_tool")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    });
    let tools = ToolRegistry::new()
        .with_tool(tool("always"))
        .with_tool(Arc::new(gated));
    let mut context = AgentContext::default();

    let result = Agent::new(Arc::new(model))
        .with_tools(tools)
        .run_with_context("hello", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
    let Some(event) = context
        .events
        .events()
        .iter()
        .find(|event| event.kind == "tools_unavailable")
    else {
        panic!("tools_unavailable event");
    };
    assert_eq!(event.payload["available"], json!(["always"]));
    assert_eq!(event.payload["unavailable"], json!(["gated"]));
}

#[tokio::test]
async fn unavailable_tools_can_fail_closed_before_model_request() {
    let model = Arc::new(TestModel::with_text("should not run"));
    let gated = Arc::new(
        FunctionTool::new(
            "gated",
            Some("Context-gated tool".to_string()),
            json!({"type": "object"}),
            |_ctx: ToolContext, args| std::future::ready(Ok(ToolResult::new(args))),
        )
        .with_availability(|context| {
            context
                .metadata
                .get("enable_gated_tool")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        }),
    );
    let tools = ToolRegistry::new()
        .with_tool(tool("always"))
        .with_tool(gated);
    let mut context = AgentContext {
        tool_config: ToolConfig {
            unavailable_tool_policy: ToolAvailabilityPolicy::FailRun,
            ..ToolConfig::default()
        },
        ..AgentContext::default()
    };

    let error = Agent::new(model.clone())
        .with_tools(tools)
        .run_with_context("hello", &mut context)
        .await
        .unwrap_err();

    assert!(
        matches!(error, AgentError::Capability(message) if message.contains("unavailable tools rejected by policy") && message.contains("gated"))
    );
    assert!(model.captured_messages().is_empty());
    let Some(event) = context
        .events
        .events()
        .iter()
        .find(|event| event.kind == "tools_unavailable")
    else {
        panic!("tools_unavailable event");
    };
    assert_eq!(event.payload["available"], json!(["always"]));
    assert_eq!(event.payload["unavailable"], json!(["gated"]));
}

#[tokio::test]
async fn prepare_tools_cannot_add_or_rename_tools() {
    let model = Arc::new(FunctionModel::new(|_messages, _settings, _info| {
        Ok(ModelResponse::text("ok"))
    }));
    let tools = ToolRegistry::new().with_tool(tool("alpha"));

    let add_error = Agent::new(model.clone())
        .with_tools(tools.clone())
        .with_capability(Arc::new(AddToolCapability))
        .run("hello")
        .await
        .unwrap_err();
    assert!(
        matches!(add_error, AgentError::Capability(message) if message.contains("cannot add or rename"))
    );

    let rename_error = Agent::new(model)
        .with_tools(tools)
        .with_capability(Arc::new(RenameToolCapability))
        .run("hello")
        .await
        .unwrap_err();
    assert!(
        matches!(rename_error, AgentError::Capability(message) if message.contains("cannot add or rename"))
    );
}

#[tokio::test]
async fn regular_prepare_tools_cannot_remove_output_functions() {
    let model = Arc::new(FunctionModel::new(|_messages, _settings, _info| {
        Ok(ModelResponse::text("ok"))
    }));

    let error = Agent::new(model)
        .with_output_function(Arc::new(final_answer_function()))
        .with_capability(Arc::new(DropOutputToolCapability))
        .run("hello")
        .await
        .unwrap_err();

    assert!(
        matches!(error, AgentError::Capability(message) if message.contains("cannot remove output tool"))
    );
}

#[tokio::test]
async fn prepare_tools_cannot_change_tool_kind_metadata() {
    let model = Arc::new(FunctionModel::new(|_messages, _settings, _info| {
        Ok(ModelResponse::text("ok"))
    }));

    let error = Agent::new(model)
        .with_tools(ToolRegistry::new().with_tool(tool("alpha")))
        .with_capability(Arc::new(ChangeToolKindCapability))
        .run("hello")
        .await
        .unwrap_err();

    assert!(
        matches!(error, AgentError::Capability(message) if message.contains("cannot change tool kind"))
    );
}

#[tokio::test]
async fn duplicate_output_function_names_are_rejected() {
    let model = Arc::new(FunctionModel::new(|_messages, _settings, _info| {
        Ok(ModelResponse::text("ok"))
    }));

    let error = Agent::new(model)
        .with_output_function(Arc::new(final_answer_function()))
        .with_output_function(Arc::new(final_answer_function()))
        .run("hello")
        .await
        .unwrap_err();

    assert!(
        matches!(error, AgentError::Capability(message) if message.contains("duplicate output function name"))
    );
}

#[tokio::test]
async fn dynamic_instructions_preserve_static_system_prompt_prefix() {
    let captured = Arc::new(Mutex::new(Vec::<Vec<ModelMessage>>::new()));
    let model_captured = captured.clone();
    let model = FunctionModel::new(move |messages, _settings, _info| {
        model_captured.lock().unwrap().push(messages);
        Ok(ModelResponse::text("ok"))
    });
    let instruction = FunctionDynamicInstruction::new(|_state: AgentRunState| async move {
        Ok("dynamic policy".to_string())
    });

    let result = Agent::new(Arc::new(model))
        .with_instruction("static policy")
        .with_dynamic_instruction(Arc::new(instruction))
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
    let captured_messages = captured.lock().unwrap().clone();
    let latest_request = captured_messages[0]
        .iter()
        .rev()
        .find_map(|message| match message {
            ModelMessage::Request(request) => Some(request),
            ModelMessage::Response(_) => None,
        })
        .unwrap();
    assert!(matches!(
        latest_request.parts.first(),
        Some(ModelRequestPart::Instruction { text, metadata })
            if text == "static policy"
                && metadata.get(INSTRUCTION_ORIGIN_METADATA) == Some(&json!(INSTRUCTION_ORIGIN_AGENT))
                && metadata.get(INSTRUCTION_DYNAMIC_METADATA) == Some(&json!(false))
    ));
    let Some(dynamic_index) = latest_request.parts.iter().position(|part| {
        matches!(part, ModelRequestPart::Instruction { text, metadata }
            if text == "dynamic policy"
                && metadata.get("starweaver_instruction_dynamic") == Some(&json!(true)))
    }) else {
        panic!("dynamic instruction should be present");
    };
    assert!(dynamic_index > 0);
}

#[tokio::test]
async fn dynamic_instructions_do_not_split_tool_return_media_control_block() {
    let calls = Arc::new(Mutex::new(0usize));
    let captured = Arc::new(Mutex::new(Vec::<Vec<ModelMessage>>::new()));
    let model_calls = calls.clone();
    let model_captured = captured.clone();
    let model = FunctionModel::new(move |messages, _settings, _info| {
        model_captured.lock().unwrap().push(messages);
        let mut calls = model_calls.lock().unwrap();
        *calls += 1;
        if *calls == 1 {
            Ok(starweaver_model::tool_call_response(
                "call_1",
                "media",
                json!({"value": "first"}),
            ))
        } else {
            Ok(ModelResponse::text("done"))
        }
    });
    let instruction = FunctionDynamicInstruction::new(|state: AgentRunState| async move {
        Ok(format!("Dynamic step {}", state.run_step))
    });

    let result = Agent::new(Arc::new(model))
        .with_dynamic_instruction(Arc::new(instruction))
        .with_tools(ToolRegistry::new().with_tool(media_tool("media")))
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    let captured_messages = captured.lock().unwrap().clone();
    assert_eq!(captured_messages.len(), 2);
    let second_latest_request = captured_messages[1]
        .iter()
        .rev()
        .find_map(|message| match message {
            ModelMessage::Request(request) => Some(request),
            ModelMessage::Response(_) => None,
        })
        .unwrap();
    assert!(matches!(
        second_latest_request.parts.first(),
        Some(ModelRequestPart::ToolReturn(_))
    ));
    assert!(matches!(
        second_latest_request.parts.get(1),
        Some(ModelRequestPart::UserPrompt { content, metadata, .. })
            if metadata.get(CONTEXT_ORIGIN_METADATA) == Some(&json!(CONTEXT_ORIGIN_TOOL_RETURN_MEDIA))
                && content.iter().any(|part| matches!(part, ContentPart::ImageUrl { .. }))
    ));
    assert!(second_latest_request.parts.iter().skip(2).any(|part| {
        matches!(part, ModelRequestPart::Instruction { text, .. } if text == "Dynamic step 1")
    }));
}

#[tokio::test]
async fn dynamic_instructions_re_evaluate_each_model_request_and_history_is_append_only() {
    let calls = Arc::new(Mutex::new(0usize));
    let captured = Arc::new(Mutex::new(Vec::<Vec<ModelMessage>>::new()));
    let model_calls = calls.clone();
    let model_captured = captured.clone();
    let model = FunctionModel::new(move |messages, _settings, _info| {
        model_captured.lock().unwrap().push(messages.clone());
        let mut calls = model_calls.lock().unwrap();
        *calls += 1;
        let expected = format!("Dynamic step {}", *calls - 1);
        let latest_request = messages
            .iter()
            .rev()
            .find_map(|message| match message {
                ModelMessage::Request(request) => Some(request),
                ModelMessage::Response(_) => None,
            })
            .unwrap();
        assert!(latest_request.parts.iter().any(|part| {
            matches!(part, ModelRequestPart::Instruction { text, metadata }
                if text == &expected
                    && metadata.get("starweaver_instruction_dynamic") == Some(&json!(true))
                    && metadata.get("starweaver_instruction_origin") == Some(&json!("dynamic_instruction")))
        }));
        if *calls == 1 {
            Ok(starweaver_model::tool_call_response(
                "call_1",
                "echo",
                json!({"value": "first"}),
            ))
        } else {
            Ok(ModelResponse::text("done"))
        }
    });
    let instruction = FunctionDynamicInstruction::new(|state: AgentRunState| async move {
        Ok(format!("Dynamic step {}", state.run_step))
    });

    let result = Agent::new(Arc::new(model))
        .with_dynamic_instruction(Arc::new(instruction))
        .with_tools(ToolRegistry::new().with_tool(tool("echo")))
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    assert_eq!(*calls.lock().unwrap(), 2);
    assert_eq!(result.messages.len(), 4);
    assert!(matches!(result.messages[0], ModelMessage::Request(_)));
    assert!(matches!(result.messages[1], ModelMessage::Response(_)));
    assert!(matches!(result.messages[2], ModelMessage::Request(_)));
    assert!(matches!(result.messages[3], ModelMessage::Response(_)));
    let captured_messages = captured.lock().unwrap().clone();
    assert_eq!(captured_messages.len(), 2);
    assert!(format!("{:?}", captured_messages[0]).contains("Dynamic step 0"));
    assert!(format!("{:?}", captured_messages[1]).contains("Dynamic step 0"));
    assert!(format!("{:?}", captured_messages[1]).contains("Dynamic step 1"));
    let second_latest_request = captured_messages[1]
        .iter()
        .rev()
        .find_map(|message| match message {
            ModelMessage::Request(request) => Some(request),
            ModelMessage::Response(_) => None,
        })
        .unwrap();
    assert!(matches!(
        second_latest_request.parts.first(),
        Some(ModelRequestPart::ToolReturn(_))
    ));
    assert!(second_latest_request.parts.iter().skip(1).any(|part| {
        matches!(part, ModelRequestPart::Instruction { text, .. } if text == "Dynamic step 1")
    }));
    assert!(format!("{:?}", result.messages).contains("Dynamic step 0"));
    assert!(format!("{:?}", result.messages).contains("Dynamic step 1"));
    for message in result.new_messages() {
        match message {
            ModelMessage::Request(request) => {
                assert_eq!(request.run_id.as_ref(), Some(&result.state.run_id));
                assert_eq!(
                    request.conversation_id.as_ref(),
                    Some(&result.state.conversation_id)
                );
                assert!(request.timestamp.is_some());
            }
            ModelMessage::Response(response) => {
                assert_eq!(response.run_id.as_ref(), Some(&result.state.run_id));
                assert_eq!(
                    response.conversation_id.as_ref(),
                    Some(&result.state.conversation_id)
                );
                assert!(response.timestamp.is_some());
            }
        }
    }
}

#[tokio::test]
async fn static_instructions_are_reinjected_for_provider_request_and_current_session_history() {
    let captured = Arc::new(Mutex::new(Vec::<Vec<ModelMessage>>::new()));
    let model_captured = captured.clone();
    let model = FunctionModel::new(move |messages, _settings, _info| {
        model_captured.lock().unwrap().push(messages);
        Ok(ModelResponse::text("ok"))
    });
    let history = vec![ModelMessage::Request(ModelRequest {
        parts: vec![ModelRequestPart::UserPrompt {
            content: vec![starweaver_model::ContentPart::Text {
                text: "prior".to_string(),
            }],
            name: None,
            metadata: Map::new(),
        }],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Map::new(),
    })];

    let result = Agent::new(Arc::new(model))
        .with_instruction("stable server policy")
        .run_with_history("next", history)
        .await
        .unwrap();

    let provider_messages = captured.lock().unwrap()[0].clone();
    let Some(latest_request) = provider_messages
        .iter()
        .rev()
        .find_map(|message| match message {
            ModelMessage::Request(request) => Some(request),
            ModelMessage::Response(_) => None,
        })
    else {
        panic!("latest provider request");
    };
    assert!(latest_request.parts.iter().any(|part| matches!(
        part,
        ModelRequestPart::Instruction { text, metadata }
            if text == "stable server policy"
                && metadata.get(INSTRUCTION_ORIGIN_METADATA) == Some(&json!(INSTRUCTION_ORIGIN_AGENT))
                && metadata.get(INSTRUCTION_DYNAMIC_METADATA) == Some(&json!(false))
    )));
    assert!(format!("{provider_messages:?}").contains("stable server policy"));
    assert!(format!("{:?}", result.messages).contains("stable server policy"));
}

#[tokio::test]
async fn run_with_history_preserves_latest_conversation_id() {
    let conversation_id = ConversationId::from_string("conversation_from_history");
    let history = vec![ModelMessage::Request(ModelRequest {
        parts: vec![ModelRequestPart::UserPrompt {
            content: vec![starweaver_model::ContentPart::Text {
                text: "prior".to_string(),
            }],
            name: None,
            metadata: Map::new(),
        }],
        timestamp: None,
        instructions: None,
        run_id: Some(RunId::from_string("prior_run")),
        conversation_id: Some(conversation_id.clone()),
        metadata: Map::new(),
    })];
    let model = FunctionModel::new(|messages, _settings, _info| {
        let latest = messages
            .iter()
            .rev()
            .find_map(|message| match message {
                ModelMessage::Request(request) => Some(request),
                ModelMessage::Response(_) => None,
            })
            .unwrap();
        assert_eq!(
            latest.conversation_id.as_ref().map(ConversationId::as_str),
            Some("conversation_from_history")
        );
        Ok(ModelResponse::text("ok"))
    });

    let result = Agent::new(Arc::new(model))
        .run_with_history("next", history)
        .await
        .unwrap();

    assert_eq!(result.state.conversation_id, conversation_id);
}

#[tokio::test]
async fn toolset_instructions_are_static_by_default_for_prompt_cache_boundaries() {
    let model = FunctionModel::new(|_messages, _settings, info| {
        let Some(instruction) = info.params.instructions.first() else {
            panic!("toolset instruction missing");
        };
        assert!(!instruction.dynamic);
        assert_eq!(
            instruction.metadata.get("starweaver_instruction_origin"),
            Some(&json!(INSTRUCTION_ORIGIN_TOOLSET))
        );
        Ok(ModelResponse::text("ok"))
    });
    let toolset = StaticToolset::new("echo-set")
        .with_tool(tool("echo"))
        .with_instruction(ToolInstruction::new("echo-set", "Use echo."));
    let toolset: DynToolset = Arc::new(toolset);

    let result = Agent::new(Arc::new(model))
        .with_tools(ToolRegistry::new().with_toolset(&toolset))
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
}

#[tokio::test]
async fn toolset_instructions_can_be_marked_dynamic_for_prompt_cache_boundaries() {
    let model = FunctionModel::new(|_messages, _settings, info| {
        let Some(instruction) = info.params.instructions.first() else {
            panic!("toolset instruction missing");
        };
        assert!(instruction.dynamic);
        assert_eq!(
            instruction.metadata.get("starweaver_instruction_origin"),
            Some(&json!(INSTRUCTION_ORIGIN_TOOLSET))
        );
        Ok(ModelResponse::text("ok"))
    });
    let toolset = StaticToolset::new("echo-set")
        .with_tool(tool("echo"))
        .with_instruction(ToolInstruction::new("echo-set", "Use echo.").with_dynamic(true));
    let toolset: DynToolset = Arc::new(toolset);

    let result = Agent::new(Arc::new(model))
        .with_tools(ToolRegistry::new().with_toolset(&toolset))
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
}
