//! Fixed two-tool proxy over many underlying toolsets.

mod format;
mod index;
mod inner;

use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::{typed_json_tool, DynTool, DynToolset, ToolInstruction, Toolset};

use inner::ToolProxyInner;

const SEARCH_TOOLS_NAME: &str = "search_tools";
const CALL_TOOL_NAME: &str = "call_tool";
const PREFIXED_SEARCH_TOOL_SUFFIX: &str = "search_tool";
const PREFIXED_CALL_TOOL_SUFFIX: &str = "call_tool";
const TOOL_PROXY_NAME: &str = "tool_proxy";
const TOOL_PROXY_INSTRUCTION_GROUP: &str = "tool-proxy";

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct SearchToolsArgs {
    /// Natural language or keyword query to search for tools.
    pub(super) query: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub(super) struct CallToolArgs {
    /// Name of the tool to invoke.
    pub(super) name: String,
    /// Arguments to pass to the tool, matching its parameter schema.
    #[serde(default)]
    pub(super) arguments: Value,
}

/// Error returned when a proxy tool name prefix is invalid.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error(
    "ToolProxyToolset name prefix must start with a letter and contain only letters, numbers, and underscores"
)]
pub struct ToolProxyNamePrefixError {
    prefix: String,
}

impl ToolProxyNamePrefixError {
    const fn new(prefix: String) -> Self {
        Self { prefix }
    }

    /// Return the rejected prefix text.
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }
}

fn normalize_prefix(prefix: String) -> Result<Option<String>, ToolProxyNamePrefixError> {
    let normalized = prefix.trim().trim_matches('_').to_string();
    if normalized.is_empty() {
        return Ok(None);
    }

    let mut chars = normalized.chars();
    let Some(first) = chars.next() else {
        return Ok(None);
    };
    if !first.is_ascii_alphabetic() || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
        return Err(ToolProxyNamePrefixError::new(prefix));
    }
    Ok(Some(normalized))
}

/// Create a fixed two-tool proxy over many underlying toolsets.
#[must_use]
pub fn dynamic_tool_proxy(toolsets: Vec<DynToolset>) -> DynToolset {
    Arc::new(ToolProxyToolset::new(toolsets))
}

/// Fixed two-tool proxy for dynamic tool discovery and invocation.
///
/// The proxy exposes `search_tools` and `call_tool` by default while keeping all
/// wrapped tool definitions out of the model-visible tool list until the model
/// searches for them. Use [`ToolProxyToolset::try_with_name_prefix`] to expose
/// `{prefix}_search_tool` and `{prefix}_call_tool` instead when multiple proxy
/// surfaces need stable, non-conflicting names.
#[derive(Clone)]
pub struct ToolProxyToolset {
    inner: Arc<ToolProxyInner>,
}

impl ToolProxyToolset {
    /// Build a proxy over wrapped toolsets.
    #[must_use]
    pub fn new(toolsets: Vec<DynToolset>) -> Self {
        Self {
            inner: Arc::new(ToolProxyInner::new(toolsets)),
        }
    }

    /// Set a stable prefix for the visible proxy tool names.
    ///
    /// A prefix is trimmed and surrounding underscores are removed. Empty prefixes
    /// restore the default unprefixed names. Non-empty prefixes must start with an
    /// ASCII letter and contain only ASCII letters, ASCII digits, and underscores.
    ///
    /// # Errors
    ///
    /// Returns [`ToolProxyNamePrefixError`] when the normalized prefix is not a valid
    /// model-facing tool-name prefix.
    pub fn try_with_name_prefix(
        mut self,
        prefix: impl Into<String>,
    ) -> Result<Self, ToolProxyNamePrefixError> {
        let prefix = normalize_prefix(prefix.into())?;
        Arc::make_mut(&mut self.inner).set_prefix(prefix);
        Ok(self)
    }

    /// Set a stable prefix for the visible proxy tool names.
    ///
    /// Prefer [`Self::try_with_name_prefix`] when the prefix comes from user input.
    ///
    /// # Panics
    ///
    /// Panics when the prefix is not a valid model-facing tool-name prefix.
    #[must_use]
    pub fn with_name_prefix(self, prefix: impl Into<String>) -> Self {
        match self.try_with_name_prefix(prefix) {
            Ok(proxy) => proxy,
            Err(error) => panic!("{error}"),
        }
    }

    /// Return the optional visible proxy tool prefix.
    #[must_use]
    pub fn prefix(&self) -> Option<&str> {
        self.inner.prefix()
    }

    /// Return the visible search proxy tool name.
    #[must_use]
    pub fn search_tool_name(&self) -> &str {
        self.inner.search_tool_name()
    }

    /// Return the visible call proxy tool name.
    #[must_use]
    pub fn call_tool_name(&self) -> &str {
        self.inner.call_tool_name()
    }

    /// Set namespace descriptions by toolset id.
    #[must_use]
    pub fn with_namespace_descriptions(
        mut self,
        descriptions: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        let descriptions = descriptions
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        Arc::make_mut(&mut self.inner).set_namespace_descriptions(descriptions);
        self
    }

    /// Set the maximum number of search matches.
    #[must_use]
    pub fn with_max_results(mut self, max_results: usize) -> Self {
        Arc::make_mut(&mut self.inner).set_max_results(max_results);
        self
    }

    /// Return the wrapped toolsets.
    #[must_use]
    pub fn toolsets(&self) -> &[DynToolset] {
        self.inner.toolsets()
    }
}

impl Toolset for ToolProxyToolset {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn get_tools(&self) -> Vec<DynTool> {
        let search_inner = self.inner.clone();
        let call_inner = self.inner.clone();
        vec![
            Arc::new(typed_json_tool::<SearchToolsArgs, _, _>(
                self.inner.search_tool_name().to_string(),
                Some("Search for available tools by keyword, description, namespace, or parameter schema. Returns XML with full parameter schemas.".to_string()),
                move |_context, arguments| {
                    let inner = search_inner.clone();
                    async move { Ok(inner.search_tools(&arguments)) }
                },
            )),
            Arc::new(typed_json_tool::<CallToolArgs, _, _>(
                self.inner.call_tool_name().to_string(),
                Some("Invoke an available tool by name with arguments matching the tool's parameter schema.".to_string()),
                move |context, arguments| {
                    let inner = call_inner.clone();
                    async move { inner.call_tool(context, arguments).await }
                },
            )),
        ]
    }

    fn max_retries(&self) -> Option<usize> {
        Some(3)
    }

    fn get_instructions(&self) -> Vec<ToolInstruction> {
        vec![ToolInstruction::new(
            self.inner.instruction_group().to_string(),
            self.inner.proxy_instruction(),
        )]
    }
}
