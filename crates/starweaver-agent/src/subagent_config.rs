//! Subagent configuration parsing for SDK and CLI inputs.

use std::{fs, path::Path};

use serde::Deserialize;
use serde_json::Value;
use starweaver_core::SubagentSpec;
use thiserror::Error;

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
    /// File I/O failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Default, Deserialize)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    instruction: Option<String>,
    tools: Option<StringList>,
    optional_tools: Option<StringList>,
    denied_tools: Option<StringList>,
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
    spec.model = frontmatter.model;
    spec.model_settings = frontmatter.model_settings;
    spec.model_config = frontmatter.model_cfg.or(frontmatter.model_config);
    if let Some(metadata) = frontmatter.metadata {
        spec.metadata.extend(metadata);
    }
    Ok(spec)
}

fn parse_frontmatter(frontmatter: &str) -> Result<Frontmatter, SubagentConfigError> {
    serde_yaml::from_str(frontmatter)
        .map_err(|error| SubagentConfigError::InvalidFrontmatter(error.to_string()))
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
