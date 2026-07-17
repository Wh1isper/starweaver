use std::ops::Range;

use crate::{
    command_catalog::{CommandArguments, CommandDescriptor, CommandSource, command_descriptors},
    profiles::SkillSummary,
};

use super::InteractiveTuiState;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::tui) struct CommandPaletteItem {
    pub(in crate::tui) label: String,
    pub(in crate::tui) detail: String,
    pub(in crate::tui) source: CommandSource,
    replacement: String,
    replacement_range: Range<usize>,
    execute_after_accept: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::tui) struct CommandPaletteState {
    pub(in crate::tui) title: String,
    pub(in crate::tui) items: Vec<CommandPaletteItem>,
    pub(in crate::tui) selected: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) enum CommandPaletteAccept {
    Updated,
    Execute,
}

impl InteractiveTuiState {
    pub(crate) fn set_skills(&mut self, skills: Vec<SkillSummary>) {
        self.skills = skills;
        self.refresh_command_palette();
    }

    pub(in crate::tui) const fn command_palette(&self) -> Option<&CommandPaletteState> {
        self.command_palette.as_ref()
    }

    pub(in crate::tui) const fn command_palette_visible(&self) -> bool {
        self.command_palette.is_some()
    }

    pub(in crate::tui) fn close_command_palette(&mut self) {
        self.command_palette = None;
        self.command_palette_dismissed_input = Some(self.input.clone());
        self.input_status = Some("command suggestions closed".to_string());
    }

    pub(in crate::tui) fn move_command_palette_selection(&mut self, delta: isize) {
        let Some(palette) = self.command_palette.as_mut() else {
            return;
        };
        let len = palette.items.len();
        if len == 0 {
            return;
        }
        let steps = delta.unsigned_abs() % len;
        palette.selected = if delta.is_negative() {
            (palette.selected + len - steps) % len
        } else {
            (palette.selected + steps) % len
        };
        self.input_status = Some("command completion".to_string());
    }

    pub(in crate::tui) fn accept_command_palette_selection(
        &mut self,
        execute: bool,
    ) -> Option<CommandPaletteAccept> {
        let item = self
            .command_palette
            .as_ref()?
            .items
            .get(self.command_palette.as_ref()?.selected)?
            .clone();
        self.input
            .replace_range(item.replacement_range.clone(), &item.replacement);
        self.input_cursor = item.replacement_range.start + item.replacement.len();
        self.input_cursor_input_len = self.input.len();
        self.composer_preferred_column = None;
        self.reset_composer_scroll();
        self.command_palette_dismissed_input = None;
        self.refresh_command_palette();
        if execute && item.execute_after_accept {
            self.command_palette = None;
            self.command_palette_dismissed_input = Some(self.input.clone());
            return Some(CommandPaletteAccept::Execute);
        }
        self.input_status = Some("command completed".to_string());
        Some(CommandPaletteAccept::Updated)
    }

    pub(in crate::tui) fn refresh_command_palette(&mut self) {
        if self.command_palette_dismissed_input.as_deref() == Some(self.input.as_str()) {
            self.command_palette = None;
            return;
        }
        let cursor = self.composer_cursor_byte();
        let Some(prefix) = self.input.get(..cursor) else {
            self.command_palette = None;
            return;
        };
        if !prefix.starts_with('/') || prefix.contains('\n') {
            self.command_palette = None;
            return;
        }
        let descriptors = command_descriptors(&self.custom_commands, &self.skills);
        let first_whitespace = prefix.find(char::is_whitespace);
        let items = first_whitespace.map_or_else(
            || Self::command_name_palette_items(&descriptors, &prefix[1..], cursor),
            |separator| {
                let invoked = &prefix[1..separator];
                self.argument_palette_items(&descriptors, invoked, separator, prefix)
            },
        );
        if items.is_empty() {
            self.command_palette = None;
            return;
        }
        let previous_label = self
            .command_palette
            .as_ref()
            .and_then(|palette| palette.items.get(palette.selected))
            .map(|item| item.label.clone());
        let selected = previous_label
            .and_then(|label| items.iter().position(|item| item.label == label))
            .unwrap_or(0);
        self.command_palette = Some(CommandPaletteState {
            title: if first_whitespace.is_some() {
                "Arguments".to_string()
            } else {
                "Commands".to_string()
            },
            items,
            selected,
        });
    }

    fn command_name_palette_items(
        descriptors: &[CommandDescriptor],
        query: &str,
        cursor: usize,
    ) -> Vec<CommandPaletteItem> {
        let query = query.to_ascii_lowercase();
        let mut items = Vec::new();
        for descriptor in descriptors {
            for invoked in std::iter::once(&descriptor.name).chain(descriptor.aliases.iter()) {
                if !invoked.to_ascii_lowercase().starts_with(&query) {
                    continue;
                }
                let alias = invoked != &descriptor.name;
                let suffix = if matches!(descriptor.arguments, CommandArguments::None) {
                    ""
                } else {
                    " "
                };
                items.push(CommandPaletteItem {
                    label: format!("/{invoked}"),
                    detail: if alias {
                        format!(
                            "{} · alias for /{}",
                            descriptor.description, descriptor.name
                        )
                    } else {
                        descriptor.description.clone()
                    },
                    source: descriptor.source,
                    replacement: format!("/{invoked}{suffix}"),
                    replacement_range: 0..cursor,
                    execute_after_accept: matches!(descriptor.arguments, CommandArguments::None),
                });
            }
        }
        items.truncate(24);
        items
    }

    fn argument_palette_items(
        &self,
        descriptors: &[CommandDescriptor],
        invoked: &str,
        separator: usize,
        prefix: &str,
    ) -> Vec<CommandPaletteItem> {
        let Some(descriptor) = descriptors.iter().find(|descriptor| {
            descriptor.name.eq_ignore_ascii_case(invoked)
                || descriptor
                    .aliases
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(invoked))
        }) else {
            return Vec::new();
        };
        let argument_start = prefix[separator..]
            .char_indices()
            .find(|(_, ch)| !ch.is_whitespace())
            .map_or(prefix.len(), |(offset, _)| separator + offset);
        let query = prefix[argument_start..].to_ascii_lowercase();
        let values = match descriptor.arguments {
            CommandArguments::DisplayMode => vec![
                ("normal".to_string(), "Full transcript detail".to_string()),
                (
                    "concise".to_string(),
                    "Compact tool and event summaries".to_string(),
                ),
                (
                    "debug".to_string(),
                    "Include provider and protocol details".to_string(),
                ),
            ],
            CommandArguments::ModelProfile => self
                .model_choices
                .iter()
                .map(|choice| {
                    (
                        choice.profile.clone(),
                        format!("{} · {}", choice.display_name(), choice.model_id),
                    )
                })
                .collect(),
            CommandArguments::Session => self
                .session_choices
                .iter()
                .map(|choice| {
                    (
                        choice.session_id.clone(),
                        format!("{} · {}", choice.display_title(), choice.status),
                    )
                })
                .collect(),
            CommandArguments::None | CommandArguments::FreeForm => Vec::new(),
        };
        values
            .into_iter()
            .filter(|(value, detail)| {
                query.is_empty()
                    || value.to_ascii_lowercase().contains(&query)
                    || detail.to_ascii_lowercase().contains(&query)
            })
            .take(24)
            .map(|(value, detail)| CommandPaletteItem {
                label: value.clone(),
                detail,
                source: descriptor.source,
                replacement: value,
                replacement_range: argument_start..prefix.len(),
                execute_after_accept: true,
            })
            .collect()
    }

    pub(in crate::tui) fn command_descriptors(&self) -> Vec<CommandDescriptor> {
        command_descriptors(&self.custom_commands, &self.skills)
    }

    pub(super) fn has_skill_named(&self, name: &str) -> bool {
        self.skills
            .iter()
            .any(|skill| skill.name.eq_ignore_ascii_case(name))
    }
}
