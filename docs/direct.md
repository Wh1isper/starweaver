# Direct APIs

Direct APIs expose model and tool calls with the same provider-neutral message and tool schema used by agents. They are useful for CLIs, service adapters, tests, and custom orchestration code that wants Starweaver's request mapping without the full agent loop.

## Direct model request

```rust
use starweaver_agent::{model_request, DirectModelRequest, TestModel};
use starweaver_model::{ContentPart, ModelMessage, ModelRequest, ModelRequestPart};

# async fn example() -> Result<(), starweaver_model::ModelError> {
let message = ModelMessage::Request(ModelRequest {
    parts: vec![ModelRequestPart::UserPrompt {
        content: vec![ContentPart::Text { text: "hello".to_string() }],
        name: None,
        metadata: serde_json::Map::new(),
    }],
    timestamp: None,
    instructions: None,
    run_id: None,
    conversation_id: None,
    metadata: serde_json::Map::new(),
});

let model = TestModel::with_text("ok");
let response = model_request(&model, DirectModelRequest::new(vec![message])).await?;
assert_eq!(response.text_output(), "ok");
# Ok(())
# }
```

## Direct stream evidence

`model_request_stream` returns canonical `ModelResponseStreamEvent` values. Provider adapters can emit part start, delta, part end, and final result events. The runtime also forwards these values inside `AgentStreamEvent::ModelStream` during `run_stream`.

```rust
use starweaver_agent::{model_request_stream, DirectModelRequest, TestModel};
use starweaver_model::{ContentPart, ModelMessage, ModelRequest, ModelRequestPart, ModelResponseStreamEvent};

# async fn example() -> Result<(), starweaver_model::ModelError> {
let message = ModelMessage::Request(ModelRequest {
    parts: vec![ModelRequestPart::UserPrompt {
        content: vec![ContentPart::Text { text: "hello".to_string() }],
        name: None,
        metadata: serde_json::Map::new(),
    }],
    timestamp: None,
    instructions: None,
    run_id: None,
    conversation_id: None,
    metadata: serde_json::Map::new(),
});

let model = TestModel::with_text("ok");
let events = model_request_stream(&model, DirectModelRequest::new(vec![message])).await?;
assert!(matches!(events.last(), Some(ModelResponseStreamEvent::FinalResult(_))));
# Ok(())
# }
```

## Direct tool call

```rust
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use starweaver_agent::{
    tool_call, typed_tool, ConversationId, RunId, ToolContext, ToolRegistry, ToolResult,
};
use starweaver_model::ToolCallPart;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
struct EchoArgs {
    value: i64,
}

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let tool = typed_tool::<EchoArgs, _, _>(
    "echo",
    Some("Echo arguments".to_string()),
    |_ctx: ToolContext, args: EchoArgs| async move {
        Ok(ToolResult::new(serde_json::json!({"value": args.value})))
    },
);
let tools = ToolRegistry::new().with_tool(Arc::new(tool));
let context = ToolContext::new(RunId::new(), ConversationId::new(), 0);
let call = ToolCallPart {
    id: "call_1".to_string(),
    name: "echo".to_string(),
    arguments: serde_json::json!({"value": 42}).into(),
};

let result = tool_call(&tools, context, &call).await;
assert_eq!(result.content["value"], 42);
# Ok(())
# }
```
