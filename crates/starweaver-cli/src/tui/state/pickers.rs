use super::{FooterMode, InteractiveTuiState, PendingSessionCommand};

impl InteractiveTuiState {
    pub(crate) fn open_session_picker(&mut self) {
        if self.running {
            self.push_transcript_notice(
                "[SYS] Session selection is available after the current run finishes.".to_string(),
            );
            self.input_status = Some("session blocked".to_string());
            return;
        }
        self.clear_composer_input();
        self.reset_composer_scroll();
        self.pending_attachments.clear();
        self.footer_mode = FooterMode::Context;
        if self.session_choices.is_empty() {
            self.session_picker_open = false;
            self.push_transcript_notice("[SYS] No sessions found.");
            self.input_status = Some("no sessions".to_string());
            return;
        }
        self.model_picker_open = false;
        self.session_picker_open = true;
        self.sync_session_picker_index_to_current();
        self.input_status = Some("session picker".to_string());
    }

    pub(in crate::tui) fn close_session_picker(&mut self) {
        self.session_picker_open = false;
        self.input_status = Some("session picker closed".to_string());
    }

    pub(in crate::tui) fn move_session_picker_selection(&mut self, delta: isize) {
        let len = self.session_choices.len();
        if len == 0 {
            self.session_picker_index = 0;
            return;
        }
        let current = self.session_picker_index.min(len.saturating_sub(1));
        let steps = delta.unsigned_abs() % len;
        self.session_picker_index = if delta.is_negative() {
            (current + len - steps) % len
        } else {
            (current + steps) % len
        };
        self.input_status = Some("session picker".to_string());
    }

    pub(in crate::tui) fn select_session_picker_choice(&mut self) {
        let Some(session_id) = self.selected_session_picker_id() else {
            self.close_session_picker();
            return;
        };
        self.pending_session_command = Some(session_id);
        self.session_picker_open = false;
        self.input_status = Some("session selected".to_string());
    }

    fn selected_session_picker_id(&self) -> Option<String> {
        self.session_choices
            .get(self.session_picker_index)
            .map(|choice| choice.session_id.clone())
    }

    pub(super) fn sync_session_picker_index_to_current(&mut self) {
        self.session_picker_index = self
            .session_id
            .as_deref()
            .and_then(|session_id| {
                self.session_choices
                    .iter()
                    .position(|choice| choice.session_id == session_id)
            })
            .unwrap_or(0)
            .min(self.session_choices.len().saturating_sub(1));
    }

    pub(in crate::tui) fn take_pending_session_command(&mut self) -> Option<PendingSessionCommand> {
        self.pending_session_command.take().map(|requested| {
            if requested.is_empty() {
                PendingSessionCommand::Current
            } else {
                PendingSessionCommand::Select(requested)
            }
        })
    }

    pub(in crate::tui) const fn model_picker_visible(&self) -> bool {
        self.model_picker_open
    }

    pub(in crate::tui) const fn model_picker_index(&self) -> usize {
        self.model_picker_index
    }

    pub(in crate::tui) fn open_model_picker(&mut self) {
        if self.running {
            self.push_transcript_notice(
                "[SYS] Model selection is available after the current run finishes.".to_string(),
            );
            self.input_status = Some("model blocked".to_string());
            return;
        }
        self.clear_composer_input();
        self.reset_composer_scroll();
        self.pending_attachments.clear();
        self.footer_mode = FooterMode::Context;
        self.session_picker_open = false;
        self.model_picker_open = true;
        self.sync_model_picker_index_to_current();
        self.input_status = Some("model picker".to_string());
    }

    pub(in crate::tui) fn close_model_picker(&mut self) {
        self.model_picker_open = false;
        self.input_status = Some("model picker closed".to_string());
    }

    pub(in crate::tui) fn move_model_picker_selection(&mut self, delta: isize) {
        let len = self.model_choices.len();
        if len == 0 {
            self.model_picker_index = 0;
            return;
        }
        let current = self.model_picker_index.min(len.saturating_sub(1));
        let steps = delta.unsigned_abs() % len;
        self.model_picker_index = if delta.is_negative() {
            (current + len - steps) % len
        } else {
            (current + steps) % len
        };
        self.input_status = Some("model picker".to_string());
    }

    pub(in crate::tui) fn select_model_picker_choice(&mut self) {
        let Some(choice) = self.model_choices.get(self.model_picker_index).cloned() else {
            self.close_model_picker();
            return;
        };
        self.apply_model_choice(&choice);
        self.model_picker_open = false;
        self.input_status = Some("model selected".to_string());
    }

    pub(super) fn sync_model_picker_index_to_current(&mut self) {
        self.model_picker_index = self
            .model_choices
            .iter()
            .position(|choice| choice.profile == self.profile)
            .unwrap_or(0)
            .min(self.model_choices.len().saturating_sub(1));
    }
}
