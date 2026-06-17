use std::{fmt::Write as _, process::Command};

use super::{
    model_choice_config_suffix, model_choice_label, push_shell_output_lines, FooterMode,
    InteractiveTuiState, LocalCommandOutcome, ModelChoice,
};
use crate::slash_commands::{expand_slash_command, SlashCommandDefinition};

impl InteractiveTuiState {
    #[allow(clippy::too_many_lines)]
    pub(super) fn take_local_command(&mut self) -> LocalCommandOutcome {
        let input = self.input.trim().to_string();
        if input == "/help" {
            self.clear_composer_input();
            self.pending_attachments.clear();
            self.footer_mode = FooterMode::Context;
            self.append_help_to_body();
            self.input_status = Some("help".to_string());
            return LocalCommandOutcome::Consumed;
        }
        if input == "/clear" {
            self.clear_composer_input();
            self.clear_context_view();
            self.pending_clear_context = true;
            self.footer_mode = FooterMode::Context;
            self.input_status = Some("context cleared".to_string());
            return LocalCommandOutcome::Consumed;
        }
        if input == "/cost" {
            self.clear_composer_input();
            self.pending_attachments.clear();
            self.footer_mode = FooterMode::Context;
            self.append_cost_summary();
            self.input_status = Some("cost".to_string());
            return LocalCommandOutcome::Consumed;
        }
        if input == "/model" || input.starts_with("/model ") {
            self.clear_composer_input();
            self.pending_attachments.clear();
            self.footer_mode = FooterMode::Context;
            self.handle_model_command(input.strip_prefix("/model").unwrap_or_default().trim());
            if !self.model_picker_open {
                self.input_status = Some("model".to_string());
            }
            return LocalCommandOutcome::Consumed;
        }
        if input == "/session" || input.starts_with("/session ") {
            self.clear_composer_input();
            self.pending_attachments.clear();
            self.footer_mode = FooterMode::Context;
            self.handle_session_command(input.strip_prefix("/session").unwrap_or_default().trim());
            return LocalCommandOutcome::Consumed;
        }
        if input == "/paste-image" {
            self.clear_composer_input();
            self.footer_mode = FooterMode::Context;
            return LocalCommandOutcome::PasteImage;
        }
        if let Some(command) = input.strip_prefix('!') {
            self.clear_composer_input();
            self.pending_attachments.clear();
            self.footer_mode = FooterMode::Context;
            self.run_shell_command(command.trim());
            self.input_status = Some("shell".to_string());
            return LocalCommandOutcome::Consumed;
        }
        if input == "/goal" || input.starts_with("/goal ") {
            self.clear_composer_input();
            let task = input.strip_prefix("/goal").unwrap_or_default().trim();
            if task.is_empty() {
                self.body
                    .push("[SYS] Usage: /goal <task description>".to_string());
                self.input_status = Some("goal usage".to_string());
                return LocalCommandOutcome::Consumed;
            }
            self.goal_task = Some(task.to_string());
            self.goal_active = true;
            self.goal_iteration = 0;
            self.goal_max_iterations = self.goal_max_iterations.max(1);
            self.body.push(format!(
                "[SYS] [Goal] Starting goal mode ({} max iterations). Ctrl+C to stop.",
                self.goal_max_iterations
            ));
            self.input_status = Some("goal".to_string());
            return LocalCommandOutcome::Submit(task.to_string());
        }
        if let Some(expanded) = expand_slash_command(&self.custom_commands, &input) {
            self.clear_composer_input();
            self.footer_mode = FooterMode::Context;
            let mut message = format!("[SYS] Expanded /{} custom command", expanded.command_name);
            if expanded.invoked_name != expanded.command_name {
                let _ = write!(message, " (alias /{})", expanded.invoked_name);
            }
            if let Some(description) = expanded
                .description
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                message.push_str(": ");
                message.push_str(description.trim());
            }
            self.body.push(message);
            self.input_status = Some(format!("command /{}", expanded.command_name));
            self.pending_submission_display_prompt = Some(expanded.prompt);
            return LocalCommandOutcome::Submit(input);
        }
        if input.starts_with('/') {
            self.clear_composer_input();
            self.pending_attachments.clear();
            self.footer_mode = FooterMode::Context;
            self.body.push(format!(
                "[SYS] Unknown command: {input}. Available commands: {}",
                self.available_command_summary()
            ));
            self.input_status = Some("unknown command".to_string());
            return LocalCommandOutcome::Consumed;
        }
        LocalCommandOutcome::None
    }

    fn append_help_to_body(&mut self) {
        self.body.extend([
            "Starweaver TUI help".to_string(),
            String::new(),
            "Commands".to_string(),
            "  /help             Show this help".to_string(),
            "  /clear            Clear transcript and start a fresh context".to_string(),
            "  /cost             Show usage and context".to_string(),
            "  /model [profile]  Open or select a model profile".to_string(),
            "  /session [id]     Open session selector or reload a session".to_string(),
            "  /goal <task>      Run toward a verified goal".to_string(),
            "  /paste-image      Attach image from system clipboard".to_string(),
            "  !<command>        Run a shell command inline".to_string(),
        ]);
        let custom_commands = self.custom_command_definitions();
        if !custom_commands.is_empty() {
            self.body.push(String::new());
            self.body.push("Custom commands".to_string());
            for command in custom_commands {
                let description = command
                    .description
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("Run configured prompt");
                let aliases = if command.aliases.is_empty() {
                    String::new()
                } else {
                    format!(
                        " (aliases: {})",
                        command
                            .aliases
                            .iter()
                            .map(|alias| format!("/{alias}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                self.body.push(format!(
                    "  /{:<16} {}{}",
                    format!("{} [instruction]", command.name),
                    description,
                    aliases
                ));
            }
        }
        self.body.extend([
            String::new(),
            "Shortcuts".to_string(),
            "  Up/Down           Browse prompt history".to_string(),
            "  Ctrl+A/E          Move to line start/end".to_string(),
            "  Alt+Left/Right    Move by word".to_string(),
            "  Cmd+Left/Right    Move to line start/end".to_string(),
            "  PageUp/PageDown   Scroll transcript".to_string(),
            "  Mouse wheel       Scroll transcript".to_string(),
            "  Enter             Send message or select model/session".to_string(),
            "  Tab               Queue a draft while running".to_string(),
            "  Ctrl+C            Interrupt or exit".to_string(),
        ]);
    }

    fn available_command_summary(&self) -> String {
        let mut commands = vec![
            "/help".to_string(),
            "/clear".to_string(),
            "/cost".to_string(),
            "/model".to_string(),
            "/session".to_string(),
            "/goal".to_string(),
            "/paste-image".to_string(),
            "!<command>".to_string(),
        ];
        commands.extend(
            self.custom_command_definitions()
                .into_iter()
                .map(|command| format!("/{}", command.name)),
        );
        commands.join(", ")
    }

    fn custom_command_definitions(&self) -> Vec<SlashCommandDefinition> {
        let mut definitions = self
            .custom_commands
            .values()
            .cloned()
            .collect::<Vec<SlashCommandDefinition>>();
        definitions.sort_by(|left, right| left.name.cmp(&right.name));
        definitions.dedup_by(|left, right| left.name == right.name);
        definitions
    }

    fn handle_model_command(&mut self, requested: &str) {
        if self.running {
            self.body.push(
                "[SYS] Model selection is available after the current run finishes.".to_string(),
            );
            return;
        }
        if requested.is_empty() {
            self.open_model_picker();
            return;
        }
        let Some(choice) = self
            .model_choices
            .iter()
            .find(|choice| choice.profile == requested || choice.display_name() == requested)
            .cloned()
        else {
            self.body
                .push(format!("[SYS] Unknown model profile: {requested}"));
            self.append_model_choices();
            return;
        };
        self.apply_model_choice(&choice);
    }

    fn handle_session_command(&mut self, requested: &str) {
        if self.running {
            self.body.push(
                "[SYS] Session selection is available after the current run finishes.".to_string(),
            );
            self.input_status = Some("session blocked".to_string());
            return;
        }
        self.model_picker_open = false;
        self.session_picker_open = false;
        self.pending_session_command = Some(requested.to_string());
        self.input_status = Some(if requested.is_empty() {
            "session".to_string()
        } else {
            "session reload".to_string()
        });
    }

    pub(super) fn apply_model_choice(&mut self, choice: &ModelChoice) {
        self.profile.clone_from(&choice.profile);
        self.model = model_choice_label(choice);
        self.set_context_window(choice.context_window);
        self.sync_model_picker_index_to_current();
        self.body.push(format!(
            "[SYS] Switched model to {} ({})",
            choice.display_name(),
            choice.model_id
        ));
    }

    fn append_model_choices(&mut self) {
        self.body.push("[SYS] Model profiles".to_string());
        self.body
            .push(format!("[SYS] Current: {} ({})", self.profile, self.model));
        if self.model_choices.is_empty() {
            self.body
                .push("[SYS] No model profiles are configured.".to_string());
            return;
        }
        for choice in &self.model_choices {
            let marker = if choice.profile == self.profile {
                "*"
            } else {
                " "
            };
            self.body.push(format!(
                "[SYS] {marker} /model {:<18} {} ({}){}",
                choice.profile,
                choice.display_name(),
                choice.model_id,
                model_choice_config_suffix(choice)
            ));
        }
    }

    fn append_cost_summary(&mut self) {
        self.body.extend(self.format_cost_summary_lines());
    }

    fn run_shell_command(&mut self, command: &str) {
        if command.is_empty() {
            self.body.push(
                "[SYS] Shell command usage: !<command> (example: !git status --short)".to_string(),
            );
            return;
        }
        self.body.push(format!("Shell command: {command}"));
        match Command::new("/bin/bash").arg("-lc").arg(command).output() {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                push_shell_output_lines(&mut self.body, "stdout", &stdout);
                push_shell_output_lines(&mut self.body, "stderr", &stderr);
                let status = output
                    .status
                    .code()
                    .map_or_else(|| "signal".to_string(), |code| code.to_string());
                if output.status.success() {
                    self.body.push(format!("Shell completed: exit {status}"));
                } else {
                    self.body.push(format!("Shell failed: exit {status}"));
                }
            }
            Err(error) => self.body.push(format!("Shell error: {error}")),
        }
    }
}
