# Agents

An agent combines a model, instructions, optional tools, output policy, capability hooks, and runtime policy.

## Basic agent

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, TestModel};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let agent = AgentBuilder::new(Arc::new(TestModel::with_text("ok")))
    .instruction("Answer with short responses.")
    .build();

let result = agent.run("Say ok").await?;
assert_eq!(result.output, "ok");
# Ok(())
# }
```

## Multimodal input

Use `AgentInput` and `ContentPart` when a prompt includes text plus media or resource references.

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, AgentInput, ContentPart, TestModel};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let app = AgentBuilder::new(Arc::new(TestModel::with_text("ok"))).build_app();

let input = AgentInput::parts(vec![
    ContentPart::text("Describe these assets."),
    ContentPart::image_url("https://example.test/image.png"),
    ContentPart::file_url("https://example.test/spec.pdf", "application/pdf"),
    ContentPart::image_bytes([1_u8, 2, 3], "image/png"),
    ContentPart::resource_ref("resource://workspace/doc-1", "application/pdf", "document"),
]);

let result = app.run(input).await?;
assert_eq!(result.output, "ok");
# Ok(())
# }
```

## Agent builder responsibilities

`AgentBuilder` lives in `starweaver-agent` and produces a `starweaver-runtime::Agent`. It can configure:

- static and dynamic instructions
- model settings and request parameters
- tools and toolsets
- structured output schemas and validators
- output functions
- usage limits
- history processors
- capability hooks and bundles
- runtime policy

## Rust mapping for decorator-style APIs

Starweaver uses builder methods and typed helper structs instead of Python decorators. The mapping is explicit:

- Python `@agent.instructions` or `@agent.system_prompt`: `AgentBuilder::instruction(...)` for static text, or `AgentBuilder::dynamic_instruction(Arc::new(FunctionDynamicInstruction::new(...)))` when the instruction depends on run state.
- Python-style system prompt template variables: `AgentBuilder::try_instruction_template(...)`
  renders `{{path.to.value}}` placeholders from serializable Rust data before adding the static instruction.
- Python `@agent.tool` or `@agent.tool_plain`: `typed_tool::<Args, _, _>(...)` for Serde and `schemars` validated structs, or `string_tool(...)` / `FunctionTool` when the schema is supplied manually.
- Python tool docstring extraction: Rust doc comments on `#[derive(JsonSchema)]` argument fields, or explicit `#[schemars(description = "...")]` attributes.
- Python `@agent.output_validator`: `AgentBuilder::output_validator(Arc::new(FunctionOutputValidator::new(...)))`.

```rust
use std::{future::ready, sync::Arc};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use starweaver_agent::{
    typed_tool, AgentBuilder, AgentRunState, FunctionDynamicInstruction,
    FunctionOutputValidator, OutputValidationError, OutputValidationResult, OutputValue,
    TestModel, ToolContext, ToolResult,
};

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
struct LookupArgs {
    /// Search query submitted by the model.
    query: String,
}

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let dynamic_instruction =
    FunctionDynamicInstruction::new(|_state: AgentRunState| async move {
        Ok("Prefer concise answers.".to_string())
    });

let output_validator =
    FunctionOutputValidator::new(|_state: &mut AgentRunState, output: &OutputValue| {
        let result: OutputValidationResult<()> = if output.as_text().is_empty() {
            Err(OutputValidationError::retry("return a non-empty answer"))
        } else {
            Ok(())
        };
        ready(result)
    });

let lookup = typed_tool::<LookupArgs, _, _>(
    "lookup",
    Some("Lookup a value".to_string()),
    |_ctx: ToolContext, args: LookupArgs| async move {
        Ok(ToolResult::new(serde_json::json!({"value": args.query})))
    },
);

let agent = AgentBuilder::new(Arc::new(TestModel::with_text("ready")))
    .instruction("Answer with short responses.")
    .dynamic_instruction(Arc::new(dynamic_instruction))
    .output_validator(Arc::new(output_validator))
    .tool(Arc::new(lookup))
    .build();

let result = agent.run("Say ready").await?;
assert_eq!(result.output, "ready");
# Ok(())
# }
```

## Runtime result

`AgentResult` contains:

- `output`: final text
- `structured_output`: parsed JSON value when configured
- `messages`: canonical message history
- `state`: checkpointable run state
- `history_len`: prior history length

Use `new_messages()` to read messages produced by the current run.
