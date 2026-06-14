use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{default_skills_dir, default_true, is_false};

/// Skill bundle configuration for fileops-loaded skills.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillBundleSpec {
    /// Whether the skill bundle is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Provider-visible roots to scan.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<String>,
    /// Primary skills directory name.
    #[serde(default = "default_skills_dir")]
    pub skills_dir_name: String,
    /// Additional directory names, such as `.agents/skills`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_dir_names: Vec<String>,
    /// Whether hot reload should happen at request boundaries.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hot_reload: bool,
    /// Stable pre-scan hook name resolved by the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_scan_hook: Option<String>,
}

impl Default for SkillBundleSpec {
    fn default() -> Self {
        Self {
            enabled: true,
            roots: Vec::new(),
            skills_dir_name: default_skills_dir(),
            extra_dir_names: Vec::new(),
            hot_reload: false,
            pre_scan_hook: None,
        }
    }
}

/// Serializable host adapter reference.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostAdapterSpec {
    /// Stable adapter kind, such as search, scrape, download, or media.
    pub kind: String,
    /// Host adapter name resolved by the SDK host.
    pub name: String,
    /// Adapter metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

/// Serializable MCP server reference.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct McpServerSpec {
    /// Stable MCP server name resolved by the SDK host.
    pub name: String,
    /// Transport kind, such as `stdio` or `streamable_http`.
    pub transport: String,
    /// Server metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

/// Template string rendered from dependency values by SDK hosts.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TemplateStringSpec {
    /// Stable template name.
    pub name: String,
    /// Template body. Variables use `{{path.to.value}}` placeholders.
    pub template: String,
    /// Target host field, such as `instruction` or `metadata.title`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// Toolset wrapper requested by an agent spec.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolsetWrapperSpec {
    /// Wrapper kind, such as `filtered`, `renamed`, `approval_required`, `dynamic`, or `deferred_loading`.
    pub kind: String,
    /// Registry key for the inner toolset when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolset: Option<String>,
    /// Wrapper parameters validated by the host.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
}

/// Serializable host adapter policy.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostPolicySpec {
    /// Host adapter kind, such as `agui`, `vercel_ai`, or `cli`.
    pub kind: String,
    /// Trust mode used by request/history sanitizers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust: Option<String>,
    /// Sanitizer names to apply at host boundaries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sanitizers: Vec<String>,
    /// Adapter-specific policy metadata.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

/// Environment/workspace policy requested by an agent spec.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspacePolicySpec {
    /// Workspace provider or profile name resolved by the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Allowed root or mount names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<String>,
    /// Shell execution policy such as `disabled`, `review`, or `trusted`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// Sandbox policy such as `local`, `docker`, or `remote`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    /// Policy metadata recorded by the host.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}
