//! Tool registry and execution dispatch.

use std::collections::BTreeMap;

use starweaver_model::{ToolCallPart, ToolDefinition, ToolReturnPart};

use crate::{error_return, DynTool, DynToolset, ToolContext, ToolError, ToolInstruction};

/// Tool registry used by agent runs.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, DynTool>,
    toolset_max_retries: BTreeMap<String, usize>,
    instructions: BTreeMap<String, String>,
    max_retries: Option<usize>,
}

impl ToolRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool.
    #[must_use]
    pub fn with_tool(mut self, tool: DynTool) -> Self {
        self.insert(tool);
        self
    }

    /// Insert or replace a tool.
    pub fn insert(&mut self, tool: DynTool) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Set an agent-level retry default for tools that do not override it.
    #[must_use]
    pub const fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = Some(max_retries);
        self
    }

    /// Update the agent-level retry default.
    pub const fn set_max_retries(&mut self, max_retries: usize) {
        self.max_retries = Some(max_retries);
    }

    /// Add an instruction block, deduplicated by group.
    pub fn insert_instruction(&mut self, instruction: ToolInstruction) {
        self.instructions
            .entry(instruction.group)
            .or_insert(instruction.content);
    }

    /// Add all tools and instructions from a toolset.
    #[must_use]
    pub fn with_toolset(mut self, toolset: &DynToolset) -> Self {
        self.insert_toolset(toolset);
        self
    }

    /// Insert all tools and instructions from a toolset.
    pub fn insert_toolset(&mut self, toolset: &DynToolset) {
        let max_retries = toolset.max_retries();
        for tool in toolset.get_tools() {
            if let Some(max_retries) = max_retries {
                if tool.max_retries().is_none() {
                    self.toolset_max_retries
                        .insert(tool.name().to_string(), max_retries);
                }
            }
            self.insert(tool);
        }
        for instruction in toolset.get_instructions() {
            self.insert_instruction(instruction);
        }
    }

    /// Insert all tools and instructions from another registry.
    pub fn insert_registry(&mut self, registry: &Self) {
        if let Some(max_retries) = registry.max_retries {
            self.max_retries = Some(max_retries);
        }
        for (name, max_retries) in &registry.toolset_max_retries {
            self.toolset_max_retries.insert(name.clone(), *max_retries);
        }
        for tool in registry.tools.values() {
            self.insert(tool.clone());
        }
        for (group, content) in &registry.instructions {
            self.insert_instruction(ToolInstruction::new(group.clone(), content.clone()));
        }
    }

    /// Return instruction text in stable group order.
    #[must_use]
    pub fn get_instructions(&self) -> Vec<String> {
        self.instructions.values().cloned().collect()
    }

    /// Return all tool definitions sorted by name.
    #[must_use]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|tool| tool.definition()).collect()
    }

    /// Return whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Execute one tool call and return a model tool return part.
    pub async fn execute_call(&self, context: ToolContext, call: &ToolCallPart) -> ToolReturnPart {
        match self.tools.get(&call.name) {
            Some(tool) => match tool.call(context, call.arguments.execution_value()).await {
                Ok(result) => ToolReturnPart {
                    tool_call_id: call.id.clone(),
                    name: call.name.clone(),
                    content: result.content,
                    is_error: false,
                    metadata: result.metadata,
                },
                Err(error) => error_return(call, &error),
            },
            None => error_return(call, &ToolError::NotFound(call.name.clone())),
        }
    }

    /// Return the effective retry limit for a registered tool.
    #[must_use]
    pub fn max_retries_for(&self, name: &str) -> usize {
        self.tools.get(name).map_or_else(
            || self.max_retries.unwrap_or(1),
            |tool| {
                tool.max_retries()
                    .or_else(|| self.toolset_max_retries.get(name).copied())
                    .or(self.max_retries)
                    .unwrap_or(1)
            },
        )
    }

    /// Return this registry's agent-level retry default.
    #[must_use]
    pub const fn max_retries(&self) -> Option<usize> {
        self.max_retries
    }

    /// Return whether a tool is registered by name.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Return registered tool names in stable order.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    /// Return all registered tools in stable name order.
    #[must_use]
    pub fn tools(&self) -> Vec<DynTool> {
        self.tools.values().cloned().collect()
    }

    /// Remove one tool by name.
    pub fn remove(&mut self, name: &str) -> Option<DynTool> {
        self.toolset_max_retries.remove(name);
        self.tools.remove(name)
    }

    /// Return a registry containing a selected subset of tools.
    #[must_use]
    pub fn select(&self, names: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        let mut selected = Self::new();
        if let Some(max_retries) = self.max_retries {
            selected.max_retries = Some(max_retries);
        }
        for name in names {
            let name = name.as_ref();
            if let Some(tool) = self.tools.get(name) {
                if let Some(max_retries) = self.toolset_max_retries.get(name) {
                    selected
                        .toolset_max_retries
                        .insert(name.to_string(), *max_retries);
                }
                selected.insert(tool.clone());
            }
        }
        selected
    }

    /// Return a registry containing tools whose metadata opts into subagent inheritance.
    #[must_use]
    pub fn auto_inherited(&self) -> Self {
        let names = self
            .tools
            .iter()
            .filter_map(|(name, tool)| {
                tool.metadata()
                    .get("auto_inherit")
                    .and_then(serde_json::Value::as_bool)
                    .filter(|enabled| *enabled)
                    .map(|_| name.clone())
            })
            .collect::<Vec<_>>();
        self.select(names)
    }

    /// Return a registered tool by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<DynTool> {
        self.tools.get(name).cloned()
    }
}
