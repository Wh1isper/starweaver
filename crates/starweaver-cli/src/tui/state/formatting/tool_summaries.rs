use serde_json::Value;

use crate::tui::timeline::value_args_preview;

use super::{
    ToolConciseSummary, ToolSummaryCategory, ToolSummaryImportance, ToolVisibility,
    compact_status_text, is_empty_result, is_task_tool_name, plural_suffix, value_text,
};

const CONCISE_ARG_MAX_CHARS: usize = 96;
const CONCISE_DETAIL_MAX_LINES: usize = 3;

pub(in crate::tui::state) fn format_streaming_tool_summary(
    name: &str,
    args_preview: Option<&str>,
) -> ToolConciseSummary {
    let category = tool_summary_category(name);
    let line = match category {
        ToolSummaryCategory::Shell => args_preview.map_or_else(
            || format!("Running {name}"),
            |args| format!("Running {name} {}", truncate_concise(args)),
        ),
        ToolSummaryCategory::Exploration => exploration_call_summary(name, None, args_preview),
        ToolSummaryCategory::Mutation => args_preview.map_or_else(
            || format!("Preparing {name}"),
            |args| format!("Preparing {name} {}", truncate_concise(args)),
        ),
        ToolSummaryCategory::Task => args_preview.map_or_else(
            || format!("Updating tasks with {name}"),
            |args| format!("Updating tasks with {name} {}", truncate_concise(args)),
        ),
        ToolSummaryCategory::Generic => args_preview.map_or_else(
            || format!("Calling {name}"),
            |args| format!("Calling {name} {}", truncate_concise(args)),
        ),
    };
    ToolConciseSummary::new(line, category, ToolSummaryImportance::Normal)
}

pub(in crate::tui::state) fn format_tool_call_summary(
    call: &starweaver_model::ToolCallPart,
) -> ToolConciseSummary {
    let args = call.arguments.replay_value();
    let args_preview = value_args_preview(&args, CONCISE_ARG_MAX_CHARS);
    format_tool_call_summary_from_parts(&call.name, Some(&args), args_preview.as_deref())
}

pub(in crate::tui::state) fn format_tool_call_summary_from_parts(
    name: &str,
    args: Option<&Value>,
    args_preview: Option<&str>,
) -> ToolConciseSummary {
    let category = tool_summary_category(name);
    let line = match category {
        ToolSummaryCategory::Shell => shell_call_summary(name, args, args_preview),
        ToolSummaryCategory::Exploration => exploration_call_summary(name, args, args_preview),
        ToolSummaryCategory::Mutation => mutation_call_summary(name, args, args_preview),
        ToolSummaryCategory::Task => task_call_summary(name, args_preview),
        ToolSummaryCategory::Generic => generic_call_summary(name, args_preview),
    };
    ToolConciseSummary::new(line, category, ToolSummaryImportance::Normal)
}

pub(in crate::tui::state) fn format_tool_return_summary(
    tool_return: &starweaver_model::ToolReturnPart,
    arguments: Option<&Value>,
    visibility: ToolVisibility,
) -> ToolConciseSummary {
    let display_value = tool_return
        .user_content
        .as_ref()
        .unwrap_or(&tool_return.content);
    let category = tool_summary_category(&tool_return.name);
    let important = tool_return.is_error
        || matches!(
            visibility,
            ToolVisibility::ApprovalRequired
                | ToolVisibility::Deferred
                | ToolVisibility::ErrorImportant
        );
    let mut summary = ToolConciseSummary::new(
        match category {
            ToolSummaryCategory::Shell => {
                shell_return_summary(tool_return, display_value, arguments)
            }
            ToolSummaryCategory::Exploration => {
                exploration_return_summary(&tool_return.name, display_value, arguments)
            }
            ToolSummaryCategory::Mutation => {
                mutation_return_summary(&tool_return.name, display_value, arguments)
            }
            ToolSummaryCategory::Task => {
                task_return_summary(&tool_return.name, display_value, arguments)
            }
            ToolSummaryCategory::Generic => generic_return_summary(&tool_return.name, arguments),
        },
        category,
        if important {
            ToolSummaryImportance::Important
        } else {
            ToolSummaryImportance::Normal
        },
    );
    if important {
        let detail_lines = if matches!(category, ToolSummaryCategory::Shell) {
            shell_detail_lines(display_value)
        } else {
            concise_detail_lines(display_value)
        };
        summary = summary.with_details(detail_lines);
    }
    summary
}

fn tool_summary_category(name: &str) -> ToolSummaryCategory {
    match name {
        "view" | "ls" | "glob" | "grep" | "search" | "scrape" | "fetch" | "pdf_convert"
        | "office_to_markdown" => ToolSummaryCategory::Exploration,
        "shell_exec" | "shell_wait" | "shell_status" | "shell_input" | "shell_signal"
        | "shell_kill" | "shell_monitor" => ToolSummaryCategory::Shell,
        "edit" | "multi_edit" | "write" => ToolSummaryCategory::Mutation,
        name if is_task_tool_name(name) => ToolSummaryCategory::Task,
        _ => ToolSummaryCategory::Generic,
    }
}

fn shell_call_summary(name: &str, args: Option<&Value>, args_preview: Option<&str>) -> String {
    match name {
        "shell_exec" | "shell_monitor" => args.and_then(command_arg).map_or_else(
            || format!("Running {name}"),
            |command| format!("Running {}", truncate_concise(command)),
        ),
        "shell_wait" => args.and_then(process_arg).map_or_else(
            || "Waiting for background command".to_string(),
            |process| format!("Waiting for background command {process}"),
        ),
        "shell_input" => args.and_then(process_arg).map_or_else(
            || "Sending input to background command".to_string(),
            |process| format!("Sending input to background command {process}"),
        ),
        "shell_kill" => args.and_then(process_arg).map_or_else(
            || "Stopping background command".to_string(),
            |process| format!("Stopping background command {process}"),
        ),
        "shell_status" => "Checking background commands".to_string(),
        "shell_signal" => args.and_then(process_arg).map_or_else(
            || "Signaling background command".to_string(),
            |process| format!("Signaling background command {process}"),
        ),
        _ => args_preview.map_or_else(
            || format!("Running {name}"),
            |args| format!("Running {name} {}", truncate_concise(args)),
        ),
    }
}

fn shell_return_summary(
    tool_return: &starweaver_model::ToolReturnPart,
    result: &Value,
    arguments: Option<&Value>,
) -> String {
    let command = result
        .get("command")
        .and_then(Value::as_str)
        .or_else(|| arguments.and_then(command_arg));
    let status = result
        .get("return_code")
        .and_then(Value::as_i64)
        .map(|code| {
            if code == 0 {
                String::new()
            } else {
                format!(" — failed exit {code}")
            }
        })
        .or_else(|| {
            result
                .get("status")
                .and_then(Value::as_str)
                .filter(|status| !status.trim().is_empty())
                .map(|status| format!(" — {status}"))
        })
        .unwrap_or_else(|| {
            if tool_return.is_error {
                " — failed".to_string()
            } else {
                String::new()
            }
        });
    match tool_return.name.as_str() {
        "shell_exec" | "shell_monitor" => command.map_or_else(
            || format!("Ran {}{status}", tool_return.name),
            |command| format!("Ran {}{status}", truncate_concise(command)),
        ),
        "shell_wait" => "Waited for background command".to_string() + &status,
        "shell_input" => "Sent input to background command".to_string() + &status,
        "shell_kill" => "Stopped background command".to_string() + &status,
        "shell_status" => "Checked background commands".to_string() + &status,
        "shell_signal" => "Signaled background command".to_string() + &status,
        _ => format!("Ran {}{status}", tool_return.name),
    }
}

fn exploration_call_summary(
    name: &str,
    args: Option<&Value>,
    args_preview: Option<&str>,
) -> String {
    match name {
        "view" => args
            .and_then(path_arg)
            .map_or_else(|| "Read file".to_string(), |path| format!("Read {path}")),
        "ls" => args.and_then(path_arg).map_or_else(
            || "Listed files".to_string(),
            |path| format!("Listed {path}"),
        ),
        "glob" => args
            .and_then(|args| args.get("pattern").and_then(Value::as_str))
            .map_or_else(
                || "Found files".to_string(),
                |pattern| format!("Found files matching {pattern}"),
            ),
        "grep" => args
            .and_then(|args| args.get("pattern").and_then(Value::as_str))
            .map_or_else(
                || "Searched files".to_string(),
                |pattern| format!("Searched {pattern}"),
            ),
        "search" => args
            .and_then(|args| args.get("query").and_then(Value::as_str))
            .map_or_else(
                || "Searched the web".to_string(),
                |query| format!("Searched the web for {query}"),
            ),
        "scrape" => args
            .and_then(url_arg)
            .map_or_else(|| "Read web page".to_string(), |url| format!("Read {url}")),
        "fetch" => args
            .and_then(url_arg)
            .map_or_else(|| "Fetched URL".to_string(), |url| format!("Fetched {url}")),
        "pdf_convert" | "office_to_markdown" => args.and_then(path_arg).map_or_else(
            || format!("Converted document with {name}"),
            |path| format!("Converted {path}"),
        ),
        _ => args_preview.map_or_else(
            || format!("Called {name}"),
            |args| format!("Called {name} {}", truncate_concise(args)),
        ),
    }
}

fn exploration_return_summary(name: &str, result: &Value, arguments: Option<&Value>) -> String {
    let args_or_result = arguments.or(Some(result));
    match name {
        "view" => result
            .get("file_path")
            .or_else(|| result.get("path"))
            .or_else(|| result.pointer("/metadata/file_path"))
            .and_then(Value::as_str)
            .or_else(|| arguments.and_then(path_arg))
            .map_or_else(|| "Read file".to_string(), |path| format!("Read {path}")),
        "ls" => args_or_result.and_then(path_arg).map_or_else(
            || "Listed files".to_string(),
            |path| format!("Listed {path}"),
        ),
        "glob" => arguments
            .and_then(|args| args.get("pattern").and_then(Value::as_str))
            .map_or_else(
                || "Found files".to_string(),
                |pattern| format!("Found files matching {pattern}"),
            ),
        "grep" => arguments
            .and_then(|args| args.get("pattern").and_then(Value::as_str))
            .map_or_else(
                || "Searched files".to_string(),
                |pattern| format!("Searched {pattern}"),
            ),
        "search" => arguments
            .and_then(|args| args.get("query").and_then(Value::as_str))
            .map_or_else(
                || "Searched the web".to_string(),
                |query| format!("Searched the web for {query}"),
            ),
        "scrape" => arguments
            .and_then(url_arg)
            .map_or_else(|| "Read web page".to_string(), |url| format!("Read {url}")),
        "fetch" => arguments
            .and_then(url_arg)
            .map_or_else(|| "Fetched URL".to_string(), |url| format!("Fetched {url}")),
        "pdf_convert" | "office_to_markdown" => arguments.and_then(path_arg).map_or_else(
            || format!("Converted document with {name}"),
            |path| format!("Converted {path}"),
        ),
        _ => format!("Called {name}"),
    }
}

fn mutation_call_summary(name: &str, args: Option<&Value>, args_preview: Option<&str>) -> String {
    match name {
        "edit" | "multi_edit" => args.and_then(path_arg).map_or_else(
            || format!("Preparing edits with {name}"),
            |path| format!("Editing {path}"),
        ),
        "write" => args.and_then(path_arg).map_or_else(
            || "Preparing file write".to_string(),
            |path| format!("Writing {path}"),
        ),
        _ => args_preview.map_or_else(
            || format!("Preparing {name}"),
            |args| format!("Preparing {name} {}", truncate_concise(args)),
        ),
    }
}

fn mutation_return_summary(name: &str, result: &Value, arguments: Option<&Value>) -> String {
    match name {
        "edit" | "multi_edit" => {
            let path = arguments.and_then(path_arg).or_else(|| path_arg(result));
            let edit_count = arguments.map_or(1, edit_operation_count).max(1);
            path.map_or_else(
                || {
                    format!(
                        "Edited file — {edit_count} edit{}",
                        plural_suffix(edit_count)
                    )
                },
                |path| {
                    format!(
                        "Edited {path} — {edit_count} edit{}",
                        plural_suffix(edit_count)
                    )
                },
            )
        }
        "write" => {
            let path = arguments.and_then(path_arg).or_else(|| path_arg(result));
            let mode = arguments
                .and_then(|args| args.get("mode").and_then(Value::as_str))
                .unwrap_or("w");
            let verb = if mode == "a" { "Appended" } else { "Wrote" };
            path.map_or_else(|| format!("{verb} file"), |path| format!("{verb} {path}"))
        }
        _ => format!("Completed {name}"),
    }
}

fn task_call_summary(name: &str, args_preview: Option<&str>) -> String {
    args_preview.map_or_else(
        || format!("Updating tasks with {name}"),
        |args| format!("Updating tasks with {name} {}", truncate_concise(args)),
    )
}

fn task_return_summary(name: &str, _result: &Value, _arguments: Option<&Value>) -> String {
    match name {
        "task_create" => "Created task".to_string(),
        "task_update" => "Updated task".to_string(),
        "task_get" => "Read task".to_string(),
        "task_list" => "Listed tasks".to_string(),
        _ => format!("Updated tasks with {name}"),
    }
}

fn generic_call_summary(name: &str, args_preview: Option<&str>) -> String {
    args_preview.map_or_else(
        || format!("Calling {name}"),
        |args| format!("Calling {name} {}", truncate_concise(args)),
    )
}

fn generic_return_summary(name: &str, arguments: Option<&Value>) -> String {
    arguments
        .and_then(|args| value_args_preview(args, CONCISE_ARG_MAX_CHARS))
        .map_or_else(
            || format!("Called {name}"),
            |args| format!("Called {name} {args}"),
        )
}

fn shell_detail_lines(value: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    for key in ["stderr", "stdout", "output", "message", "error"] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            lines.extend(
                text.lines()
                    .filter(|line| !line.trim().is_empty())
                    .take(CONCISE_DETAIL_MAX_LINES.saturating_sub(lines.len()))
                    .map(|line| format!("  {}", truncate_concise(line))),
            );
            if lines.len() >= CONCISE_DETAIL_MAX_LINES {
                break;
            }
        }
    }
    if lines.is_empty() {
        concise_detail_lines(value)
    } else {
        lines
    }
}

fn concise_detail_lines(value: &Value) -> Vec<String> {
    let text = value_text(value);
    let mut lines = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(CONCISE_DETAIL_MAX_LINES)
        .map(|line| format!("  {}", truncate_concise(line)))
        .collect::<Vec<_>>();
    if lines.is_empty() && !is_empty_result(value) {
        lines.push(format!("  {}", truncate_concise(&value_text(value))));
    }
    lines
}

fn command_arg(value: &Value) -> Option<&str> {
    value.get("command").and_then(Value::as_str)
}

fn process_arg(value: &Value) -> Option<&str> {
    value
        .get("process_id")
        .or_else(|| value.get("processId"))
        .and_then(Value::as_str)
}

fn path_arg(value: &Value) -> Option<&str> {
    value
        .get("file_path")
        .or_else(|| value.get("path"))
        .or_else(|| value.get("root"))
        .and_then(Value::as_str)
}

fn url_arg(value: &Value) -> Option<&str> {
    value.get("url").and_then(Value::as_str)
}

fn edit_operation_count(value: &Value) -> usize {
    value
        .get("edits")
        .and_then(Value::as_array)
        .map_or(1, Vec::len)
}

fn truncate_concise(text: &str) -> String {
    compact_status_text(text, CONCISE_ARG_MAX_CHARS)
}
