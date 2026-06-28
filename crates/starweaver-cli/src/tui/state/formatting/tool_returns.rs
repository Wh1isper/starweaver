use super::*;

pub(super) fn format_tool_return_display_lines(
    tool_return: &starweaver_model::ToolReturnPart,
    arguments: Option<&Value>,
) -> Vec<String> {
    let display_value = tool_return
        .user_content
        .as_ref()
        .unwrap_or(&tool_return.content);
    let mut lines = if tool_return.is_error {
        vec![format!(
            "Tool error: {} {}",
            tool_return.name,
            full_value_text(display_value)
        )]
    } else {
        match tool_return.name.as_str() {
            "edit" | "multi_edit" => {
                format_edit_tool_lines(&tool_return.name, arguments, display_value)
            }
            "write" => format_write_tool_lines(display_value, arguments),
            "view" => format_view_tool_lines(display_value, arguments),
            "summarize" => format_summarize_tool_lines(display_value, arguments),
            "shell_exec" | "shell_wait" | "shell_status" | "shell_input" | "shell_signal"
            | "shell_kill" => format_shell_tool_lines(&tool_return.name, display_value, arguments),
            "task_create" | "task_get" | "task_update" | "task_list" => {
                format_task_tool_lines(&tool_return.name, &tool_return.content, display_value)
            }
            _ => format_generic_tool_lines(&tool_return.name, display_value),
        }
    };
    if !is_task_tool_name(&tool_return.name) {
        if let Some(duration) = tool_duration_label(&tool_return.metadata) {
            lines.push(format!("  Duration: {duration}"));
        }
    }
    lines
}

fn format_shell_tool_lines(name: &str, result: &Value, arguments: Option<&Value>) -> Vec<String> {
    let mut lines = vec![format!("Tool result: {name}")];
    if let Some(command) = shell_command(result, arguments) {
        lines.push("  Command:".to_string());
        for line in full_command_lines(command) {
            lines.push(format!("    │ {line}"));
        }
    }
    if let Some(cwd) = result
        .get("cwd")
        .or_else(|| arguments.and_then(|args| args.get("cwd")))
        .and_then(Value::as_str)
        .filter(|cwd| !cwd.trim().is_empty())
    {
        lines.push(format!("  Cwd: {cwd}"));
    }
    if let Some(process_id) = result.get("process_id").and_then(Value::as_str) {
        lines.push(format!("  Process: {process_id}"));
    }
    if let Some(status) = shell_status_text(result) {
        lines.push(format!("  Status: {status}"));
    }
    if let Some(stdout) = result.get("stdout").and_then(Value::as_str) {
        push_shell_stream_preview(&mut lines, "stdout", stdout);
    }
    if let Some(stderr) = result.get("stderr").and_then(Value::as_str) {
        push_shell_stream_preview(&mut lines, "stderr", stderr);
    }
    for field in ["stdout_file_path", "stderr_file_path"] {
        if let Some(path) = result.get(field).and_then(Value::as_str) {
            lines.push(format!("  {field}: {path}"));
        }
    }
    if lines.len() == 1 && !is_empty_result(result) {
        push_indented_preview(&mut lines, &value_text(result), 12);
    }
    lines
}

pub(super) fn shell_command<'a>(
    result: &'a Value,
    arguments: Option<&'a Value>,
) -> Option<&'a str> {
    result
        .get("command")
        .or_else(|| arguments.and_then(|args| args.get("command")))
        .and_then(Value::as_str)
        .filter(|command| !command.trim().is_empty())
}

fn shell_status_text(result: &Value) -> Option<String> {
    result
        .get("return_code")
        .and_then(Value::as_i64)
        .map(|code| format!("exit {code}"))
        .or_else(|| {
            result
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn push_shell_stream_preview(lines: &mut Vec<String>, label: &str, output: &str) {
    if output.trim().is_empty() {
        return;
    }
    lines.push(format!("  {label}:"));
    for line in preview_lines(output, SHELL_STREAM_PREVIEW_MAX_LINES) {
        lines.push(format!("    │ {line}"));
    }
}

fn full_command_lines(command: &str) -> Vec<String> {
    let mut lines = command.lines().collect::<Vec<_>>();
    if command.ends_with('\n') || lines.is_empty() {
        lines.push("");
    }
    lines
        .into_iter()
        .flat_map(|line| split_sanitized_line(line, TOOL_PREVIEW_MAX_CHARS))
        .collect()
}

fn split_sanitized_line(line: &str, max_chars: usize) -> Vec<String> {
    let line = sanitize_control_chars(line);
    let max_chars = max_chars.max(1);
    if line.is_empty() {
        return vec![String::new()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    for ch in line.chars() {
        if current.chars().count() >= max_chars {
            chunks.push(std::mem::take(&mut current));
        }
        current.push(ch);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn format_generic_tool_lines(name: &str, result: &Value) -> Vec<String> {
    let mut lines = vec![format!("Tool result: {name}")];
    if is_empty_result(result) {
        return lines;
    }
    push_indented_preview(&mut lines, &value_text(result), 12);
    lines
}

fn format_edit_tool_lines(name: &str, arguments: Option<&Value>, result: &Value) -> Vec<String> {
    let mut lines = vec![format!("Tool result: {name}")];
    let Some(args) = arguments else {
        if let Some(file_path) = result_path(result) {
            lines.push(format!("  Editing file: {file_path}"));
        }
        if let Some(status) = edit_result_status(result) {
            lines.push(format!("  Status: {status}"));
        }
        if !is_empty_result(result) {
            lines.push(format!("  Result: {}", value_preview(result)));
        }
        return lines;
    };
    let file_path = file_path_arg(args).unwrap_or("unknown");
    lines.push(format!("  Editing file: {file_path}"));
    let edits = edit_operations(args);
    if edits.is_empty() {
        let old_string = string_field(args, "old_string");
        let new_string = string_field(args, "new_string");
        let is_new_file = old_string.is_empty();
        lines.extend(format_one_edit(
            1,
            old_string,
            new_string,
            false,
            is_new_file,
        ));
    } else {
        let new_files = edits
            .iter()
            .enumerate()
            .filter(|(index, edit)| *index == 0 && edit.old_string.is_empty())
            .count();
        let modifications = edits.len().saturating_sub(new_files);
        let replace_all = edits.iter().filter(|edit| edit.replace_all).count();
        lines.push(format!(
            "  Summary: {} edit{} ({} new file{}, {} modification{}, {} replace-all operation{})",
            edits.len(),
            plural_suffix(edits.len()),
            new_files,
            plural_suffix(new_files),
            modifications,
            plural_suffix(modifications),
            replace_all,
            plural_suffix(replace_all)
        ));
        for (index, edit) in edits.iter().enumerate() {
            lines.extend(format_one_edit(
                index + 1,
                &edit.old_string,
                &edit.new_string,
                edit.replace_all,
                index == 0 && edit.old_string.is_empty(),
            ));
        }
    }
    if !is_empty_result(result) {
        lines.push(format!("  Result: {}", full_value_text(result)));
    }
    lines
}

fn format_write_tool_lines(result: &Value, arguments: Option<&Value>) -> Vec<String> {
    let mut lines = vec!["Tool result: write".to_string()];
    let path = result_path(result)
        .or_else(|| arguments.and_then(file_path_arg))
        .unwrap_or("unknown");
    lines.push(format!("  Writing file: {path}"));

    if let Some(args) = arguments {
        let mode = args
            .get("mode")
            .and_then(Value::as_str)
            .map_or("overwrite", write_mode_label);
        lines.push(format!("  Mode: {mode}"));
        let content = string_field(args, "content");
        if content.is_empty() {
            lines.push("  Edit #1: Empty file write".to_string());
            lines.push("    Empty file".to_string());
        } else {
            let operation = if args.get("mode").and_then(Value::as_str) == Some("a") {
                "Append content"
            } else {
                "File content"
            };
            lines.push(format!("  Edit #1: {operation}"));
            for line in preview_lines(content, 20) {
                lines.push(format!("    +{line}"));
            }
        }
    }

    if result.get("written").and_then(Value::as_bool) == Some(true) {
        lines.push("  Status: written".to_string());
    } else if !is_empty_result(result) {
        lines.push(format!("  Result: {}", value_preview(result)));
    }
    lines
}

fn write_mode_label(mode: &str) -> &'static str {
    match mode {
        "a" => "append",
        _ => "overwrite",
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EditOperation {
    old_string: String,
    new_string: String,
    replace_all: bool,
}

fn edit_operations(args: &Value) -> Vec<EditOperation> {
    args.get("edits")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.as_object().map(|object| EditOperation {
                        old_string: object
                            .get("old_string")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        new_string: object
                            .get("new_string")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        replace_all: object
                            .get("replace_all")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn format_one_edit(
    index: usize,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
    is_new_file: bool,
) -> Vec<String> {
    let operation_type = if is_new_file {
        "New file creation"
    } else {
        "Content modification"
    };
    let replace_suffix = if replace_all { " (replace all)" } else { "" };
    let mut lines = vec![format!("  Edit #{index}: {operation_type}{replace_suffix}")];
    if old_string.is_empty() {
        if is_new_file {
            if new_string.is_empty() {
                lines.push("    Empty file".to_string());
            } else {
                for line in preview_lines(new_string, 15) {
                    lines.push(format!("    +{line}"));
                }
            }
        } else {
            lines.push("    Empty match string".to_string());
            for line in preview_lines(new_string, 15) {
                lines.push(format!("    +{line}"));
            }
        }
    } else {
        lines.extend(unified_diff_lines(old_string, new_string, 18));
    }
    lines
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiffLine<'a> {
    Text(&'a str),
    EofNewline,
}

fn unified_diff_lines(old_string: &str, new_string: &str, max_lines: usize) -> Vec<String> {
    if old_string == new_string {
        return vec!["    No changes detected".to_string()];
    }
    let old_lines = split_diff_lines(old_string);
    let new_lines = split_diff_lines(new_string);
    let old_len = old_lines.len();
    let new_len = new_lines.len();
    let mut prefix = 0usize;
    while prefix < old_len && prefix < new_len && old_lines[prefix] == new_lines[prefix] {
        prefix += 1;
    }

    let mut suffix = 0usize;
    while suffix < old_len.saturating_sub(prefix)
        && suffix < new_len.saturating_sub(prefix)
        && old_lines[old_len - suffix - 1] == new_lines[new_len - suffix - 1]
    {
        suffix += 1;
    }

    let old_change_end = old_len.saturating_sub(suffix);
    let new_change_end = new_len.saturating_sub(suffix);
    let context = 2usize;
    let old_context_start = prefix.saturating_sub(context);
    let new_context_start = prefix.saturating_sub(context);
    let old_after_end = (old_change_end + context).min(old_len);
    let new_after_end = (new_change_end + context).min(new_len);
    let old_start = old_context_start + 1;
    let new_start = new_context_start + 1;
    let old_span = old_after_end.saturating_sub(old_context_start);
    let new_span = new_after_end.saturating_sub(new_context_start);

    let mut lines = vec![format!(
        "    @@ -{old_start},{old_span} +{new_start},{new_span} @@"
    )];
    let mut truncated = false;
    if old_context_start > 0 || new_context_start > 0 {
        let omitted = old_context_start.max(new_context_start);
        push_diff_preview_line(
            &mut lines,
            &mut truncated,
            max_lines,
            format!("     ... ({omitted} unchanged lines before)"),
        );
    }
    for line in &old_lines[old_context_start..prefix] {
        push_diff_preview_line(
            &mut lines,
            &mut truncated,
            max_lines,
            format!("     {}", preview_diff_line(*line)),
        );
    }
    for line in &old_lines[prefix..old_change_end] {
        push_diff_preview_line(
            &mut lines,
            &mut truncated,
            max_lines,
            format!("    -{}", preview_diff_line(*line)),
        );
    }
    for line in &new_lines[prefix..new_change_end] {
        push_diff_preview_line(
            &mut lines,
            &mut truncated,
            max_lines,
            format!("    +{}", preview_diff_line(*line)),
        );
    }
    for line in &old_lines[old_change_end..old_after_end] {
        push_diff_preview_line(
            &mut lines,
            &mut truncated,
            max_lines,
            format!("     {}", preview_diff_line(*line)),
        );
    }
    if old_after_end < old_len || new_after_end < new_len {
        let omitted = old_len
            .saturating_sub(old_after_end)
            .max(new_len.saturating_sub(new_after_end));
        push_diff_preview_line(
            &mut lines,
            &mut truncated,
            max_lines,
            format!("     ... ({omitted} unchanged lines after)"),
        );
    }
    if truncated {
        if lines.len() >= max_lines.max(2) {
            lines.pop();
        }
        lines.push("    ... (diff truncated)".to_string());
    }
    lines
}

fn split_diff_lines(content: &str) -> Vec<DiffLine<'_>> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut parts = content.split('\n').collect::<Vec<_>>();
    let has_eof_newline = content.ends_with('\n');
    if has_eof_newline {
        parts.pop();
    }
    let mut lines = parts.into_iter().map(DiffLine::Text).collect::<Vec<_>>();
    if has_eof_newline {
        lines.push(DiffLine::EofNewline);
    }
    lines
}

fn preview_diff_line(line: DiffLine<'_>) -> String {
    match line {
        DiffLine::Text("") => "<blank line>".to_string(),
        DiffLine::Text(line) => preview_line(line),
        DiffLine::EofNewline => "<EOF newline>".to_string(),
    }
}

fn push_diff_preview_line(
    lines: &mut Vec<String>,
    truncated: &mut bool,
    max_lines: usize,
    line: String,
) {
    if lines.len() < max_lines.max(2) {
        lines.push(line);
    } else {
        *truncated = true;
    }
}

fn format_view_tool_lines(result: &Value, arguments: Option<&Value>) -> Vec<String> {
    let mut lines = vec!["Tool result: view".to_string()];
    let path = result
        .get("file_path")
        .or_else(|| result.get("path"))
        .or_else(|| result.pointer("/metadata/file_path"))
        .or_else(|| arguments.and_then(|args| args.get("file_path")))
        .or_else(|| arguments.and_then(|args| args.get("path")))
        .and_then(Value::as_str);
    if let Some(path) = path {
        lines.push(format!("  Viewing file: {path}"));
    }

    let start_line = result
        .pointer("/metadata/current_segment/start_line")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok());
    let end_line = result
        .pointer("/metadata/current_segment/end_line")
        .and_then(Value::as_u64);
    let total_lines = result
        .pointer("/metadata/total_lines")
        .and_then(Value::as_u64);
    if let (Some(start), Some(end), Some(total)) = (start_line, end_line, total_lines) {
        lines.push(format!("  Lines: {start}-{end} of {total}"));
    }

    if let Some(content) = result
        .get("content")
        .and_then(Value::as_str)
        .or_else(|| result.as_str())
    {
        lines.extend(format_view_content_lines(content, start_line));
    } else if let Some(message) = result
        .get("message")
        .or_else(|| result.get("error"))
        .and_then(Value::as_str)
    {
        lines.push(format!("  {message}"));
    } else {
        lines.push(format!("  {}", value_preview(result)));
    }
    if let Some(metadata) = result.get("metadata") {
        if let Some(truncation) = metadata.get("truncation_info") {
            lines.push(format!("  Truncation: {}", value_preview(truncation)));
        }
    }
    lines
}

fn format_view_content_lines(content: &str, start_line: Option<usize>) -> Vec<String> {
    if content.is_empty() {
        return vec!["    Empty file".to_string()];
    }
    let preview = preview_lines(content, 20);
    let line_number_width = start_line.map_or(0, |start| {
        start.saturating_add(preview.len()).to_string().len().max(4)
    });
    preview
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            if line.starts_with("... (") {
                return format!("    {line}");
            }
            start_line.map_or_else(
                || format!("    │ {line}"),
                |start| {
                    format!(
                        "    {:>line_number_width$} │ {line}",
                        start.saturating_add(index)
                    )
                },
            )
        })
        .collect()
}

fn format_summarize_tool_lines(result: &Value, arguments: Option<&Value>) -> Vec<String> {
    let payload = result.get("payload").unwrap_or(result);
    let content = payload_string(
        payload,
        &["content", "handoff_content", "summary_markdown", "summary"],
    )
    .or_else(|| arguments.and_then(|args| payload_string(args, &["content", "summary"])))
    .unwrap_or_default();
    let auto_load_files = payload_string_array(payload, "auto_load_files")
        .or_else(|| arguments.and_then(|args| payload_string_array(args, "auto_load_files")))
        .unwrap_or_default();

    let mut lines = vec!["Tool result: summarize".to_string()];
    lines.push("  Summary: Progress summarized, continuing with fresh context".to_string());
    if !content.trim().is_empty() {
        for line in full_content_lines(&content) {
            lines.push(format!("    │ {line}"));
        }
    }
    if !auto_load_files.is_empty() {
        lines.push(format!(
            "  Auto-load files: {}",
            auto_load_files
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    lines
}

#[allow(clippy::cast_precision_loss)]
fn tool_duration_label(metadata: &serde_json::Map<String, Value>) -> Option<String> {
    let millis = metadata.get("duration_ms").and_then(Value::as_u64)?;
    if millis < 1_000 {
        Some(format!("{millis}ms"))
    } else {
        Some(format!("{:.2}s", millis as f64 / 1_000.0))
    }
}

fn result_path(value: &Value) -> Option<&str> {
    file_path_arg(value)
}

fn file_path_arg(value: &Value) -> Option<&str> {
    value
        .get("file_path")
        .or_else(|| value.get("path"))
        .and_then(Value::as_str)
}

fn edit_result_status(value: &Value) -> Option<&'static str> {
    if value.get("created").and_then(Value::as_bool) == Some(true) {
        Some("created")
    } else if value.get("edited").and_then(Value::as_bool) == Some(true) {
        Some("edited")
    } else {
        None
    }
}
