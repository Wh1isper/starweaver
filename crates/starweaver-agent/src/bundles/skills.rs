//! Fileops-loaded skill package support.

use std::{collections::BTreeMap, sync::Arc};

use serde::{Deserialize, Serialize};
use starweaver_context::AgentContext;
use starweaver_core::Metadata;
use starweaver_environment::{DynEnvironmentProvider, EnvironmentError, FileGlobOptions};
use starweaver_tools::{DynToolset, StaticToolset, ToolInstruction};
use thiserror::Error;

/// One skill package loaded from provider-visible files.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillPackage {
    /// Stable skill name.
    pub name: String,
    /// Short model-facing description.
    pub description: String,
    /// Provider path to the `SKILL.md` file.
    pub path: String,
    /// Markdown body loaded from the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Extra frontmatter fields preserved for hosts.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl SkillPackage {
    /// Return the compact instruction summary for this skill.
    #[must_use]
    pub fn summary_line(&self) -> String {
        format!("- {}: {} ({})", self.name, self.description, self.path)
    }
}

/// Skill discovery configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillSourceScope {
    /// Root path scanned through the environment provider.
    pub root: String,
    /// Directory names searched under the root.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub directories: Vec<String>,
}

impl SkillSourceScope {
    /// Build a scope with ya-mono-compatible directories.
    #[must_use]
    pub fn new(root: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            directories: vec!["skills".to_string(), ".agents/skills".to_string()],
        }
    }
}

/// Fileops-loaded skill registry.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillRegistry {
    packages: BTreeMap<String, SkillPackage>,
}

impl SkillRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register discovered skill markdown files as relaxed view paths on an agent context.
    pub fn register_relaxed_view_patterns(&self, context: &mut AgentContext) {
        let patterns = self.relaxed_markdown_patterns();
        if patterns.is_empty() {
            context
                .tool_config
                .unregister_view_relaxed_text_patterns(SKILL_RELAXED_VIEW_SOURCE);
        } else {
            context
                .tool_config
                .register_view_relaxed_text_patterns(SKILL_RELAXED_VIEW_SOURCE, patterns);
        }
    }

    /// Return relaxed view regex patterns for all markdown files inside skill directories.
    #[must_use]
    pub fn relaxed_markdown_patterns(&self) -> Vec<String> {
        self.packages
            .values()
            .filter_map(|package| parent_path(&normalize_skill_path(&package.path)))
            .map(|directory| format!("re:^{}/.*\\.md$", regex_escape(&directory)))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Insert or replace a skill package.
    pub fn insert(&mut self, package: SkillPackage) {
        self.packages.insert(package.name.clone(), package);
    }

    /// Return a skill by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&SkillPackage> {
        self.packages.get(name)
    }

    /// Return all skill packages in stable name order.
    #[must_use]
    pub fn packages(&self) -> Vec<SkillPackage> {
        self.packages.values().cloned().collect()
    }

    /// Return whether the registry has no skills.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }

    /// Load skill summaries from provider-visible `SKILL.md` files.
    ///
    /// # Errors
    ///
    /// Returns an error when a discovered skill file is malformed.
    pub async fn scan(
        provider: DynEnvironmentProvider,
        scopes: &[SkillSourceScope],
    ) -> Result<Self, SkillError> {
        let mut registry = Self::new();
        for scope in scopes {
            for directory in &scope.directories {
                let base = join_path(&scope.root, directory);
                let matches = match provider
                    .glob(
                        &base,
                        "*/SKILL.md",
                        FileGlobOptions {
                            include_hidden: true,
                            include_ignored: true,
                            max_results: 0,
                        },
                    )
                    .await
                {
                    Ok(matches) => matches,
                    Err(EnvironmentError::NotFound(_) | EnvironmentError::AccessDenied(_)) => {
                        Vec::new()
                    }
                    Err(error) => return Err(SkillError::Environment(error)),
                };
                for entry in matches {
                    let text = provider.read_text(&entry.path).await?;
                    let mut package = parse_skill_markdown(&entry.path, &text)?;
                    package.body = None;
                    registry.insert(package);
                }
            }
        }
        Ok(registry)
    }

    /// Load one skill body from its provider path.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or parsed.
    pub async fn activate(
        provider: DynEnvironmentProvider,
        path: &str,
    ) -> Result<SkillPackage, SkillError> {
        let text = provider.read_text(path).await?;
        parse_skill_markdown(path, &text)
    }

    /// Convert loaded summaries into a model-facing instruction toolset.
    #[must_use]
    pub fn toolset(&self) -> DynToolset {
        skill_tools(self.packages())
    }
}

/// Create a toolset contributing available-skill instructions.
#[must_use]
pub fn skill_tools(packages: impl IntoIterator<Item = SkillPackage>) -> DynToolset {
    let mut lines = packages
        .into_iter()
        .map(|package| package.summary_line())
        .collect::<Vec<_>>();
    lines.sort();
    let content = if lines.is_empty() {
        "No fileops-loaded skills are currently available.".to_string()
    } else {
        format!("Available fileops-loaded skills:\n{}", lines.join("\n"))
    };
    Arc::new(
        StaticToolset::new("skills")
            .with_id("skills")
            .with_instruction(ToolInstruction::new("skills", content)),
    )
}

/// Skill loading error.
#[derive(Debug, Error)]
pub enum SkillError {
    /// File did not include frontmatter delimiters.
    #[error("invalid skill markdown: expected frontmatter delimited by ---")]
    MissingFrontmatter,
    /// Frontmatter could not be parsed.
    #[error("invalid skill frontmatter: {0}")]
    InvalidFrontmatter(String),
    /// Required field was absent.
    #[error("missing required skill field: {0}")]
    MissingField(&'static str),
    /// Provider operation failed.
    #[error(transparent)]
    Environment(#[from] starweaver_environment::EnvironmentError),
}

#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    #[serde(flatten)]
    extra: Metadata,
}

/// Parse one `SKILL.md` package.
///
/// # Errors
///
/// Returns an error when frontmatter is malformed or required fields are missing.
pub fn parse_skill_markdown(path: &str, content: &str) -> Result<SkillPackage, SkillError> {
    let trimmed = content.trim();
    let body_start = trimmed
        .strip_prefix("---")
        .ok_or(SkillError::MissingFrontmatter)?
        .trim_start_matches(['\r', '\n']);
    let (frontmatter, body) = body_start
        .split_once("\n---")
        .ok_or(SkillError::MissingFrontmatter)?;
    let body = body
        .strip_prefix("---")
        .unwrap_or(body)
        .trim_start_matches(['\r', '\n'])
        .trim()
        .to_string();
    let frontmatter: SkillFrontmatter = serde_yaml::from_str(frontmatter)
        .map_err(|error| SkillError::InvalidFrontmatter(error.to_string()))?;
    Ok(SkillPackage {
        name: frontmatter.name.ok_or(SkillError::MissingField("name"))?,
        description: frontmatter
            .description
            .ok_or(SkillError::MissingField("description"))?,
        path: path.to_string(),
        body: Some(body),
        metadata: frontmatter.extra,
    })
}

const SKILL_RELAXED_VIEW_SOURCE: &str = "skills:markdown";

fn normalize_skill_path(path: &str) -> String {
    let mut normalized = path.replace('\\', "/");
    if let Some(stripped) = normalized.strip_prefix("./") {
        normalized = stripped.to_string();
    }
    normalized.trim_end_matches('/').to_string()
}

fn parent_path(path: &str) -> Option<String> {
    path.rsplit_once('/').map(|(parent, _)| parent.to_string())
}

fn regex_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(
            ch,
            '.' | '+' | '*' | '?' | '^' | '$' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\'
        ) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

fn join_path(root: &str, path: &str) -> String {
    let root = if root == "/" {
        "/"
    } else {
        root.trim_end_matches('/')
    };
    let path = path.trim_matches('/');
    match (root.is_empty(), path.is_empty()) {
        (true, true) => String::new(),
        (true, false) => path.to_string(),
        (false, true) => root.to_string(),
        (false, false) if root == "/" => format!("/{path}"),
        (false, false) => format!("{root}/{path}"),
    }
}
