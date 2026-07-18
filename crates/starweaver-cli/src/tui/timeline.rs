use std::fmt::Write as _;

use serde_json::Value;

use crate::args::TuiRenderMode;

use super::markdown::{ASSISTANT_CONTENT_PREFIX, CONCISE_TOOL_SUMMARY_PREFIX};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct TuiItemId(u64);

#[derive(Clone, Debug, Default)]
pub(super) struct TuiTimeline {
    next_id: u64,
    generation: u64,
    items: Vec<TuiTimelineItem>,
}

#[derive(Clone, Debug)]
pub(super) struct TuiTimelineItem {
    pub id: TuiItemId,
    pub kind: TuiItemKind,
}

#[derive(Clone, Debug)]
pub(super) enum TuiItemKind {
    UserPrompt {
        text: String,
    },
    AssistantText {
        text: String,
        streaming: bool,
    },
    Thinking {
        text: String,
        streaming: bool,
    },
    ToolCall(ToolTimelineItem),
    Subagent(SubagentTimelineItem),
    ContextEvent {
        category: ContextEventCategory,
        lines: Vec<String>,
    },
    SystemNotice {
        level: NoticeLevel,
        lines: Vec<String>,
    },
    RunStatus {
        line: String,
    },
}

#[derive(Clone, Debug)]
pub(super) struct ToolTimelineItem {
    pub call_id: String,
    pub name: String,
    pub args_preview: Option<String>,
    pub call_line: String,
    pub status: ToolActivityStatus,
    pub return_lines: Vec<String>,
    pub visibility: ToolVisibility,
    pub concise: ToolConciseSummary,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ToolConciseSummary {
    pub line: String,
    pub detail_lines: Vec<String>,
    pub category: ToolSummaryCategory,
    pub importance: ToolSummaryImportance,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ToolSummaryCategory {
    Exploration,
    Shell,
    Mutation,
    Task,
    Generic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ToolSummaryImportance {
    Normal,
    Important,
}

impl ToolConciseSummary {
    pub(super) fn new(
        line: impl Into<String>,
        category: ToolSummaryCategory,
        importance: ToolSummaryImportance,
    ) -> Self {
        Self {
            line: line.into(),
            detail_lines: Vec::new(),
            category,
            importance,
        }
    }

    pub(super) fn with_details(mut self, detail_lines: Vec<String>) -> Self {
        self.detail_lines = detail_lines;
        self
    }
}

#[derive(Clone, Debug)]
pub(super) struct SubagentTimelineItem {
    pub agent_id: String,
    pub agent_name: String,
    pub status: SubagentStatus,
    pub tool_names: Vec<String>,
    pub output_preview: String,
    pub output_markdown: String,
    pub request_count: usize,
    pub duration_label: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ToolActivityStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ToolVisibility {
    Ordinary,
    ApprovalRequired,
    Deferred,
    TaskPanel,
    ErrorImportant,
    ContextHandoff,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SubagentStatus {
    Running,
    Done,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ContextEventCategory {
    Summary,
    Compaction,
    Goal,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NoticeLevel {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct ActiveToolActivity {
    pub line: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct TuiProjection {
    pub lines: Vec<String>,
    pub active_tools: Vec<ActiveToolActivity>,
}

impl TuiTimeline {
    pub(super) const fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) fn clear(&mut self) {
        self.items.clear();
        self.generation = self.generation.saturating_add(1);
    }

    pub(super) fn push_user_prompt(&mut self, text: impl Into<String>) -> TuiItemId {
        self.push(TuiItemKind::UserPrompt { text: text.into() })
    }

    pub(super) fn push_assistant_text(
        &mut self,
        text: impl Into<String>,
        streaming: bool,
    ) -> TuiItemId {
        self.push(TuiItemKind::AssistantText {
            text: text.into(),
            streaming,
        })
    }

    pub(super) fn push_thinking(&mut self, text: impl Into<String>, streaming: bool) -> TuiItemId {
        self.push(TuiItemKind::Thinking {
            text: text.into(),
            streaming,
        })
    }

    pub(super) fn push_context_event(
        &mut self,
        category: ContextEventCategory,
        lines: Vec<String>,
    ) -> TuiItemId {
        self.push(TuiItemKind::ContextEvent { category, lines })
    }

    pub(super) fn push_system_lines(
        &mut self,
        level: NoticeLevel,
        lines: Vec<String>,
    ) -> TuiItemId {
        self.push(TuiItemKind::SystemNotice { level, lines })
    }

    pub(super) fn push_run_status(&mut self, line: impl Into<String>) -> TuiItemId {
        self.push(TuiItemKind::RunStatus { line: line.into() })
    }

    pub(super) fn push_tool_call(&mut self, tool: ToolTimelineItem) -> TuiItemId {
        self.push(TuiItemKind::ToolCall(tool))
    }

    pub(super) fn push_subagent(&mut self, subagent: SubagentTimelineItem) -> TuiItemId {
        self.push(TuiItemKind::Subagent(subagent))
    }

    pub(super) fn append_text(&mut self, id: TuiItemId, delta: &str) {
        if delta.is_empty() {
            return;
        }
        if let Some(item) = self.item_mut(id) {
            match &mut item.kind {
                TuiItemKind::AssistantText { text, .. } | TuiItemKind::Thinking { text, .. } => {
                    text.push_str(delta);
                    self.generation = self.generation.saturating_add(1);
                }
                _ => {}
            }
        }
    }

    pub(super) fn finish_text_item(&mut self, id: TuiItemId) {
        if let Some(item) = self.item_mut(id) {
            match &mut item.kind {
                TuiItemKind::AssistantText { streaming, .. }
                | TuiItemKind::Thinking { streaming, .. } => {
                    *streaming = false;
                    self.generation = self.generation.saturating_add(1);
                }
                _ => {}
            }
        }
    }

    pub(super) fn update_tool_call(
        &mut self,
        id: TuiItemId,
        name: Option<String>,
        args_preview: Option<String>,
        call_line: Option<String>,
        visibility: Option<ToolVisibility>,
        concise: Option<ToolConciseSummary>,
    ) {
        if let Some(TuiTimelineItem {
            kind: TuiItemKind::ToolCall(tool),
            ..
        }) = self.item_mut(id)
        {
            if let Some(name) = name.filter(|name| !name.trim().is_empty()) {
                tool.name = name;
            }
            if args_preview
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            {
                tool.args_preview = args_preview;
            }
            if let Some(call_line) = call_line.filter(|line| !line.trim().is_empty()) {
                tool.call_line = call_line;
            }
            if let Some(visibility) = visibility {
                tool.visibility = visibility;
            }
            if let Some(concise) = concise {
                tool.concise = concise;
            }
            self.generation = self.generation.saturating_add(1);
        }
    }

    pub(super) fn finish_tool_call(
        &mut self,
        id: TuiItemId,
        status: ToolActivityStatus,
        return_lines: Vec<String>,
        visibility: ToolVisibility,
        concise: ToolConciseSummary,
    ) {
        if let Some(TuiTimelineItem {
            kind: TuiItemKind::ToolCall(tool),
            ..
        }) = self.item_mut(id)
        {
            tool.status = status;
            tool.return_lines = return_lines;
            tool.visibility = visibility;
            tool.concise = concise;
            self.generation = self.generation.saturating_add(1);
        }
    }

    pub(super) fn tool_status(&self, id: TuiItemId) -> Option<ToolActivityStatus> {
        self.items.iter().find_map(|item| {
            (item.id == id).then_some(match &item.kind {
                TuiItemKind::ToolCall(tool) => Some(tool.status),
                _ => None,
            })?
        })
    }

    pub(super) fn is_tail_item(&self, id: TuiItemId) -> bool {
        self.items.last().is_some_and(|item| item.id == id)
    }

    pub(super) fn update_subagent(&mut self, id: TuiItemId, update: SubagentUpdate) {
        if let Some(TuiTimelineItem {
            kind: TuiItemKind::Subagent(subagent),
            ..
        }) = self.item_mut(id)
        {
            if let Some(agent_name) = update.agent_name.filter(|name| !name.trim().is_empty()) {
                subagent.agent_name = agent_name;
            }
            if let Some(status) = update.status {
                subagent.status = status;
            }
            if let Some(duration_label) = update.duration_label {
                subagent.duration_label = Some(duration_label);
            }
            if let Some(output_preview) = update.output_preview {
                subagent.output_preview = output_preview;
            }
            if let Some(output_markdown) = update.output_markdown {
                subagent.output_markdown = output_markdown;
            }
            if let Some(request_count) = update.request_count {
                subagent.request_count = request_count;
            }
            for tool_name in update.tool_names {
                if !subagent.tool_names.iter().any(|name| name == &tool_name) {
                    subagent.tool_names.push(tool_name);
                }
            }
            self.generation = self.generation.saturating_add(1);
        }
    }

    fn push(&mut self, kind: TuiItemKind) -> TuiItemId {
        let id = TuiItemId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        self.items.push(TuiTimelineItem { id, kind });
        self.generation = self.generation.saturating_add(1);
        id
    }

    fn item_mut(&mut self, id: TuiItemId) -> Option<&mut TuiTimelineItem> {
        self.items.iter_mut().find(|item| item.id == id)
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct SubagentUpdate {
    pub agent_name: Option<String>,
    pub status: Option<SubagentStatus>,
    pub tool_names: Vec<String>,
    pub output_preview: Option<String>,
    pub output_markdown: Option<String>,
    pub request_count: Option<usize>,
    pub duration_label: Option<String>,
}

impl TuiProjection {
    pub(super) fn active_tool_label(&self) -> Option<String> {
        let first = self.active_tools.first()?;
        if self.active_tools.len() == 1 {
            return Some(first.line.clone());
        }
        Some(format!(
            "Running tools: {} +{} more",
            first.line,
            self.active_tools.len().saturating_sub(1)
        ))
    }
}

pub(super) fn project_timeline(timeline: &TuiTimeline, mode: TuiRenderMode) -> TuiProjection {
    let mut projection = TuiProjection::default();
    let mut exploration_group = ConciseExplorationGroup::default();
    let mut previous_projected_thinking = false;
    for item in &timeline.items {
        let follows_thinking = previous_projected_thinking && exploration_group.lines.is_empty();
        let line_count_before = projection.lines.len();
        let projects_visible_thinking = matches!(
            &item.kind,
            TuiItemKind::Thinking { text, streaming }
                if !text.trim().is_empty() || *streaming
        );
        match &item.kind {
            TuiItemKind::UserPrompt { text } => {
                flush_concise_exploration_group(&mut exploration_group, &mut projection.lines);
                push_section_gap(&mut projection.lines);
                push_user_prompt_lines(&mut projection.lines, text);
            }
            TuiItemKind::AssistantText { text, streaming } => {
                flush_concise_exploration_group(&mut exploration_group, &mut projection.lines);
                if !text.trim().is_empty() || *streaming {
                    push_section_gap(&mut projection.lines);
                    projection.lines.push("Assistant:".to_string());
                    push_markdown_lines(&mut projection.lines, text);
                    if text.is_empty() && *streaming {
                        projection.lines.push(ASSISTANT_CONTENT_PREFIX.to_string());
                    }
                }
            }
            TuiItemKind::Thinking { text, streaming } => {
                flush_concise_exploration_group(&mut exploration_group, &mut projection.lines);
                project_thinking(
                    text,
                    *streaming,
                    mode,
                    !follows_thinking,
                    &mut projection.lines,
                );
            }
            TuiItemKind::ToolCall(tool)
                if matches!(mode, TuiRenderMode::Concise) && is_groupable_exploration(tool) =>
            {
                update_active_tool_activity(tool, &mut projection);
                exploration_group.push(tool);
            }
            TuiItemKind::ToolCall(tool) => {
                flush_concise_exploration_group(&mut exploration_group, &mut projection.lines);
                project_tool(tool, mode, &mut projection);
            }
            TuiItemKind::Subagent(subagent) => {
                flush_concise_exploration_group(&mut exploration_group, &mut projection.lines);
                project_subagent(subagent, mode, &mut projection.lines);
            }
            TuiItemKind::ContextEvent { category, lines } => {
                if matches!(mode, TuiRenderMode::Concise)
                    && matches!(category, ContextEventCategory::Other)
                {
                    continue;
                }
                flush_concise_exploration_group(&mut exploration_group, &mut projection.lines);
                push_section_gap(&mut projection.lines);
                projection.lines.extend(lines.iter().cloned());
            }
            TuiItemKind::SystemNotice { level, lines } => {
                if matches!(mode, TuiRenderMode::Concise)
                    && !matches!(
                        level,
                        NoticeLevel::Info | NoticeLevel::Warning | NoticeLevel::Error
                    )
                {
                    continue;
                }
                flush_concise_exploration_group(&mut exploration_group, &mut projection.lines);
                push_section_gap(&mut projection.lines);
                projection.lines.extend(lines.iter().cloned());
            }
            TuiItemKind::RunStatus { line } => {
                flush_concise_exploration_group(&mut exploration_group, &mut projection.lines);
                push_section_gap(&mut projection.lines);
                projection.lines.push(line.clone());
            }
        }
        if projection.lines.len() > line_count_before {
            previous_projected_thinking = projects_visible_thinking;
        }
    }
    flush_concise_exploration_group(&mut exploration_group, &mut projection.lines);
    projection
}

fn project_thinking(
    text: &str,
    streaming: bool,
    _mode: TuiRenderMode,
    separate_section: bool,
    lines: &mut Vec<String>,
) {
    if text.trim().is_empty() && !streaming {
        return;
    }
    if separate_section {
        push_section_gap(lines);
    }
    push_markdown_lines(lines, &format_thinking_markdown(text, streaming));
}

fn project_tool(tool: &ToolTimelineItem, mode: TuiRenderMode, projection: &mut TuiProjection) {
    update_active_tool_activity(tool, projection);
    match mode {
        TuiRenderMode::Normal => project_tool_full(tool, projection, false),
        TuiRenderMode::Debug => project_tool_full(tool, projection, true),
        TuiRenderMode::Concise => project_tool_concise(tool, projection),
    }
}

fn update_active_tool_activity(tool: &ToolTimelineItem, projection: &mut TuiProjection) {
    if tool.status == ToolActivityStatus::Running && tool.visibility == ToolVisibility::Ordinary {
        projection.active_tools.push(ActiveToolActivity {
            line: tool.concise.line.clone(),
        });
    }
}

fn project_tool_full(tool: &ToolTimelineItem, projection: &mut TuiProjection, debug: bool) {
    push_section_gap(&mut projection.lines);
    projection.lines.push(tool.call_line.clone());
    if debug {
        projection.lines.push(format!(
            "[debug] tool_call_id={} status={:?} visibility={:?} summary_category={:?} summary_importance={:?}",
            tool.call_id,
            tool.status,
            tool.visibility,
            tool.concise.category,
            tool.concise.importance
        ));
    }
    projection.lines.extend(tool.return_lines.iter().cloned());
}

fn project_tool_concise(tool: &ToolTimelineItem, projection: &mut TuiProjection) {
    if matches!(tool.visibility, ToolVisibility::ContextHandoff) {
        project_context_handoff_tool_concise(tool, projection);
        return;
    }
    if tool.concise.line.trim().is_empty() {
        return;
    }
    push_section_gap(&mut projection.lines);
    projection.lines.push(format_concise_tool_summary_line(
        &tool.concise.line,
        tool.concise.importance,
    ));
    if matches!(tool.concise.importance, ToolSummaryImportance::Important)
        || matches!(tool.visibility, ToolVisibility::ErrorImportant)
    {
        projection
            .lines
            .extend(tool.concise.detail_lines.iter().cloned());
    }
}

fn project_context_handoff_tool_concise(tool: &ToolTimelineItem, projection: &mut TuiProjection) {
    if tool.return_lines.is_empty() {
        if !tool.concise.line.trim().is_empty() {
            push_section_gap(&mut projection.lines);
            projection.lines.push(format_concise_tool_summary_line(
                &tool.concise.line,
                ToolSummaryImportance::Important,
            ));
        }
        return;
    }
    push_section_gap(&mut projection.lines);
    for (index, line) in tool.return_lines.iter().enumerate() {
        if index == 0 && line == "Tool result: summarize" {
            projection.lines.push("Summary complete".to_string());
        } else {
            projection.lines.push(line.clone());
        }
    }
}

#[derive(Default)]
struct ConciseExplorationGroup {
    lines: Vec<String>,
    running: bool,
}

impl ConciseExplorationGroup {
    fn push(&mut self, tool: &ToolTimelineItem) {
        self.running |= tool.status == ToolActivityStatus::Running;
        if !tool.concise.line.trim().is_empty()
            && !self.lines.iter().any(|line| line == &tool.concise.line)
        {
            self.lines.push(tool.concise.line.clone());
        }
    }
}

const fn is_groupable_exploration(tool: &ToolTimelineItem) -> bool {
    matches!(tool.concise.category, ToolSummaryCategory::Exploration)
        && matches!(tool.concise.importance, ToolSummaryImportance::Normal)
        && matches!(tool.visibility, ToolVisibility::Ordinary)
}

fn flush_concise_exploration_group(group: &mut ConciseExplorationGroup, lines: &mut Vec<String>) {
    if group.lines.is_empty() {
        return;
    }
    push_section_gap(lines);
    if group.running {
        lines.push(format_concise_tool_summary_line(
            "Exploring",
            ToolSummaryImportance::Normal,
        ));
    } else {
        lines.push(format_concise_tool_summary_line(
            "Explored",
            ToolSummaryImportance::Normal,
        ));
    }
    lines.extend(group.lines.iter().map(|line| {
        format_concise_tool_summary_line(&format!("  {line}"), ToolSummaryImportance::Normal)
    }));
    group.lines.clear();
    group.running = false;
}

fn format_concise_tool_summary_line(line: &str, importance: ToolSummaryImportance) -> String {
    if matches!(importance, ToolSummaryImportance::Important) {
        line.to_string()
    } else {
        format!("{CONCISE_TOOL_SUMMARY_PREFIX}{line}")
    }
}

fn project_subagent(subagent: &SubagentTimelineItem, mode: TuiRenderMode, lines: &mut Vec<String>) {
    push_section_gap(lines);
    lines.push(format_subagent_line(subagent));
    if matches!(mode, TuiRenderMode::Debug) {
        lines.push(format!("[debug] subagent_id={}", subagent.agent_id));
    }
    if !subagent.output_markdown.trim().is_empty() {
        push_markdown_lines(lines, &subagent.output_markdown);
    }
}

fn format_subagent_line(subagent: &SubagentTimelineItem) -> String {
    let status = match subagent.status {
        SubagentStatus::Running => "running",
        SubagentStatus::Done => "done",
        SubagentStatus::Failed => "failed",
    };
    let mut line = format!("[{}] {status}", subagent.agent_name);
    if let Some(duration) = subagent.duration_label.as_deref() {
        line.push_str(" (");
        line.push_str(duration);
        line.push(')');
    }
    if subagent.request_count > 0 {
        let _ = write!(line, " | {} reqs", subagent.request_count);
    }
    if !subagent.tool_names.is_empty() {
        let _ = write!(line, " | tools: {}", subagent.tool_names.join(", "));
    }
    if !subagent.output_preview.trim().is_empty() && subagent.output_markdown.trim().is_empty() {
        let _ = write!(line, " | \"{}\"", subagent.output_preview.trim());
    }
    line
}

fn push_user_prompt_lines(lines: &mut Vec<String>, prompt: &str) {
    let mut prompt_lines = prompt.lines();
    if let Some(first) = prompt_lines.next() {
        lines.push(format!("User: {first}"));
        lines.extend(prompt_lines.map(|line| format!("  {line}")));
    } else {
        lines.push("User:".to_string());
    }
}

fn push_markdown_lines(lines: &mut Vec<String>, markdown: &str) {
    if markdown.is_empty() {
        return;
    }
    for line in markdown.lines() {
        lines.push(format!("{ASSISTANT_CONTENT_PREFIX}{line}"));
    }
    if markdown.ends_with('\n') {
        lines.push(ASSISTANT_CONTENT_PREFIX.to_string());
    }
}

fn format_thinking_markdown(text: &str, streaming: bool) -> String {
    if text.is_empty() && streaming {
        return "> ".to_string();
    }
    text.lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn push_section_gap(lines: &mut Vec<String>) {
    if lines.last().is_some_and(|line| !line.is_empty()) {
        lines.push(String::new());
    }
}

pub(super) fn value_args_preview(value: &Value, max_chars: usize) -> Option<String> {
    let text = value.to_string();
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed == "{}" || trimmed == "null" {
        return None;
    }
    Some(truncate_chars(trimmed, max_chars))
}

pub(super) fn truncate_chars(text: &str, max_chars: usize) -> String {
    let max_chars = max_chars.max(1);
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    format!("{}…", text.chars().take(keep).collect::<String>())
}
