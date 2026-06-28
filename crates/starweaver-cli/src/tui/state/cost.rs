use std::collections::BTreeMap;

use serde_json::Value;
use starweaver_usage::{
    add_optional_pricing, PricingEstimate, Usage, UsageAgentTotal, UsageSnapshot,
};

use super::{
    cache_hit_rate_label, format_u64_with_commas, push_usage_entry_lines, InteractiveTuiState,
};

impl InteractiveTuiState {
    pub(super) fn apply_usage_snapshot_payload(&mut self, payload: &Value, sequence: usize) {
        if let Ok(snapshot) = serde_json::from_value::<UsageSnapshot>(payload.clone()) {
            if let Some(context_tokens) = latest_request_total_tokens(&snapshot) {
                self.latest_request_total_tokens = Some(context_tokens);
                self.context_tokens = Some(
                    self.context_tokens
                        .map_or(context_tokens, |current| current.max(context_tokens)),
                );
            }
            let key = if snapshot.run_id.is_empty() {
                format!("sequence:{sequence}")
            } else {
                snapshot.run_id.clone()
            };
            if !snapshot.total_usage.is_empty()
                && current_run_matches_snapshot(self.current_run_id.as_deref(), &snapshot)
            {
                self.current_run_usage = Some(snapshot.total_usage.clone());
            }
            self.usage_snapshots.insert(key, snapshot);
        }
    }

    pub(super) fn format_cost_summary_lines(&self) -> Vec<String> {
        let mut lines = format_cost_summary_header(self);
        let summary = UsageSummary::from_snapshots(self.usage_snapshots.values());

        if summary.is_empty() {
            lines.push("[SYS] No usage data available.".to_string());
            return lines;
        }

        lines.push(String::new());
        push_model_summary_lines(&mut lines, &summary);
        push_agent_summary_lines(&mut lines, &summary);
        push_total_summary_lines(&mut lines, &summary);
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

struct UsageSummary {
    model_usages: BTreeMap<String, Usage>,
    model_pricing: BTreeMap<String, PricingEstimate>,
    agent_usages: BTreeMap<String, (Usage, Option<PricingEstimate>)>,
    total_pricing: Option<PricingEstimate>,
}

impl UsageSummary {
    fn from_snapshots<'a>(snapshots: impl Iterator<Item = &'a UsageSnapshot>) -> Self {
        let mut summary = Self {
            model_usages: BTreeMap::new(),
            model_pricing: BTreeMap::new(),
            agent_usages: BTreeMap::new(),
            total_pricing: None,
        };
        for snapshot in snapshots {
            summary.add_snapshot(snapshot);
        }
        summary
    }

    fn is_empty(&self) -> bool {
        self.model_usages.is_empty() && self.agent_usages.is_empty()
    }

    fn total_usage(&self) -> Usage {
        let mut total = Usage::default();
        for usage in self.model_usages.values() {
            total.add_assign(usage);
        }
        total
    }

    fn add_snapshot(&mut self, snapshot: &UsageSnapshot) {
        add_snapshot_pricing(&mut self.total_pricing, snapshot);
        for (model_id, usage) in &snapshot.model_usages {
            self.model_usages
                .entry(model_id.clone())
                .or_default()
                .add_assign(usage);
        }
        for (model_id, pricing) in &snapshot.model_estimate_pricing {
            self.model_pricing
                .entry(model_id.clone())
                .or_default()
                .add_assign(pricing);
        }
        for (agent_id, total) in &snapshot.agent_usages {
            add_agent_usage(&mut self.agent_usages, agent_id, total);
        }
    }
}

fn format_cost_summary_header(state: &InteractiveTuiState) -> Vec<String> {
    let mut lines = vec!["[SYS] Token Usage Summary:".to_string(), String::new()];
    lines.push(format!(
        "[SYS] Latest request total tokens: {}",
        format_u64_with_commas(state.latest_request_total_tokens.unwrap_or_default())
    ));
    lines.push(format!(
        "[SYS] Displayed context high-water: {}",
        format_u64_with_commas(state.context_tokens.unwrap_or_default())
    ));
    if let Some(window) = state.context_window {
        lines.push(format!(
            "[SYS] Context window: {}",
            format_u64_with_commas(window)
        ));
        lines.push(format!(
            "[SYS] Context used: {}",
            state.context_percent_label()
        ));
    }
    lines
}

fn add_snapshot_pricing(total_pricing: &mut Option<PricingEstimate>, snapshot: &UsageSnapshot) {
    if snapshot.estimate_pricing.is_some() {
        add_optional_pricing(total_pricing, snapshot.estimate_pricing.as_ref());
    } else {
        for pricing in snapshot.model_estimate_pricing.values() {
            add_optional_pricing(total_pricing, Some(pricing));
        }
    }
}

fn add_agent_usage(
    agent_usages: &mut BTreeMap<String, (Usage, Option<PricingEstimate>)>,
    agent_id: &str,
    total: &UsageAgentTotal,
) {
    let (usage, pricing) = agent_usages.entry(agent_id.to_string()).or_default();
    usage.add_assign(&total.usage);
    add_optional_pricing(pricing, total.estimate_pricing.as_ref());
}

fn push_model_summary_lines(lines: &mut Vec<String>, summary: &UsageSummary) {
    lines.push("[SYS] By Model:".to_string());
    for (model_id, usage) in &summary.model_usages {
        push_usage_entry_lines(lines, model_id, usage);
        if let Some(pricing) = summary.model_pricing.get(model_id) {
            push_pricing_line(lines, pricing);
        }
        lines.push(String::new());
    }
}

fn push_agent_summary_lines(lines: &mut Vec<String>, summary: &UsageSummary) {
    lines.push("[SYS] By Agent:".to_string());
    for (agent_id, (usage, pricing)) in &summary.agent_usages {
        push_usage_entry_lines(lines, agent_id, usage);
        if let Some(pricing) = pricing {
            push_pricing_line(lines, pricing);
        }
        lines.push(String::new());
    }
}

fn push_total_summary_lines(lines: &mut Vec<String>, summary: &UsageSummary) {
    let total = summary.total_usage();
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
    if let Some(cache_hit_rate) = cache_hit_rate_label(&total) {
        lines.push(format!("[SYS]   Cache Hit Rate: {cache_hit_rate}"));
    }
    lines.push(format!(
        "[SYS]   Total:  {} tokens",
        format_u64_with_commas(total.total_tokens)
    ));
    lines.push(format!("[SYS]   Requests: {}", total.requests));
    if total.tool_calls > 0 {
        lines.push(format!("[SYS]   Tool calls: {}", total.tool_calls));
    }
    if let Some(pricing) = &summary.total_pricing {
        lines.push(format!(
            "[SYS]   Estimated pricing: {} USD",
            format_usd_pricing(pricing)
        ));
    }
}

fn push_pricing_line(lines: &mut Vec<String>, pricing: &PricingEstimate) {
    lines.push(format!(
        "[SYS]     Estimated pricing: {} USD",
        format_usd_pricing(pricing)
    ));
}

fn current_run_matches_snapshot(current_run_id: Option<&str>, snapshot: &UsageSnapshot) -> bool {
    current_run_id.is_none_or(|run_id| snapshot.run_id.is_empty() || snapshot.run_id == run_id)
}

fn latest_request_total_tokens(snapshot: &UsageSnapshot) -> Option<u64> {
    snapshot
        .latest_usage
        .as_ref()
        .and_then(|usage| (usage.total_tokens > 0).then_some(usage.total_tokens))
        .or_else(|| {
            (snapshot.total_usage.requests <= 1 && snapshot.total_usage.total_tokens > 0)
                .then_some(snapshot.total_usage.total_tokens)
        })
}

fn format_usd_pricing(estimate: &PricingEstimate) -> String {
    let whole = estimate.amount_micros_usd / 1_000_000;
    let micros = estimate.amount_micros_usd % 1_000_000;
    format!("${whole}.{micros:06}")
}
