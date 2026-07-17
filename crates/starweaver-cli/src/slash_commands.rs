//! Config-backed slash command expansion.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use starweaver_agent::{SkillPackage, SkillRegistry};

/// Config-defined slash command prompt.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SlashCommandDefinition {
    /// Canonical command name without a leading slash.
    pub name: String,
    /// Prompt submitted when the command is invoked.
    pub prompt: String,
    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Additional aliases without a leading slash.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

/// Expanded prompt produced by invoking a config-backed slash command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpandedSlashCommand {
    /// Alias or command name typed by the user, normalized without `/`.
    pub invoked_name: String,
    /// Canonical command name, normalized without `/`.
    pub command_name: String,
    /// Prompt submitted to the agent.
    pub prompt: String,
    /// Free-form instruction text after the slash command name.
    pub args: String,
    /// Human-readable description.
    pub description: Option<String>,
}

/// One skill explicitly selected through a leading `/skill` or `@skill` token.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplicitSkillSelection {
    /// Name typed by the user without the leading marker.
    pub invoked_name: String,
    /// Resolved skill package.
    pub package: SkillPackage,
}

/// Prompt and ordered skill packages produced by explicit skill prefixes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpandedExplicitSkills {
    /// User request after removing all recognized leading skill tokens.
    pub prompt: String,
    /// Selected skills in first-seen order, deduplicated by canonical name.
    pub skills: Vec<ExplicitSkillSelection>,
}

/// Parse consecutive leading `/skill` or `@skill` tokens from `input`.
///
/// Every consecutive marker token must resolve to a loaded skill. Unknown tokens leave the input
/// untouched by returning `None`, preserving ordinary slash-prefixed prompts and configured slash
/// command precedence.
#[must_use]
pub fn expand_explicit_skills(
    registry: &SkillRegistry,
    input: &str,
) -> Option<ExpandedExplicitSkills> {
    let packages = registry.packages();
    let mut rest = input.trim();
    let mut skills = Vec::new();
    let mut selected = BTreeSet::new();

    while let Some(marker) = rest.chars().next().filter(|ch| matches!(ch, '/' | '@')) {
        let token_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let token = &rest[marker.len_utf8()..token_end];
        if !valid_command_name(token) || (marker == '/' && reserved_explicit_slash_name(token)) {
            return None;
        }
        let package = packages
            .iter()
            .find(|package| package.name == token)
            .or_else(|| {
                packages
                    .iter()
                    .find(|package| package.name.eq_ignore_ascii_case(token))
            })?
            .clone();
        if selected.insert(package.name.clone()) {
            skills.push(ExplicitSkillSelection {
                invoked_name: token.to_string(),
                package,
            });
        }
        rest = rest[token_end..].trim_start();
    }

    (!skills.is_empty()).then(|| ExpandedExplicitSkills {
        prompt: rest.trim_end().to_string(),
        skills,
    })
}

fn reserved_explicit_slash_name(name: &str) -> bool {
    matches!(
        normalize_command_name(name).as_str(),
        "help"
            | "config"
            | "loop"
            | "tasks"
            | "session"
            | "dump"
            | "load"
            | "clear"
            | "cost"
            | "exit"
            | "model"
            | "paste-image"
            | "goal"
            | "display"
    )
}

/// Expand `input` when it invokes a configured slash command.
pub fn expand_slash_command(
    commands: &BTreeMap<String, SlashCommandDefinition>,
    input: &str,
) -> Option<ExpandedSlashCommand> {
    let trimmed = input.trim();
    let rest = trimmed.strip_prefix('/')?.trim_start();
    if rest.is_empty() {
        return None;
    }
    let name_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let invoked_name = normalize_command_name(&rest[..name_end]);
    if invoked_name.is_empty() {
        return None;
    }
    let args = rest[name_end..].trim().to_string();
    let definition = commands.get(&invoked_name)?;
    Some(ExpandedSlashCommand {
        invoked_name,
        command_name: normalize_command_name(&definition.name),
        prompt: prompt_with_args(&definition.prompt, &args),
        args,
        description: definition.description.clone(),
    })
}

/// Normalize a slash command name for map storage and lookup.
#[must_use]
pub fn normalize_command_name(name: &str) -> String {
    name.trim().trim_start_matches('/').to_ascii_lowercase()
}

/// Return whether a normalized slash command name can be invoked as one token.
#[must_use]
pub fn valid_command_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

const ARG_PLACEHOLDERS: [(&str, usize); 4] = [
    ("{{instruction}}", "{{instruction}}".len()),
    ("{{args}}", "{{args}}".len()),
    ("{instruction}", "{instruction}".len()),
    ("{args}", "{args}".len()),
];

fn next_args_placeholder(value: &str) -> Option<(usize, usize)> {
    ARG_PLACEHOLDERS
        .iter()
        .filter_map(|(placeholder, len)| value.find(placeholder).map(|index| (index, *len)))
        .min_by_key(|(index, _)| *index)
}

fn prompt_with_args(prompt: &str, args: &str) -> String {
    if next_args_placeholder(prompt).is_some() {
        let mut output = String::new();
        let mut rest = prompt;
        while let Some((index, placeholder_len)) = next_args_placeholder(rest) {
            output.push_str(&rest[..index]);
            output.push_str(args);
            rest = &rest[index + placeholder_len..];
        }
        output.push_str(rest);
        return output;
    }
    if args.is_empty() {
        return prompt.to_string();
    }
    let mut expanded = prompt.trim_end().to_string();
    if !expanded.is_empty() {
        expanded.push_str("\n\n");
    }
    expanded.push_str("User instruction: ");
    expanded.push_str(args);
    expanded
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn command(prompt: &str) -> SlashCommandDefinition {
        SlashCommandDefinition {
            name: "review".to_string(),
            prompt: prompt.to_string(),
            description: Some("Review changes".to_string()),
            aliases: vec!["rv".to_string()],
        }
    }

    #[test]
    fn expands_command_alias_and_arguments() {
        let mut commands = BTreeMap::new();
        let definition = command("Review the changes.");
        commands.insert("review".to_string(), definition.clone());
        commands.insert("rv".to_string(), definition);

        let expanded = expand_slash_command(&commands, " /RV src/lib.rs ").unwrap();
        assert_eq!(expanded.invoked_name, "rv");
        assert_eq!(expanded.command_name, "review");
        assert_eq!(
            expanded.prompt,
            "Review the changes.\n\nUser instruction: src/lib.rs"
        );
    }

    #[test]
    fn replaces_args_placeholders_when_present() {
        let mut commands = BTreeMap::new();
        commands.insert("test".to_string(), command("Run tests for {{args}}."));

        let expanded = expand_slash_command(&commands, "/test crates/starweaver-cli").unwrap();
        assert_eq!(expanded.prompt, "Run tests for crates/starweaver-cli.");
    }

    #[test]
    fn replaces_instruction_placeholders_when_present() {
        let mut commands = BTreeMap::new();
        commands.insert(
            "commit".to_string(),
            command("Create a commit using {instruction}."),
        );

        let expanded = expand_slash_command(&commands, "/commit staged fixes").unwrap();
        assert_eq!(expanded.prompt, "Create a commit using staged fixes.");
    }

    #[test]
    fn ignores_non_commands_and_unknown_commands() {
        let commands = BTreeMap::new();
        assert!(expand_slash_command(&commands, "hello").is_none());
        assert!(expand_slash_command(&commands, "/missing args").is_none());
    }

    fn skill(name: &str) -> SkillPackage {
        SkillPackage {
            name: name.to_string(),
            description: format!("Use {name}"),
            path: format!("/skills/{name}/SKILL.md"),
            body: Some(format!("# {name}")),
            metadata: serde_json::Map::default(),
        }
    }

    #[test]
    fn expands_multiple_explicit_skills_in_user_order() {
        let mut registry = SkillRegistry::new();
        registry.insert(skill("lark-cli"));
        registry.insert(skill("building-agent"));

        let expanded =
            expand_explicit_skills(&registry, "/lark-cli @building-agent create an agent").unwrap();

        assert_eq!(expanded.prompt, "create an agent");
        assert_eq!(
            expanded
                .skills
                .iter()
                .map(|skill| skill.package.name.as_str())
                .collect::<Vec<_>>(),
            ["lark-cli", "building-agent"]
        );
    }

    #[test]
    fn explicit_skills_are_case_insensitive_and_deduplicated() {
        let mut registry = SkillRegistry::new();
        registry.insert(skill("lark-cli"));

        let expanded =
            expand_explicit_skills(&registry, "/LARK-CLI @lark-cli send a message").unwrap();

        assert_eq!(expanded.prompt, "send a message");
        assert_eq!(expanded.skills.len(), 1);
        assert_eq!(expanded.skills[0].invoked_name, "LARK-CLI");
    }

    #[test]
    fn unknown_consecutive_skill_token_leaves_input_untouched() {
        let mut registry = SkillRegistry::new();
        registry.insert(skill("lark-cli"));

        assert!(expand_explicit_skills(&registry, "/lark-cli /missing send a message").is_none());
        assert!(expand_explicit_skills(&registry, "/missing send a message").is_none());
    }

    #[test]
    fn reserved_slash_names_win_but_at_alias_can_activate_same_named_skill() {
        let mut registry = SkillRegistry::new();
        registry.insert(skill("help"));

        assert!(expand_explicit_skills(&registry, "/help explain").is_none());
        assert_eq!(
            expand_explicit_skills(&registry, "@help explain")
                .unwrap()
                .prompt,
            "explain"
        );
    }
}
