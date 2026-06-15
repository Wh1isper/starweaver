//! Tool instruction blocks.

use serde::{Deserialize, Serialize};
use starweaver_core::XmlWriter;

/// Toolset or tool instruction block.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolInstruction {
    /// Instruction group used for deduplication.
    pub group: String,
    /// Instruction content.
    pub content: String,
    /// Whether this instruction is dynamic for provider cache-boundary placement.
    #[serde(default)]
    pub dynamic: bool,
}

impl ToolInstruction {
    /// Create an instruction block.
    #[must_use]
    pub fn new(group: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            group: group.into(),
            content: content.into(),
            dynamic: false,
        }
    }

    /// Mark this instruction as dynamic or static for provider cache-boundary placement.
    #[must_use]
    pub const fn with_dynamic(mut self, dynamic: bool) -> Self {
        self.dynamic = dynamic;
        self
    }

    /// Render this instruction block in the ya-agent-sdk tool-instruction XML shape.
    #[must_use]
    pub fn render_xml(&self) -> String {
        let mut xml = XmlWriter::new();
        xml.text_element_attrs(
            "tool-instruction",
            [("name", self.group.as_str())],
            &self.content,
        );
        xml.finish()
    }
}
