//! Shared command metadata for TUI discovery, completion, and help.

use std::collections::{BTreeMap, BTreeSet};

use crate::{profiles::SkillSummary, slash_commands::SlashCommandDefinition};

/// Origin of a command shown by the TUI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandSource {
    BuiltIn,
    Config,
    Skill,
}

impl CommandSource {
    pub const fn label(self) -> &'static str {
        match self {
            Self::BuiltIn => "built-in",
            Self::Config => "custom",
            Self::Skill => "skill",
        }
    }
}

/// Argument completion strategy for one command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandArguments {
    None,
    FreeForm,
    DisplayMode,
    ModelProfile,
    Session,
}

/// One discoverable command and its aliases.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandDescriptor {
    pub name: String,
    pub aliases: Vec<String>,
    pub usage: String,
    pub description: String,
    pub source: CommandSource,
    pub arguments: CommandArguments,
    pub show_on_startup: bool,
}

/// One discoverable keyboard shortcut shared by help surfaces.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyBindingDescriptor {
    pub keys: &'static str,
    pub description: &'static str,
}

/// Keyboard shortcut metadata used by startup, overlay, and transcript help.
pub const fn key_binding_descriptors() -> &'static [KeyBindingDescriptor] {
    &[
        KeyBindingDescriptor {
            keys: "Enter",
            description: "Send, steer, or confirm the active modal",
        },
        KeyBindingDescriptor {
            keys: "Tab",
            description: "Complete the selected slash command",
        },
        KeyBindingDescriptor {
            keys: "Ctrl+O",
            description: "Insert a newline",
        },
        KeyBindingDescriptor {
            keys: "Ctrl+V",
            description: "Attach an image from the system clipboard",
        },
        KeyBindingDescriptor {
            keys: "Ctrl+P/N",
            description: "Browse prompt history",
        },
        KeyBindingDescriptor {
            keys: "Ctrl+R",
            description: "Search prompt history",
        },
        KeyBindingDescriptor {
            keys: "Up/Down",
            description: "Move across visual composer lines",
        },
        KeyBindingDescriptor {
            keys: "Ctrl+A/E",
            description: "Move to line start/end",
        },
        KeyBindingDescriptor {
            keys: "Alt+Left/Right",
            description: "Move by word",
        },
        KeyBindingDescriptor {
            keys: "Command+Left/Right",
            description: "Move to line start/end",
        },
        KeyBindingDescriptor {
            keys: "Alt+Up/Down",
            description: "Scroll multiline input",
        },
        KeyBindingDescriptor {
            keys: "PageUp/PageDown",
            description: "Scroll transcript",
        },
        KeyBindingDescriptor {
            keys: "Mouse wheel",
            description: "Scroll transcript",
        },
        KeyBindingDescriptor {
            keys: "Ctrl+L",
            description: "Jump to live output",
        },
        KeyBindingDescriptor {
            keys: "? or F1",
            description: "Open contextual help from an empty composer",
        },
        KeyBindingDescriptor {
            keys: "Esc",
            description: "Close the active modal or select transcript",
        },
        KeyBindingDescriptor {
            keys: "Ctrl+C",
            description: "Interrupt, clear a draft, or exit",
        },
        KeyBindingDescriptor {
            keys: "Ctrl+D",
            description: "Exit only from an empty idle composer",
        },
        KeyBindingDescriptor {
            keys: "A/Y or R/N",
            description: "Approve or reject a pending action",
        },
    ]
}

/// Built-in command metadata. Runtime handlers and all help surfaces share this list.
pub fn builtin_command_descriptors() -> Vec<CommandDescriptor> {
    [
        (
            "help",
            "/help",
            "Print command and shortcut help in the transcript",
            CommandArguments::None,
            true,
        ),
        (
            "clear",
            "/clear",
            "Clear output and start a fresh context",
            CommandArguments::None,
            false,
        ),
        (
            "cost",
            "/cost",
            "Show usage, context, and estimated cost",
            CommandArguments::None,
            false,
        ),
        (
            "display",
            "/display [normal|concise|debug]",
            "Show or change transcript detail",
            CommandArguments::DisplayMode,
            true,
        ),
        (
            "model",
            "/model [profile]",
            "Open or select a model profile",
            CommandArguments::ModelProfile,
            true,
        ),
        (
            "session",
            "/session [id]",
            "Open the session selector or reload a session",
            CommandArguments::Session,
            true,
        ),
        (
            "tasks",
            "/tasks",
            "Open the task list",
            CommandArguments::None,
            false,
        ),
        (
            "goal",
            "/goal <task>",
            "Run toward a verified goal",
            CommandArguments::FreeForm,
            false,
        ),
        (
            "paste-image",
            "/paste-image",
            "Attach an image from the system clipboard",
            CommandArguments::None,
            false,
        ),
    ]
    .into_iter()
    .map(
        |(name, usage, description, arguments, show_on_startup)| CommandDescriptor {
            name: name.to_string(),
            aliases: Vec::new(),
            usage: usage.to_string(),
            description: description.to_string(),
            source: CommandSource::BuiltIn,
            arguments,
            show_on_startup,
        },
    )
    .collect()
}

pub fn command_descriptors(
    custom_commands: &BTreeMap<String, SlashCommandDefinition>,
    skills: &[SkillSummary],
) -> Vec<CommandDescriptor> {
    let mut descriptors = builtin_command_descriptors();
    let reserved = builtin_command_names();
    let mut custom_seen = BTreeSet::new();
    for definition in custom_commands.values() {
        let name = definition
            .name
            .trim()
            .trim_start_matches('/')
            .to_ascii_lowercase();
        if name.is_empty() || reserved.contains(name.as_str()) || !custom_seen.insert(name.clone())
        {
            continue;
        }
        let mut aliases = definition
            .aliases
            .iter()
            .map(|alias| alias.trim().trim_start_matches('/').to_ascii_lowercase())
            .filter(|alias| !alias.is_empty() && !reserved.contains(alias.as_str()))
            .collect::<Vec<_>>();
        aliases.sort();
        aliases.dedup();
        descriptors.push(CommandDescriptor {
            usage: format!("/{name} [instruction]"),
            description: definition
                .description
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "Run configured prompt".to_string()),
            name,
            aliases,
            source: CommandSource::Config,
            arguments: CommandArguments::FreeForm,
            show_on_startup: false,
        });
    }
    let occupied = descriptors
        .iter()
        .flat_map(|descriptor| {
            std::iter::once(descriptor.name.clone()).chain(descriptor.aliases.iter().cloned())
        })
        .collect::<BTreeSet<_>>();
    for skill in skills {
        let name = skill.name.trim().to_string();
        if name.is_empty() || occupied.contains(name.as_str()) {
            continue;
        }
        descriptors.push(CommandDescriptor {
            usage: format!("/{name} [task]"),
            description: skill.description.clone(),
            name,
            aliases: Vec::new(),
            source: CommandSource::Skill,
            arguments: CommandArguments::FreeForm,
            show_on_startup: false,
        });
    }
    descriptors.sort_by(|left, right| {
        command_source_rank(left.source)
            .cmp(&command_source_rank(right.source))
            .then_with(|| left.name.cmp(&right.name))
    });
    descriptors
}

pub fn builtin_command_names() -> BTreeSet<&'static str> {
    BTreeSet::from([
        "help",
        "clear",
        "cost",
        "display",
        "model",
        "session",
        "tasks",
        "goal",
        "paste-image",
    ])
}

pub fn closest_builtin_name(name: &str) -> Option<&'static str> {
    let normalized = name.trim().trim_start_matches('/').to_ascii_lowercase();
    builtin_command_names()
        .into_iter()
        .map(|candidate| (edit_distance(&normalized, candidate), candidate))
        .filter(|(distance, _)| *distance <= 2)
        .min_by_key(|(distance, candidate)| (*distance, *candidate))
        .map(|(_, candidate)| candidate)
}

const fn command_source_rank(source: CommandSource) -> u8 {
    match source {
        CommandSource::BuiltIn => 0,
        CommandSource::Config => 1,
        CommandSource::Skill => 2,
    }
}

fn edit_distance(left: &str, right: &str) -> usize {
    let mut previous = (0..=right.chars().count()).collect::<Vec<_>>();
    for (left_index, left_char) in left.chars().enumerate() {
        let mut current = vec![left_index + 1];
        for (right_index, right_char) in right.chars().enumerate() {
            let insert = current[right_index] + 1;
            let delete = previous[right_index + 1] + 1;
            let replace = previous[right_index] + usize::from(left_char != right_char);
            current.push(insert.min(delete).min(replace));
        }
        previous = current;
    }
    previous.last().copied().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_and_skill_descriptors_respect_command_precedence() {
        let mut custom = BTreeMap::new();
        custom.insert(
            "rv".to_string(),
            SlashCommandDefinition {
                name: "review".to_string(),
                prompt: "Review".to_string(),
                description: Some("Review changes".to_string()),
                aliases: vec!["rv".to_string(), "model".to_string()],
            },
        );
        let skills = vec![
            SkillSummary {
                name: "review".to_string(),
                description: "Skill review".to_string(),
                path: "review/SKILL.md".to_string(),
            },
            SkillSummary {
                name: "research".to_string(),
                description: "Research".to_string(),
                path: "research/SKILL.md".to_string(),
            },
        ];

        let descriptors = command_descriptors(&custom, &skills);
        let Some(review) = descriptors
            .iter()
            .find(|descriptor| descriptor.name == "review")
        else {
            panic!("custom review command should be present");
        };
        assert_eq!(review.source, CommandSource::Config);
        assert_eq!(review.aliases, ["rv"]);
        assert!(descriptors.iter().any(|descriptor| {
            descriptor.name == "research" && descriptor.source == CommandSource::Skill
        }));
    }

    #[test]
    fn typo_matching_is_bounded() {
        assert_eq!(closest_builtin_name("modle"), Some("model"));
        assert_eq!(closest_builtin_name("/sesion"), Some("session"));
        assert_eq!(closest_builtin_name("completely-unrelated"), None);
    }
}
