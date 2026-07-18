use super::{InteractiveTuiState, TaskPanelItem};

impl InteractiveTuiState {
    pub(in crate::tui) fn reset_task_panel_for_session(&mut self) {
        self.task_panel_items.clear();
        self.task_panel_open = false;
        self.task_panel_index = 0;
        self.task_panel_completed_hidden = false;
    }

    pub(in crate::tui) fn set_task_panel_items(&mut self, items: Vec<TaskPanelItem>) {
        let had_items = !self.task_panel_items.is_empty();
        let was_complete = had_items
            && self
                .task_panel_items
                .iter()
                .all(|item| item.status == "completed");
        let is_complete = !items.is_empty() && items.iter().all(|item| item.status == "completed");
        let selected_task_id = self
            .task_panel_items
            .get(self.task_panel_index)
            .map(|item| item.id.as_str());
        let latest_changed_index = items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                self.task_panel_items
                    .iter()
                    .find(|previous| previous.id == item.id)
                    != Some(*item)
            })
            .map(|(index, _)| index)
            .next_back();
        let retained_index = selected_task_id
            .and_then(|id| items.iter().position(|item| item.id == id))
            .unwrap_or(0);
        self.task_panel_index = latest_changed_index
            .unwrap_or(retained_index)
            .min(items.len().saturating_sub(1));
        self.task_panel_items = items;
        if is_complete {
            self.task_panel_open = false;
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
            if (!had_items || was_complete) && !self.task_panel_items.is_empty() {
                self.task_panel_open = true;
                self.task_panel_index = self
                    .task_panel_items
                    .iter()
                    .position(|item| item.status.starts_with("in_progress"))
                    .unwrap_or(0);
            }
        }
    }

    pub(in crate::tui) fn toggle_task_panel(&mut self) {
        if self.task_panel_open {
            self.close_task_panel();
        } else {
            self.open_task_panel();
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
        self.input_status = Some("tasks minimized".to_string());
    }

    pub(in crate::tui) const fn task_panel_expanded(&self) -> bool {
        self.task_panel_open
    }

    pub(in crate::tui) const fn task_panel_index(&self) -> usize {
        self.task_panel_index
    }

    pub(in crate::tui) const fn task_summary_visible(&self) -> bool {
        !self.task_panel_items.is_empty() && !self.task_panel_completed_hidden
    }
}
