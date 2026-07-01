//! Tool proxy inner implementation.

use std::collections::{BTreeMap, BTreeSet};

use starweaver_context::{AgentContext, AgentContextHandle, AgentEvent};

use super::format::{format_search_results, format_tool_call_error, xml_escape, xml_result};
use super::index::{IndexedTool, SearchEntry, ToolProxyIndex, score_entry};
use super::publish_tool_search_query_event;
use super::{
    CALL_TOOL_NAME, PREFIXED_CALL_TOOL_SUFFIX, PREFIXED_SEARCH_TOOL_SUFFIX, SEARCH_TOOLS_NAME,
    TOOL_PROXY_INSTRUCTION_GROUP, TOOL_PROXY_NAME, TOOL_SEARCH_FAILED_EVENT_KIND,
    TOOL_SEARCH_NO_MATCH_EVENT_KIND,
};
use super::{CallToolArgs, SearchToolsArgs};
use super::{ToolSearchInitializationReport, ToolSearchNamespaceReport, ToolSearchNamespaceStatus};
use crate::{DynToolset, ToolContext, ToolError, ToolResult, Toolset};

#[derive(Clone)]
pub(super) struct ToolProxyInner {
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
    pub(super) fn new(toolsets: Vec<DynToolset>) -> Self {
        Self {
            name: TOOL_PROXY_NAME.to_string(),
            prefix: None,
            search_tool_name: SEARCH_TOOLS_NAME.to_string(),
            call_tool_name: CALL_TOOL_NAME.to_string(),
            instruction_group: TOOL_PROXY_INSTRUCTION_GROUP.to_string(),
            toolsets,
            namespace_descriptions: BTreeMap::new(),
            max_results: 5,
        }
    }

    pub(super) fn name(&self) -> &str {
        &self.name
    }

    pub(super) fn prefix(&self) -> Option<&str> {
        self.prefix.as_deref()
    }

    pub(super) fn search_tool_name(&self) -> &str {
        &self.search_tool_name
    }

    pub(super) fn call_tool_name(&self) -> &str {
        &self.call_tool_name
    }

    pub(super) fn instruction_group(&self) -> &str {
        &self.instruction_group
    }

    pub(super) fn toolsets(&self) -> &[DynToolset] {
        &self.toolsets
    }

    pub(super) fn set_namespace_descriptions(&mut self, descriptions: BTreeMap<String, String>) {
        self.namespace_descriptions = descriptions;
    }

    pub(super) const fn set_max_results(&mut self, max_results: usize) {
        self.max_results = max_results;
    }

    pub(super) fn initialization_report(
        &self,
        toolset_name: &str,
        search_tool_name: &str,
        context: Option<&AgentContext>,
    ) -> ToolSearchInitializationReport {
        let index = self.index_tools_with_extra_hidden_names(&[search_tool_name]);
        let availability_checked = context.is_some();
        let mut available_tools = 0usize;
        let mut unavailable_tools = 0usize;
        let mut loose_tools = Vec::new();

        for tool in index.tools.values() {
            if let Some(context) = context {
                if tool.tool.is_available(context) {
                    available_tools = available_tools.saturating_add(1);
                } else {
                    unavailable_tools = unavailable_tools.saturating_add(1);
                }
            }
            if tool.namespace.is_none() {
                loose_tools.push(tool.name.clone());
            }
        }
        loose_tools.sort();

        let mut namespaces = Vec::new();
        for (namespace, tool_names) in &index.namespace_tools {
            let mut tools = tool_names.clone();
            tools.sort();
            let (namespace_available, namespace_unavailable) = context.map_or((0, 0), |context| {
                tools.iter().filter_map(|name| index.tools.get(name)).fold(
                    (0usize, 0usize),
                    |(available, unavailable), tool| {
                        if tool.tool.is_available(context) {
                            (available.saturating_add(1), unavailable)
                        } else {
                            (available, unavailable.saturating_add(1))
                        }
                    },
                )
            });
            let status = if tools.is_empty() {
                ToolSearchNamespaceStatus::Empty
            } else if availability_checked && namespace_available == 0 {
                ToolSearchNamespaceStatus::Unavailable
            } else {
                ToolSearchNamespaceStatus::Connected
            };
            namespaces.push(ToolSearchNamespaceReport {
                namespace: namespace.clone(),
                status,
                total_tools: tools.len(),
                tools,
                available_tools: namespace_available,
                unavailable_tools: namespace_unavailable,
            });
        }

        ToolSearchInitializationReport {
            toolset_name: toolset_name.to_string(),
            search_tool_name: search_tool_name.to_string(),
            total_tools: index.tools.len(),
            total_namespaces: namespaces.len(),
            loose_tools,
            namespaces,
            available_tools,
            unavailable_tools,
            availability_checked,
            max_results: self.max_results,
        }
    }

    pub(super) fn set_prefix(&mut self, prefix: Option<String>) {
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

    pub(super) fn search_tools(
        &self,
        context: &ToolContext,
        arguments: &SearchToolsArgs,
    ) -> ToolResult {
        if arguments.query.trim().is_empty() {
            publish_tool_search_query_event(
                context,
                TOOL_SEARCH_FAILED_EVENT_KIND,
                &self.search_tool_name,
                &arguments.query,
                "empty_query",
                "Parameter 'query' is required.",
            );
            return xml_result("<error>Parameter 'query' is required.</error>");
        }

        let tools_to_show = self.search_matching_tools(&arguments.query, &[]);

        if tools_to_show.is_empty() {
            publish_tool_search_query_event(
                context,
                TOOL_SEARCH_NO_MATCH_EVENT_KIND,
                &self.search_tool_name,
                &arguments.query,
                "no_match",
                "No tools matched the query.",
            );
        } else {
            Self::record_loaded_tools(context, tools_to_show.values());
        }
        xml_result(format_search_results(
            &arguments.query,
            tools_to_show.values(),
        ))
    }

    pub(super) async fn call_tool(
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

        match tool.tool.call(context.clone(), arguments.arguments).await {
            Ok(result) => {
                Self::record_loaded_tools(&context, std::iter::once(tool));
                Ok(result)
            }
            Err(error @ (ToolError::ApprovalRequired { .. } | ToolError::CallDeferred { .. })) => {
                Err(error)
            }
            Err(error) => Ok(xml_result(format_tool_call_error(tool, &error.to_string()))),
        }
    }

    pub(super) fn search_matching_tools(
        &self,
        query: &str,
        extra_hidden_names: &[&str],
    ) -> BTreeMap<String, IndexedTool> {
        let index = self.index_tools_with_extra_hidden_names(extra_hidden_names);
        let mut scored = index
            .search_entries
            .iter()
            .filter_map(|entry| score_entry(query, entry).map(|score| (score, entry)))
            .collect::<Vec<_>>();
        scored.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| left.name.cmp(&right.name))
        });

        let mut matched = BTreeMap::<String, IndexedTool>::new();
        for (_, entry) in scored.into_iter().take(self.max_results) {
            if entry.is_namespace_entry {
                if let Some(namespace) = entry.namespace.as_deref() {
                    for tool in index.tools_for_namespace(namespace) {
                        matched.insert(tool.name.clone(), tool.clone());
                    }
                }
            } else if let Some(tool) = index.tools.get(&entry.name) {
                matched.insert(tool.name.clone(), tool.clone());
            }
        }
        matched
    }

    pub(super) fn index_tools(&self) -> ToolProxyIndex {
        self.index_tools_with_extra_hidden_names(&[])
    }

    pub(super) fn index_tools_with_extra_hidden_names(
        &self,
        extra_hidden_names: &[&str],
    ) -> ToolProxyIndex {
        let mut tools = BTreeMap::new();
        let mut namespace_tools = BTreeMap::<String, Vec<String>>::new();
        let mut search_entries = Vec::new();

        for toolset in &self.toolsets {
            let namespace = toolset.id().map(str::to_string);
            for tool in toolset.get_tools() {
                if self.is_visible_proxy_tool_name(tool.name())
                    || extra_hidden_names.contains(&tool.name())
                {
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

            if let Some(namespace) = namespace.as_ref()
                && namespace_tools.contains_key(namespace)
            {
                search_entries.push(SearchEntry::namespace(
                    namespace,
                    self.namespace_description(toolset.as_ref(), namespace),
                ));
            }
        }

        ToolProxyIndex::new(tools, namespace_tools, search_entries)
    }

    fn is_visible_proxy_tool_name(&self, name: &str) -> bool {
        name == self.search_tool_name || name == self.call_tool_name
    }

    pub(super) fn record_loaded_tools<'a>(
        context: &ToolContext,
        tools: impl IntoIterator<Item = &'a IndexedTool>,
    ) {
        let Some(handle) = context.dependency::<AgentContextHandle>() else {
            return;
        };
        let mut tool_names = Vec::new();
        let mut namespaces = BTreeSet::new();
        for tool in tools {
            tool_names.push(tool.name.clone());
            if let Some(namespace) = tool.namespace.as_ref() {
                namespaces.insert(namespace.clone());
            }
        }
        handle.update(|agent_context| {
            agent_context.record_tool_search_loaded(tool_names.clone(), namespaces.clone());
            agent_context.publish_event(AgentEvent::new(
                "tool_search_loaded",
                serde_json::json!({
                    "loaded_tools": tool_names,
                    "loaded_namespaces": namespaces,
                }),
            ));
        });
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

    pub(super) fn proxy_instruction(&self) -> String {
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
