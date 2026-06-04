use pulldown_cmark::{CodeBlockKind, Event as MarkdownEvent, Options, Parser, Tag, TagEnd};

use super::render::{visible_width, SegmentStyle, StyledLine};

pub(super) fn render_transcript_lines(lines: &[String], width: usize) -> Vec<StyledLine> {
    let mut rendered = Vec::new();
    let mut assistant_markdown = Vec::new();
    let mut in_assistant = false;
    for line in lines {
        if line == "Assistant:" {
            flush_assistant_markdown(&mut rendered, &mut assistant_markdown, width);
            rendered.push(StyledLine::styled("Assistant", SegmentStyle::bold()));
            in_assistant = true;
            continue;
        }
        if is_transcript_boundary(line) {
            flush_assistant_markdown(&mut rendered, &mut assistant_markdown, width);
            in_assistant = false;
            rendered.push(render_transcript_status_line(line));
            continue;
        }
        if in_assistant {
            assistant_markdown.push(line.clone());
        } else {
            rendered.push(render_transcript_status_line(line));
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
        || line.starts_with("Thinking:")
        || line.starts_with("Error:")
        || line.starts_with("Suspended:")
        || line.starts_with("Output retry:")
        || line.starts_with("Run completed:")
}

fn render_transcript_status_line(line: &str) -> StyledLine {
    if line.starts_with("User:") {
        return StyledLine::styled(
            line,
            SegmentStyle::bold().merge(SegmentStyle::list_marker()),
        );
    }
    if line.starts_with("Tool call:") || line.starts_with("Tool result:") {
        return StyledLine::styled(line, SegmentStyle::dim().merge(SegmentStyle::code()));
    }
    if line.starts_with("Error:") {
        return StyledLine::styled(line, SegmentStyle::bold().merge(SegmentStyle::dim()));
    }
    if line.starts_with("Run completed:") {
        return StyledLine::styled(line, SegmentStyle::dim());
    }
    StyledLine::plain(line)
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
