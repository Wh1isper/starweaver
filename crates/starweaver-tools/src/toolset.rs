//! Reusable toolsets.

use std::sync::Arc;

use crate::{DynTool, ToolInstruction};

/// Shared reference to a runtime toolset.
pub type DynToolset = Arc<dyn Toolset>;

/// Reusable group of tools and instructions.
pub trait Toolset: Send + Sync {
    /// Toolset name.
    fn name(&self) -> &str;

    /// Tools currently available from this toolset.
    fn tools(&self) -> Vec<DynTool>;

    /// Retry default inherited by tools that do not set their own limit.
    fn max_retries(&self) -> Option<usize> {
        None
    }

    /// Instruction blocks contributed by this toolset.
    fn instructions(&self) -> Vec<ToolInstruction> {
        Vec::new()
    }
}

/// Static reusable toolset.
#[derive(Clone, Default)]
pub struct StaticToolset {
    name: String,
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
            tools: Vec::new(),
            instructions: Vec::new(),
            max_retries: None,
        }
    }

    /// Add a tool.
    #[must_use]
    pub fn with_tool(mut self, tool: DynTool) -> Self {
        self.tools.push(tool);
        self
    }

    /// Add an instruction.
    #[must_use]
    pub fn with_instruction(mut self, instruction: ToolInstruction) -> Self {
        self.instructions.push(instruction);
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

    fn tools(&self) -> Vec<DynTool> {
        self.tools.clone()
    }

    fn max_retries(&self) -> Option<usize> {
        self.max_retries
    }

    fn instructions(&self) -> Vec<ToolInstruction> {
        self.instructions.clone()
    }
}
