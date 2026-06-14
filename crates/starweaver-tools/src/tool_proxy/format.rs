//! XML formatting helpers for tool proxy results.

use super::index::IndexedTool;
use crate::ToolResult;

pub(super) fn format_search_results<'a>(
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

pub(super) fn format_tool_call_error(tool: &IndexedTool, message: &str) -> String {
    format!(
        "<tool-call-error tool=\"{}\">\n<message>{}</message>\n<parameters>{}</parameters>\n</tool-call-error>",
        xml_escape_attr(&tool.name),
        xml_escape(message),
        xml_escape(&tool.parameters.to_string())
    )
}

pub(super) fn xml_result(content: impl Into<String>) -> ToolResult {
    ToolResult::new(serde_json::json!({"content": content.into()}))
}

pub(super) fn xml_escape(value: &str) -> String {
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
