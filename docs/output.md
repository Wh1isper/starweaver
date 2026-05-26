# Structured Output

Starweaver supports structured JSON output through `OutputSchema`, typed parsing helpers, output validators, and output functions.

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
