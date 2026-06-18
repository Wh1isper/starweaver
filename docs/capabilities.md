# Capabilities

Capabilities are runtime extension hooks. They can inspect context, annotate tools, mutate model requests, skip model calls, validate output, and observe completion.

```rust
use std::sync::Arc;

use async_trait::async_trait;
use starweaver_agent::{
    AgentBuilder, AgentCapability, AgentContext, AgentRunState, CapabilityResult, TestModel,
};

struct CompleteMarker;

#[async_trait]
impl AgentCapability for CompleteMarker {
    async fn on_run_complete_with_context(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
    ) -> CapabilityResult<()> {
        context.state.set(
            "last_output",
            serde_json::json!(state.output.clone().unwrap_or_default()),
        );
        Ok(())
    }
}

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let mut context = AgentContext::default();
let agent = AgentBuilder::new(Arc::new(TestModel::with_text("ok")))
    .capability(Arc::new(CompleteMarker))
    .build();

let result = agent.run_with_context("hello", &mut context).await?;
assert_eq!(result.output, "ok");
assert_eq!(context.state.get("last_output"), Some(&serde_json::json!("ok")));
# Ok(())
# }
```

Capability bundles can package hooks, instructions, tools, settings, output validators, output functions, history processors, and usage limits.

Run input that must be adjusted before the first model request belongs in `prepare_run_input_with_context`. This hook receives the initialized run state and context before request assembly, so it can rewrite the incoming `AgentInput` or add pending tool returns for durable resume paths that already have matching prior tool calls in history.

```rust
use std::sync::Arc;

use async_trait::async_trait;
use starweaver_agent::{
    AgentBuilder, AgentCapability, AgentInput, AgentRunState, CapabilityResult, TestModel,
};

struct PromptPrefix;

#[async_trait]
impl AgentCapability for PromptPrefix {
    async fn prepare_run_input(
        &self,
        _state: &mut AgentRunState,
        input: AgentInput,
    ) -> CapabilityResult<AgentInput> {
        Ok(AgentInput::text(format!(
            "Use the support policy.\n\n{}",
            input.content
                .iter()
                .filter_map(|part| match part {
                    starweaver_agent::ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        )))
    }
}

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let agent = AgentBuilder::new(Arc::new(TestModel::with_text("ok")))
    .capability(Arc::new(PromptPrefix))
    .build();

let result = agent.run("hello").await?;
assert_eq!(result.output, "ok");
# Ok(())
# }
```

Model-visible context belongs in `prepare_model_messages_with_context`, because mutations from that hook are captured in canonical session history and remain part of future prompt-cache prefixes. Runtime-owned context is installed by the runtime as the built-in `starweaver.runtime.context` capability in this same canonical pipeline. Use `prepare_provider_messages_with_context` only for provider-bound rewrites that must not be stored in session history.

Capabilities can order themselves around the built-in runtime context injector with `RUNTIME_CONTEXT_CAPABILITY_ID`:

```rust
use starweaver_agent::{CapabilityOrdering, CapabilitySpec, RUNTIME_CONTEXT_CAPABILITY_ID};

# fn example() {
let spec = CapabilitySpec::new("my.after_runtime_context")
    .with_ordering(CapabilityOrdering::default().after(RUNTIME_CONTEXT_CAPABILITY_ID));
# let _ = spec;
# }
```
