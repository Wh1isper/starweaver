//! Fixed two-tool proxy over many underlying toolsets.

use std::{collections::BTreeMap, sync::Arc};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::{
    typed_tool, DynTool, DynToolset, ToolContext, ToolError, ToolInstruction, ToolResult, Toolset,
};

const SEARCH_TOOLS_NAME: &str = "search_tools";
const CALL_TOOL_NAME: &str = "call_tool";
const PREFIXED_SEARCH_TOOL_SUFFIX: &str = "search_tool";
const PREFIXED_CALL_TOOL_SUFFIX: &str = "call_tool";
const TOOL_PROXY_NAME: &str = "tool_proxy";
const TOOL_PROXY_INSTRUCTION_GROUP: &str = "tool-proxy";

/// Error returned when a proxy tool prefix is invalid.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error(
    "ToolProxyToolset prefix must start with a letter and contain only letters, numbers, and underscores"
)]
pub struct ToolProxyPrefixError {
    prefix: String,
}

impl ToolProxyPrefixError {
    /// Return the rejected prefix text.
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }
}

/// Create a fixed two-tool proxy over many underlying toolsets.
#[must_use]
pub fn tool_proxy_toolset(toolsets: Vec<DynToolset>) -> DynToolset {
    Arc::new(ToolProxyToolset::new(toolsets))
}

/// Fixed two-tool proxy for dynamic tool discovery and invocation.
///
/// The proxy exposes `search_tools` and `call_tool` by default while keeping all
/// wrapped tool definitions out of the model-visible tool list until the model
/// searches for them. Use [`ToolProxyToolset::try_with_prefix`] to expose
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
            inner: Arc::new(ToolProxyInner {
                name: TOOL_PROXY_NAME.to_string(),
                prefix: None,
                search_tool_name: SEARCH_TOOLS_NAME.to_string(),
                call_tool_name: CALL_TOOL_NAME.to_string(),
                instruction_group: TOOL_PROXY_INSTRUCTION_GROUP.to_string(),
                toolsets,
                namespace_descriptions: BTreeMap::new(),
                max_results: 5,
            }),
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
    /// Returns [`ToolProxyPrefixError`] when the normalized prefix is not a valid
    /// model-facing tool-name prefix.
    pub fn try_with_prefix(
        mut self,
        prefix: impl Into<String>,
    ) -> Result<Self, ToolProxyPrefixError> {
        let prefix = normalize_prefix(prefix.into())?;
        Arc::make_mut(&mut self.inner).set_prefix(prefix);
        Ok(self)
    }

    /// Set a stable prefix for the visible proxy tool names.
    ///
    /// Prefer [`Self::try_with_prefix`] when the prefix comes from user input.
    ///
    /// # Panics
    ///
    /// Panics when the prefix is not a valid model-facing tool-name prefix.
    #[must_use]
    pub fn with_prefix(self, prefix: impl Into<String>) -> Self {
        match self.try_with_prefix(prefix) {
            Ok(proxy) => proxy,
            Err(error) => panic!("{error}"),
        }
    }

    /// Return the optional visible proxy tool prefix.
    #[must_use]
    pub fn prefix(&self) -> Option<&str> {
        self.inner.prefix.as_deref()
    }

    /// Return the visible search proxy tool name.
    #[must_use]
    pub fn search_tool_name(&self) -> &str {
        &self.inner.search_tool_name
    }

    /// Return the visible call proxy tool name.
    #[must_use]
    pub fn call_tool_name(&self) -> &str {
        &self.inner.call_tool_name
    }

    /// Set namespace descriptions by toolset id.
    #[must_use]
    pub fn with_namespace_descriptions(
        mut self,
        descriptions: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        let inner = Arc::make_mut(&mut self.inner);
        inner.namespace_descriptions = descriptions
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        self
    }

    /// Set the maximum number of search matches.
    #[must_use]
    pub fn with_max_results(mut self, max_results: usize) -> Self {
        Arc::make_mut(&mut self.inner).max_results = max_results;
        self
    }

    /// Return the wrapped toolsets.
    #[must_use]
    pub fn toolsets(&self) -> &[DynToolset] {
        &self.inner.toolsets
    }
}

impl Toolset for ToolProxyToolset {
    fn name(&self) -> &str {
        &self.inner.name
    }

    fn get_tools(&self) -> Vec<DynTool> {
        let search_inner = self.inner.clone();
        let call_inner = self.inner.clone();
        vec![
            Arc::new(typed_tool::<SearchToolsArgs, _, _>(
                self.inner.search_tool_name.clone(),
                Some("Search for available tools by keyword, description, namespace, or parameter schema. Returns XML with full parameter schemas.".to_string()),
                move |_context, arguments| {
                    let inner = search_inner.clone();
                    async move { Ok(inner.search_tools(&arguments)) }
                },
            )),
            Arc::new(typed_tool::<CallToolArgs, _, _>(
                self.inner.call_tool_name.clone(),
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
            self.inner.instruction_group.clone(),
            self.inner.proxy_instruction(),
        )]
    }
}

#[derive(Clone)]
struct ToolProxyInner {
    name: String,
    prefix: Option<String>,
    search_tool_name: String,
    call_tool_name: String,
    instruction_group: String,
    toolsets: Vec<DynToolset>,
    namespace_descriptions: BTreeMap<String, String>,
    max_results: usize,
}

impl ToolProxyInner {
    fn set_prefix(&mut self, prefix: Option<String>) {
        self.prefix = prefix;
        self.name = TOOL_PROXY_NAME.to_string();
        if let Some(prefix) = self.prefix.as_deref() {
            self.search_tool_name = format!("{prefix}_{PREFIXED_SEARCH_TOOL_SUFFIX}");
            self.call_tool_name = format!("{prefix}_{PREFIXED_CALL_TOOL_SUFFIX}");
            self.instruction_group = format!("{prefix}-{TOOL_PROXY_INSTRUCTION_GROUP}");
        } else {
            self.search_tool_name = SEARCH_TOOLS_NAME.to_string();
            self.call_tool_name = CALL_TOOL_NAME.to_string();
            self.instruction_group = TOOL_PROXY_INSTRUCTION_GROUP.to_string();
        }
    }

    fn search_tools(&self, arguments: &SearchToolsArgs) -> ToolResult {
        if arguments.query.trim().is_empty() {
            return xml_result("<error>Parameter 'query' is required.</error>");
        }

        let index = self.index_tools();
        let mut scored = index
            .search_entries
            .iter()
            .filter_map(|entry| score_entry(&arguments.query, entry).map(|score| (score, entry)))
            .collect::<Vec<_>>();
        scored.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| left.name.cmp(&right.name))
        });

        let mut tools_to_show = BTreeMap::<String, IndexedTool>::new();
        for (_, entry) in scored.into_iter().take(self.max_results) {
            if entry.is_namespace_entry {
                if let Some(namespace) = entry.namespace.as_deref() {
                    for tool in index.tools_for_namespace(namespace) {
                        tools_to_show.insert(tool.name.clone(), tool.clone());
                    }
                }
            } else if let Some(tool) = index.tools.get(&entry.name) {
                tools_to_show.insert(tool.name.clone(), tool.clone());
            }
        }

        xml_result(format_search_results(
            &arguments.query,
            tools_to_show.values(),
        ))
    }

    async fn call_tool(
        &self,
        context: ToolContext,
        arguments: CallToolArgs,
    ) -> Result<ToolResult, ToolError> {
        if arguments.name.trim().is_empty() {
            return Ok(xml_result("<error>Parameter 'name' is required.</error>"));
        }

        let index = self.index_tools();
        let Some(tool) = index.tools.get(&arguments.name) else {
            return Ok(xml_result(format!(
                "<error>Tool \"{}\" not found. Use {} to discover available tools.</error>",
                xml_escape(&arguments.name),
                self.search_tool_name
            )));
        };

        match tool.tool.call(context, arguments.arguments).await {
            Ok(result) => Ok(result),
            Err(error @ (ToolError::ApprovalRequired { .. } | ToolError::CallDeferred { .. })) => {
                Err(error)
            }
            Err(error) => Ok(xml_result(format_tool_call_error(tool, &error.to_string()))),
        }
    }

    fn index_tools(&self) -> ToolProxyIndex {
        let mut tools = BTreeMap::new();
        let mut namespace_tools = BTreeMap::<String, Vec<String>>::new();
        let mut search_entries = Vec::new();

        for toolset in &self.toolsets {
            let namespace = toolset.id().map(str::to_string);
            for tool in toolset.get_tools() {
                if self.is_visible_proxy_tool_name(tool.name()) {
                    continue;
                }
                let name = tool.name().to_string();
                let indexed = IndexedTool::new(toolset.name(), namespace.as_deref(), tool);
                if let Some(namespace) = namespace.as_ref() {
                    namespace_tools
                        .entry(namespace.clone())
                        .or_default()
                        .push(name.clone());
                }
                search_entries.push(SearchEntry::tool(&indexed));
                tools.insert(name, indexed);
            }

            if let Some(namespace) = namespace.as_ref() {
                if namespace_tools.contains_key(namespace) {
                    search_entries.push(SearchEntry::namespace(
                        namespace,
                        self.namespace_description(toolset.as_ref(), namespace),
                    ));
                }
            }
        }

        ToolProxyIndex {
            tools,
            namespace_tools,
            search_entries,
        }
    }

    fn is_visible_proxy_tool_name(&self, name: &str) -> bool {
        name == self.search_tool_name || name == self.call_tool_name
    }

    fn namespace_description(&self, toolset: &dyn Toolset, namespace: &str) -> String {
        self.namespace_descriptions
            .get(namespace)
            .cloned()
            .or_else(|| {
                toolset
                    .get_instructions()
                    .into_iter()
                    .map(|instruction| instruction.content)
                    .find(|content| !content.trim().is_empty())
            })
            .unwrap_or_else(|| format!("Toolset: {namespace}"))
    }

    fn proxy_instruction(&self) -> String {
        let index = self.index_tools();
        let mut lines = vec![
            format!(
                "Use {} to discover available tools by keyword, action, namespace, or parameter name.",
                self.search_tool_name
            ),
            format!(
                "Use {} with a discovered tool name and a JSON arguments object matching the returned schema.",
                self.call_tool_name
            ),
            format!(
                "{} returns XML with tool names, descriptions, namespaces, and full JSON parameter schemas.",
                self.search_tool_name
            ),
            format!(
                "{} can be used directly when the tool name and schema are already known.",
                self.call_tool_name
            ),
            "Search uses keyword matching over tool names, descriptions, namespaces, and parameter schemas.".to_string(),
        ];

        let tool_count = index.tools.len();
        if tool_count > 0 {
            lines.push(format!(
                "There are {tool_count} tools available through the proxy."
            ));
        }
        if !index.namespace_tools.is_empty() {
            lines.push("Available tool namespaces:".to_string());
            for (namespace, tools) in index.namespace_tools {
                let description = self
                    .namespace_descriptions
                    .get(&namespace)
                    .cloned()
                    .unwrap_or_else(|| format!("Toolset: {namespace}"));
                lines.push(format!(
                    "- {namespace}: {description} ({} tools)",
                    tools.len()
                ));
            }
        }
        lines.join("\n")
    }
}

struct ToolProxyIndex {
    tools: BTreeMap<String, IndexedTool>,
    namespace_tools: BTreeMap<String, Vec<String>>,
    search_entries: Vec<SearchEntry>,
}

impl ToolProxyIndex {
    fn tools_for_namespace(&self, namespace: &str) -> Vec<&IndexedTool> {
        self.namespace_tools
            .get(namespace)
            .into_iter()
            .flatten()
            .filter_map(|name| self.tools.get(name))
            .collect()
    }
}

#[derive(Clone)]
struct IndexedTool {
    toolset: String,
    namespace: Option<String>,
    name: String,
    description: String,
    parameters: Value,
    tool: DynTool,
}

impl IndexedTool {
    fn new(toolset: &str, namespace: Option<&str>, tool: DynTool) -> Self {
        Self {
            toolset: toolset.to_string(),
            namespace: namespace.map(str::to_string),
            name: tool.name().to_string(),
            description: tool.description().unwrap_or_default().to_string(),
            parameters: tool.parameters_schema(),
            tool,
        }
    }
}

struct SearchEntry {
    name: String,
    description: String,
    namespace: Option<String>,
    parameters: Value,
    is_namespace_entry: bool,
}

impl SearchEntry {
    fn tool(tool: &IndexedTool) -> Self {
        Self {
            name: tool.name.clone(),
            description: tool.description.clone(),
            namespace: tool.namespace.clone(),
            parameters: tool.parameters.clone(),
            is_namespace_entry: false,
        }
    }

    fn namespace(name: &str, description: String) -> Self {
        Self {
            name: name.to_string(),
            description,
            namespace: Some(name.to_string()),
            parameters: Value::Null,
            is_namespace_entry: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct SearchToolsArgs {
    /// Natural language or keyword query to search for tools.
    query: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
struct CallToolArgs {
    /// Name of the tool to invoke.
    name: String,
    /// Arguments to pass to the tool, matching its parameter schema.
    #[serde(default)]
    arguments: Value,
}

fn normalize_prefix(prefix: String) -> Result<Option<String>, ToolProxyPrefixError> {
    let normalized = prefix.trim().trim_matches('_').to_string();
    if normalized.is_empty() {
        return Ok(None);
    }

    let mut chars = normalized.chars();
    let Some(first) = chars.next() else {
        return Ok(None);
    };
    if !first.is_ascii_alphabetic() || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
        return Err(ToolProxyPrefixError { prefix });
    }
    Ok(Some(normalized))
}

fn score_entry(query: &str, entry: &SearchEntry) -> Option<usize> {
    let terms = query
        .split_whitespace()
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();
    let terms = if terms.is_empty() { vec![query] } else { terms };

    let mut score = 0;
    for term in terms {
        let term = term.to_ascii_lowercase();
        if entry.name.to_ascii_lowercase().contains(&term) {
            score += 3;
        }
        if entry.description.to_ascii_lowercase().contains(&term) {
            score += 2;
        }
        if entry
            .namespace
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains(&term)
        {
            score += 2;
        }
        if entry
            .parameters
            .to_string()
            .to_ascii_lowercase()
            .contains(&term)
        {
            score += 1;
        }
    }

    (score > 0).then_some(score)
}

fn format_search_results<'a>(
    query: &str,
    tools: impl IntoIterator<Item = &'a IndexedTool>,
) -> String {
    let tools = tools.into_iter().collect::<Vec<_>>();
    let mut lines = vec![format!(
        "<search-results query=\"{}\" count=\"{}\">",
        xml_escape_attr(query),
        tools.len()
    )];

    if tools.is_empty() {
        lines.push("No tools found matching query. Try different keywords.".to_string());
    }

    for tool in tools {
        let namespace = tool
            .namespace
            .as_deref()
            .map(|namespace| format!(" namespace=\"{}\"", xml_escape_attr(namespace)))
            .unwrap_or_default();
        lines.push(format!(
            "<tool name=\"{}\" toolset=\"{}\"{}>",
            xml_escape_attr(&tool.name),
            xml_escape_attr(&tool.toolset),
            namespace
        ));
        lines.push(format!(
            "<description>{}</description>",
            xml_escape(&tool.description)
        ));
        lines.push(format!(
            "<parameters>{}</parameters>",
            xml_escape(&tool.parameters.to_string())
        ));
        lines.push("</tool>".to_string());
    }

    lines.push("</search-results>".to_string());
    lines.join("\n")
}

fn format_tool_call_error(tool: &IndexedTool, message: &str) -> String {
    format!(
        "<tool-call-error tool=\"{}\">\n<message>{}</message>\n<parameters>{}</parameters>\n</tool-call-error>",
        xml_escape_attr(&tool.name),
        xml_escape(message),
        xml_escape(&tool.parameters.to_string())
    )
}

fn xml_result(content: impl Into<String>) -> ToolResult {
    ToolResult::new(serde_json::json!({"content": content.into()}))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn xml_escape_attr(value: &str) -> String {
    xml_escape(value)
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
