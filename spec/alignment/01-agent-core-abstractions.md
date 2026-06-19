# Agent Core Abstractions

## Scope

This document lists only remaining core agent abstraction gaps and Rust-native non-goals.

## Provider Replay Status

- Fixture coverage proves existing provider request fixtures remain stable after JSON restore of history, settings, tools, and native tools.
- OpenAI Responses has previous-response, conversation, and compaction-boundary continuation fixtures.
- Anthropic provider-owned thinking signatures are covered by `anthropic_private_thinking_replay_fixture_maps_signature_natively` and `anthropic/provider_thinking_replay.json`.
- No current provider-private replay abstraction gap remains for known adapters. Future provider adapters that expose durable private replay identifiers or continuation payloads must add same-provider fixtures before parity is claimed.

## Rust-Native Decisions

- Multi-output selector semantics remain a product choice; current typed output, output function, and `AgentEndStrategy` APIs cover the adopted behavior.
- Public graph inspection exists through the deterministic graph APIs; live graph iteration, node override, and node hook contexts remain internal unless Starweaver chooses graph control as a stable SDK surface.
- Decorator-like registration is a language-specific syntax pattern; Rust-native builders and typed helpers remain the primary API unless macros remove clear boilerplate without creating a second public API style.

## Acceptance

- Provider replay fixture failures print normalized expected and actual provider requests.
- Future provider-private replay fixtures fail with actionable normalized diffs.
- Any new hook or iterator surface is Starweaver-native and has deterministic runtime tests.
