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
