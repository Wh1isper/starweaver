//! Reusable toolsets.

use std::sync::Arc;

use crate::{DynTool, ToolInstruction};

/// Shared reference to a runtime toolset.
pub type DynToolset = Arc<dyn Toolset>;

/// Reusable group of tools and instructions.
pub trait Toolset: Send + Sync {
    /// Toolset name.
    fn name(&self) -> &str;

    /// Optional stable toolset identifier for durable runtimes and namespace-level loading.
    fn id(&self) -> Option<&str> {
        None
    }

    /// Tools currently available from this toolset.
    fn get_tools(&self) -> Vec<DynTool>;

    /// Retry default inherited by tools that do not set their own limit.
    fn max_retries(&self) -> Option<usize> {
        None
    }

    /// Instruction blocks contributed by this toolset.
    fn get_instructions(&self) -> Vec<ToolInstruction> {
        Vec::new()
    }
}

/// Static reusable toolset.
#[derive(Clone, Default)]
pub struct StaticToolset {
    name: String,
    id: Option<String>,
    tools: Vec<DynTool>,
    instructions: Vec<ToolInstruction>,
    max_retries: Option<usize>,
}

impl StaticToolset {
    /// Create an empty static toolset.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            id: None,
            tools: Vec::new(),
            instructions: Vec::new(),
            max_retries: None,
        }
    }

    /// Set a stable toolset identifier.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Add a tool.
    #[must_use]
    pub fn with_tool(mut self, tool: DynTool) -> Self {
        self.tools.push(tool);
        self
    }

    /// Add many tools.
    #[must_use]
    pub fn with_tools(mut self, tools: impl IntoIterator<Item = DynTool>) -> Self {
        self.tools.extend(tools);
        self
    }

    /// Add an instruction.
    #[must_use]
    pub fn with_instruction(mut self, instruction: ToolInstruction) -> Self {
        self.instructions.push(instruction);
        self
    }

    /// Add many instructions.
    #[must_use]
    pub fn with_instructions(
        mut self,
        instructions: impl IntoIterator<Item = ToolInstruction>,
    ) -> Self {
        self.instructions.extend(instructions);
        self
    }

    /// Set a toolset-level retry default.
    #[must_use]
    pub const fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = Some(max_retries);
        self
    }
}

impl Toolset for StaticToolset {
    fn name(&self) -> &str {
        &self.name
    }

    fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn get_tools(&self) -> Vec<DynTool> {
        self.tools.clone()
    }

    fn max_retries(&self) -> Option<usize> {
        self.max_retries
    }

    fn get_instructions(&self) -> Vec<ToolInstruction> {
        self.instructions.clone()
    }
}
