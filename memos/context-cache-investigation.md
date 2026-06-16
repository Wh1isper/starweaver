# Context and Prompt Cache Investigation

## Problem

Starweaver CLI showed significantly higher initial context size and a lower prompt-cache hit rate than YAACLI. The investigation focused on provider instruction semantics, message-history persistence, transient context placement, and repeated model-facing payload sources.

## Reference Semantics

### Pydantic AI implementation details

Relevant implementation behavior from Pydantic AI and its providers:

- `InstructionPart.sorted()` keeps static instructions before dynamic instructions. This creates a stable prefix for provider prompt caching while still allowing dynamic instructions to change per request.
- Anthropic and Bedrock can place cache points at the static/dynamic instruction boundary when cache settings are enabled. A volatile instruction placed before stable material prevents downstream stable content from being reused.
- OpenAI Responses maps current-agent instructions to the top-level `instructions` field and can reduce repeated history payload with `previous_response_id` / provider conversation features.
- OpenAI Chat Completions and Gemini preserve request message ordering but do not provide the same inline cache-point semantics, so request ordering is still the main cache lever.
- Current-agent/current-run instructions should not become durable user message history unless they are actual user intent.

### YAACLI / ya-agent-sdk pattern

The YAACLI/ya-agent-sdk implementation follows the same practical rules:

- Stable system/developer guidance is canonicalized and kept separate from user conversation history.
- Runtime context and environment context are transient processors, not durable user prompt content.
- Headless `AGENTS.md` / `RULES.md` content is treated as injected context with identifiable tags so compaction and continuation do not repeatedly accumulate it as user text.
- Toolset instructions are delegated to Pydantic AI instruction semantics.
- Anthropic prompt-cache settings are enabled by default for instruction/tool/message boundaries.
- Large external tool surfaces are kept stable through proxy/discovery patterns rather than eagerly expanding unstable details into every request.

## Starweaver Findings and Fixes

### 1. Project guidance and user rules were persisted as user prompt content

Before this change:

- `crates/starweaver-cli/src/service.rs` loaded `AGENTS.md` and global `RULES.md` in `append_guidance_files`.
- The files were appended to `PromptInput.extra_text_parts`.
- `CliPromptContentAdapter` rewrote the first user prompt before the request was pushed to runtime `message_history`.

Effect:

- `AGENTS.md` and `RULES.md` were stored in message history as user content.
- On every continued or resumed CLI run, the same guidance was appended again to the new user prompt.
- Context grew roughly by the size of project guidance and user rules per turn.
- This violated the instruction/history split used by Pydantic AI and YAACLI.

Implemented fix:

- Added `PromptInput.guidance_text_parts` in `crates/starweaver-cli/src/prompt_input.rs`.
- Changed `append_guidance_files` to populate transient guidance instead of persisted extra user text.
- Added `CliGuidanceAdapter` in `crates/starweaver-cli/src/runner.rs`.
- The adapter injects guidance as current-request `ModelRequestPart::Instruction` with:
  - `starweaver_instruction_origin = "cli_guidance"`
  - `starweaver_instruction_dynamic = false`
- Guidance is inserted after control parts and existing static instruction prefix, before dynamic/environment/runtime context and before the user prompt.
- `CliPromptContentAdapter` remains responsible only for actual prompt content parts: user text attachments and explicit extra text parts.

Regression coverage:

- `guidance_files_append_project_guidance_and_user_rules_as_transient_guidance` asserts guidance files are no longer added to `extra_text_parts`.
- `cli_guidance_adapter_injects_guidance_as_static_request_instructions` asserts provider-facing ordering and metadata.
- `cli_guidance_is_model_facing_but_not_persisted_in_session_history` asserts guidance appears in each provider request exactly once while exported session message history contains no guidance text.

### 2. Runtime context was ordered before larger stable environment context

Before this change:

- `EnvironmentContextCapability` injected environment context.
- `Agent::inject_runtime_context` then inserted runtime context at an early instruction insertion index.
- Existing tests expected runtime context before environment context.

Effect:

- Runtime context contains highly volatile fields such as current time, elapsed time, token usage, and active task state.
- Putting it before environment context caused the following environment file tree and other slow-changing context to miss provider prompt caches every request.

Implemented fix:

- Added `request_instruction_end_index` in `crates/starweaver-runtime/src/agent/runtime_helpers/request_parts.rs`.
- Runtime context now inserts after the existing instruction/context prefix and before the user prompt or continuation control payload.
- Environment context insertion in `crates/starweaver-agent/src/bundles/environment/handle.rs` keeps environment context after stable instructions but before the user prompt.
- The resulting order is:
  1. provider/control continuation parts,
  2. stable system/static instructions,
  3. CLI guidance,
  4. toolset/dynamic/environment instructions,
  5. runtime context,
  6. user prompt / retry / tool continuation.

Regression coverage:

- `builder_injects_environment_context_before_runtime_context_and_user_prompt` asserts environment context precedes runtime context and both precede the user prompt.

### 3. Local environment context duplicated nested allowed roots

Before this change:

- `LocalEnvironmentProvider::render_environment_context` renders a file-tree block for every allowed path.
- Allowed paths can include nested roots, for example workspace plus a workspace subdirectory, or global config plus nested skill/subagent directories.

Effect:

- The access policy was correct, but the model-facing environment context could repeat visible nested directories.
- Duplicate file-tree blocks increased initial context and lowered cache efficiency.

Implemented fix:

- Added `context_file_tree_roots` and `path_is_visible_under_root` in `crates/starweaver-environment/src/local_provider.rs`.
- Filesystem access remains unchanged: all allowed paths are still kept for policy checks.
- Model-facing file-tree rendering skips nested roots only when the parent rendered tree should already cover that nested root.
- The dedupe check is intentionally conservative:
  - hidden/skipped path components keep the nested root,
  - roots at or beyond the parent file-tree depth keep the nested root,
  - roots ignored by the parent root `.gitignore` keep the nested root.
- This prevents important explicit roots such as skills/subagents under config directories, gitignored generated directories, or deep nested roots from disappearing from context.

Regression coverage:

- `local_context_file_tree_deduplicates_visible_nested_allowed_roots`
- `local_context_file_tree_keeps_hidden_nested_allowed_roots`
- `local_context_file_tree_keeps_gitignored_nested_allowed_roots`
- `local_context_file_tree_keeps_deep_nested_allowed_roots`
- Existing local context file-tree tests continue to pass.

## Verification

Commands run successfully:

```bash
cargo fmt --all --check
cargo test -p starweaver-cli runner::tests
cargo test -p starweaver-cli service::tests
cargo test -p starweaver-agent builder_injects_environment_context_before_runtime_context_and_user_prompt
cargo test -p starweaver-environment local_context_file_tree
cargo check --workspace
make fmt-check && make check && make test
```

A focused review also checked the cache/context changes; the main actionable item was to make nested-root dedupe conservative for gitignored and deep explicit roots, which is now covered by regression tests.

## Remaining Follow-ups

- Provider-side conversation reuse, especially OpenAI Responses `previous_response_id` / conversation support, can further reduce repeated history payload but is separate from this immediate cache-ordering fix.
- First-party tool surfaces may still be large. A smaller default surface or discovery/proxy-first pattern would further reduce initial context.
- Anthropic cache settings already include instruction/tool/message cache options in model presets; no provider preset change was required in this pass.

## Real OpenAI Responses Cache Validation

A follow-up investigation used the configured real provider/model from `~/.starweaver/config.toml` without printing secrets. The active profile targeted OpenAI Responses with `gpt-5.5` through the configured provider gateway. Request bodies were inspected with a temporary local dump hook during the investigation and the hook was removed after validation.

### Baseline real-request observations

Initial real requests showed low prompt-cache reuse despite stable user prompts:

- Fresh single-turn sample: `input_tokens = 18475`, `cache_read_tokens = 6656`, cache hit rate about `36%`.
- Starweaver's usage parser correctly maps OpenAI Responses `usage.input_tokens_details.cached_tokens` to `Usage.cache_read_tokens`.
- The low hit rate was not a usage accounting bug.

Request body diffs showed two cache-breaking patterns:

1. The environment context rendered volatile `tmp-directory` before the larger stable file-tree block. Any tmp directory change shifted the stable file tree behind volatile bytes.
2. OpenAI Responses full-history requests placed request-scope dynamic instructions in `input` after accumulated user/assistant history. That made the large tool/runtime/environment instruction block move on every turn, even though the actual conversation history was a stable append-only prefix.

### Implemented real-measurement fixes

Additional fixes from the real API investigation:

- `crates/starweaver-environment/src/context_xml.rs` now renders `<file-trees>` before volatile `<tmp-directory>` and `<tmp-directory-note>`.
- `crates/starweaver-model/src/providers/openai_responses/request.rs` no longer pushes `SystemPrompt` or `Instruction` request parts into Responses `input`.
- `crates/starweaver-model/src/providers/openai_responses/request/instructions.rs` now merges static instructions with the latest request's dynamic instructions into top-level Responses `instructions`.
- OpenAI Responses `input` is reserved for real conversation turns, tool returns, retries, and assistant replay items. This matches the Pydantic AI pattern where instruction parts are separated from conversation input.
- A regression test locks full-history OpenAI Responses behavior so second-turn `input` keeps the first-turn `input` as a stable prefix and contains no runtime-context instruction text.
- A regression test locks environment XML ordering so stable file trees render before the volatile tmp directory.

### Server-side continuation note

The configured provider gateway rejected OpenAI Responses `store=true` with:

```text
Store must be set to false
```

Therefore this pass does not rely on `previous_response_id=auto` / server-side stored continuation for the user's current provider configuration. The cache fix targets full-history prompt-cache stability with `store=false`.

### Final real multi-turn validation

After the OpenAI Responses mapping fix and with `store=false`, a real four-turn session completed with stable request structure:

- `tools` hash stayed stable across all turns.
- `input` lengths were `1 -> 3 -> 5 -> 7`.
- Each later request's `input` preserved the previous conversation as a stable prefix.
- Runtime/environment/toolset instructions were present in top-level `instructions`, not in `input`.

Observed real usage per turn:

| Turn | Input tokens | Cached tokens | Cache hit rate |
| ---: | -----------: | ------------: | -------------: |
|    1 |       18,367 |         6,656 |          36.2% |
|    2 |       18,416 |        17,920 |          97.3% |
|    3 |       18,440 |        17,920 |          97.2% |
|    4 |       18,465 |        17,920 |          97.0% |

The first turn still depends on provider-side cache warmth and routing. The multi-turn behavior that the user reported as unstable is now stable and reaches about `97%` cache reuse on subsequent turns with the current gateway constraints.

## Cross-Provider Cache-Shape Audit

A follow-up review checked the same instability class across all provider request mappers without relying on unavailable external APIs for other models. The audit focused on deterministic request shape rather than provider-side cache counters.

Findings:

- OpenAI Chat Completions keeps system/user/assistant/tool entries in `messages`; the sequence is append-only across turns and tools remain top-level stable definitions.
- OpenAI Responses is fixed as described above: `input` is conversation-only and top-level `instructions` carries static plus current dynamic instructions.
- Anthropic Messages, Gemini generateContent, and Bedrock Converse use top-level system/systemInstruction fields. Before this follow-up, shared normalization and provider system collection could lift dynamic runtime/environment/toolset instructions from every historical request into those top-level system fields. That made volatile context appear before the stable conversation body and could grow or shift across turns.

Implemented follow-up fix, corrected to match Pydantic AI instruction semantics:

- `dynamic` instruction metadata means the instruction may differ across runs or sessions and should define a cache boundary; it does not mean the instruction should be downgraded to user content.
- Profile normalization for `SystemField` / `SystemInstruction` lifts both static and dynamic instruction parts into the provider system/systemInstruction area, preserving static-before-dynamic ordering.
- Anthropic and Bedrock use the static/dynamic instruction boundary to place provider cache points when instruction caching is enabled. Gemini has no equivalent explicit cache point in this mapper, but still keeps instruction parts in `systemInstruction`.
- OpenAI Responses keeps conversation `input` pure and maps static plus current dynamic instructions to top-level `instructions`, matching the Responses API separation used by Pydantic AI.
- OpenAI Chat Completions has no top-level `instructions`, so current instruction parts are emitted as leading system messages before the conversation body.
- Runtime/environment/toolset instructions are injected into the model-facing request only and are not persisted into durable `message_history`; later turns get the current request's instruction parts rather than historical transient copies.
- Regression coverage now exercises all supported protocol families:
  - OpenAI Chat: leading system instructions are current-request material; the conversation body remains append-only.
  - OpenAI Responses: `input` prefix is append-only; old runtime context is not retained in current top-level instructions.
  - Anthropic: dynamic instructions remain in `system`, and cache control lands after the static instruction boundary.
  - Gemini: dynamic instructions remain in `systemInstruction`, while `contents` stays conversation-only.
  - Bedrock: dynamic instructions remain in `system`, and `cachePoint` lands after the static instruction boundary; tool-definition cache points are also covered.

This does not prove provider-side cache counters for APIs that were not available, but it proves the request-shape invariant needed for Pydantic AI-style cache placement: stable instructions precede dynamic instructions, dynamic instruction material is current-request rather than durable history, and provider conversation bodies remain append-only.
