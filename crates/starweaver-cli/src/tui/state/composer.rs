use super::{
    format_size_bytes, pasted_image_paths, previous_char_boundary, FooterMode, InteractiveTuiState,
    LocalCommandOutcome, PromptAttachment, PromptInput, SteeringSubmission, SubmissionKind,
};

impl InteractiveTuiState {
    pub(in crate::tui) fn apply_paste(&mut self, text: &str) {
        self.footer_mode = FooterMode::Context;
        let image_paths = pasted_image_paths(text);
        if image_paths.is_empty() {
            self.insert_composer_str(text);
            self.input_status = Some(format!("pasted {} chars", text.chars().count()));
            return;
        }

        self.move_composer_cursor_to_end();
        if !self.input.is_empty() && !self.input.ends_with([' ', '\n']) {
            self.insert_composer_str(" ");
        }
        for path in image_paths {
            if !self.input.is_empty() && !self.input.ends_with([' ', '\n']) {
                self.insert_composer_str(" ");
            }
            self.insert_composer_str(&path);
        }
        self.input_status = Some("image path pasted".to_string());
    }

    pub(crate) fn attach_image(&mut self, attachment: PromptAttachment) {
        let placeholder = attachment.placeholder.clone();
        self.move_composer_cursor_to_end();
        if !self.input.is_empty() && !self.input.ends_with([' ', '\n']) {
            self.insert_composer_str(" ");
        }
        self.insert_composer_str(&placeholder);
        self.insert_composer_str(" ");
        self.pending_attachments.push(attachment);
        self.reset_composer_scroll();
        let count = self.pending_attachments.len();
        self.input_status = Some(if count == 1 {
            format!(
                "image attached: {}",
                self.pending_attachments[0].description()
            )
        } else {
            let total_size = self
                .pending_attachments
                .iter()
                .map(|attachment| attachment.size_bytes)
                .sum::<usize>();
            format!(
                "images attached: {count} ({})",
                format_size_bytes(total_size)
            )
        });
    }

    pub(in crate::tui) fn take_submission_prompt(&mut self) -> Option<PromptInput> {
        self.take_prompt(SubmissionKind::Message)
    }

    pub(in crate::tui) fn take_steering_prompt(&mut self) -> Option<SteeringSubmission> {
        self.retain_visible_attachments();
        if !self.pending_attachments.is_empty() {
            self.input_status = Some("image steering unsupported while running".to_string());
            return None;
        }
        self.take_prompt(SubmissionKind::Steering)
            .map(|input| self.record_steering_message(input.display_text()))
    }

    pub(crate) fn take_pending_submission_display_prompt(&mut self) -> Option<String> {
        self.pending_submission_display_prompt.take()
    }

    pub(in crate::tui) fn take_paste_image_command(&mut self) -> bool {
        if self.input.trim() != "/paste-image" {
            return false;
        }
        self.input.clear();
        self.reset_composer_scroll();
        self.footer_mode = FooterMode::Context;
        true
    }

    fn take_prompt(&mut self, kind: SubmissionKind) -> Option<PromptInput> {
        self.retain_visible_attachments();
        let command = self.take_local_command();
        if matches!(
            command,
            LocalCommandOutcome::Consumed | LocalCommandOutcome::PasteImage
        ) {
            return None;
        }
        let prompt = match command {
            LocalCommandOutcome::Submit(prompt) => prompt,
            LocalCommandOutcome::Consumed | LocalCommandOutcome::PasteImage => {
                unreachable!("handled above")
            }
            LocalCommandOutcome::None => self.input.trim().to_string(),
        };
        if prompt.is_empty() && self.pending_attachments.is_empty() {
            self.clear_composer_input();
            self.reset_composer_scroll();
            return None;
        }
        let attachments = std::mem::take(&mut self.pending_attachments);
        self.clear_composer_input();
        self.reset_composer_scroll();
        match kind {
            SubmissionKind::Message => self.input_status = Some("message sent".to_string()),
            SubmissionKind::Steering => {
                self.input_status = Some("steer sent".to_string());
            }
        }
        Some(PromptInput {
            text: prompt,
            attachments,
            extra_text_parts: Vec::new(),
            guidance_text_parts: Vec::new(),
        })
    }

    pub(in crate::tui) fn clear_composer(&mut self) {
        self.clear_composer_input();
        self.reset_composer_scroll();
        self.pending_attachments.clear();
        self.input_status = None;
        self.history_index = None;
        self.footer_mode = FooterMode::Context;
    }

    pub(in crate::tui) fn backspace_composer(&mut self) {
        if self.remove_trailing_attachment_placeholder() {
            return;
        }
        let cursor = self.composer_cursor_byte();
        let Some(previous) = self.input[..cursor]
            .char_indices()
            .last()
            .map(|(index, _)| index)
        else {
            self.remove_last_pasted_image();
            return;
        };
        self.input.replace_range(previous..cursor, "");
        self.input_cursor = previous;
        self.input_cursor_input_len = self.input.len();
        self.reset_composer_scroll();
    }

    fn retain_visible_attachments(&mut self) {
        self.pending_attachments
            .retain(|attachment| self.input.contains(&attachment.placeholder));
    }

    fn remove_trailing_attachment_placeholder(&mut self) -> bool {
        let Some(attachment) = self.pending_attachments.last() else {
            return false;
        };
        let trimmed_input = self.input.trim_end_matches([' ', '\n']);
        let Some(prefix) = trimmed_input.strip_suffix(&attachment.placeholder) else {
            return false;
        };
        self.input.truncate(prefix.len());
        self.input_cursor = self.input.len();
        self.input_cursor_input_len = self.input.len();
        self.reset_composer_scroll();
        self.remove_last_pasted_image();
        true
    }

    fn remove_last_pasted_image(&mut self) {
        if self.pending_attachments.pop().is_some() {
            self.input_status = Some(if self.pending_attachments.is_empty() {
                "image detached".to_string()
            } else {
                format!("images attached: {}", self.pending_attachments.len())
            });
        }
    }

    pub(in crate::tui) fn composer_is_empty(&self) -> bool {
        self.input.is_empty() && self.pending_attachments.is_empty()
    }

    pub(super) fn composer_has_draft(&self) -> bool {
        !self.input.trim().is_empty() || !self.pending_attachments.is_empty()
    }

    pub(in crate::tui) const fn composer_scroll_offset(&self) -> usize {
        self.input_scroll_offset
    }

    pub(in crate::tui::state) fn reset_composer_scroll(&mut self) {
        self.input_scroll_offset = 0;
    }

    pub(in crate::tui) fn scroll_composer_up(&mut self, amount: usize) {
        self.input_scroll_offset = self.input_scroll_offset.saturating_add(amount);
    }

    pub(in crate::tui) fn scroll_composer_down(&mut self, amount: usize) {
        self.input_scroll_offset = self.input_scroll_offset.saturating_sub(amount);
    }

    pub(in crate::tui) fn clear_composer_input(&mut self) {
        self.input.clear();
        self.input_cursor = 0;
        self.input_cursor_input_len = 0;
    }

    pub(in crate::tui) fn composer_cursor_byte(&self) -> usize {
        if self.input_cursor_input_len != self.input.len() {
            return self.input.len();
        }
        previous_char_boundary(&self.input, self.input_cursor.min(self.input.len()))
    }

    pub(in crate::tui) fn move_composer_cursor_left(&mut self) {
        let cursor = self.composer_cursor_byte();
        if let Some(previous) = self.input[..cursor]
            .char_indices()
            .last()
            .map(|(index, _)| index)
        {
            self.input_cursor = previous;
            self.input_cursor_input_len = self.input.len();
            self.reset_composer_scroll();
        }
    }

    pub(in crate::tui) fn move_composer_cursor_right(&mut self) {
        let cursor = self.composer_cursor_byte();
        if cursor >= self.input.len() {
            self.move_composer_cursor_to_end();
            return;
        }
        let next = self.input[cursor..]
            .chars()
            .next()
            .map_or(self.input.len(), |ch| cursor + ch.len_utf8());
        self.input_cursor = next;
        self.input_cursor_input_len = self.input.len();
        self.reset_composer_scroll();
    }

    pub(in crate::tui) fn move_composer_cursor_to_line_start(&mut self) {
        let cursor = self.composer_cursor_byte();
        self.input_cursor = self.input[..cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        self.input_cursor_input_len = self.input.len();
        self.reset_composer_scroll();
    }

    pub(in crate::tui) fn move_composer_cursor_to_line_end(&mut self) {
        let cursor = self.composer_cursor_byte();
        self.input_cursor = self.input[cursor..]
            .find('\n')
            .map_or(self.input.len(), |offset| cursor + offset);
        self.input_cursor_input_len = self.input.len();
        self.reset_composer_scroll();
    }

    pub(in crate::tui::state) fn move_composer_cursor_to_end(&mut self) {
        self.input_cursor = self.input.len();
        self.input_cursor_input_len = self.input.len();
    }

    pub(in crate::tui::state) fn insert_composer_str(&mut self, text: &str) {
        let cursor = self.composer_cursor_byte();
        self.input.insert_str(cursor, text);
        self.input_cursor = cursor + text.len();
        self.input_cursor_input_len = self.input.len();
        self.reset_composer_scroll();
        self.history_index = None;
    }

    pub(in crate::tui) fn push_composer_char(&mut self, ch: char) {
        let mut buffer = [0; 4];
        self.insert_composer_str(ch.encode_utf8(&mut buffer));
        self.input_status = None;
    }

    pub(in crate::tui) fn insert_composer_newline(&mut self) {
        self.insert_composer_str("\n");
    }

    pub(crate) fn pasted_image_count(&self) -> usize {
        self.pending_attachments.len()
    }

    pub(in crate::tui) fn push_history(&mut self, prompt: String) {
        if self.history.last() != Some(&prompt) {
            self.history.push(prompt);
        }
        self.history_index = None;
        self.history_draft.clear();
    }

    pub(in crate::tui) fn previous_history(&mut self) {
        if self.history.is_empty() {
            return;
        }
        if self.history_index.is_none() {
            self.history_draft = self.input.clone();
            self.history_index = Some(self.history.len().saturating_sub(1));
        } else if let Some(index) = self.history_index.as_mut() {
            *index = index.saturating_sub(1);
        }
        if let Some(index) = self.history_index {
            self.input = self.history[index].clone();
            self.move_composer_cursor_to_end();
            self.reset_composer_scroll();
        }
    }

    pub(in crate::tui) fn next_history(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 >= self.history.len() {
            self.history_index = None;
            self.input = self.history_draft.clone();
            self.history_draft.clear();
        } else {
            let next = index + 1;
            self.history_index = Some(next);
            self.input = self.history[next].clone();
        }
        self.move_composer_cursor_to_end();
        self.reset_composer_scroll();
    }
}
