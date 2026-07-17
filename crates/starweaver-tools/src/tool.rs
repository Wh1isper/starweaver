//! Tool trait, function-backed tools, and tool result values.

mod function;
mod result;
mod typed;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use starweaver_context::AgentContext;
use starweaver_core::Metadata;
use starweaver_model::ToolDefinition;

use crate::{ToolContext, ToolError};

pub use function::FunctionTool;
pub use result::{DynTool, ToolResult};
pub use typed::TypedFunctionTool;

/// Metadata key for capability tags attached to a tool definition.
pub const TOOL_METADATA_TAGS_KEY: &str = "tags";

/// Metadata key for active capability tags that should hide this tool.
pub const TOOL_METADATA_HIDDEN_BY_TAGS_KEY: &str = "hidden_by_tags";

/// Metadata key for provider-neutral tool kind taxonomy.
pub const TOOL_METADATA_KIND_KEY: &str = "starweaver_tool_kind";

/// Metadata key for tools that can actively reduce or refresh conversation context.
pub const TOOL_METADATA_CONTEXT_MANAGEMENT_KEY: &str = "starweaver_context_management";

/// Provider-neutral tool kind used by runtime, output, approval, deferred, and host adapters.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// Normal model-callable function tool.
    Function,
    /// Synthetic final-output tool.
    Output,
    /// Host or provider external tool.
    External,
    /// Tool call that requires human approval before execution.
    Unapproved,
    /// Tool call that is deferred to an external worker or later run.
    Deferred,
}

impl ToolKind {
    /// Return the stable metadata string for this kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Output => "output",
            Self::External => "external",
            Self::Unapproved => "unapproved",
            Self::Deferred => "deferred",
        }
    }

    /// Parse a stable metadata string.
    #[must_use]
    pub fn from_metadata_value(value: &str) -> Option<Self> {
        match value {
            "function" => Some(Self::Function),
            "output" => Some(Self::Output),
            "external" => Some(Self::External),
            "unapproved" => Some(Self::Unapproved),
            "deferred" => Some(Self::Deferred),
            _ => None,
        }
    }
}

/// Empty object arguments for tools without input fields.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct EmptyToolArgs {}

/// Result from preprocessing human input for an approved HITL tool call.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolUserInputPreprocessResult {
    /// Optional replacement arguments for the approved tool call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_arguments: Option<Value>,
    /// Additional approval metadata derived from the user input.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl ToolUserInputPreprocessResult {
    /// Build an empty preprocessing result.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach replacement arguments.
    #[must_use]
    pub fn with_override_arguments(mut self, arguments: Value) -> Self {
        self.override_arguments = Some(arguments);
        self
    }

    /// Attach metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Return whether no preprocessing changes were produced.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.override_arguments.is_none() && self.metadata.is_empty()
    }
}

fn normalized_tag(tag: impl Into<String>) -> Option<String> {
    let tag = tag.into();
    let tag = tag.trim();
    (!tag.is_empty()).then(|| tag.to_string())
}

fn read_tool_metadata_string_list(metadata: &Metadata, key: &str) -> Vec<String> {
    let Some(Value::Array(values)) = metadata.get(key) else {
        return Vec::new();
    };
    values
        .iter()
        .filter_map(Value::as_str)
        .filter_map(normalized_tag)
        .fold(Vec::new(), |mut tags, tag| {
            if !tags.contains(&tag) {
                tags.push(tag);
            }
            tags
        })
}

fn extend_tool_metadata_string_list<I, S>(metadata: &mut Metadata, key: &str, tags: I)
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut normalized = read_tool_metadata_string_list(metadata, key);
    for tag in tags.into_iter().filter_map(normalized_tag) {
        if !normalized.contains(&tag) {
            normalized.push(tag);
        }
    }
    if normalized.is_empty() {
        metadata.remove(key);
        return;
    }
    metadata.insert(
        key.to_string(),
        Value::Array(normalized.into_iter().map(Value::String).collect()),
    );
}

/// Read normalized capability tags from tool metadata.
#[must_use]
pub fn tool_metadata_tags(metadata: &Metadata) -> Vec<String> {
    read_tool_metadata_string_list(metadata, TOOL_METADATA_TAGS_KEY)
}

/// Read normalized capability tags that hide a tool from tool metadata.
#[must_use]
pub fn tool_metadata_hidden_by_tags(metadata: &Metadata) -> Vec<String> {
    read_tool_metadata_string_list(metadata, TOOL_METADATA_HIDDEN_BY_TAGS_KEY)
}

/// Read provider-neutral tool kind from tool metadata.
#[must_use]
pub fn tool_metadata_kind(metadata: &Metadata) -> Option<ToolKind> {
    metadata
        .get(TOOL_METADATA_KIND_KEY)
        .and_then(Value::as_str)
        .and_then(ToolKind::from_metadata_value)
}

/// Set provider-neutral tool kind on tool metadata.
pub fn set_tool_metadata_kind(metadata: &mut Metadata, kind: ToolKind) {
    metadata.insert(
        TOOL_METADATA_KIND_KEY.to_string(),
        Value::String(kind.as_str().to_string()),
    );
}

/// Add capability tags to tool metadata, trimming blanks and preserving first-seen order.
pub fn extend_tool_metadata_tags<I, S>(metadata: &mut Metadata, tags: I)
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    extend_tool_metadata_string_list(metadata, TOOL_METADATA_TAGS_KEY, tags);
}

/// Add capability tags that hide a tool, trimming blanks and preserving first-seen order.
pub fn extend_tool_metadata_hidden_by_tags<I, S>(metadata: &mut Metadata, tags: I)
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    extend_tool_metadata_string_list(metadata, TOOL_METADATA_HIDDEN_BY_TAGS_KEY, tags);
}

/// Metadata key for tools that own their HITL control flow and must not receive an outer approval gate.
pub const TOOL_METADATA_SELF_MANAGED_HITL_KEY: &str = "starweaver_self_managed_hitl";

/// Provider-neutral function tool trait.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name exposed to the model.
    fn name(&self) -> &str;

    /// Tool description.
    fn description(&self) -> Option<&str>;

    /// JSON schema for tool arguments.
    fn parameters_schema(&self) -> Value;

    /// Runtime metadata attached to this tool definition.
    fn metadata(&self) -> Metadata {
        Metadata::default()
    }

    /// Per-tool retry override.
    fn max_retries(&self) -> Option<usize> {
        None
    }

    /// Per-tool execution timeout in milliseconds.
    fn timeout_ms(&self) -> Option<u64> {
        None
    }

    /// JSON schema for successful tool results when known.
    fn return_schema(&self) -> Option<Value> {
        None
    }

    /// Provider strict schema preference for this tool.
    fn strict_schema(&self) -> Option<bool> {
        None
    }

    /// Sequential execution preference for this tool.
    fn sequential(&self) -> Option<bool> {
        None
    }

    /// Return whether this tool should be exposed for the current agent context.
    fn is_available(&self, _context: &AgentContext) -> bool {
        true
    }

    /// Execute a tool call.
    ///
    /// # Errors
    ///
    /// Returns an error when validation, approval, deferral, or execution fails.
    async fn call(&self, context: ToolContext, arguments: Value) -> Result<ToolResult, ToolError>;

    /// Preprocess host/user input supplied while approving a HITL tool call.
    ///
    /// Implementations can turn UI input into replacement tool arguments and approval metadata.
    /// The runtime still executes the approved tool through the ordinary call path, so argument
    /// validators and approval gates remain authoritative.
    ///
    /// # Errors
    ///
    /// Returns an error when the user input cannot be converted into a valid HITL decision.
    async fn preprocess_user_input(
        &self,
        _context: ToolContext,
        _user_input: Value,
    ) -> Result<ToolUserInputPreprocessResult, ToolError> {
        Ok(ToolUserInputPreprocessResult::default())
    }

    /// Convert this tool into a provider-neutral model definition.
    fn definition(&self) -> ToolDefinition {
        let mut metadata = self.metadata();
        if let Some(max_retries) = self.max_retries() {
            metadata.insert("max_retries".to_string(), serde_json::json!(max_retries));
        }
        if let Some(timeout_ms) = self.timeout_ms() {
            metadata.insert("timeout_ms".to_string(), serde_json::json!(timeout_ms));
        }
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().map(str::to_string),
            parameters: self.parameters_schema(),
            return_schema: self.return_schema(),
            strict: self.strict_schema(),
            sequential: self.sequential(),
            metadata,
        }
    }

    /// Prepare a model-facing definition for one agent context.
    ///
    /// Returning `None` hides the tool for this prepared request without changing
    /// the registry or execution dispatch table.
    fn prepare_definition(
        &self,
        _context: &AgentContext,
        definition: ToolDefinition,
    ) -> Option<ToolDefinition> {
        Some(definition)
    }
}

/// Create a JSON-returning tool from an async function over raw JSON arguments.
#[must_use]
pub fn json_tool<F, Fut>(
    name: impl Into<String>,
    description: impl Into<Option<String>>,
    parameters: Value,
    function: F,
) -> FunctionTool<impl Send + Sync + Fn(ToolContext, Value) -> Fut>
where
    F: Send + Sync + Fn(ToolContext, Value) -> Fut,
    Fut: Send + std::future::Future<Output = Result<ToolResult, ToolError>>,
{
    FunctionTool::new(name, description, parameters, function)
}

/// Create a JSON-returning tool from an async function over typed arguments.
#[must_use]
pub fn typed_json_tool<Args, F, Fut>(
    name: impl Into<String>,
    description: impl Into<Option<String>>,
    function: F,
) -> TypedFunctionTool<Args, impl Send + Sync + Fn(ToolContext, Args) -> Fut>
where
    Args: DeserializeOwned + JsonSchema + Send + 'static,
    F: Send + Sync + Fn(ToolContext, Args) -> Fut,
    Fut: Send + std::future::Future<Output = Result<ToolResult, ToolError>>,
{
    TypedFunctionTool::new(name, description, function)
}
