use serde::{Deserialize, Serialize};

/// Built-in model capability/config preset names.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ModelConfigPreset {
    /// Claude 200K context profile.
    #[serde(rename = "claude_200k")]
    Claude200k,
    /// Claude 400K context profile.
    #[serde(rename = "claude_400k")]
    Claude400k,
    /// Claude 1M context profile.
    #[serde(rename = "claude_1m")]
    Claude1m,
    /// GPT-5 270K context profile.
    #[serde(rename = "gpt5_270k")]
    Gpt5_270k,
    /// GPT-5 350K context profile.
    #[serde(rename = "gpt5_350k")]
    Gpt5_350k,
    /// GPT-5 1M class context profile.
    #[serde(rename = "gpt5_1m")]
    Gpt5_1m,
    /// `DeepSeek` V4 400K context profile.
    #[serde(rename = "deepseek_v4_400k")]
    DeepSeekV4_400k,
    /// `DeepSeek` V4 1M context profile.
    #[serde(rename = "deepseek_v4_1m")]
    DeepSeekV4_1m,
    /// Grok 4.5 500K context profile.
    #[serde(rename = "grok_4_5_500k")]
    Grok45_500k,
    /// `MiMo` V2.5 1M context profile.
    #[serde(rename = "mimo_v2_5_1m")]
    MimoV25_1m,
    /// `MiMo` V2.5 Pro 1M context profile.
    #[serde(rename = "mimo_v2_5_pro_1m")]
    MimoV25Pro1m,
    /// Gemini 200K context profile.
    #[serde(rename = "gemini_200k")]
    Gemini200k,
    /// Gemini 1M context profile.
    #[serde(rename = "gemini_1m")]
    Gemini1m,
}

impl ModelConfigPreset {
    /// Return the canonical preset name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude200k => "claude_200k",
            Self::Claude400k => "claude_400k",
            Self::Claude1m => "claude_1m",
            Self::Gpt5_270k => "gpt5_270k",
            Self::Gpt5_350k => "gpt5_350k",
            Self::Gpt5_1m => "gpt5_1m",
            Self::DeepSeekV4_400k => "deepseek_v4_400k",
            Self::DeepSeekV4_1m => "deepseek_v4_1m",
            Self::Grok45_500k => "grok_4_5_500k",
            Self::MimoV25_1m => "mimo_v2_5_1m",
            Self::MimoV25Pro1m => "mimo_v2_5_pro_1m",
            Self::Gemini200k => "gemini_200k",
            Self::Gemini1m => "gemini_1m",
        }
    }
}
