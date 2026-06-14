//! Tool proxy search index.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::DynTool;

pub(super) struct ToolProxyIndex {
    pub(super) tools: BTreeMap<String, IndexedTool>,
    pub(super) namespace_tools: BTreeMap<String, Vec<String>>,
    pub(super) search_entries: Vec<SearchEntry>,
}

impl ToolProxyIndex {
    pub(super) const fn new(
        tools: BTreeMap<String, IndexedTool>,
        namespace_tools: BTreeMap<String, Vec<String>>,
        search_entries: Vec<SearchEntry>,
    ) -> Self {
        Self {
            tools,
            namespace_tools,
            search_entries,
        }
    }

    pub(super) fn tools_for_namespace(&self, namespace: &str) -> Vec<&IndexedTool> {
        self.namespace_tools
            .get(namespace)
            .into_iter()
            .flatten()
            .filter_map(|name| self.tools.get(name))
            .collect()
    }
}

#[derive(Clone)]
pub(super) struct IndexedTool {
    pub(super) toolset: String,
    pub(super) namespace: Option<String>,
    pub(super) name: String,
    pub(super) description: String,
    pub(super) parameters: Value,
    pub(super) tool: DynTool,
}

impl IndexedTool {
    pub(super) fn new(toolset: &str, namespace: Option<&str>, tool: DynTool) -> Self {
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

pub(super) struct SearchEntry {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) namespace: Option<String>,
    pub(super) parameters: Value,
    pub(super) is_namespace_entry: bool,
}

impl SearchEntry {
    pub(super) fn tool(tool: &IndexedTool) -> Self {
        Self {
            name: tool.name.clone(),
            description: tool.description.clone(),
            namespace: tool.namespace.clone(),
            parameters: tool.parameters.clone(),
            is_namespace_entry: false,
        }
    }

    pub(super) fn namespace(name: &str, description: String) -> Self {
        Self {
            name: name.to_string(),
            description,
            namespace: Some(name.to_string()),
            parameters: Value::Null,
            is_namespace_entry: true,
        }
    }
}

pub(super) fn score_entry(query: &str, entry: &SearchEntry) -> Option<usize> {
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
