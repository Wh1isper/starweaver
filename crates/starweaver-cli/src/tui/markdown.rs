use pulldown_cmark::{CodeBlockKind, Event as MarkdownEvent, Options, Parser, Tag, TagEnd};
use unicode_width::UnicodeWidthChar;

use super::render::{truncate_line_center, visible_width, SegmentStyle, StyledLine};

pub(super) const ASSISTANT_CONTENT_PREFIX: &str = "\u{200b}";

pub(super) fn render_transcript_lines(lines: &[String], width: usize) -> Vec<StyledLine> {
    let mut rendered = Vec::new();
    let mut assistant_markdown = Vec::new();
    let mut in_assistant = false;
    for line in lines {
        if let Some(content) = line.strip_prefix(ASSISTANT_CONTENT_PREFIX) {
            if in_assistant {
                assistant_markdown.push(content.to_string());
            } else {
                rendered.extend(render_markdown_lines(
                    &[content.to_string()],
                    width.saturating_sub(2).max(1),
                ));
            }
            continue;
        }
        if line == "Assistant:" {
            flush_assistant_markdown(&mut rendered, &mut assistant_markdown, width);
            in_assistant = true;
            continue;
        }
        if is_transcript_boundary(line) {
            flush_assistant_markdown(&mut rendered, &mut assistant_markdown, width);
            in_assistant = false;
            rendered.extend(render_transcript_status_lines(line, width));
            continue;
        }
        if in_assistant {
            assistant_markdown.push(line.clone());
        } else {
            rendered.extend(render_transcript_status_lines(line, width));
        }
    }
    flush_assistant_markdown(&mut rendered, &mut assistant_markdown, width);
    rendered
}

fn flush_assistant_markdown(
    rendered: &mut Vec<StyledLine>,
    markdown: &mut Vec<String>,
    width: usize,
) {
    if markdown.is_empty() {
        return;
    }
    rendered.extend(render_markdown_lines(
        markdown,
        width.saturating_sub(2).max(1),
    ));
    markdown.clear();
}

fn is_transcript_boundary(line: &str) -> bool {
    line.is_empty()
        || line.starts_with("User:")
        || line.starts_with("Tool call:")
        || line.starts_with("Tool result:")
        || line.starts_with("Tool error:")
        || line.starts_with("Thinking:")
        || line.starts_with("Error:")
        || line.starts_with("Suspended:")
        || line.starts_with("Output retry:")
        || line.starts_with("Run completed:")
}

fn render_transcript_status_lines(line: &str, width: usize) -> Vec<StyledLine> {
    if let Some(prompt) = line.strip_prefix("User:") {
        let mut rendered = StyledLine::styled("› ", SegmentStyle::bold());
        rendered.push(prompt.trim_start(), SegmentStyle::bold());
        return vec![rendered];
    }
    if let Some(tool) = line.strip_prefix("Tool call:") {
        return render_tool_lines(tool.trim_start(), ToolLineKind::Call, width);
    }
    if let Some(tool) = line.strip_prefix("Tool result:") {
        return render_tool_lines(tool.trim_start(), ToolLineKind::Result, width);
    }
    if let Some(tool) = line.strip_prefix("Tool error:") {
        return render_tool_lines(tool.trim_start(), ToolLineKind::Error, width);
    }
    if let Some(thinking) = line.strip_prefix("Thinking:") {
        let mut rendered = StyledLine::styled("  ◌ thinking", SegmentStyle::warning());
        let detail = thinking.trim_start();
        if !detail.is_empty() {
            rendered.push(" ", SegmentStyle::dim());
            rendered.push(detail, SegmentStyle::dim());
        }
        return vec![rendered];
    }
    if let Some(error) = line.strip_prefix("Error:") {
        let mut rendered = StyledLine::styled(
            "  ✕ error",
            SegmentStyle::error().merge(SegmentStyle::bold()),
        );
        let detail = error.trim_start();
        if !detail.is_empty() {
            rendered.push(" ", SegmentStyle::dim());
            rendered.push(detail, SegmentStyle::dim());
        }
        return vec![rendered];
    }
    if let Some(status) = line.strip_prefix("Suspended:") {
        let mut rendered = StyledLine::styled(
            "  ◷ waiting",
            SegmentStyle::warning().merge(SegmentStyle::bold()),
        );
        let detail = status.trim_start();
        if !detail.is_empty() {
            rendered.push(" ", SegmentStyle::dim());
            rendered.push(detail, SegmentStyle::dim());
        }
        return vec![rendered];
    }
    if let Some(retry) = line.strip_prefix("Output retry:") {
        let mut rendered = StyledLine::styled("  ↻ retry", SegmentStyle::warning());
        let detail = retry.trim_start();
        if !detail.is_empty() {
            rendered.push(" ", SegmentStyle::dim());
            rendered.push(detail, SegmentStyle::dim());
        }
        return vec![rendered];
    }
    if let Some(status) = line.strip_prefix("Run completed:") {
        let mut rendered = StyledLine::styled("  ✓ completed", SegmentStyle::blockquote());
        rendered.push(" ", SegmentStyle::dim());
        rendered.push(status.trim_start(), SegmentStyle::dim());
        return vec![rendered];
    }
    if let Some(rendered) = render_file_tool_detail_line(line) {
        return vec![rendered];
    }
    vec![StyledLine::plain(line)]
}

fn render_file_tool_detail_line(line: &str) -> Option<StyledLine> {
    render_file_header_line(line)
        .or_else(|| render_file_metadata_line(line))
        .or_else(|| render_file_content_line(line))
}

fn render_file_header_line(line: &str) -> Option<StyledLine> {
    for prefix in ["  Editing file: ", "  Writing file: ", "  Viewing file: "] {
        if let Some(path) = line.strip_prefix(prefix) {
            let mut rendered = StyledLine::styled(prefix, SegmentStyle::dim());
            rendered.push(path, SegmentStyle::code().merge(SegmentStyle::bold()));
            return Some(rendered);
        }
    }
    None
}

fn render_file_metadata_line(line: &str) -> Option<StyledLine> {
    for (prefix, value_style) in [
        ("  Summary: ", SegmentStyle::blockquote()),
        ("  Status: ", SegmentStyle::blockquote()),
        ("  Result: ", SegmentStyle::dim()),
        ("  Duration: ", SegmentStyle::dim()),
        ("  Mode: ", SegmentStyle::warning()),
        ("  Lines: ", SegmentStyle::list_marker()),
        ("  Truncation: ", SegmentStyle::warning()),
    ] {
        if let Some(value) = line.strip_prefix(prefix) {
            let mut rendered = StyledLine::styled(prefix, SegmentStyle::dim());
            rendered.push(value, value_style);
            return Some(rendered);
        }
    }
    if let Some(detail) = line.strip_prefix("  Edit #") {
        let mut rendered = StyledLine::styled("  Edit #", SegmentStyle::list_marker());
        rendered.push(
            detail,
            SegmentStyle::list_marker().merge(SegmentStyle::bold()),
        );
        return Some(rendered);
    }
    if matches!(
        line,
        "    Empty file" | "    Empty match string" | "    No changes detected"
    ) || line.starts_with("    ...")
    {
        return Some(StyledLine::styled(line, SegmentStyle::dim()));
    }
    None
}

fn render_file_content_line(line: &str) -> Option<StyledLine> {
    if line.starts_with("    @@") {
        return Some(StyledLine::styled(
            line,
            SegmentStyle::warning().merge(SegmentStyle::bold()),
        ));
    }
    if let Some(content) = line.strip_prefix("    +") {
        let mut rendered = StyledLine::styled(
            "    +",
            SegmentStyle::blockquote().merge(SegmentStyle::bold()),
        );
        rendered.push(content, SegmentStyle::blockquote());
        return Some(rendered);
    }
    if let Some(content) = line.strip_prefix("    -") {
        let mut rendered =
            StyledLine::styled("    -", SegmentStyle::error().merge(SegmentStyle::bold()));
        rendered.push(content, SegmentStyle::error());
        return Some(rendered);
    }
    if let Some(rest) = line.strip_prefix("    ") {
        if let Some((line_number, content)) = rest.split_once(" │ ") {
            if line_number.trim().chars().all(|ch| ch.is_ascii_digit()) {
                let mut rendered = StyledLine::styled("    ", SegmentStyle::dim());
                rendered.push(line_number, SegmentStyle::list_marker());
                rendered.push(" │ ", SegmentStyle::dim());
                rendered.push(content, SegmentStyle::code_block());
                return Some(rendered);
            }
        }
        if let Some(content) = rest.strip_prefix("│ ") {
            let mut rendered = StyledLine::styled("    │ ", SegmentStyle::dim());
            rendered.push(content, SegmentStyle::code_block());
            return Some(rendered);
        }
    }
    if let Some(content) = line.strip_prefix("     ") {
        let mut rendered = StyledLine::styled("     ", SegmentStyle::dim());
        rendered.push(content, SegmentStyle::dim());
        return Some(rendered);
    }
    None
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToolLineKind {
    Call,
    Result,
    Error,
}

fn render_tool_lines(detail: &str, kind: ToolLineKind, width: usize) -> Vec<StyledLine> {
    let (prefix, name_style, label_style) = match kind {
        ToolLineKind::Call => (
            "Calling: ",
            SegmentStyle::code().merge(SegmentStyle::bold()),
            SegmentStyle::dim(),
        ),
        ToolLineKind::Result => (
            "Complete: ",
            SegmentStyle::blockquote().merge(SegmentStyle::bold()),
            SegmentStyle::dim(),
        ),
        ToolLineKind::Error => (
            "x Error: ",
            SegmentStyle::error().merge(SegmentStyle::bold()),
            SegmentStyle::dim(),
        ),
    };
    let (name, rest) = split_tool_detail(detail);
    let mut rendered = StyledLine::plain("  ");
    rendered.push(prefix, label_style);
    rendered.push(name, name_style);
    if rest.is_empty() {
        return vec![rendered];
    }

    let result_label = if matches!(kind, ToolLineKind::Error) {
        " | Error: "
    } else if matches!(kind, ToolLineKind::Result) {
        " | Output: "
    } else {
        " | Args: "
    };
    let label_style = label_style.merge(if matches!(kind, ToolLineKind::Error) {
        SegmentStyle::error()
    } else {
        SegmentStyle::warning()
    });
    rendered.push(result_label, label_style);

    let available = tool_detail_available_width(width, prefix, name, result_label);
    if matches!(kind, ToolLineKind::Error) {
        let mut chunks = wrap_status_text(rest, available.max(1));
        if let Some(first) = chunks.first() {
            rendered.push(first, SegmentStyle::dim());
        }
        let mut lines = vec![rendered];
        for chunk in chunks.drain(1..) {
            let mut continuation = StyledLine::plain("    ");
            continuation.push(chunk, SegmentStyle::dim());
            lines.push(continuation);
        }
        lines
    } else {
        rendered.push(
            truncate_line_center(rest, available.max(1)),
            SegmentStyle::dim(),
        );
        vec![rendered]
    }
}

fn tool_detail_available_width(
    width: usize,
    prefix: &str,
    name: &str,
    result_label: &str,
) -> usize {
    let fixed_width = visible_width("  ")
        .saturating_add(visible_width(prefix))
        .saturating_add(visible_width(name))
        .saturating_add(visible_width(result_label));
    width.max(40).saturating_sub(fixed_width).max(1)
}

fn wrap_status_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let compact = text.replace('\n', " ");
    if compact.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut used = 0usize;
    for ch in compact.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used > 0 && used.saturating_add(ch_width) > width {
            lines.push(current);
            current = String::new();
            used = 0;
        }
        current.push(ch);
        used = used.saturating_add(ch_width);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn split_tool_detail(detail: &str) -> (&str, &str) {
    let detail = detail.trim();
    detail
        .split_once(' ')
        .map_or((detail, ""), |(name, rest)| (name, rest.trim()))
}

pub(super) fn render_markdown_lines(lines: &[String], width: usize) -> Vec<StyledLine> {
    let source = lines.join("\n");
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(&source, options);
    let mut renderer = MarkdownRenderer::new(width);
    renderer.run(parser);
    renderer.finish()
}

struct MarkdownRenderer {
    lines: Vec<StyledLine>,
    current: StyledLine,
    style_stack: Vec<SegmentStyle>,
    indent_stack: Vec<IndentContext>,
    list_stack: Vec<Option<u64>>,
    link_destination: Option<String>,
    in_code_block: bool,
    code_language: Option<String>,
    code_buffer: String,
    paragraph_open: bool,
    width: usize,
}

impl MarkdownRenderer {
    fn new(width: usize) -> Self {
        Self {
            lines: Vec::new(),
            current: StyledLine {
                segments: Vec::new(),
            },
            style_stack: Vec::new(),
            indent_stack: Vec::new(),
            list_stack: Vec::new(),
            link_destination: None,
            in_code_block: false,
            code_language: None,
            code_buffer: String::new(),
            paragraph_open: false,
            width: width.max(1),
        }
    }

    fn run(&mut self, parser: Parser<'_>) {
        for event in parser {
            self.handle_event(event);
        }
    }

    fn finish(mut self) -> Vec<StyledLine> {
        self.flush_current();
        if self.lines.is_empty() {
            self.lines.push(StyledLine::plain(""));
        }
        self.lines
    }

    fn handle_event(&mut self, event: MarkdownEvent<'_>) {
        match event {
            MarkdownEvent::Start(tag) => self.start_tag(tag),
            MarkdownEvent::End(tag) => self.end_tag(tag),
            MarkdownEvent::Text(text) => self.push_wrapped_text(&text),
            MarkdownEvent::Code(code) => self.push_text(&code, SegmentStyle::code()),
            MarkdownEvent::SoftBreak => self.push_wrapped_text(" "),
            MarkdownEvent::HardBreak => self.flush_current(),
            MarkdownEvent::Rule => self.push_rule(),
            MarkdownEvent::Html(html) | MarkdownEvent::InlineHtml(html) => {
                self.push_wrapped_text(&html);
            }
            MarkdownEvent::FootnoteReference(_) | MarkdownEvent::TaskListMarker(_) => {}
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Heading { level, .. } => {
                self.start_paragraph();
                self.style_stack.push(heading_style(level));
            }
            Tag::Paragraph | Tag::Table(_) => self.start_paragraph(),
            Tag::BlockQuote => self.indent_stack.push(IndentContext::blockquote()),
            Tag::CodeBlock(kind) => self.start_code_block(kind),
            Tag::List(start) => self.list_stack.push(start),
            Tag::Item => self.start_list_item(),
            Tag::Emphasis => self.style_stack.push(SegmentStyle::italic()),
            Tag::Strong => self.style_stack.push(SegmentStyle::bold()),
            Tag::Strikethrough => self.style_stack.push(SegmentStyle::dim()),
            Tag::Link { dest_url, .. } => {
                self.link_destination = Some(dest_url.to_string());
                self.style_stack.push(SegmentStyle::link());
            }
            Tag::TableHead
            | Tag::TableRow
            | Tag::TableCell
            | Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::Image { .. }
            | Tag::MetadataBlock(_) => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                self.style_stack.pop();
                self.end_paragraph();
            }
            TagEnd::Paragraph | TagEnd::Table => self.end_paragraph(),
            TagEnd::BlockQuote | TagEnd::Item => {
                self.flush_current();
                self.indent_stack.pop();
            }
            TagEnd::CodeBlock => self.end_code_block(),
            TagEnd::List(_) => {
                self.flush_current();
                self.list_stack.pop();
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.style_stack.pop();
            }
            TagEnd::Link => {
                self.style_stack.pop();
                if let Some(destination) = self.link_destination.take() {
                    if !destination.is_empty() {
                        self.push_text(&format!(" <{destination}>"), SegmentStyle::dim());
                    }
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => self.flush_current(),
            TagEnd::TableCell => self.push_wrapped_text("  "),
            TagEnd::HtmlBlock
            | TagEnd::FootnoteDefinition
            | TagEnd::Image
            | TagEnd::MetadataBlock(_) => {}
        }
    }

    fn start_paragraph(&mut self) {
        if self.paragraph_open {
            return;
        }
        if !self.current.segments.is_empty() {
            self.flush_current();
        }
        self.paragraph_open = true;
        self.apply_pending_prefix();
    }

    fn end_paragraph(&mut self) {
        self.flush_current();
        self.paragraph_open = false;
    }

    fn start_code_block(&mut self, kind: CodeBlockKind<'_>) {
        self.flush_current();
        self.in_code_block = true;
        self.code_language = match kind {
            CodeBlockKind::Fenced(language) if !language.is_empty() => Some(language.to_string()),
            CodeBlockKind::Fenced(_) | CodeBlockKind::Indented => None,
        };
        self.code_buffer.clear();
        if let Some(language) = self.code_language.as_deref() {
            self.lines.push(StyledLine::styled(
                format!("╭─ {language}"),
                SegmentStyle::dim(),
            ));
        } else {
            self.lines
                .push(StyledLine::styled("╭─ code", SegmentStyle::dim()));
        }
    }

    fn end_code_block(&mut self) {
        for line in self.code_buffer.trim_end_matches('\n').lines() {
            self.lines.push(StyledLine::styled(
                format!("│ {line}"),
                SegmentStyle::code_block(),
            ));
        }
        self.lines
            .push(StyledLine::styled("╰────", SegmentStyle::dim()));
        self.in_code_block = false;
        self.code_language = None;
        self.code_buffer.clear();
    }

    fn start_list_item(&mut self) {
        self.flush_current();
        let marker = match self.list_stack.last_mut() {
            Some(Some(index)) => {
                let marker = format!("{index}. ");
                *index += 1;
                marker
            }
            _ => "• ".to_string(),
        };
        self.indent_stack.push(IndentContext::list_item(marker));
    }

    fn push_rule(&mut self) {
        self.flush_current();
        self.lines.push(StyledLine::styled(
            "─".repeat(self.width),
            SegmentStyle::dim(),
        ));
    }

    fn push_wrapped_text(&mut self, text: &str) {
        if self.in_code_block {
            self.code_buffer.push_str(text);
            return;
        }
        let style = self.current_style();
        for word in split_preserving_whitespace(text) {
            if word == "\n" {
                self.flush_current();
                continue;
            }
            let word_width = visible_width(word);
            if self.current_width() + word_width > self.width {
                self.flush_current();
                if word.trim().is_empty() {
                    continue;
                }
                self.apply_continuation_prefix();
            }
            self.push_text(word, style);
        }
    }

    fn push_text(&mut self, text: &str, style: SegmentStyle) {
        if self.in_code_block {
            self.code_buffer.push_str(text);
            return;
        }
        if text.is_empty() {
            return;
        }
        self.apply_pending_prefix();
        self.current.push(text, style);
    }

    fn flush_current(&mut self) {
        if self.current.segments.is_empty() {
            return;
        }
        self.lines.push(std::mem::replace(
            &mut self.current,
            StyledLine {
                segments: Vec::new(),
            },
        ));
    }

    fn apply_pending_prefix(&mut self) {
        if !self.current.segments.is_empty() {
            return;
        }
        for context in &mut self.indent_stack {
            if let Some(marker) = context.marker.take() {
                self.current.push(marker, context.marker_style);
            } else {
                self.current
                    .push(context.continuation.clone(), context.marker_style);
            }
        }
    }

    fn apply_continuation_prefix(&mut self) {
        for context in &self.indent_stack {
            self.current
                .push(context.continuation.clone(), context.marker_style);
        }
    }

    fn current_style(&self) -> SegmentStyle {
        self.style_stack
            .iter()
            .copied()
            .fold(SegmentStyle::default(), SegmentStyle::merge)
    }

    fn current_width(&self) -> usize {
        self.current
            .segments
            .iter()
            .map(|segment| visible_width(&segment.text))
            .sum()
    }
}

#[derive(Clone, Debug)]
struct IndentContext {
    marker: Option<String>,
    continuation: String,
    marker_style: SegmentStyle,
}

impl IndentContext {
    fn blockquote() -> Self {
        Self {
            marker: Some("│ ".to_string()),
            continuation: "│ ".to_string(),
            marker_style: SegmentStyle::blockquote(),
        }
    }

    fn list_item(marker: String) -> Self {
        let continuation = " ".repeat(visible_width(&marker));
        Self {
            marker: Some(marker),
            continuation,
            marker_style: SegmentStyle::list_marker(),
        }
    }
}

const fn heading_style(level: pulldown_cmark::HeadingLevel) -> SegmentStyle {
    match level {
        pulldown_cmark::HeadingLevel::H1 => SegmentStyle::bold().merge(SegmentStyle::underlined()),
        pulldown_cmark::HeadingLevel::H2 => SegmentStyle::bold(),
        pulldown_cmark::HeadingLevel::H3 => SegmentStyle::bold().merge(SegmentStyle::italic()),
        pulldown_cmark::HeadingLevel::H4
        | pulldown_cmark::HeadingLevel::H5
        | pulldown_cmark::HeadingLevel::H6 => SegmentStyle::italic(),
    }
}

fn split_preserving_whitespace(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_whitespace = None;
    for (index, ch) in text.char_indices() {
        if ch == '\n' {
            if start < index {
                parts.push(&text[start..index]);
            }
            parts.push("\n");
            start = index + ch.len_utf8();
            in_whitespace = None;
            continue;
        }
        let whitespace = ch.is_whitespace();
        match in_whitespace {
            Some(current) if current == whitespace => {}
            Some(_) => {
                if start < index {
                    parts.push(&text[start..index]);
                }
                start = index;
                in_whitespace = Some(whitespace);
            }
            None => in_whitespace = Some(whitespace),
        }
    }
    if start < text.len() {
        parts.push(&text[start..]);
    }
    parts
}
