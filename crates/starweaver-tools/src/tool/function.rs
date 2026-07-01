use async_trait::async_trait;
use serde_json::Value;
use starweaver_context::AgentContext;
use starweaver_core::Metadata;
use starweaver_model::ToolDefinition;
use std::{future::Future, pin::Pin, sync::Arc};

use crate::{ToolContext, ToolError};

use super::{
    Tool, ToolKind, ToolResult, ToolUserInputPreprocessResult, extend_tool_metadata_hidden_by_tags,
    extend_tool_metadata_tags, set_tool_metadata_kind,
};

type ArgumentValidator =
    Arc<dyn Fn(&ToolContext, &mut Value) -> Result<(), ToolError> + Send + Sync>;
type PrepareDefinition =
    Arc<dyn Fn(&AgentContext, ToolDefinition) -> Option<ToolDefinition> + Send + Sync>;
type UserInputPreprocessor = Arc<
    dyn Fn(
            ToolContext,
            Value,
        )
            -> Pin<Box<dyn Future<Output = Result<ToolUserInputPreprocessResult, ToolError>> + Send>>
        + Send
        + Sync,
>;

/// Function-backed tool with a caller-provided JSON schema.
pub struct FunctionTool<F> {
    name: String,
    description: Option<String>,
    parameters: Value,
    metadata: Metadata,
    max_retries: Option<usize>,
    timeout_ms: Option<u64>,
    return_schema: Option<Value>,
    strict_schema: Option<bool>,
    sequential: Option<bool>,
    is_available: Arc<dyn Fn(&AgentContext) -> bool + Send + Sync>,
    prepare_definition: PrepareDefinition,
    argument_validators: Vec<ArgumentValidator>,
    user_input_preprocessor: UserInputPreprocessor,
    function: F,
}

impl<F> FunctionTool<F> {
    /// Build a function-backed tool.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<Option<String>>,
        parameters: Value,
        function: F,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            metadata: Metadata::default(),
            max_retries: None,
            timeout_ms: None,
            return_schema: None,
            strict_schema: None,
            sequential: None,
            is_available: Arc::new(|_| true),
            prepare_definition: Arc::new(|_, definition| Some(definition)),
            argument_validators: Vec::new(),
            user_input_preprocessor: Arc::new(|_, _| {
                Box::pin(async { Ok(ToolUserInputPreprocessResult::default()) })
            }),
            function,
        }
    }

    /// Attach runtime metadata to this tool.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Set provider-neutral tool kind metadata.
    #[must_use]
    pub fn with_kind(mut self, kind: ToolKind) -> Self {
        set_tool_metadata_kind(&mut self.metadata, kind);
        self
    }

    /// Add one capability tag to this tool's provider-neutral metadata.
    #[must_use]
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        extend_tool_metadata_tags(&mut self.metadata, [tag.into()]);
        self
    }

    /// Add capability tags to this tool's provider-neutral metadata.
    #[must_use]
    pub fn with_tags<I, S>(mut self, tags: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        extend_tool_metadata_tags(&mut self.metadata, tags);
        self
    }

    /// Add one active capability tag that should hide this tool.
    #[must_use]
    pub fn with_hidden_by_tag(mut self, tag: impl Into<String>) -> Self {
        extend_tool_metadata_hidden_by_tags(&mut self.metadata, [tag.into()]);
        self
    }

    /// Add active capability tags that should hide this tool.
    #[must_use]
    pub fn with_hidden_by_tags<I, S>(mut self, tags: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        extend_tool_metadata_hidden_by_tags(&mut self.metadata, tags);
        self
    }

    /// Override the retry budget for this tool.
    #[must_use]
    pub const fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = Some(max_retries);
        self
    }

    /// Override the execution timeout for this tool.
    #[must_use]
    pub const fn with_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    /// Attach a JSON schema for successful tool results.
    #[must_use]
    pub fn with_return_schema(mut self, return_schema: Value) -> Self {
        self.return_schema = Some(return_schema);
        self
    }

    /// Set provider strict schema preference for this tool.
    #[must_use]
    pub const fn with_strict_schema(mut self, strict: bool) -> Self {
        self.strict_schema = Some(strict);
        self
    }

    /// Set sequential execution preference for this tool.
    #[must_use]
    pub const fn with_sequential(mut self, sequential: bool) -> Self {
        self.sequential = Some(sequential);
        self
    }

    /// Set a context-aware availability predicate.
    #[must_use]
    pub fn with_availability(
        mut self,
        is_available: impl Fn(&AgentContext) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.is_available = Arc::new(is_available);
        self
    }

    /// Set a context-aware model-facing definition prepare hook.
    #[must_use]
    pub fn with_prepare_definition(
        mut self,
        prepare_definition: impl Fn(&AgentContext, ToolDefinition) -> Option<ToolDefinition>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.prepare_definition = Arc::new(prepare_definition);
        self
    }

    /// Add an ordered raw-JSON argument validator.
    #[must_use]
    pub fn with_argument_validator(
        mut self,
        validator: impl Fn(&ToolContext, &mut Value) -> Result<(), ToolError> + Send + Sync + 'static,
    ) -> Self {
        self.argument_validators.push(Arc::new(validator));
        self
    }

    /// Set a HITL user-input preprocessor for approved tool calls.
    #[must_use]
    pub fn with_user_input_preprocessor<P, Fut>(mut self, preprocessor: P) -> Self
    where
        P: Fn(ToolContext, Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<ToolUserInputPreprocessResult, ToolError>> + Send + 'static,
    {
        self.user_input_preprocessor =
            Arc::new(move |context, user_input| Box::pin(preprocessor(context, user_input)));
        self
    }
}

#[async_trait]
impl<F, Fut> Tool for FunctionTool<F>
where
    F: Send + Sync + Fn(ToolContext, Value) -> Fut,
    Fut: Send + std::future::Future<Output = Result<ToolResult, ToolError>>,
{
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    fn parameters_schema(&self) -> Value {
        self.parameters.clone()
    }

    fn metadata(&self) -> Metadata {
        self.metadata.clone()
    }

    fn max_retries(&self) -> Option<usize> {
        self.max_retries
    }

    fn timeout_ms(&self) -> Option<u64> {
        self.timeout_ms
    }

    fn return_schema(&self) -> Option<Value> {
        self.return_schema.clone()
    }

    fn strict_schema(&self) -> Option<bool> {
        self.strict_schema
    }

    fn sequential(&self) -> Option<bool> {
        self.sequential
    }

    fn is_available(&self, context: &AgentContext) -> bool {
        (self.is_available)(context)
    }

    fn prepare_definition(
        &self,
        context: &AgentContext,
        definition: ToolDefinition,
    ) -> Option<ToolDefinition> {
        (self.prepare_definition)(context, definition)
    }

    async fn call(
        &self,
        context: ToolContext,
        mut arguments: Value,
    ) -> Result<ToolResult, ToolError> {
        for validator in &self.argument_validators {
            validator(&context, &mut arguments)?;
        }
        (self.function)(context, arguments).await
    }

    async fn preprocess_user_input(
        &self,
        context: ToolContext,
        user_input: Value,
    ) -> Result<ToolUserInputPreprocessResult, ToolError> {
        (self.user_input_preprocessor)(context, user_input).await
    }
}
