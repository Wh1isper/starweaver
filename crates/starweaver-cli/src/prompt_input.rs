//! CLI prompt input parts and media attachment helpers.

use serde::{Deserialize, Serialize};
use starweaver_model::ContentPart;

/// Pending binary attachment submitted with one CLI prompt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PromptAttachment {
    /// Binary payload.
    pub data: Vec<u8>,
    /// MIME media type.
    pub media_type: String,
    /// Original byte length.
    pub size_bytes: usize,
    /// Visible placeholder inserted into the composer.
    pub placeholder: String,
}

impl PromptAttachment {
    /// Build an image attachment.
    #[must_use]
    pub fn image(index: usize, data: Vec<u8>, media_type: impl Into<String>) -> Self {
        let media_type = media_type.into();
        let size_bytes = data.len();
        let placeholder = attachment_placeholder(index, &media_type, size_bytes);
        Self {
            data,
            media_type,
            size_bytes,
            placeholder,
        }
    }

    /// Return a compact user-facing description.
    #[must_use]
    pub fn description(&self) -> String {
        format!("{} {}", self.media_type, format_size_bytes(self.size_bytes))
    }

    /// Convert into a provider-neutral model content part.
    #[must_use]
    pub fn into_content_part(self) -> ContentPart {
        ContentPart::Binary {
            data: self.data,
            media_type: self.media_type,
        }
    }
}

/// Submitted prompt text plus optional binary attachments.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PromptInput {
    /// Natural language prompt text.
    pub text: String,
    /// Attached binary media.
    #[serde(default)]
    pub attachments: Vec<PromptAttachment>,
    /// Additional text-only context parts appended to the first user prompt.
    #[serde(default)]
    pub extra_text_parts: Vec<String>,
    /// Cacheable guidance loaded for the current run and synchronized into canonical history.
    #[serde(default)]
    pub guidance_text_parts: Vec<String>,
}

impl PromptInput {
    /// Build text-only input.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            attachments: Vec::new(),
            extra_text_parts: Vec::new(),
            guidance_text_parts: Vec::new(),
        }
    }

    /// Return true when this input needs first-request content-part rewriting.
    ///
    /// Deliberately excludes `guidance_text_parts`: guidance is injected by
    /// `CliGuidanceAdapter` as cacheable canonical system prompts, not user prompt content.
    #[must_use]
    pub fn has_content_parts(&self) -> bool {
        !self.attachments.is_empty() || !self.extra_text_parts.is_empty()
    }

    /// Append cacheable guidance for the current run.
    pub fn push_guidance_text_part(&mut self, text: impl Into<String>) {
        let text = text.into();
        if !text.trim().is_empty() {
            self.guidance_text_parts.push(text);
        }
    }

    /// Strip generated placeholders out of text and append attachments and extra text parts.
    #[must_use]
    pub fn into_content_parts(self) -> Vec<ContentPart> {
        let mut text = self.text;
        for attachment in &self.attachments {
            if !attachment.placeholder.is_empty() {
                text = text.replace(&attachment.placeholder, "");
            }
        }
        let text = text.trim().to_string();
        let mut parts = Vec::new();
        if !text.is_empty() {
            parts.push(ContentPart::Text { text });
        }
        parts.extend(
            self.attachments
                .into_iter()
                .map(PromptAttachment::into_content_part),
        );
        parts.extend(
            self.extra_text_parts
                .into_iter()
                .filter(|text| !text.trim().is_empty())
                .map(|text| ContentPart::Text { text }),
        );
        parts
    }

    /// Return text suitable for transcript display and prompt history.
    #[must_use]
    pub fn display_text(&self) -> String {
        if !self.text.trim().is_empty() {
            return self.text.trim().to_string();
        }
        self.attachments
            .iter()
            .map(|attachment| attachment.placeholder.clone())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Format attachment placeholder used in the composer.
#[must_use]
pub fn attachment_placeholder(index: usize, media_type: &str, size_bytes: usize) -> String {
    format!(
        "[Attached image {index}: {media_type} {}]",
        format_size_bytes(size_bytes)
    )
}

/// Format byte size for compact UI display.
#[must_use]
pub fn format_size_bytes(size_bytes: usize) -> String {
    if size_bytes < 1024 {
        return format!("{size_bytes}B");
    }
    if size_bytes < 1024 * 1024 {
        return format!("{}KB", size_bytes.saturating_add(512) / 1024);
    }
    let tenths = size_bytes.saturating_mul(10).saturating_add(512 * 1024) / (1024 * 1024);
    format!("{}.{:01}MB", tenths / 10, tenths % 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_input_strips_placeholders_and_appends_binary_parts() {
        let attachment = PromptAttachment::image(1, b"image-bytes".to_vec(), "image/png");
        let placeholder = attachment.placeholder.clone();
        let input = PromptInput {
            text: format!("look at this {placeholder} please"),
            attachments: vec![attachment],
            extra_text_parts: Vec::new(),
            guidance_text_parts: Vec::new(),
        };

        let parts = input.into_content_parts();
        assert_eq!(
            parts,
            vec![
                ContentPart::Text {
                    text: "look at this  please".to_string(),
                },
                ContentPart::Binary {
                    data: b"image-bytes".to_vec(),
                    media_type: "image/png".to_string(),
                },
            ]
        );
    }

    #[test]
    fn prompt_input_appends_extra_text_parts_after_attachments() {
        let mut input = PromptInput::text("inspect");
        input.extra_text_parts.push(
            "<user-rules location=/tmp/RULES.md>\nPrefer concise output.\n</user-rules>"
                .to_string(),
        );

        let parts = input.into_content_parts();
        assert_eq!(
            parts,
            vec![
                ContentPart::Text {
                    text: "inspect".to_string(),
                },
                ContentPart::Text {
                    text:
                        "<user-rules location=/tmp/RULES.md>\nPrefer concise output.\n</user-rules>"
                            .to_string(),
                },
            ]
        );
    }

    #[test]
    fn display_text_falls_back_to_attachment_placeholder() {
        let input = PromptInput {
            text: "   ".to_string(),
            attachments: vec![PromptAttachment::image(1, vec![1, 2, 3], "image/png")],
            extra_text_parts: Vec::new(),
            guidance_text_parts: Vec::new(),
        };

        assert_eq!(input.display_text(), "[Attached image 1: image/png 3B]");
        assert_eq!(format_size_bytes(1024), "1KB");
        assert_eq!(format_size_bytes(1024 * 1024), "1.0MB");
    }
}
