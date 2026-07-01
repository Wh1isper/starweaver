//! Prepare-tools safety validation.

use std::collections::{BTreeMap, BTreeSet};

use starweaver_model::ToolDefinition;
use starweaver_tools::{ToolKind, tool_metadata_kind};

use crate::agent::AgentError;

pub(in crate::agent) fn validate_prepared_tools(
    original: &[ToolDefinition],
    prepared: Vec<ToolDefinition>,
) -> Result<Vec<ToolDefinition>, AgentError> {
    let mut original_by_name = BTreeMap::new();
    for tool in original {
        original_by_name.insert(tool.name.as_str(), tool);
    }

    let prepared_names = prepared
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<BTreeSet<_>>();
    for tool in original {
        if tool_metadata_kind(&tool.metadata) == Some(ToolKind::Output)
            && !prepared_names.contains(tool.name.as_str())
        {
            return Err(AgentError::Capability(format!(
                "prepare_tools cannot remove output tool {:?}",
                tool.name
            )));
        }
    }

    let mut seen = BTreeSet::new();
    for tool in &prepared {
        if !seen.insert(tool.name.as_str()) {
            return Err(AgentError::Capability(format!(
                "prepare_tools returned duplicate tool {:?}",
                tool.name
            )));
        }
        let Some(original_tool) = original_by_name.get(tool.name.as_str()) else {
            return Err(AgentError::Capability(format!(
                "prepare_tools cannot add or rename tool {:?}",
                tool.name
            )));
        };
        if tool_metadata_kind(&original_tool.metadata) != tool_metadata_kind(&tool.metadata) {
            return Err(AgentError::Capability(format!(
                "prepare_tools cannot change tool kind for {:?}",
                tool.name
            )));
        }
    }

    let mut result_by_name = prepared
        .into_iter()
        .map(|tool| (tool.name.clone(), tool))
        .collect::<BTreeMap<_, _>>();
    let mut stable = Vec::new();
    for tool in original {
        if let Some(prepared_tool) = result_by_name.remove(&tool.name) {
            stable.push(prepared_tool);
        }
    }
    Ok(stable)
}
