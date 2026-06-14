pub(super) fn is_html(content_type: Option<&str>, body: &str) -> bool {
    content_type.is_some_and(|content_type| content_type.to_ascii_lowercase().contains("html"))
        || body.to_ascii_lowercase().contains("<html")
        || body.to_ascii_lowercase().contains("<!doctype html")
}

pub(super) fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let tag_end = lower[start..].find('>')? + start + 1;
    let end = lower[tag_end..].find("</title>")? + tag_end;
    Some(
        decode_basic_entities(html[tag_end..end].trim())
            .trim()
            .to_string(),
    )
    .filter(|title| !title.is_empty())
}

pub(super) fn html_to_markdown(html: &str) -> String {
    let without_blocks = remove_html_block(html, "script");
    let without_blocks = remove_html_block(&without_blocks, "style");
    let mut output = String::new();
    let mut tag = String::new();
    let mut in_tag = false;
    for character in without_blocks.chars() {
        if in_tag {
            if character == '>' {
                append_tag_boundary(&mut output, &tag);
                tag.clear();
                in_tag = false;
            } else {
                tag.push(character);
            }
        } else if character == '<' {
            in_tag = true;
        } else {
            output.push(character);
        }
    }
    collapse_markdown_whitespace(&decode_basic_entities(&output))
}

fn remove_html_block(input: &str, tag: &str) -> String {
    let mut output = String::new();
    let mut remaining = input;
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    loop {
        let lower = remaining.to_ascii_lowercase();
        let Some(start) = lower.find(&open) else {
            output.push_str(remaining);
            break;
        };
        output.push_str(&remaining[..start]);
        let Some(end) = lower[start..].find(&close) else {
            break;
        };
        remaining = &remaining[start + end + close.len()..];
    }
    output
}

fn append_tag_boundary(output: &mut String, raw_tag: &str) {
    let tag = raw_tag
        .trim()
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match tag.as_str() {
        "br" | "p" | "div" | "section" | "article" | "header" | "footer" | "tr" => {
            output.push('\n');
        }
        "li" => output.push_str("\n- "),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => output.push_str("\n\n"),
        _ => {}
    }
}

fn decode_basic_entities(input: &str) -> String {
    input
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn collapse_markdown_whitespace(input: &str) -> String {
    let mut output = String::new();
    let mut blank_lines = 0_usize;
    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank_lines += 1;
            if blank_lines <= 1 && !output.is_empty() {
                output.push('\n');
            }
        } else {
            blank_lines = 0;
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(trimmed);
            output.push('\n');
        }
    }
    output.trim().to_string()
}
