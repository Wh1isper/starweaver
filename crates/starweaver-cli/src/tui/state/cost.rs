use std::collections::BTreeMap;

use serde_json::Value;
use starweaver_core::{Usage, UsageSnapshot};

use super::{format_u64_with_commas, push_usage_entry_lines, InteractiveTuiState};

impl InteractiveTuiState {
    pub(super) fn apply_usage_snapshot_payload(&mut self, payload: &Value, sequence: usize) {
        if let Ok(snapshot) = serde_json::from_value::<UsageSnapshot>(payload.clone()) {
            if snapshot.total_usage.total_tokens > 0 {
                self.context_tokens = Some(snapshot.total_usage.total_tokens);
            }
            let key = if snapshot.run_id.is_empty() {
                format!("sequence:{sequence}")
            } else {
                snapshot.run_id.clone()
            };
            self.usage_snapshots.insert(key, snapshot);
        }
    }

    pub(super) fn format_cost_summary_lines(&self) -> Vec<String> {
        let mut lines = vec!["[SYS] Token Usage Summary:".to_string(), String::new()];
        lines.push(format!(
            "[SYS] Latest request context tokens: {}",
            format_u64_with_commas(self.context_tokens.unwrap_or_default())
        ));
        if let Some(window) = self.context_window {
            lines.push(format!(
                "[SYS] Context window: {}",
                format_u64_with_commas(window)
            ));
            lines.push(format!(
                "[SYS] Context used: {}",
                self.context_percent_label()
            ));
        }

        let mut model_usages = BTreeMap::<String, Usage>::new();
        let mut agent_usages = BTreeMap::<String, Usage>::new();
        for snapshot in self.usage_snapshots.values() {
            for (model_id, usage) in &snapshot.model_usages {
                model_usages
                    .entry(model_id.clone())
                    .or_default()
                    .add_assign(usage);
            }
            for (agent_id, total) in &snapshot.agent_usages {
                agent_usages
                    .entry(agent_id.clone())
                    .or_default()
                    .add_assign(&total.usage);
            }
        }

        if model_usages.is_empty() && agent_usages.is_empty() {
            lines.push("[SYS] No usage data available.".to_string());
            return lines;
        }

        lines.push(String::new());
        lines.push("[SYS] By Model:".to_string());
        for (model_id, usage) in &model_usages {
            push_usage_entry_lines(&mut lines, model_id, usage);
            lines.push(String::new());
        }

        lines.push("[SYS] By Agent:".to_string());
        for (agent_id, usage) in &agent_usages {
            push_usage_entry_lines(&mut lines, agent_id, usage);
            lines.push(String::new());
        }

        let mut total = Usage::default();
        for usage in model_usages.values() {
            total.add_assign(usage);
        }
        lines.push("[SYS] Total:".to_string());
        lines.push(format!(
            "[SYS]   Input:  {} tokens",
            format_u64_with_commas(total.input_tokens)
        ));
        lines.push(format!(
            "[SYS]   Output: {} tokens",
            format_u64_with_commas(total.output_tokens)
        ));
        if total.cache_write_tokens > 0 {
            lines.push(format!(
                "[SYS]   Cache Write: {} tokens",
                format_u64_with_commas(total.cache_write_tokens)
            ));
        }
        if total.cache_read_tokens > 0 {
            lines.push(format!(
                "[SYS]   Cache Read:  {} tokens",
                format_u64_with_commas(total.cache_read_tokens)
            ));
        }
        lines.push(format!(
            "[SYS]   Total:  {} tokens",
            format_u64_with_commas(total.total_tokens)
        ));
        lines.push(format!("[SYS]   Requests: {}", total.requests));
        if total.tool_calls > 0 {
            lines.push(format!("[SYS]   Tool calls: {}", total.tool_calls));
        }
        lines
    }

    pub(in crate::tui) fn context_percent_label(&self) -> String {
        match (self.context_tokens, self.context_window) {
            (Some(tokens), Some(window)) if window > 0 => {
                format!(
                    "{}%",
                    tokens.saturating_mul(100).saturating_add(window / 2) / window
                )
            }
            (None, Some(window)) if window > 0 => "0%".to_string(),
            _ => "--%".to_string(),
        }
    }
}
