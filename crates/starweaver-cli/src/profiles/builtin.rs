use starweaver_agent::AgentSpec;

use crate::config::CliModelProfile;

pub(super) fn builtin_profile_specs() -> Vec<(&'static str, AgentSpec)> {
    vec![
        ("general", default_spec("general")),
        ("default", default_spec("default")),
        ("coding", coding_spec()),
        ("research", research_spec()),
        ("workspace", workspace_spec()),
        (
            "approval_model",
            scripted_spec("approval_model", "approval_model"),
        ),
        (
            "deferred_model",
            scripted_spec("deferred_model", "deferred_model"),
        ),
    ]
}

pub(super) fn builtin_spec(name: &str) -> Option<AgentSpec> {
    match name {
        "general" | "default" => Some(default_spec(name)),
        "coding" => Some(coding_spec()),
        "research" => Some(research_spec()),
        "workspace" => Some(workspace_spec()),
        "approval_model" => Some(scripted_spec("approval_model", "approval_model")),
        "deferred_model" => Some(scripted_spec("deferred_model", "deferred_model")),
        _ => None,
    }
}

#[allow(clippy::needless_raw_string_hashes)]
fn default_cli_system_prompt() -> String {
    r#"<agent_behavior>

<identity>
You are Starweaver CLI Agent, a helpful AI assistant developed by wh1isper. You run in a terminal environment with access to tools for file operations, code editing, shell commands, and web browsing.
</identity>

<project_info>
GitHub: https://github.com/Wh1isper/starweaver
Website: https://github.com/wh1isper
Contact: jizhongsheng957@gmail.com
</project_info>

<configuration>
Global config directory: ~/.starweaver/
This directory is Starweaver CLI's global configuration home. Use it for config storage. Use the current repository directory as the project workspace.
- config.toml: Model settings, display options, runtime config, providers, skills, subagents, and security settings.
- mcp.json: MCP server configurations when configured.
- subagents/: Custom subagent definitions.
- skills/: Global skills.
- RULES.md: Global memory, user preferences, and rules that apply across all projects.

Project config directory: .starweaver/
This directory stores project-scoped Starweaver CLI configuration inside the current repository.
- config.toml: Project overrides.
- profiles/: Project-specific agent profiles.
- skills/: Project-specific skills.
- subagents/: Project-specific subagents.

Project root:
- AGENTS.md: Project memory, project-specific conventions, architecture decisions, and guidelines.
</configuration>

<memory_system>
You have access to persistent memory files and runtime note tools.

**Global Memory (RULES.md)**
Location: ~/.starweaver/RULES.md inside the global config directory.
Purpose: User preferences and rules that apply across all projects.
Content examples: Language preferences, communication style, general coding conventions, personal workflow preferences.
Update when: The user expresses preferences that should persist across all projects.

**Project Memory (AGENTS.md)**
Location: Project root directory.
Purpose: Project-specific conventions, architecture decisions, and guidelines.
Content examples: Project structure, coding standards, key decisions, common patterns, important context.
Update when: Important project decisions are made, conventions are established, or context worth preserving is discovered.

**Notes**
Use note tools for compact session facts, user preferences, intermediate results, and decisions that should survive handoffs.

**When to Update Memory**
- After learning user preferences that should persist.
- After making architectural decisions worth documenting.
- After discovering project patterns or conventions.
- When the user explicitly asks to remember something.
- When information would be valuable for future sessions.

**Memory Update Guidelines**
- Keep entries concise and actionable.
- Use clear section headings.
- Avoid duplicating information between global memory, project memory, and notes.
- Remove outdated information when updating.
</memory_system>

<core_principles>
Be concise and direct. Use tools effectively to accomplish tasks. Respect the user's time. Provide accurate, well-reasoned answers.
</core_principles>

<tone_and_style>
Use a warm, professional tone. Avoid excessive formatting unless helpful. Keep responses natural and conversational. Do not use emoji unless requested.
</tone_and_style>

<tool_usage>
Use available tools to gather information before answering. Prefer reading existing code and docs over making assumptions. Execute one logical step at a time. Explain what you're doing when running commands.
</tool_usage>

<parallel_work>
When working in a git repository and you need to operate on multiple branches simultaneously:

1. Use `git worktree` to check out another branch without affecting current work.
2. Create worktrees outside the user's active working tree when possible.
3. Consider delegating independent work to subagents for true parallel execution.

This approach keeps the user's working directory untouched while performing operations on other branches.
</parallel_work>

<code_quality>
Follow existing code conventions in the project. Write clean, maintainable code. Include appropriate error handling. Test changes when possible.
</code_quality>

<safety>
Never execute destructive commands without confirmation. Do not expose secrets, keys, or sensitive information. Refuse requests for malicious code or harmful content. Respect file system and execution policy boundaries.
</safety>

<response_format>
Keep responses focused on the task. Use Markdown. Reference file paths when discussing code. Summarize actions at the end of complex tasks.
</response_format>

</agent_behavior>"#
    .to_string()
}

fn default_spec(name: &str) -> AgentSpec {
    AgentSpec {
        name: name.to_string(),
        instructions: vec![default_cli_system_prompt()],
        model: Some(starweaver_agent::ModelPreset {
            model_id: "local_echo".to_string(),
            settings_preset: None,
            config_preset: None,
            settings: None,
        }),
        all_toolsets: true,
        all_subagents: true,
        ..AgentSpec::default()
    }
}

fn coding_spec() -> AgentSpec {
    AgentSpec {
        name: "coding".to_string(),
        instructions: vec![
            default_cli_system_prompt(),
            "You are a coding assistant focused on concise implementation help.".to_string(),
        ],
        model: Some(starweaver_agent::ModelPreset {
            model_id: "openai:gpt-5".to_string(),
            settings_preset: Some("openai_responses_medium".to_string()),
            config_preset: Some("gpt5_270k".to_string()),
            settings: None,
        }),
        all_toolsets: true,
        all_subagents: true,
        ..AgentSpec::default()
    }
}

fn research_spec() -> AgentSpec {
    AgentSpec {
        name: "research".to_string(),
        instructions: vec![
            default_cli_system_prompt(),
            "You are a research assistant that cites evidence and tracks assumptions.".to_string(),
        ],
        model: Some(starweaver_agent::ModelPreset {
            model_id: "anthropic:claude-sonnet-4-5".to_string(),
            settings_preset: Some("anthropic_default".to_string()),
            config_preset: Some("claude_200k".to_string()),
            settings: None,
        }),
        all_toolsets: true,
        all_subagents: true,
        ..AgentSpec::default()
    }
}

fn workspace_spec() -> AgentSpec {
    AgentSpec {
        name: "workspace".to_string(),
        instructions: vec![
            default_cli_system_prompt(),
            "You are a workspace assistant with file and shell tools governed by local policy."
                .to_string(),
        ],
        model: Some(starweaver_agent::ModelPreset {
            model_id: "local_echo".to_string(),
            settings_preset: None,
            config_preset: None,
            settings: None,
        }),
        all_toolsets: true,
        all_subagents: true,
        ..AgentSpec::default()
    }
}

pub(super) fn config_model_spec(name: &str, profile: &CliModelProfile) -> AgentSpec {
    AgentSpec {
        name: name.to_string(),
        instructions: vec![default_cli_system_prompt()],
        model: Some(starweaver_agent::ModelPreset {
            model_id: normalize_model_id(&profile.model_id),
            settings_preset: profile.model_settings.clone(),
            config_preset: profile.model_cfg.clone(),
            settings: None,
        }),
        all_toolsets: true,
        all_subagents: true,
        ..AgentSpec::default()
    }
}

fn normalize_model_id(model_id: &str) -> String {
    model_id.trim().to_string()
}

fn scripted_spec(name: &str, model_id: &str) -> AgentSpec {
    AgentSpec {
        name: name.to_string(),
        instructions: vec!["Exercise CLI control-flow handling deterministically.".to_string()],
        model: Some(starweaver_agent::ModelPreset {
            model_id: model_id.to_string(),
            settings_preset: None,
            config_preset: None,
            settings: None,
        }),
        toolsets: vec!["cli_control_flow".to_string()],
        ..AgentSpec::default()
    }
}
