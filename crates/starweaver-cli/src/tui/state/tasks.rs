use super::{InteractiveTuiState, TaskPanelItem};

impl InteractiveTuiState {
    pub(in crate::tui) fn set_task_panel_items(&mut self, items: Vec<TaskPanelItem>) {
        let was_complete = !self.task_panel_items.is_empty()
            && self
                .task_panel_items
                .iter()
                .all(|item| item.status == "completed");
        let is_complete = !items.is_empty() && items.iter().all(|item| item.status == "completed");
        self.task_panel_items = items;
        self.task_panel_index = self
            .task_panel_index
            .min(self.task_panel_items.len().saturating_sub(1));
        if is_complete {
            self.task_panel_open = false;
            self.task_panel_detail = false;
            self.task_panel_completed_hidden = true;
            if !was_complete {
                self.input_status = Some(format!(
                    "tasks completed {}/{}",
                    self.task_panel_items.len(),
                    self.task_panel_items.len()
                ));
            }
        } else {
            self.task_panel_completed_hidden = false;
        }
    }

    pub(in crate::tui) fn open_task_panel(&mut self) {
        if self.task_panel_items.is_empty() {
            self.input_status = Some("no tasks".to_string());
            return;
        }
        self.command_palette = None;
        self.model_picker_open = false;
        self.session_picker_open = false;
        self.selection_mode = false;
        self.task_panel_open = true;
        self.task_panel_detail = false;
        self.task_panel_completed_hidden = false;
        self.task_panel_index = self
            .task_panel_items
            .iter()
            .position(|item| item.status.starts_with("in_progress"))
            .unwrap_or(0);
        self.input_status = Some("task list".to_string());
    }

    pub(in crate::tui) fn close_task_panel(&mut self) {
        self.task_panel_open = false;
        self.task_panel_detail = false;
        self.input_status = Some("task list closed".to_string());
    }

    pub(in crate::tui) const fn task_panel_expanded(&self) -> bool {
        self.task_panel_open
    }

    pub(in crate::tui) fn move_task_panel_selection(&mut self, delta: isize) {
        let len = self.task_panel_items.len();
        if len == 0 {
            return;
        }
        let steps = delta.unsigned_abs() % len;
        self.task_panel_index = if delta.is_negative() {
            (self.task_panel_index + len - steps) % len
        } else {
            (self.task_panel_index + steps) % len
        };
        self.task_panel_detail = false;
        self.input_status = Some("task list".to_string());
    }

    pub(in crate::tui) fn toggle_task_panel_detail(&mut self) {
        if self.task_panel_items.is_empty() {
            return;
        }
        self.task_panel_detail = !self.task_panel_detail;
        self.input_status = Some(if self.task_panel_detail {
            "task details".to_string()
        } else {
            "task list".to_string()
        });
    }

    pub(in crate::tui) const fn task_panel_detail_visible(&self) -> bool {
        self.task_panel_detail
    }

    pub(in crate::tui) const fn task_panel_index(&self) -> usize {
        self.task_panel_index
    }

    pub(in crate::tui) const fn task_summary_visible(&self) -> bool {
        !self.task_panel_items.is_empty() && !self.task_panel_completed_hidden
    }

    pub(in crate::tui) fn selected_task(&self) -> Option<&TaskPanelItem> {
        self.task_panel_items.get(self.task_panel_index)
    }
}
