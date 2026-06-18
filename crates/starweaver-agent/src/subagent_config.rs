//! Subagent configuration parsing for SDK and CLI inputs.

use std::{fs, path::Path};

use serde::Deserialize;
use serde_json::Value;
use starweaver_core::SubagentSpec;
use starweaver_model::ModelSettings;
use thiserror::Error;

use crate::{
    AgentSpec, ModelPreset, SubagentCapabilityInheritancePolicy, SubagentToolInheritancePolicy,
};

/// Error returned while parsing subagent markdown configuration.
#[derive(Debug, Error)]
pub enum SubagentConfigError {
    /// Markdown content does not contain YAML/TOML-style frontmatter.
    #[error("invalid subagent markdown: expected frontmatter delimited by ---")]
    MissingFrontmatter,
    /// Frontmatter is malformed.
    #[error("invalid subagent frontmatter: {0}")]
    InvalidFrontmatter(String),
    /// A required field is missing.
    #[error("missing required subagent field: {0}")]
    MissingField(&'static str),
    /// Subagent requested inherited model configuration but no inherited model id was supplied.
    #[error("subagent model is inherited but no inherited model id was supplied")]
    MissingInheritedModel,
    /// Inline model settings could not be parsed.
    #[error("invalid subagent model settings: {0}")]
    InvalidModelSettings(String),
    /// Inline model config could not be projected into an agent spec.
    #[error("invalid subagent model config: {0}")]
    InvalidModelConfig(String),
    /// File I/O failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Projection from serializable subagent config into runtime agent spec inputs.
#[derive(Clone, Debug, PartialEq)]
pub struct SubagentSpecProjection {
    /// Runtime-free child agent spec.
    pub agent_spec: AgentSpec,
    /// Parent-tool inheritance policy for the executable subagent config.
    pub tool_inheritance: SubagentToolInheritancePolicy,
    /// Parent-capability inheritance policy for the executable subagent config.
    pub capability_inheritance: SubagentCapabilityInheritancePolicy,
}

#[derive(Debug, Default, Deserialize)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    instruction: Option<String>,
    tools: Option<StringList>,
    optional_tools: Option<StringList>,
    denied_tools: Option<StringList>,
    inherit_hooks: Option<bool>,
    inherit_capabilities: Option<bool>,
    denied_capabilities: Option<StringList>,
    model: Option<String>,
    model_settings: Option<Value>,
    model_cfg: Option<Value>,
    model_config: Option<Value>,
    metadata: Option<serde_json::Map<String, Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StringList {
    One(String),
    Many(Vec<String>),
}

impl StringList {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(value) => value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect(),
            Self::Many(values) => values,
        }
    }
}

/// Parse a subagent markdown file with `---` frontmatter into a serializable spec.
///
/// # Errors
///
/// Returns an error when frontmatter is missing, malformed, or lacks required fields.
pub fn parse_subagent_markdown(content: &str) -> Result<SubagentSpec, SubagentConfigError> {
    let trimmed = content.trim();
    let body_start = trimmed
        .strip_prefix("---")
        .ok_or(SubagentConfigError::MissingFrontmatter)?
        .trim_start_matches(['\r', '\n']);
    let (frontmatter, body) = body_start
        .split_once("\n---")
        .ok_or(SubagentConfigError::MissingFrontmatter)?;
    let body = body
        .strip_prefix("---")
        .unwrap_or(body)
        .trim_start_matches(['\r', '\n'])
        .trim()
        .to_string();
    let frontmatter = parse_frontmatter(frontmatter)?;
    let name = frontmatter
        .name
        .ok_or(SubagentConfigError::MissingField("name"))?;
    let description = frontmatter
        .description
        .ok_or(SubagentConfigError::MissingField("description"))?;
    let mut spec = SubagentSpec::new(name, description, body);
    spec.instruction = frontmatter.instruction;
    spec.tools = frontmatter
        .tools
        .map_or_else(Vec::new, StringList::into_vec);
    spec.optional_tools = frontmatter
        .optional_tools
        .map_or_else(Vec::new, StringList::into_vec);
    if let Some(denied_tools) = frontmatter.denied_tools {
        spec.metadata.insert(
            "denied_tools".to_string(),
            serde_json::json!(denied_tools.into_vec()),
        );
    }
    if let Some(inherit_hooks) = frontmatter.inherit_hooks {
        spec.metadata.insert(
            "inherit_hooks".to_string(),
            serde_json::json!(inherit_hooks),
        );
    }
    if let Some(inherit_capabilities) = frontmatter.inherit_capabilities {
        spec.metadata.insert(
            "inherit_capabilities".to_string(),
            serde_json::json!(inherit_capabilities),
        );
    }
    if let Some(denied_capabilities) = frontmatter.denied_capabilities {
        spec.metadata.insert(
            "denied_capabilities".to_string(),
            serde_json::json!(denied_capabilities.into_vec()),
        );
    }
    spec.model = frontmatter.model;
    spec.model_settings = frontmatter.model_settings;
    spec.model_config = frontmatter.model_cfg.or(frontmatter.model_config);
    if let Some(metadata) = frontmatter.metadata {
        spec.metadata.extend(metadata);
    }
    Ok(spec)
}

/// Project a serializable subagent spec into an agent spec plus tool inheritance policy.
///
/// If `spec.model` is absent or equals `inherit`, `inherited_model_id` is used as the concrete
/// model id because executable Rust agents require an explicit model adapter.
///
/// # Errors
///
/// Returns an error when inherited model resolution is required but absent, or when structured
/// model settings/config cannot be represented by the current agent-spec contract.
pub fn project_subagent_spec(
    spec: &SubagentSpec,
    inherited_model_id: Option<&str>,
) -> Result<SubagentSpecProjection, SubagentConfigError> {
    let model_id = match spec.model.as_deref().map(str::trim) {
        Some(model) if !model.is_empty() && !is_inherit(model) => model.to_string(),
        _ => inherited_model_id
            .filter(|model| !model.trim().is_empty())
            .map(str::to_string)
            .ok_or(SubagentConfigError::MissingInheritedModel)?,
    };
    let (settings_preset, settings) = spec
        .model_settings
        .clone()
        .map(parse_model_settings)
        .transpose()?
        .unwrap_or_default();
    let config_preset = spec
        .model_config
        .clone()
        .map(parse_model_config_preset)
        .transpose()?
        .flatten();
    let mut agent_spec = AgentSpec {
        name: spec.name.clone(),
        description: Some(spec.description.clone()),
        instructions: vec![spec.system_prompt.clone()],
        model: Some(ModelPreset {
            model_id,
            settings_preset,
            config_preset,
            settings,
        }),
        metadata: spec.metadata.clone(),
        ..AgentSpec::default()
    };
    if let Some(instruction) = spec.instruction.as_ref() {
        agent_spec.metadata.insert(
            "subagent_instruction".to_string(),
            serde_json::json!(instruction),
        );
    }
    let denied_tools = denied_tools_from_metadata(&spec.metadata);
    let inherit_all_when_empty = spec.tools.is_empty() && spec.optional_tools.is_empty();
    let tool_inheritance =
        SubagentToolInheritancePolicy::new(spec.tools.clone(), spec.optional_tools.clone())
            .with_denied_tools(denied_tools)
            .with_inherit_all_when_empty(inherit_all_when_empty);
    let capability_inheritance = capability_inheritance_from_metadata(&spec.metadata);
    Ok(SubagentSpecProjection {
        agent_spec,
        tool_inheritance,
        capability_inheritance,
    })
}

fn parse_frontmatter(frontmatter: &str) -> Result<Frontmatter, SubagentConfigError> {
    yaml_serde::from_str(frontmatter)
        .map_err(|error| SubagentConfigError::InvalidFrontmatter(error.to_string()))
}

fn parse_model_settings(
    value: Value,
) -> Result<(Option<String>, Option<ModelSettings>), SubagentConfigError> {
    match value {
        Value::Null => Ok((None, None)),
        Value::String(value) if is_inherit(&value) => Ok((None, None)),
        Value::String(value) => Ok((Some(value), None)),
        value => serde_json::from_value(value)
            .map(|settings| (None, Some(settings)))
            .map_err(|error| SubagentConfigError::InvalidModelSettings(error.to_string())),
    }
}

fn parse_model_config_preset(value: Value) -> Result<Option<String>, SubagentConfigError> {
    match value {
        Value::Null => Ok(None),
        Value::String(value) if is_inherit(&value) => Ok(None),
        Value::String(value) => Ok(Some(value)),
        _ => Err(SubagentConfigError::InvalidModelConfig(
            "structured model config is not representable by AgentSpec; use a model_config preset name"
                .to_string(),
        )),
    }
}

fn denied_tools_from_metadata(metadata: &serde_json::Map<String, Value>) -> Vec<String> {
    metadata
        .get("denied_tools")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

fn capability_inheritance_from_metadata(
    metadata: &serde_json::Map<String, Value>,
) -> SubagentCapabilityInheritancePolicy {
    let inherit_hooks = metadata
        .get("inherit_hooks")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let inherit_capabilities = metadata
        .get("inherit_capabilities")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let denied_capabilities = metadata
        .get("denied_capabilities")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    SubagentCapabilityInheritancePolicy::default()
        .with_hooks(inherit_hooks)
        .with_capability_bundles(inherit_capabilities)
        .with_denied_capabilities(denied_capabilities)
}

fn is_inherit(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("inherit")
}

/// Load a subagent markdown file into a serializable spec.
///
/// # Errors
///
/// Returns an error when the file cannot be read or parsed.
pub fn load_subagent_from_file(
    path: impl AsRef<Path>,
) -> Result<SubagentSpec, SubagentConfigError> {
    parse_subagent_markdown(&fs::read_to_string(path)?)
}

/// Load every valid `*.md` subagent config from a directory.
///
/// # Errors
///
/// Returns an error when the directory cannot be read.
pub fn load_subagents_from_dir(
    dir_path: impl AsRef<Path>,
) -> Result<Vec<SubagentSpec>, SubagentConfigError> {
    let mut specs = Vec::new();
    for entry in fs::read_dir(dir_path)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension == "md") {
            if let Ok(spec) = load_subagent_from_file(&path) {
                specs.push(spec);
            }
        }
    }
    specs.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(specs)
}
