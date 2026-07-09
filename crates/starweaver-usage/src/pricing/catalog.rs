//! Built-in model pricing catalog and alias lookup.

use super::normalize_model_id;
use super::profile::{
    ModelPricingDetails, ModelPricingProfile, ModelPricingTier, cny_millis_to_usd_micros,
    scale_rate,
};

#[derive(Clone, Copy, Debug)]
struct PricingRecord {
    aliases: &'static [&'static str],
    profile: ModelPricingProfile,
}

impl PricingRecord {
    const fn new(aliases: &'static [&'static str], pricing: ModelPricingDetails) -> Self {
        Self {
            aliases,
            profile: ModelPricingProfile::from_details(pricing),
        }
    }

    const fn tiered(aliases: &'static [&'static str], tiers: &'static [ModelPricingTier]) -> Self {
        Self {
            aliases,
            profile: ModelPricingProfile::from_tiers(tiers),
        }
    }
}

const fn with_cache_read(
    pricing: ModelPricingDetails,
    cache_read_micros: u64,
) -> ModelPricingDetails {
    pricing.with_cache_read_micros_per_million_tokens(cache_read_micros)
}

const fn with_cache(
    pricing: ModelPricingDetails,
    cache_write_micros: u64,
    cache_read_micros: u64,
) -> ModelPricingDetails {
    pricing
        .with_cache_write_micros_per_million_tokens(cache_write_micros)
        .with_cache_read_micros_per_million_tokens(cache_read_micros)
}

const fn openai_gpt56_pricing(input_micros: u64, output_micros: u64) -> ModelPricingDetails {
    with_cache(
        ModelPricingDetails::new(input_micros, output_micros),
        scale_rate(input_micros, 125, 100),
        scale_rate(input_micros, 10, 100),
    )
}

const fn qwen_pricing(input_cny_millis: u64, output_cny_millis: u64) -> ModelPricingDetails {
    let input = cny_millis_to_usd_micros(input_cny_millis);
    ModelPricingDetails::new(input, cny_millis_to_usd_micros(output_cny_millis))
        .with_cache_write_micros_per_million_tokens(scale_rate(input, 125, 100))
        .with_cache_read_micros_per_million_tokens(scale_rate(input, 10, 100))
}

const fn qwen_tier(
    max_input_tokens: Option<u64>,
    input_cny_millis: u64,
    output_cny_millis: u64,
) -> ModelPricingTier {
    ModelPricingTier::new(
        max_input_tokens,
        qwen_pricing(input_cny_millis, output_cny_millis),
    )
}

// Google Gemini Developer API prompt-length tiers. Source: <https://ai.google.dev/gemini-api/docs/pricing>
const GEMINI_3_1_PRO_PREVIEW_TIERS: &[ModelPricingTier] = &[
    ModelPricingTier::new(
        Some(200_000),
        with_cache_read(ModelPricingDetails::new(2_000_000, 12_000_000), 200_000),
    ),
    ModelPricingTier::new(
        None,
        with_cache_read(ModelPricingDetails::new(4_000_000, 18_000_000), 400_000),
    ),
];

const GEMINI_2_5_PRO_TIERS: &[ModelPricingTier] = &[
    ModelPricingTier::new(
        Some(200_000),
        with_cache_read(ModelPricingDetails::new(1_250_000, 10_000_000), 125_000),
    ),
    ModelPricingTier::new(
        None,
        with_cache_read(ModelPricingDetails::new(2_500_000, 15_000_000), 250_000),
    ),
];

// MiniMax-M3 standard and priority service tiers. Source: <https://platform.minimax.io/docs/guides/pricing-paygo>
const MINIMAX_M3_STANDARD_TIERS: &[ModelPricingTier] = &[
    ModelPricingTier::new(
        Some(512_000),
        with_cache_read(ModelPricingDetails::new(300_000, 1_200_000), 60_000),
    ),
    ModelPricingTier::new(
        None,
        with_cache_read(ModelPricingDetails::new(600_000, 2_400_000), 120_000),
    ),
];

const MINIMAX_M3_PRIORITY_TIERS: &[ModelPricingTier] = &[
    ModelPricingTier::new(
        Some(512_000),
        with_cache_read(ModelPricingDetails::new(450_000, 1_800_000), 90_000),
    ),
    ModelPricingTier::new(
        None,
        with_cache_read(ModelPricingDetails::new(900_000, 3_600_000), 180_000),
    ),
];

// Alibaba Qwen tiered prices are published in CNY per million tokens; we convert
// to approximate USD with 7.2 CNY/USD and model cache write/read as 125%/10% of
// the selected standard input rate. Source: <https://help.aliyun.com/zh/model-studio/model-pricing>
const QWEN3_MAX_TIERS: &[ModelPricingTier] = &[
    qwen_tier(Some(32_000), 2_500, 10_000),
    qwen_tier(Some(128_000), 4_000, 16_000),
    qwen_tier(None, 7_000, 28_000),
];

const QWEN3_MAX_PREVIEW_TIERS: &[ModelPricingTier] = &[
    qwen_tier(Some(32_000), 6_000, 24_000),
    qwen_tier(Some(128_000), 10_000, 40_000),
    qwen_tier(None, 15_000, 60_000),
];

const QWEN3_7_PLUS_TIERS: &[ModelPricingTier] = &[
    qwen_tier(Some(256_000), 2_000, 8_000),
    qwen_tier(None, 6_000, 24_000),
];

const QWEN3_6_PLUS_TIERS: &[ModelPricingTier] = &[
    qwen_tier(Some(256_000), 2_000, 12_000),
    qwen_tier(None, 8_000, 48_000),
];

const QWEN3_5_PLUS_TIERS: &[ModelPricingTier] = &[
    qwen_tier(Some(128_000), 800, 4_800),
    qwen_tier(Some(256_000), 2_000, 12_000),
    qwen_tier(None, 4_000, 24_000),
];

const QWEN_PLUS_TIERS: &[ModelPricingTier] = &[
    qwen_tier(Some(128_000), 800, 2_000),
    qwen_tier(Some(256_000), 2_400, 20_000),
    qwen_tier(None, 4_800, 48_000),
];

const QWEN3_6_FLASH_TIERS: &[ModelPricingTier] = &[
    qwen_tier(Some(256_000), 1_200, 7_200),
    qwen_tier(None, 4_800, 28_800),
];

const QWEN3_5_FLASH_TIERS: &[ModelPricingTier] = &[
    qwen_tier(Some(128_000), 200, 2_000),
    qwen_tier(Some(256_000), 800, 8_000),
    qwen_tier(None, 1_200, 12_000),
];

const QWEN_FLASH_TIERS: &[ModelPricingTier] = &[
    qwen_tier(Some(128_000), 150, 1_500),
    qwen_tier(Some(256_000), 600, 6_000),
    qwen_tier(None, 1_200, 12_000),
];

// OpenAI GPT-5.4/5.5/5.6 long-context surcharges are intentionally left as
// fixed standard rows until a clear public context threshold can be represented.

// Built-in catalog of standard model prices.
//
// Source pages checked when rows are added or updated:
// - OpenAI: <https://developers.openai.com/api/docs/pricing>
// - Anthropic: <https://platform.claude.com/docs/en/about-claude/pricing>
// - Google Gemini: <https://ai.google.dev/gemini-api/docs/pricing>
// - Z.AI GLM: <https://docs.z.ai/guides/overview/pricing>
// - Alibaba Qwen: <https://help.aliyun.com/zh/model-studio/model-pricing>
// - MiniMax: <https://platform.minimax.io/docs/guides/pricing-paygo>
// - Moonshot Kimi: <https://platform.kimi.ai/docs/pricing/chat>
// - Xiaomi MiMo: <https://platform.xiaomimimo.com/docs/en-US/price/pay-as-you-go>
// - DeepSeek: <https://api-docs.deepseek.com/quick_start/pricing>
// - xAI Grok: <https://docs.x.ai/developers/models/grok-4.5>
const MODEL_PRICING_CATALOG: &[PricingRecord] = &[
    // xAI Grok. Cached input is represented as cache-read pricing.
    PricingRecord::new(
        &[
            "grok-4.5",
            "grok-4-5",
            "grok-4.5-latest",
            "grok-4-5-latest",
            "grok-build-latest",
        ],
        with_cache_read(ModelPricingDetails::new(2_000_000, 6_000_000), 500_000),
    ),
    // OpenAI GPT and o-series. Cached input is represented as cache-read pricing.
    PricingRecord::new(
        &[
            "gpt-5.6",
            "gpt-5-6",
            "gpt-5.6-sol",
            "gpt-5-6-sol",
            "gpt-5.6-sol-ultra",
            "gpt-5-6-sol-ultra",
        ],
        openai_gpt56_pricing(5_000_000, 30_000_000),
    ),
    PricingRecord::new(
        &["gpt-5.6-terra", "gpt-5-6-terra"],
        openai_gpt56_pricing(2_500_000, 15_000_000),
    ),
    PricingRecord::new(
        &["gpt-5.6-luna", "gpt-5-6-luna"],
        openai_gpt56_pricing(1_000_000, 6_000_000),
    ),
    PricingRecord::new(
        &[
            "gpt-5.5",
            "gpt-5-5",
            "gpt-5.5-chat-latest",
            "gpt-5-5-chat-latest",
        ],
        with_cache_read(ModelPricingDetails::new(5_000_000, 30_000_000), 500_000),
    ),
    PricingRecord::new(
        &["gpt-5.5-pro", "gpt-5-5-pro"],
        ModelPricingDetails::new(30_000_000, 180_000_000),
    ),
    PricingRecord::new(
        &["gpt-5.4-pro", "gpt-5-4-pro"],
        ModelPricingDetails::new(30_000_000, 180_000_000),
    ),
    PricingRecord::new(
        &["gpt-5.4", "gpt-5-4"],
        with_cache_read(ModelPricingDetails::new(2_500_000, 15_000_000), 250_000),
    ),
    PricingRecord::new(
        &["gpt-5.4-mini", "gpt-5-4-mini"],
        with_cache_read(ModelPricingDetails::new(750_000, 4_500_000), 75_000),
    ),
    PricingRecord::new(
        &["gpt-5.4-nano", "gpt-5-4-nano"],
        with_cache_read(ModelPricingDetails::new(200_000, 1_250_000), 20_000),
    ),
    PricingRecord::new(
        &["gpt-5", "gpt-5-chat-latest"],
        with_cache_read(ModelPricingDetails::new(1_250_000, 10_000_000), 125_000),
    ),
    PricingRecord::new(
        &["gpt-5-mini"],
        with_cache_read(ModelPricingDetails::new(250_000, 2_000_000), 25_000),
    ),
    PricingRecord::new(
        &["gpt-5-nano"],
        with_cache_read(ModelPricingDetails::new(50_000, 400_000), 5_000),
    ),
    PricingRecord::new(
        &[
            "gpt-4.1",
            "gpt-4-1",
            "gpt-4.1-2025-04-14",
            "gpt-4-1-2025-04-14",
        ],
        with_cache_read(ModelPricingDetails::new(2_000_000, 8_000_000), 500_000),
    ),
    PricingRecord::new(
        &[
            "gpt-4.1-mini",
            "gpt-4-1-mini",
            "gpt-4.1-mini-2025-04-14",
            "gpt-4-1-mini-2025-04-14",
        ],
        with_cache_read(ModelPricingDetails::new(400_000, 1_600_000), 100_000),
    ),
    PricingRecord::new(
        &[
            "gpt-4.1-nano",
            "gpt-4-1-nano",
            "gpt-4.1-nano-2025-04-14",
            "gpt-4-1-nano-2025-04-14",
        ],
        with_cache_read(ModelPricingDetails::new(100_000, 400_000), 25_000),
    ),
    PricingRecord::new(
        &[
            "gpt-4o",
            "gpt-4o-2024-05-13",
            "gpt-4o-2024-08-06",
            "gpt-4o-2024-11-20",
            "chatgpt-4o-latest",
        ],
        with_cache_read(ModelPricingDetails::new(2_500_000, 10_000_000), 1_250_000),
    ),
    PricingRecord::new(
        &["gpt-4o-mini", "gpt-4o-mini-2024-07-18"],
        with_cache_read(ModelPricingDetails::new(150_000, 600_000), 75_000),
    ),
    PricingRecord::new(
        &["o3", "o3-2025-04-16"],
        with_cache_read(ModelPricingDetails::new(2_000_000, 8_000_000), 500_000),
    ),
    PricingRecord::new(
        &["o4-mini", "o4-mini-2025-04-16"],
        with_cache_read(ModelPricingDetails::new(1_100_000, 4_400_000), 275_000),
    ),
    PricingRecord::new(
        &[
            "o3-mini",
            "o3-mini-2025-01-31",
            "o1-mini",
            "o1-mini-2024-09-12",
        ],
        with_cache_read(ModelPricingDetails::new(1_100_000, 4_400_000), 550_000),
    ),
    PricingRecord::new(
        &["o1", "o1-2024-12-17", "o1-preview", "o1-preview-2024-09-12"],
        with_cache_read(ModelPricingDetails::new(15_000_000, 60_000_000), 7_500_000),
    ),
    PricingRecord::new(
        &["o3-pro", "o3-pro-2025-06-10"],
        ModelPricingDetails::new(20_000_000, 80_000_000),
    ),
    PricingRecord::new(
        &["o1-pro", "o1-pro-2025-03-19"],
        ModelPricingDetails::new(150_000_000, 600_000_000),
    ),
    // Anthropic Claude. Cache-write uses the default 5-minute cache creation rate.
    PricingRecord::new(
        &[
            "claude-opus-4.8",
            "claude-opus-4-8",
            "claude-opus-4.7",
            "claude-opus-4-7",
            "claude-opus-4.6",
            "claude-opus-4-6",
            "claude-opus-4.5",
            "claude-opus-4-5",
        ],
        with_cache(
            ModelPricingDetails::new(5_000_000, 25_000_000),
            6_250_000,
            500_000,
        ),
    ),
    PricingRecord::new(
        &[
            "claude-opus-4.1",
            "claude-opus-4-1",
            "claude-opus-4-1-20250805",
        ],
        with_cache(
            ModelPricingDetails::new(15_000_000, 75_000_000),
            18_750_000,
            1_500_000,
        ),
    ),
    PricingRecord::new(
        &["claude-opus-4", "claude-opus-4-20250514"],
        with_cache(
            ModelPricingDetails::new(15_000_000, 75_000_000),
            18_750_000,
            1_500_000,
        ),
    ),
    PricingRecord::new(
        &[
            "claude-sonnet-4.6",
            "claude-sonnet-4-6",
            "claude-sonnet-4.5",
            "claude-sonnet-4-5",
            "claude-sonnet-4-5-20250929",
            "claude-sonnet-4",
            "claude-sonnet-4-20250514",
            "claude-3.7-sonnet",
            "claude-3-7-sonnet",
            "claude-3-7-sonnet-20250219",
            "claude-3.5-sonnet",
            "claude-3-5-sonnet",
            "claude-3-5-sonnet-latest",
            "claude-3-5-sonnet-20241022",
            "claude-3-5-sonnet-20240620",
            "claude-3-sonnet",
            "claude-3-sonnet-20240229",
        ],
        with_cache(
            ModelPricingDetails::new(3_000_000, 15_000_000),
            3_750_000,
            300_000,
        ),
    ),
    PricingRecord::new(
        &[
            "claude-haiku-4.5",
            "claude-haiku-4-5",
            "claude-haiku-4-5-20251001",
        ],
        with_cache(
            ModelPricingDetails::new(1_000_000, 5_000_000),
            1_250_000,
            100_000,
        ),
    ),
    PricingRecord::new(
        &[
            "claude-3.5-haiku",
            "claude-3-5-haiku",
            "claude-3-5-haiku-latest",
            "claude-3-5-haiku-20241022",
        ],
        with_cache(
            ModelPricingDetails::new(800_000, 4_000_000),
            1_000_000,
            80_000,
        ),
    ),
    PricingRecord::new(
        &["claude-3-opus", "claude-3-opus-20240229"],
        with_cache(
            ModelPricingDetails::new(15_000_000, 75_000_000),
            18_750_000,
            1_500_000,
        ),
    ),
    PricingRecord::new(
        &["claude-3-haiku", "claude-3-haiku-20240307"],
        with_cache(
            ModelPricingDetails::new(250_000, 1_250_000),
            312_500,
            25_000,
        ),
    ),
    // Google Gemini Developer API. Gemini 3.1 Pro and 2.5 Pro have prompt-length tiers; cache storage is not represented.
    PricingRecord::new(
        &["gemini-3.5-flash", "gemini-3-5-flash"],
        with_cache_read(ModelPricingDetails::new(1_500_000, 9_000_000), 150_000),
    ),
    PricingRecord::new(
        &[
            "gemini-3.5-live-translate-preview",
            "gemini-3-5-live-translate-preview",
        ],
        ModelPricingDetails::new(3_500_000, 21_000_000),
    ),
    PricingRecord::tiered(
        &[
            "gemini-3.1-pro-preview",
            "gemini-3-1-pro-preview",
            "gemini-3.1-pro-preview-customtools",
            "gemini-3-1-pro-preview-customtools",
        ],
        GEMINI_3_1_PRO_PREVIEW_TIERS,
    ),
    PricingRecord::new(
        &["gemini-3.1-flash-lite", "gemini-3-1-flash-lite"],
        with_cache_read(ModelPricingDetails::new(250_000, 1_500_000), 25_000),
    ),
    PricingRecord::new(
        &[
            "gemini-3.1-flash-live-preview",
            "gemini-3-1-flash-live-preview",
        ],
        ModelPricingDetails::new(750_000, 4_500_000),
    ),
    PricingRecord::new(
        &["gemini-3.1-flash-image", "gemini-3-1-flash-image"],
        ModelPricingDetails::new(500_000, 3_000_000),
    ),
    PricingRecord::new(
        &[
            "gemini-3.1-flash-tts-preview",
            "gemini-3-1-flash-tts-preview",
        ],
        ModelPricingDetails::new(1_000_000, 20_000_000),
    ),
    PricingRecord::new(
        &["gemini-3-flash-preview", "gemini-3.0-flash-preview"],
        with_cache_read(ModelPricingDetails::new(500_000, 3_000_000), 50_000),
    ),
    PricingRecord::new(
        &["gemini-3-pro-image"],
        ModelPricingDetails::new(2_000_000, 12_000_000),
    ),
    PricingRecord::tiered(
        &["gemini-2.5-pro", "gemini-2-5-pro", "gemini-2.5-pro-preview"],
        GEMINI_2_5_PRO_TIERS,
    ),
    PricingRecord::new(
        &[
            "gemini-2.5-flash",
            "gemini-2-5-flash",
            "gemini-2.5-flash-preview",
        ],
        with_cache_read(ModelPricingDetails::new(300_000, 2_500_000), 30_000),
    ),
    PricingRecord::new(
        &[
            "gemini-2.5-flash-lite",
            "gemini-2-5-flash-lite",
            "gemini-2.5-flash-lite-preview",
        ],
        with_cache_read(ModelPricingDetails::new(100_000, 400_000), 10_000),
    ),
    PricingRecord::new(
        &[
            "gemini-2.0-flash",
            "gemini-2-0-flash",
            "gemini-2.0-flash-001",
        ],
        with_cache_read(ModelPricingDetails::new(100_000, 400_000), 25_000),
    ),
    PricingRecord::new(
        &[
            "gemini-2.0-flash-lite",
            "gemini-2-0-flash-lite",
            "gemini-2.0-flash-lite-001",
        ],
        ModelPricingDetails::new(75_000, 300_000),
    ),
    PricingRecord::new(
        &[
            "gemini-1.5-flash",
            "gemini-1-5-flash",
            "gemini-1.5-flash-latest",
        ],
        ModelPricingDetails::new(75_000, 300_000),
    ),
    PricingRecord::new(
        &["gemini-1.5-pro", "gemini-1-5-pro", "gemini-1.5-pro-latest"],
        ModelPricingDetails::new(1_250_000, 5_000_000),
    ),
    // Z.AI GLM.
    PricingRecord::new(
        &["glm-5.1", "glm-5-1"],
        with_cache_read(ModelPricingDetails::new(1_400_000, 4_400_000), 260_000),
    ),
    PricingRecord::new(
        &["glm-5"],
        with_cache_read(ModelPricingDetails::new(1_000_000, 3_200_000), 200_000),
    ),
    PricingRecord::new(
        &["glm-5-turbo"],
        with_cache_read(ModelPricingDetails::new(1_200_000, 4_000_000), 240_000),
    ),
    PricingRecord::new(
        &[
            "glm-4.7", "glm-4-7", "glm-4.6", "glm-4-6", "glm-4.5", "glm-4-5",
        ],
        with_cache_read(ModelPricingDetails::new(600_000, 2_200_000), 110_000),
    ),
    PricingRecord::new(
        &["glm-4.7-flashx", "glm-4-7-flashx"],
        with_cache_read(ModelPricingDetails::new(70_000, 400_000), 10_000),
    ),
    PricingRecord::new(
        &["glm-4.5-x", "glm-4-5-x"],
        with_cache_read(ModelPricingDetails::new(2_200_000, 8_900_000), 450_000),
    ),
    PricingRecord::new(
        &["glm-4.5-air", "glm-4-5-air"],
        with_cache_read(ModelPricingDetails::new(200_000, 1_100_000), 30_000),
    ),
    PricingRecord::new(
        &["glm-4.5-airx", "glm-4-5-airx"],
        with_cache_read(ModelPricingDetails::new(1_100_000, 4_500_000), 220_000),
    ),
    PricingRecord::new(
        &["glm-4-32b-0414-128k", "glm-4-32b-128k"],
        ModelPricingDetails::new(100_000, 100_000),
    ),
    PricingRecord::new(
        &[
            "glm-4.7-flash",
            "glm-4-7-flash",
            "glm-4.5-flash",
            "glm-4-5-flash",
        ],
        with_cache_read(ModelPricingDetails::new(0, 0), 0),
    ),
    // MiniMax. M3 has standard and priority service-tier rows split at 512k input tokens.
    PricingRecord::tiered(
        &["minimax-m3", "minimax-m3-standard"],
        MINIMAX_M3_STANDARD_TIERS,
    ),
    PricingRecord::tiered(&["minimax-m3-priority"], MINIMAX_M3_PRIORITY_TIERS),
    PricingRecord::new(
        &["minimax-m2.7", "minimax-m2-7"],
        with_cache(
            ModelPricingDetails::new(300_000, 1_200_000),
            375_000,
            60_000,
        ),
    ),
    PricingRecord::new(
        &["minimax-m2.7-highspeed", "minimax-m2-7-highspeed"],
        with_cache(
            ModelPricingDetails::new(600_000, 2_400_000),
            375_000,
            60_000,
        ),
    ),
    PricingRecord::new(
        &[
            "minimax-m2.5",
            "minimax-m2-5",
            "minimax-m2.1",
            "minimax-m2-1",
            "minimax-m2",
        ],
        with_cache(
            ModelPricingDetails::new(300_000, 1_200_000),
            375_000,
            30_000,
        ),
    ),
    PricingRecord::new(
        &[
            "minimax-m2.5-highspeed",
            "minimax-m2-5-highspeed",
            "minimax-m2.1-highspeed",
            "minimax-m2-1-highspeed",
        ],
        with_cache(
            ModelPricingDetails::new(600_000, 2_400_000),
            375_000,
            30_000,
        ),
    ),
    // Alibaba Qwen. Values are approximate USD conversions from published CNY prices.
    PricingRecord::new(
        &[
            "qwen3.7-max",
            "qwen3-7-max",
            "qwen3.7-max-2026-06-08",
            "qwen3-7-max-2026-06-08",
            "qwen3.7-max-2026-05-20",
            "qwen3-7-max-2026-05-20",
        ],
        qwen_pricing(12_000, 36_000),
    ),
    PricingRecord::tiered(&["qwen3-max", "qwen3-max-2026-01-23"], QWEN3_MAX_TIERS),
    PricingRecord::tiered(&["qwen3-max-preview"], QWEN3_MAX_PREVIEW_TIERS),
    PricingRecord::new(&["qwen-max"], qwen_pricing(2_400, 9_600)),
    PricingRecord::tiered(
        &[
            "qwen3.7-plus",
            "qwen3-7-plus",
            "qwen3.7-plus-2026-05-26",
            "qwen3-7-plus-2026-05-26",
        ],
        QWEN3_7_PLUS_TIERS,
    ),
    PricingRecord::tiered(&["qwen3.6-plus", "qwen3-6-plus"], QWEN3_6_PLUS_TIERS),
    PricingRecord::tiered(&["qwen3.5-plus", "qwen3-5-plus"], QWEN3_5_PLUS_TIERS),
    PricingRecord::tiered(
        &[
            "qwen-plus",
            "qwen-plus-latest",
            "qwen-plus-2025-12-01",
            "qwen2.5-plus",
            "qwen2-5-plus",
        ],
        QWEN_PLUS_TIERS,
    ),
    PricingRecord::tiered(&["qwen3.6-flash", "qwen3-6-flash"], QWEN3_6_FLASH_TIERS),
    PricingRecord::tiered(&["qwen3.5-flash", "qwen3-5-flash"], QWEN3_5_FLASH_TIERS),
    PricingRecord::tiered(&["qwen-flash", "qwen-flash-2025-07-28"], QWEN_FLASH_TIERS),
    PricingRecord::new(&["qwen-turbo"], qwen_pricing(300, 600)),
    // Moonshot Kimi.
    PricingRecord::new(
        &["kimi-k2.7-code", "kimi-k2-7-code"],
        with_cache_read(ModelPricingDetails::new(950_000, 4_000_000), 190_000),
    ),
    PricingRecord::new(
        &["kimi-k2.7-code-highspeed", "kimi-k2-7-code-highspeed"],
        with_cache_read(ModelPricingDetails::new(1_900_000, 8_000_000), 380_000),
    ),
    PricingRecord::new(
        &["kimi-k2.6", "kimi-k2-6"],
        with_cache_read(ModelPricingDetails::new(950_000, 4_000_000), 160_000),
    ),
    PricingRecord::new(
        &["kimi-k2.5", "kimi-k2-5"],
        with_cache_read(ModelPricingDetails::new(600_000, 3_000_000), 100_000),
    ),
    PricingRecord::new(
        &["moonshot-v1-8k", "moonshot-v1-8k-vision-preview"],
        ModelPricingDetails::new(200_000, 2_000_000),
    ),
    PricingRecord::new(
        &["moonshot-v1-32k", "moonshot-v1-32k-vision-preview"],
        ModelPricingDetails::new(1_000_000, 3_000_000),
    ),
    PricingRecord::new(
        &["moonshot-v1-128k", "moonshot-v1-128k-vision-preview"],
        ModelPricingDetails::new(2_000_000, 5_000_000),
    ),
    // Xiaomi MiMo. Cache-write is listed as limited-time free.
    PricingRecord::new(
        &["mimo-v2.5-pro", "mimo-v2-5-pro", "mimo-v2-pro"],
        with_cache(ModelPricingDetails::new(435_000, 870_000), 0, 3_600),
    ),
    PricingRecord::new(
        &["mimo-v2.5", "mimo-v2-5", "mimo-v2-omni"],
        with_cache(ModelPricingDetails::new(140_000, 280_000), 0, 2_800),
    ),
    PricingRecord::new(
        &["mimo-v2-flash"],
        with_cache(ModelPricingDetails::new(100_000, 300_000), 0, 10_000),
    ),
    // DeepSeek. Alternate public names currently route to deepseek-v4-flash pricing.
    PricingRecord::new(
        &["deepseek-v4-flash", "deepseek-chat", "deepseek-reasoner"],
        with_cache_read(ModelPricingDetails::new(140_000, 280_000), 2_800),
    ),
    PricingRecord::new(
        &["deepseek-v4-pro"],
        with_cache_read(ModelPricingDetails::new(435_000, 870_000), 3_625),
    ),
];

/// Return built-in best-effort pricing profile for a known normalized or raw model id.
#[must_use]
pub(super) fn lookup_model_pricing_profile(model_id: &str) -> Option<ModelPricingProfile> {
    let normalized = normalize_model_id(model_id);
    let normalized = normalized.as_str();
    MODEL_PRICING_CATALOG
        .iter()
        .find(|record| record.aliases.contains(&normalized))
        .map(|record| record.profile)
}
