//! Config-backed slash command expansion.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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
}
