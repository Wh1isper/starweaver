//! Provider cache-point wire mapping tests.
#![allow(clippy::unwrap_used)]

use serde_json::{Map, json};
use starweaver_model::{
    CachePointTtl, ContentPart, ModelError, ModelMessage, ModelRequest, ModelRequestPart,
    ModelResponse, ModelResponsePart, ModelSettings, OpenAiChatSettings, OpenAiPromptCacheMode,
    OpenAiPromptCacheOptions, OpenAiPromptCacheTtl, OpenAiResponsesSettings, ProviderPartInfo,
    ProviderSettings,
    providers::{
        anthropic::AnthropicMessagesAdapter, openai_chat::OpenAiChatAdapter,
        openai_responses::OpenAiResponsesAdapter,
    },
};

fn user_message(content: Vec<ContentPart>) -> ModelMessage {
    ModelMessage::Request(ModelRequest {
        parts: vec![ModelRequestPart::UserPrompt {
            content,
            name: None,
            metadata: Map::new(),
        }],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Map::new(),
    })
}

fn cache_content(ttl: Option<CachePointTtl>) -> Vec<ContentPart> {
    vec![
        ContentPart::text("stable"),
        ContentPart::CachePoint { ttl },
        ContentPart::text("dynamic"),
    ]
}

#[test]
fn cache_point_serializes_as_provider_neutral_marker() {
    let point = ContentPart::cache_point_with_ttl(CachePointTtl::OneHour);
    assert_eq!(
        serde_json::to_value(&point).unwrap(),
        json!({"kind": "cache_point", "ttl": "1h"})
    );
    assert_eq!(
        serde_json::from_value::<ContentPart>(json!({"kind": "cache_point"})).unwrap(),
        ContentPart::cache_point()
    );
}

#[test]
fn gpt_5_6_chat_maps_explicit_breakpoint_and_request_policy() {
    let settings = ModelSettings {
        provider_settings: ProviderSettings {
            openai_chat: Some(OpenAiChatSettings {
                prompt_cache_key: Some("shared-prefix".to_string()),
                prompt_cache_options: Some(OpenAiPromptCacheOptions {
                    mode: OpenAiPromptCacheMode::Explicit,
                    ttl: Some(OpenAiPromptCacheTtl::ThirtyMinutes),
                }),
                ..OpenAiChatSettings::default()
            }),
            ..ProviderSettings::default()
        },
        ..ModelSettings::default()
    };
    let request = OpenAiChatAdapter::build_request(
        "openrouter:openai/gpt-5-6",
        &[user_message(cache_content(None))],
        Some(&settings),
        &[],
    )
    .unwrap();

    assert_eq!(request["prompt_cache_key"], "shared-prefix");
    assert_eq!(
        request["prompt_cache_options"],
        json!({"mode": "explicit", "ttl": "30m"})
    );
    assert_eq!(
        request["messages"][0]["content"][0]["prompt_cache_breakpoint"],
        json!({"mode": "explicit"})
    );
}

#[test]
fn gpt_5_6_responses_maps_explicit_breakpoint() {
    let settings = ModelSettings {
        provider_settings: ProviderSettings {
            openai_responses: Some(OpenAiResponsesSettings {
                prompt_cache_options: Some(OpenAiPromptCacheOptions {
                    mode: OpenAiPromptCacheMode::Implicit,
                    ttl: None,
                }),
                ..OpenAiResponsesSettings::default()
            }),
            ..ProviderSettings::default()
        },
        ..ModelSettings::default()
    };
    let request = OpenAiResponsesAdapter::build_request(
        "gpt-5.6-2026-07-01",
        &[user_message(cache_content(None))],
        Some(&settings),
        &[],
        &[],
    )
    .unwrap();

    assert_eq!(request["prompt_cache_options"], json!({"mode": "implicit"}));
    assert_eq!(
        request["input"][0]["content"][0]["prompt_cache_breakpoint"],
        json!({"mode": "explicit"})
    );
}

#[test]
fn older_openai_models_filter_markers_and_reject_new_options() {
    let messages = [user_message(cache_content(None))];
    let request = OpenAiChatAdapter::build_request("gpt-5.5", &messages, None, &[]).unwrap();
    assert_eq!(
        request["messages"][0]["content"].as_array().unwrap().len(),
        2
    );
    assert!(
        request["messages"][0]["content"][0]
            .get("prompt_cache_breakpoint")
            .is_none()
    );

    let settings = ModelSettings {
        provider_settings: ProviderSettings {
            openai_chat: Some(OpenAiChatSettings {
                prompt_cache_options: Some(OpenAiPromptCacheOptions {
                    mode: OpenAiPromptCacheMode::Explicit,
                    ttl: None,
                }),
                ..OpenAiChatSettings::default()
            }),
            ..ProviderSettings::default()
        },
        ..ModelSettings::default()
    };
    let error =
        OpenAiChatAdapter::build_request("gpt-5.5", &messages, Some(&settings), &[]).unwrap_err();
    assert!(
        matches!(error, ModelError::MessageMapping(message) if message.contains("does not support"))
    );
}

#[test]
fn gpt_5_6_rejects_legacy_and_new_cache_options_together() {
    let options = OpenAiPromptCacheOptions {
        mode: OpenAiPromptCacheMode::Explicit,
        ttl: Some(OpenAiPromptCacheTtl::ThirtyMinutes),
    };
    let chat_settings = ModelSettings {
        provider_settings: ProviderSettings {
            openai_chat: Some(OpenAiChatSettings {
                prompt_cache_retention: Some("24h".to_string()),
                prompt_cache_options: Some(options.clone()),
                ..OpenAiChatSettings::default()
            }),
            ..ProviderSettings::default()
        },
        ..ModelSettings::default()
    };
    let error = OpenAiChatAdapter::build_request(
        "gpt-5.6",
        &[user_message(vec![ContentPart::text("hello")])],
        Some(&chat_settings),
        &[],
    )
    .unwrap_err();
    assert!(
        matches!(error, ModelError::MessageMapping(message) if message.contains("cannot both be configured"))
    );

    let responses_settings = ModelSettings {
        provider_settings: ProviderSettings {
            openai_responses: Some(OpenAiResponsesSettings {
                prompt_cache_retention: Some("24h".to_string()),
                prompt_cache_options: Some(options),
                ..OpenAiResponsesSettings::default()
            }),
            ..ProviderSettings::default()
        },
        ..ModelSettings::default()
    };
    let error = OpenAiResponsesAdapter::build_request(
        "gpt-5.6",
        &[user_message(vec![ContentPart::text("hello")])],
        Some(&responses_settings),
        &[],
        &[],
    )
    .unwrap_err();
    assert!(
        matches!(error, ModelError::MessageMapping(message) if message.contains("cannot both be configured"))
    );
}

#[test]
fn openai_rejects_per_point_ttl() {
    let error = OpenAiResponsesAdapter::build_request(
        "gpt-5.6",
        &[user_message(cache_content(Some(
            CachePointTtl::FiveMinutes,
        )))],
        None,
        &[],
        &[],
    )
    .unwrap_err();
    assert!(
        matches!(error, ModelError::MessageMapping(message) if message.contains("request-wide 30m"))
    );
}

#[test]
fn anthropic_maps_explicit_ttl_and_rejects_leading_marker() {
    let request = AnthropicMessagesAdapter::build_request(
        "claude-sonnet-4-6",
        &[user_message(cache_content(Some(CachePointTtl::OneHour)))],
        None,
        &[],
    )
    .unwrap();
    assert_eq!(
        request["messages"][0]["content"][0]["cache_control"],
        json!({"type": "ephemeral", "ttl": "1h"})
    );

    let error = AnthropicMessagesAdapter::build_request(
        "claude-sonnet-4-6",
        &[user_message(vec![
            ContentPart::cache_point(),
            ContentPart::text("x"),
        ])],
        None,
        &[],
    )
    .unwrap_err();
    assert!(
        matches!(error, ModelError::MessageMapping(message) if message.contains("cannot be the first"))
    );
}

#[test]
fn anthropic_automatic_cache_reserves_one_slot_and_keeps_newest_points() {
    let messages = (0..4)
        .map(|index| {
            user_message(vec![
                ContentPart::text(format!("stable-{index}")),
                ContentPart::cache_point_with_ttl(CachePointTtl::OneHour),
            ])
        })
        .collect::<Vec<_>>();
    let settings = ModelSettings {
        provider_options: Some(json!({"anthropic_cache": "1h"})),
        ..ModelSettings::default()
    };
    let request = AnthropicMessagesAdapter::build_request(
        "claude-sonnet-4-6",
        &messages,
        Some(&settings),
        &[],
    )
    .unwrap();

    assert_eq!(
        request["cache_control"],
        json!({"type": "ephemeral", "ttl": "1h"})
    );
    let wire_messages = request["messages"].as_array().unwrap();
    assert!(
        wire_messages[0]["content"][0]
            .get("cache_control")
            .is_none()
    );
    assert!(
        wire_messages[1..]
            .iter()
            .all(|message| message["content"][0].get("cache_control").is_some())
    );
}

#[test]
fn anthropic_rejects_mixed_ttls_in_the_wrong_order() {
    let error = AnthropicMessagesAdapter::build_request(
        "claude-sonnet-4-6",
        &[
            user_message(cache_content(Some(CachePointTtl::FiveMinutes))),
            user_message(cache_content(Some(CachePointTtl::OneHour))),
        ],
        None,
        &[],
    )
    .unwrap_err();
    assert!(
        matches!(error, ModelError::MessageMapping(message) if message.contains("1h cache points must precede all 5m"))
    );

    AnthropicMessagesAdapter::build_request(
        "claude-sonnet-4-6",
        &[
            user_message(cache_content(Some(CachePointTtl::OneHour))),
            user_message(cache_content(Some(CachePointTtl::FiveMinutes))),
        ],
        None,
        &[],
    )
    .unwrap();
}

#[test]
fn anthropic_rejects_automatic_cache_ttl_conflict() {
    let settings = ModelSettings {
        provider_options: Some(json!({"anthropic_cache": "1h"})),
        ..ModelSettings::default()
    };
    let error = AnthropicMessagesAdapter::build_request(
        "claude-sonnet-4-6",
        &[user_message(vec![
            ContentPart::text("stable"),
            ContentPart::cache_point(),
        ])],
        Some(&settings),
        &[],
    )
    .unwrap_err();
    assert!(
        matches!(error, ModelError::MessageMapping(message) if message.contains("conflicts with the explicit 5m"))
    );
}

#[test]
fn anthropic_message_cache_skips_non_cacheable_final_blocks() {
    let mut thinking = ModelResponse::text("");
    thinking.parts = vec![ModelResponsePart::ProviderThinking {
        text: "inspect".to_string(),
        signature: Some("signature".to_string()),
        provider: ProviderPartInfo::new("anthropic"),
    }];
    let settings = ModelSettings {
        provider_options: Some(json!({"anthropic_cache_messages": true})),
        ..ModelSettings::default()
    };
    let request = AnthropicMessagesAdapter::build_request(
        "claude-sonnet-4-6",
        &[
            user_message(vec![ContentPart::text("stable")]),
            ModelMessage::Response(thinking),
        ],
        Some(&settings),
        &[],
    )
    .unwrap();

    assert_eq!(
        request["messages"][0]["content"][0]["cache_control"],
        json!({"type": "ephemeral", "ttl": "5m"})
    );
    assert!(
        request["messages"][1]["content"][0]
            .get("cache_control")
            .is_none()
    );
}

#[test]
fn anthropic_message_cache_preserves_existing_explicit_ttl() {
    let settings = ModelSettings {
        provider_options: Some(json!({"anthropic_cache_messages": true})),
        ..ModelSettings::default()
    };
    let request = AnthropicMessagesAdapter::build_request(
        "claude-sonnet-4-6",
        &[user_message(cache_content(Some(CachePointTtl::OneHour)))],
        Some(&settings),
        &[],
    )
    .unwrap();

    assert_eq!(
        request["messages"][0]["content"][0]["cache_control"],
        json!({"type": "ephemeral", "ttl": "1h"})
    );
}

#[test]
fn anthropic_message_and_automatic_cache_are_mutually_exclusive() {
    let settings = ModelSettings {
        provider_options: Some(json!({
            "anthropic_cache": true,
            "anthropic_cache_messages": true
        })),
        ..ModelSettings::default()
    };
    let error = AnthropicMessagesAdapter::build_request(
        "claude-sonnet-4-6",
        &[user_message(vec![ContentPart::text("hello")])],
        Some(&settings),
        &[],
    )
    .unwrap_err();
    assert!(
        matches!(error, ModelError::MessageMapping(message) if message.contains("cannot both"))
    );
}
