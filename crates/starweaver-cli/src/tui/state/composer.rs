use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::history::{HISTORY_MAX_ENTRY_BYTES, bound_entries};
use super::{
    FooterMode, InteractiveTuiState, LocalCommandOutcome, PromptAttachment, PromptInput,
    SteeringSubmission, SubmissionKind, format_size_bytes, pasted_image_paths,
};

fn is_composer_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn normalize_pasted_text(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                output.push('\n');
            }
            '\n' => output.push('\n'),
            '\t' => output.push_str("    "),
            '\u{1b}' => skip_ansi_escape_sequence(&mut chars),
            ch if ch.is_control() => {}
            ch => output.push(ch),
        }
    }
    output
}

fn skip_ansi_escape_sequence(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.peek().copied() {
        Some('[') => {
            chars.next();
            for ch in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&ch) {
                    break;
                }
            }
        }
        Some(']') => {
            chars.next();
            while let Some(ch) = chars.next() {
                if ch == '\u{7}' {
                    break;
                }
                if ch == '\u{1b}' && chars.peek() == Some(&'\\') {
                    chars.next();
                    break;
                }
            }
        }
        Some('(' | ')' | '*' | '+' | '-' | '.' | '/') => {
            chars.next();
            chars.next();
        }
        _ => {}
    }
}

fn paste_contains_only_image_paths(text: &str, image_paths: &[String]) -> bool {
    if image_paths.is_empty() {
        return false;
    }
    let token_count = text
        .split_whitespace()
        .map(|part| part.trim_matches(['\'', '"']))
        .filter(|part| !part.is_empty())
        .count();
    token_count == image_paths.len()
}

fn visual_cursor_positions(input: &str, width: usize) -> Vec<(usize, usize, usize)> {
    let width = width.max(1);
    let mut positions = Vec::new();
    let mut row = 0usize;
    let mut col = 0usize;
    for (byte, grapheme) in input.grapheme_indices(true) {
        positions.push((byte, row, col));
        if grapheme == "\n" {
            row = row.saturating_add(1);
            col = 0;
            continue;
        }
        let grapheme_width = UnicodeWidthStr::width(grapheme);
        if col > 0 && col.saturating_add(grapheme_width) > width {
            row = row.saturating_add(1);
            col = 0;
        }
        col = col.saturating_add(grapheme_width);
        if col >= width {
            row = row.saturating_add(1);
            col = 0;
        }
    }
    positions.push((input.len(), row, col));
    positions
}

impl InteractiveTuiState {
    pub(in crate::tui) fn apply_paste(&mut self, text: &str) {
        self.footer_mode = FooterMode::Context;
        let normalized = normalize_pasted_text(text);
        let image_paths = pasted_image_paths(&normalized);
        if !paste_contains_only_image_paths(&normalized, &image_paths) {
            self.insert_composer_str(&normalized);
            self.input_status = Some(format!("pasted {} chars", normalized.chars().count()));
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

    pub(crate) const fn take_pending_submission_display_prompt(&mut self) -> Option<String> {
        self.pending_submission_display_prompt.take()
    }

    pub(crate) fn restore_submission_prompt(
        &mut self,
        prompt: PromptInput,
        goal: Option<crate::args::GoalCommandOptions>,
    ) {
        self.input = prompt.text;
        self.pending_attachments = prompt.attachments;
        self.restored_prompt_parts = Some((prompt.extra_text_parts, prompt.guidance_text_parts));
        self.pending_submission_display_prompt = None;
        self.history_index = None;
        self.history_draft.clear();
        self.footer_mode = FooterMode::Context;
        self.input_cursor = self.input.len();
        self.input_cursor_input_len = self.input.len();
        self.composer_preferred_column = None;
        self.reset_composer_scroll();
        if let Some(goal) = goal {
            self.goal_task = Some(goal.objective.clone());
            self.goal_active = true;
            self.goal_max_iterations = goal.max_iterations.max(1);
            self.pending_goal_submission = Some(goal.objective);
        }
        self.input_status = Some("queued prompt restored after run failure".to_string());
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
        let restored_prompt_parts = self.restored_prompt_parts.take();
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
        let (extra_text_parts, guidance_text_parts) = restored_prompt_parts.unwrap_or_default();
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
            extra_text_parts,
            guidance_text_parts,
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
        let cursor = self.composer_cursor_byte();
        if let Some((start, end, attachment_index)) = self
            .attachment_ranges()
            .into_iter()
            .find(|(start, end, _)| cursor > *start && cursor <= *end)
        {
            self.input.replace_range(start..end, "");
            self.input_cursor = start;
            self.input_cursor_input_len = self.input.len();
            self.composer_preferred_column = None;
            self.pending_attachments.remove(attachment_index);
            self.update_attachment_status("image detached");
            self.reset_composer_scroll();
            self.command_palette_dismissed_input = None;
            self.refresh_command_palette();
            return;
        }

        let Some((previous, _)) = self.input[..cursor].grapheme_indices(true).next_back() else {
            return;
        };
        self.input.replace_range(previous..cursor, "");
        self.input_cursor = previous;
        self.input_cursor_input_len = self.input.len();
        self.composer_preferred_column = None;
        self.reset_composer_scroll();
        self.command_palette_dismissed_input = None;
        self.refresh_command_palette();
    }

    fn retain_visible_attachments(&mut self) {
        let mut remaining = self.input.as_str();
        self.pending_attachments.retain(|attachment| {
            let Some(index) = remaining.find(&attachment.placeholder) else {
                return false;
            };
            remaining = &remaining[index + attachment.placeholder.len()..];
            true
        });
    }

    fn attachment_ranges(&self) -> Vec<(usize, usize, usize)> {
        let mut ranges = Vec::new();
        let mut search_start = 0usize;
        for (attachment_index, attachment) in self.pending_attachments.iter().enumerate() {
            let Some(relative_start) = self.input[search_start..].find(&attachment.placeholder)
            else {
                continue;
            };
            let start = search_start.saturating_add(relative_start);
            let placeholder_end = start.saturating_add(attachment.placeholder.len());
            // The separator inserted with an attachment belongs to the atomic
            // span only while it is still directly adjacent to the placeholder.
            let end = if self.input[placeholder_end..].starts_with(' ') {
                placeholder_end.saturating_add(1)
            } else {
                placeholder_end
            };
            ranges.push((start, end, attachment_index));
            search_start = end;
        }
        ranges
    }

    fn update_attachment_status(&mut self, detached: &str) {
        self.input_status = Some(if self.pending_attachments.is_empty() {
            detached.to_string()
        } else {
            format!("images attached: {}", self.pending_attachments.len())
        });
    }

    fn atomic_cursor_target(&self, target: usize, toward_end: bool) -> usize {
        self.attachment_ranges()
            .into_iter()
            .find(|(start, end, _)| target > *start && target < *end)
            .map_or(
                target,
                |(start, end, _)| if toward_end { end } else { start },
            )
    }

    pub(in crate::tui) const fn composer_is_empty(&self) -> bool {
        self.input.is_empty() && self.pending_attachments.is_empty()
    }

    pub(in crate::tui) fn composer_has_draft(&self) -> bool {
        !self.input.trim().is_empty() || !self.pending_attachments.is_empty()
    }

    pub(in crate::tui) const fn composer_scroll_offset(&self) -> usize {
        self.input_scroll_offset
    }

    pub(in crate::tui::state) const fn reset_composer_scroll(&mut self) {
        self.input_scroll_offset = 0;
    }

    pub(in crate::tui) const fn scroll_composer_up(&mut self, amount: usize) {
        self.input_scroll_offset = self.input_scroll_offset.saturating_add(amount);
    }

    pub(in crate::tui) const fn scroll_composer_down(&mut self, amount: usize) {
        self.input_scroll_offset = self.input_scroll_offset.saturating_sub(amount);
    }

    pub(in crate::tui) fn clear_composer_input(&mut self) {
        self.input.clear();
        self.input_cursor = 0;
        self.input_cursor_input_len = 0;
        self.composer_preferred_column = None;
        self.restored_prompt_parts = None;
        self.command_palette = None;
        self.command_palette_dismissed_input = None;
    }

    pub(in crate::tui) fn composer_cursor_byte(&self) -> usize {
        if self.input_cursor_input_len != self.input.len() {
            return self.input.len();
        }
        let requested = self.input_cursor.min(self.input.len());
        if requested == self.input.len() {
            return requested;
        }
        self.input
            .grapheme_indices(true)
            .map(|(index, _)| index)
            .take_while(|index| *index <= requested)
            .last()
            .unwrap_or(0)
    }

    pub(in crate::tui) fn move_composer_cursor_left(&mut self) {
        let cursor = self.composer_cursor_byte();
        let target = self
            .attachment_ranges()
            .into_iter()
            .find(|(start, end, _)| cursor > *start && cursor <= *end)
            .map(|(start, _, _)| start)
            .or_else(|| {
                self.input[..cursor]
                    .grapheme_indices(true)
                    .next_back()
                    .map(|(index, _)| index)
            });
        if let Some(target) = target {
            self.set_composer_cursor(target, false);
        }
    }

    pub(in crate::tui) fn move_composer_cursor_right(&mut self) {
        let cursor = self.composer_cursor_byte();
        if cursor >= self.input.len() {
            self.move_composer_cursor_to_end();
            return;
        }
        let target = self
            .attachment_ranges()
            .into_iter()
            .find(|(start, end, _)| cursor >= *start && cursor < *end)
            .map_or_else(
                || {
                    self.input[cursor..]
                        .graphemes(true)
                        .next()
                        .map_or(self.input.len(), |grapheme| cursor + grapheme.len())
                },
                |(_, end, _)| end,
            );
        self.set_composer_cursor(target, false);
    }

    pub(in crate::tui) fn move_composer_cursor_word_left(&mut self) {
        let cursor = self.composer_cursor_byte();
        if cursor == 0 {
            return;
        }

        let mut target = cursor;
        let mut chars = self.input[..cursor].char_indices().rev().peekable();
        while let Some(&(index, ch)) = chars.peek() {
            if is_composer_word_char(ch) {
                break;
            }
            target = index;
            chars.next();
        }
        while let Some(&(index, ch)) = chars.peek() {
            if !is_composer_word_char(ch) {
                break;
            }
            target = index;
            chars.next();
        }

        let target = self.atomic_cursor_target(target, false);
        self.set_composer_cursor(target, false);
    }

    pub(in crate::tui) fn move_composer_cursor_word_right(&mut self) {
        let cursor = self.composer_cursor_byte();
        if cursor >= self.input.len() {
            self.move_composer_cursor_to_end();
            return;
        }

        let mut target = cursor;
        let mut chars = self.input[cursor..].char_indices().peekable();
        while let Some(&(offset, ch)) = chars.peek() {
            if is_composer_word_char(ch) {
                break;
            }
            target = cursor + offset + ch.len_utf8();
            chars.next();
        }
        while let Some(&(offset, ch)) = chars.peek() {
            if !is_composer_word_char(ch) {
                break;
            }
            target = cursor + offset + ch.len_utf8();
            chars.next();
        }

        let target = self.atomic_cursor_target(target, true);
        self.set_composer_cursor(target, false);
    }

    pub(in crate::tui) fn move_composer_cursor_to_line_start(&mut self) {
        let cursor = self.composer_cursor_byte();
        let target = self.input[..cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        self.set_composer_cursor(target, false);
    }

    pub(in crate::tui) fn move_composer_cursor_to_line_end(&mut self) {
        let cursor = self.composer_cursor_byte();
        let target = self.input[cursor..]
            .find('\n')
            .map_or(self.input.len(), |offset| cursor + offset);
        self.set_composer_cursor(target, false);
    }

    pub(in crate::tui) const fn move_composer_cursor_to_end(&mut self) {
        self.input_cursor = self.input.len();
        self.input_cursor_input_len = self.input.len();
        self.composer_preferred_column = None;
    }

    fn set_composer_cursor(&mut self, cursor: usize, preserve_preferred_column: bool) {
        self.input_cursor = cursor.min(self.input.len());
        self.input_cursor_input_len = self.input.len();
        if !preserve_preferred_column {
            self.composer_preferred_column = None;
        }
        self.reset_composer_scroll();
        self.refresh_command_palette();
    }

    pub(in crate::tui) fn update_composer_content_width(&mut self, width: usize) {
        self.composer_content_width = width.max(1);
    }

    pub(in crate::tui) fn move_composer_cursor_vertical(&mut self, direction: isize) {
        let cursor = self.composer_cursor_byte();
        let attachment_ranges = self.attachment_ranges();
        let positions = visual_cursor_positions(&self.input, self.composer_content_width.max(1))
            .into_iter()
            .filter(|(byte, _, _)| {
                !attachment_ranges
                    .iter()
                    .any(|(start, end, _)| *byte > *start && *byte < *end)
            })
            .collect::<Vec<_>>();
        let Some((_, current_row, current_col)) = positions
            .iter()
            .copied()
            .find(|(byte, _, _)| *byte == cursor)
        else {
            return;
        };
        let Some(target_row) = current_row.checked_add_signed(direction) else {
            return;
        };
        let desired_col = self.composer_preferred_column.unwrap_or(current_col);
        let target = positions
            .iter()
            .copied()
            .filter(|(_, row, _)| *row == target_row)
            .min_by_key(|(_, _, col)| col.abs_diff(desired_col));
        if let Some((target_byte, _, _)) = target {
            self.composer_preferred_column = Some(desired_col);
            self.set_composer_cursor(target_byte, true);
        }
    }

    pub(in crate::tui::state) fn insert_composer_str(&mut self, text: &str) {
        let cursor = self.composer_cursor_byte();
        self.input.insert_str(cursor, text);
        self.input_cursor = cursor + text.len();
        self.input_cursor_input_len = self.input.len();
        self.composer_preferred_column = None;
        self.reset_composer_scroll();
        self.history_index = None;
        self.command_palette_dismissed_input = None;
        self.refresh_command_palette();
    }

    pub(in crate::tui) fn push_composer_char(&mut self, ch: char) {
        let mut buffer = [0; 4];
        self.insert_composer_str(ch.encode_utf8(&mut buffer));
        self.input_status = None;
    }

    pub(in crate::tui) fn insert_composer_newline(&mut self) {
        self.insert_composer_str("\n");
    }

    pub(crate) const fn pasted_image_count(&self) -> usize {
        self.pending_attachments.len()
    }

    #[allow(clippy::needless_pass_by_value)]
    pub(in crate::tui) fn push_history(&mut self, prompt: String) {
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() || prompt.len() > HISTORY_MAX_ENTRY_BYTES {
            if prompt.len() > HISTORY_MAX_ENTRY_BYTES {
                self.input_status = Some("prompt too large for history".to_string());
            }
            return;
        }
        if self.history.last() != Some(&prompt) {
            self.history.push(prompt);
            self.history = bound_entries(std::mem::take(&mut self.history));
            self.persist_history();
        }
        self.history_index = None;
        self.history_draft.clear();
    }

    pub(in crate::tui) const fn history_recall_active(&self) -> bool {
        self.history_index.is_some()
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
            self.command_palette_dismissed_input = None;
            self.refresh_command_palette();
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
        self.command_palette_dismissed_input = None;
        self.refresh_command_palette();
    }
}
