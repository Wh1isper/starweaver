//! Execution profile management.

use std::{collections::BTreeMap, fs, path::Path};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::{ClawError, ClawResult, ClawSettings, WorkspaceBackend};

/// Reusable agent execution profile.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentProfile {
    /// Profile name.
    pub name: String,
    /// Model name or provider-qualified model id.
    pub model: String,
    /// Optional model settings preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_settings_preset: Option<String>,
    /// Optional model settings override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_settings_override: Option<Value>,
    /// Optional model config preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_config_preset: Option<String>,
    /// Optional model config override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_config_override: Option<Value>,
    /// System prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Built-in toolsets to enable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub builtin_toolsets: Vec<String>,
    /// Subagent specs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subagents: Vec<Value>,
    /// Include first-party subagents.
    #[serde(default)]
    pub include_builtin_subagents: bool,
    /// Use unified subagent model.
    #[serde(default)]
    pub unified_subagents: bool,
    /// Tool approval list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub need_user_approve_tools: Vec<String>,
    /// MCP approval list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub need_user_approve_mcps: Vec<String>,
    /// Enabled MCP servers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enabled_mcps: Vec<String>,
    /// Disabled MCP servers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_mcps: Vec<String>,
    /// Inline MCP server config.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub mcp_servers: serde_json::Map<String, Value>,
    /// Backend hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_backend_hint: Option<WorkspaceBackend>,
    /// Enabled flag.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Source type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,
    /// Source version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_version: Option<String>,
    /// Creation time.
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    /// Update time.
    #[serde(default = "Utc::now")]
    pub updated_at: DateTime<Utc>,
}

impl AgentProfile {
    /// Build a minimal default profile.
    #[must_use]
    pub fn default_named(name: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            name: name.into(),
            model: "test".to_string(),
            model_settings_preset: None,
            model_settings_override: None,
            model_config_preset: None,
            model_config_override: None,
            system_prompt: Some("You are a Starweaver Claw agent.".to_string()),
            builtin_toolsets: vec!["filesystem".to_string(), "shell".to_string()],
            subagents: Vec::new(),
            include_builtin_subagents: false,
            unified_subagents: false,
            need_user_approve_tools: Vec::new(),
            need_user_approve_mcps: Vec::new(),
            enabled_mcps: Vec::new(),
            disabled_mcps: Vec::new(),
            mcp_servers: serde_json::Map::new(),
            workspace_backend_hint: None,
            enabled: true,
            source_type: Some("generated".to_string()),
            source_version: None,
            created_at: now,
            updated_at: now,
        }
    }
}

/// In-process profile resolver.
#[derive(Debug)]
pub struct ProfileResolver {
    default_profile: String,
    profiles: RwLock<BTreeMap<String, AgentProfile>>,
}

impl ProfileResolver {
    /// Build resolver with one default profile.
    #[must_use]
    pub fn new(settings: &ClawSettings) -> Self {
        let default_profile = settings.default_profile.clone();
        let mut profiles = BTreeMap::new();
        profiles.insert(
            default_profile.clone(),
            AgentProfile::default_named(default_profile.clone()),
        );
        Self {
            default_profile,
            profiles: RwLock::new(profiles),
        }
    }

    /// Seed profiles from YAML.
    ///
    /// # Errors
    ///
    /// Returns parse errors for invalid YAML profile documents.
    pub async fn seed_yaml_file(&self, path: impl AsRef<Path>) -> ClawResult<Vec<AgentProfile>> {
        let content = fs::read_to_string(path)?;
        let seed: ProfileSeed = serde_yaml::from_str(&content)?;
        let mut inserted = Vec::new();
        let mut profiles = self.profiles.write().await;
        for mut profile in seed.into_profiles() {
            profile.updated_at = Utc::now();
            profiles.insert(profile.name.clone(), profile.clone());
            inserted.push(profile);
        }
        Ok(inserted)
    }

    /// Upsert one profile.
    pub async fn upsert(&self, mut profile: AgentProfile) -> AgentProfile {
        profile.updated_at = Utc::now();
        let mut profiles = self.profiles.write().await;
        profiles.insert(profile.name.clone(), profile.clone());
        profile
    }

    /// Resolve a profile name or the default.
    ///
    /// # Errors
    ///
    /// Returns missing or disabled profile errors.
    pub async fn resolve(&self, name: Option<&str>) -> ClawResult<AgentProfile> {
        let name = name.unwrap_or(&self.default_profile);
        let profiles = self.profiles.read().await;
        let profile = profiles
            .get(name)
            .cloned()
            .ok_or_else(|| ClawError::NotFound(format!("profile '{name}' was not found")))?;
        if profile.enabled {
            Ok(profile)
        } else {
            Err(ClawError::InvalidRequest(format!(
                "profile '{name}' is disabled"
            )))
        }
    }

    /// Get one profile by name.
    pub async fn get(&self, name: &str) -> Option<AgentProfile> {
        self.profiles.read().await.get(name).cloned()
    }

    /// Delete one profile by name.
    pub async fn delete(&self, name: &str) -> bool {
        self.profiles.write().await.remove(name).is_some()
    }

    /// List profiles.
    pub async fn list(&self) -> Vec<AgentProfile> {
        self.profiles.read().await.values().cloned().collect()
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ProfileSeed {
    List(Vec<AgentProfile>),
    Wrapped { profiles: Vec<AgentProfile> },
    Map(BTreeMap<String, AgentProfileSeedItem>),
}

impl ProfileSeed {
    fn into_profiles(self) -> Vec<AgentProfile> {
        match self {
            Self::List(profiles) | Self::Wrapped { profiles } => profiles,
            Self::Map(map) => map
                .into_iter()
                .map(|(name, item)| item.into_profile(name))
                .collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct AgentProfileSeedItem {
    model: String,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default)]
    builtin_toolsets: Vec<String>,
    #[serde(default)]
    enabled: Option<bool>,
}

impl AgentProfileSeedItem {
    fn into_profile(self, name: String) -> AgentProfile {
        let mut profile = AgentProfile::default_named(name);
        profile.model = self.model;
        profile.system_prompt = self.system_prompt;
        profile.builtin_toolsets = self.builtin_toolsets;
        profile.enabled = self.enabled.unwrap_or(true);
        profile.source_type = Some("yaml".to_string());
        profile
    }
}

const fn default_true() -> bool {
    true
}
