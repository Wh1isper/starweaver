# Structured Output

Starweaver supports structured JSON output through `OutputSchema`, typed parsing helpers, output validators, output functions, and `OutputPolicy`. For SDK users, `OutputPolicy` is the smooth path because it groups schema, validators, output functions, and retry budget into one object.

## Output schema

```rust
use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;
use starweaver_agent::{AgentBuilder, OutputSchema, TestModel};

#[derive(Debug, Deserialize, PartialEq)]
struct Answer {
    answer: String,
}

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let schema = OutputSchema::new(
    "answer",
    json!({
        "type": "object",
        "properties": {"answer": {"type": "string"}},
        "required": ["answer"]
    }),
);
let agent = AgentBuilder::new(Arc::new(TestModel::with_text(r#"{"answer":"Paris"}"#)))
    .output_schema(schema)
    .build();

let result = agent.run("Return JSON").await?;
let answer: Answer = result.structured()?;
assert_eq!(answer, Answer { answer: "Paris".to_string() });
# Ok(())
# }
```

## Validation and retry

Output validators can ask the model for a semantic retry. Runtime retry uses `RetryPrompt` and respects `AgentRuntimePolicy::output_retries`.

## Output policy

Use `OutputPolicy` when an application wants one reusable output contract for a builder, CLI command, or durable service profile.

```rust
use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;
use starweaver_agent::{AgentBuilder, OutputPolicy, OutputSchema, TestModel};
use starweaver_model::ModelResponse;

#[derive(Debug, Deserialize, PartialEq)]
struct Answer {
    answer: String,
}

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let output = OutputPolicy::structured(OutputSchema::new(
    "answer",
    json!({
        "type": "object",
        "properties": {"answer": {"type": "string"}},
        "required": ["answer"]
    }),
))
.with_retries(2);

let agent = AgentBuilder::new(Arc::new(TestModel::with_responses(vec![
    ModelResponse::text("text first"),
    ModelResponse::text(r#"{"answer":"Paris"}"#),
])))
.output_policy(output)
.build();

let result = agent.run("Return JSON").await?;
let answer: Answer = result.structured()?;
assert_eq!(answer, Answer { answer: "Paris".to_string() });
# Ok(())
# }
```

`OutputPolicy` keeps application construction concise while preserving runtime primitives. A service can store the selected policy name in its own configuration and still persist runtime `AgentCheckpoint` evidence independently.

When a model response contains both a valid output function call and ordinary
tool calls, `AgentRuntimePolicy::end_strategy` defines the boundary behavior.
`Early` is the default and completes immediately. `Graceful` and `Exhaustive`
execute ordinary tools from the same response, append their returns to message
history for future continuation, and still complete with the first valid output
function result without sending those tool returns to another model request.

## Typed output per run

Use `OutputPolicy::typed<T>()` when a Rust type should define the JSON Schema and parse target. `AgentRunOptions::output_policy` applies the contract to one run without mutating the reusable session agent.

```rust
use std::sync::Arc;

use schemars::JsonSchema;
use serde::Deserialize;
use starweaver_agent::{AgentBuilder, AgentRunOptions, OutputPolicy, TestModel};
use starweaver_model::ModelResponse;

#[derive(Debug, Deserialize, JsonSchema, PartialEq)]
struct Answer {
    answer: String,
}

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let mut session = AgentBuilder::new(Arc::new(TestModel::with_responses(vec![
    ModelResponse::text(r#"{"answer":"Paris"}"#),
    ModelResponse::text("plain text"),
])))
.build_app()
.session();

let result = session
    .run_with_options(
        "Return JSON",
        AgentRunOptions::new().output_policy(OutputPolicy::typed::<Answer>()),
    )
    .await?;
let answer: Answer = result.structured()?;
assert_eq!(answer, Answer { answer: "Paris".to_string() });

let plain = session.run("Return text").await?;
assert_eq!(plain.output, "plain text");
# Ok(())
# }
```

## Image and media outputs

Use `OutputPolicy::image()` when the provider can return generated images or files. `AgentResult::media_outputs()` returns typed wrappers for final response file parts, and `image_outputs()` filters that list to image media. Provider request preparation falls back to text output with diagnostic metadata when the active model profile does not support generated image output.

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, OutputMedia, OutputPolicy, TestModel};
use starweaver_model::{ModelResponse, ModelResponsePart};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let mut response = ModelResponse::text("generated image");
response.parts.push(ModelResponsePart::File {
    url: "resource://generated/image-1".to_string(),
    media_type: "image/png".to_string(),
});

let result = AgentBuilder::new(Arc::new(TestModel::with_responses(vec![response])))
    .output_policy(OutputPolicy::image())
    .build()
    .run("Draw a diagram")
    .await?;

assert_eq!(
    result.image_outputs(),
    vec![OutputMedia::new("resource://generated/image-1", "image/png")]
);
# Ok(())
# }
```
