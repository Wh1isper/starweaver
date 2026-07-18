use std::{fmt::Write as _, time::Duration};

use starweaver_environment::{ShellProcessSnapshot, ShellProcessStatus};

use super::{
    FooterMode, InteractiveTuiState, LocalCommandOutcome, ModelChoice, model_choice_config_suffix,
    model_choice_label, push_shell_output_lines, render_mode_label,
};
use crate::{
    args::TuiRenderMode,
    command_catalog::{closest_builtin_name, key_binding_descriptors},
    slash_commands::expand_slash_command,
};

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
            self.pending_clear_context = true;
            self.footer_mode = FooterMode::Context;
            self.input_status = Some("clearing context".to_string());
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
        if input == "/display" || input.starts_with("/display ") {
            self.clear_composer_input();
            self.pending_attachments.clear();
            self.footer_mode = FooterMode::Context;
            self.handle_display_command(input.strip_prefix("/display").unwrap_or_default().trim());
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
        if input == "/tasks" {
            self.clear_composer_input();
            self.pending_attachments.clear();
            self.footer_mode = FooterMode::Context;
            self.toggle_task_panel();
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
            let command = command.trim();
            if command.is_empty() {
                self.push_transcript_notice(
                    "[SYS] Shell command usage: !<command> (example: !git status --short)"
                        .to_string(),
                );
                self.input_status = Some("shell usage".to_string());
            } else {
                self.pending_shell_command = Some(command.to_string());
                self.input_status = Some("starting shell".to_string());
            }
            return LocalCommandOutcome::Consumed;
        }
        if input == "/goal" || input.starts_with("/goal ") {
            self.clear_composer_input();
            let task = input.strip_prefix("/goal").unwrap_or_default().trim();
            if task.is_empty() {
                self.push_transcript_notice("[SYS] Usage: /goal <task description>");
                self.input_status = Some("goal usage".to_string());
                return LocalCommandOutcome::Consumed;
            }
            self.goal_task = Some(task.to_string());
            self.goal_active = true;
            self.goal_iteration = 0;
            self.goal_max_iterations = self.goal_max_iterations.max(1);
            self.pending_goal_submission = Some(task.to_string());
            self.push_transcript_notice(format!(
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
            self.push_transcript_notice(message);
            self.input_status = Some(format!("command /{}", expanded.command_name));
            self.pending_submission_display_prompt = Some(expanded.prompt);
            return LocalCommandOutcome::Submit(input);
        }
        if let Some(invoked) = input
            .strip_prefix('/')
            .and_then(|rest| rest.split_whitespace().next())
            .filter(|name| !name.is_empty() && !self.has_skill_named(name))
            && let Some(suggestion) = closest_builtin_name(invoked)
            && !suggestion.eq_ignore_ascii_case(invoked)
        {
            self.clear_composer_input();
            self.push_transcript_notice(format!(
                "[SYS] Unknown command /{invoked}. Did you mean /{suggestion}?"
            ));
            self.input_status = Some("command typo".to_string());
            return LocalCommandOutcome::Consumed;
        }
        LocalCommandOutcome::None
    }

    fn append_help_to_body(&mut self) {
        let mut lines = vec![
            "Starweaver TUI help".to_string(),
            String::new(),
            "Commands".to_string(),
        ];
        for command in self.command_descriptors() {
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
            lines.push(format!(
                "  {:<24} {}{} [{}]",
                command.usage,
                command.description,
                aliases,
                command.source.label()
            ));
        }
        lines.extend([
            "  @<skill> [task]        Activate a loaded skill without slash-name conflicts"
                .to_string(),
            "  !<command>             Run a shell command inline".to_string(),
            String::new(),
            "Shortcuts".to_string(),
        ]);
        lines.extend(
            key_binding_descriptors()
                .iter()
                .map(|binding| format!("  {:<18} {}", binding.keys, binding.description)),
        );
        self.push_transcript_lines(lines);
    }

    fn handle_display_command(&mut self, requested: &str) {
        if requested.is_empty() {
            self.push_transcript_notice(format!(
                "[SYS] Display mode: {}. Available: normal, concise, debug",
                render_mode_label(self.render_mode())
            ));
            return;
        }
        let mode = match requested {
            "normal" => TuiRenderMode::Normal,
            "concise" => TuiRenderMode::Concise,
            "debug" => TuiRenderMode::Debug,
            other => {
                self.push_transcript_notice(format!(
                    "[SYS] Unknown display mode: {other}. Available: normal, concise, debug"
                ));
                self.input_status = Some("display mode".to_string());
                return;
            }
        };
        self.set_render_mode(mode);
    }

    fn handle_model_command(&mut self, requested: &str) {
        if self.running {
            self.push_transcript_notice(
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
            self.push_transcript_notice(format!("[SYS] Unknown model profile: {requested}"));
            self.append_model_choices();
            return;
        };
        self.apply_model_choice(&choice);
    }

    fn handle_session_command(&mut self, requested: &str) {
        if self.running {
            self.push_transcript_notice(
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
        self.push_transcript_notice(format!(
            "[SYS] Switched model to {} ({})",
            choice.display_name(),
            choice.model_id
        ));
    }

    fn append_model_choices(&mut self) {
        let mut lines = vec![
            "[SYS] Model profiles".to_string(),
            format!("[SYS] Current: {} ({})", self.profile, self.model),
        ];
        if self.model_choices.is_empty() {
            lines.push("[SYS] No model profiles are configured.".to_string());
            self.push_transcript_lines(lines);
            return;
        }
        for choice in &self.model_choices {
            let marker = if choice.profile == self.profile {
                "*"
            } else {
                " "
            };
            lines.push(format!(
                "[SYS] {marker} /model {:<18} {} ({}){}",
                choice.profile,
                choice.display_name(),
                choice.model_id,
                model_choice_config_suffix(choice)
            ));
        }
        self.push_transcript_lines(lines);
    }

    fn append_cost_summary(&mut self) {
        self.push_transcript_lines(self.format_cost_summary_lines());
    }

    pub(crate) const fn take_pending_shell_command(&mut self) -> Option<String> {
        self.pending_shell_command.take()
    }

    pub(crate) fn queue_shell_command(&mut self, command: &str) {
        self.shell_running = true;
        self.cancel_requested = false;
        self.status = "SHELL".to_string();
        self.phase = "shell starting".to_string();
        self.input_status = Some("shell starting".to_string());
        self.push_transcript_lines(vec![
            format!("Shell command: {command}"),
            "Shell starting".to_string(),
        ]);
    }

    pub(crate) fn mark_shell_started(&mut self, process_id: &str) {
        self.phase = "shell running".to_string();
        self.input_status = Some(format!("shell process {process_id}"));
        self.push_transcript_lines(vec![format!("Shell started: {process_id}")]);
    }

    pub(crate) fn finish_shell_command(
        &mut self,
        snapshot: &ShellProcessSnapshot,
        elapsed: Duration,
    ) {
        self.shell_running = false;
        self.cancel_requested = false;
        self.status = "IDLE".to_string();
        self.phase = match snapshot.status {
            ShellProcessStatus::Completed => "shell completed",
            ShellProcessStatus::Failed => "shell failed",
            ShellProcessStatus::Killed => "shell cancelled",
            ShellProcessStatus::Running => "shell running",
        }
        .to_string();
        let mut lines = Vec::new();
        push_shell_output_lines(&mut lines, "stdout", &snapshot.stdout);
        push_shell_output_lines(&mut lines, "stderr", &snapshot.stderr);
        let exit = snapshot
            .return_code
            .map_or_else(|| "signal".to_string(), |code| code.to_string());
        let elapsed = format!("{:.2}s", elapsed.as_secs_f64());
        lines.push(match snapshot.status {
            ShellProcessStatus::Completed => {
                format!("Shell completed: exit {exit} duration={elapsed}")
            }
            ShellProcessStatus::Failed => {
                format!("Shell failed: exit {exit} duration={elapsed}")
            }
            ShellProcessStatus::Killed => format!("Shell cancelled: duration={elapsed}"),
            ShellProcessStatus::Running => format!("Shell still running: duration={elapsed}"),
        });
        self.input_status = Some(self.phase.clone());
        self.push_transcript_lines(lines);
    }

    pub(crate) fn fail_shell_command(&mut self, error: &str) {
        self.shell_running = false;
        self.cancel_requested = false;
        self.status = "IDLE".to_string();
        self.phase = "shell error".to_string();
        self.input_status = Some("shell error".to_string());
        self.push_transcript_notice(format!("[SYS] Shell error: {error}"));
    }
}
