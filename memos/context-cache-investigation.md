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
- Guidance is inserted after control parts and the existing static instruction prefix, before dynamic instruction material and before conversation/context prompt parts.
- Environment and runtime context remain SDK context prompts, not instruction/system material.
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
- Runtime context is injected by the SDK `RuntimeContextCapability` only for the current provider request, as a context `UserPrompt` part after the latest user/environment/handoff context. Tool-return/retry control parts remain in the control prefix.
- Environment context insertion in `crates/starweaver-agent/src/bundles/environment/handle.rs` keeps environment context after the first user prompt, is skipped for tool-return/retry requests unless forced, and is not treated as instruction/system material.
- The resulting user-facing request order is:
  1. provider/control continuation parts when present,
  2. stable system/static instructions,
  3. CLI guidance and other static instruction material,
  4. toolset/dynamic instruction material,
  5. user prompt / retry / tool continuation payload,
  6. initial environment context on the first user-facing request,
  7. current runtime context on the provider request.

Regression coverage:

- `builder_persists_environment_and_runtime_context_for_prefix_stability` asserts the first request order is user prompt, environment context, then runtime context; it also asserts environment context is durable initial context while runtime context is not persisted.
- `multi_run_session_preserves_previous_model_request_prefix` asserts the second OpenAI Responses request preserves the first request's input prefix while adding only the current runtime context.

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
cargo test -p starweaver-agent builder_persists_environment_and_runtime_context_for_prefix_stability
cargo test -p starweaver-agent multi_run_session_preserves_previous_model_request_prefix
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

## Corrected OpenAI Prompt Cache Routing Follow-up

A later audit corrected an important interpretation error: OpenAI `store=false` is not a prompt-cache disable switch. It only controls response/conversation storage. Prompt caching remains automatic for sufficiently long prompts and is measured through `usage.input_tokens_details.cached_tokens`, which Starweaver maps to `Usage.cache_read_tokens`.

### Real TUI session evidence

The user-reported TUI session `session_143a9ff4-285b-4fe7-ad79-b1b291bbac44` was inspected from the local Starweaver SQLite store. The final state contained 22 history messages and 11 model responses. Aggregate usage was:

- `input_tokens = 248,480`
- `cache_read_tokens = 150,016`
- aggregate cache hit rate about `60.37%`

Per response cached tokens were:

| Response | Input tokens | Cached tokens | Hit rate |
| -------: | -----------: | ------------: | -------: |
|        1 |       18,790 |             0 |     0.0% |
|        2 |       19,853 |        16,384 |    82.5% |
|        3 |       21,362 |        16,384 |    76.7% |
|        4 |       26,557 |        16,384 |    61.7% |
|        5 |       22,144 |         2,560 |    11.6% |
|        6 |       23,542 |        16,384 |    69.6% |
|        7 |       22,550 |        16,384 |    72.7% |
|        8 |       22,260 |        16,384 |    73.6% |
|        9 |       24,673 |        16,384 |    66.4% |
|       10 |       23,206 |        16,384 |    70.6% |
|       11 |       23,543 |        16,384 |    69.6% |

The low aggregate rate is mostly explained by the cold first request plus response 5 only receiving a 2,560-token cache hit. Responses after that returned to the same 16,384-token cached prefix, so the stable prefix was still cacheable.

The same session also persisted OpenAI provider reasoning replay evidence for responses 1 through 9: each had a `provider_thinking` part with an OpenAI reasoning item id and encrypted content/signature. Current request construction maps these parts back to OpenAI Responses `type: "reasoning"` replay items and requests `include: ["reasoning.encrypted_content"]` when thinking is enabled. Therefore the 60% aggregate cache rate is not explained by total absence of reasoning replay.

### Root cause refinement

The corrected root-cause assessment is:

- `store=false` did not disable prompt caching.
- Starweaver usage accounting for OpenAI cached tokens was already correct.
- Current OpenAI Responses history reconstruction preserves encrypted reasoning replay items when available.
- The anomalous 2,560-token cache hit is most consistent with OpenAI best-effort cache routing or cache-shard instability, not with unstable top-level instructions or missing reasoning replay.
- OpenAI prompt caching is exact-prefix and routing-sensitive. The provider routes by a prefix hash, and `prompt_cache_key` can improve routing stickiness but does not guarantee a cache hit.

### Implemented routing hardening

OpenAI-specific request finalization now supports prompt-cache routing without moving session identity into generic headers:

- `crates/starweaver-model/src/providers/client/request_options.rs` finalizes OpenAI Chat Completions and OpenAI Responses HTTP bodies after all settings, request params, and HTTP config `extra_body` values have been merged.
- For OpenAI GPT-family model names, if the final request body does not already contain `prompt_cache_key`, Starweaver derives a stable key from runtime metadata `starweaver.session_id` or `cli.session_id` as `sw_<session-id>`, truncated to the OpenAI key length budget.
- Explicit `prompt_cache_key` and `prompt_cache_retention` values from `ModelSettings.extra_body`, request params, HTTP config `extra_body`, or metadata override the derived session key.
- Internal Starweaver/OpenAI replay aliases such as `openai_include_encrypted_reasoning` are stripped from the final OpenAI Responses body after all body overlays, so they cannot leak through HTTP config `extra_body`.
- Codex OAuth Responses requests are excluded from the automatic derived key path because Codex has its own body patching and policy constraints.
- OpenAI-compatible non-OpenAI model names such as `mimo-v2.5-pro` do not receive an automatic derived `prompt_cache_key`; callers can still set explicit provider body fields if their gateway supports them.

Regression coverage in `crates/starweaver-model/tests/request_parameters.rs` asserts:

- OpenAI Responses derives `prompt_cache_key` from session metadata.
- OpenAI Chat Completions derives `prompt_cache_key` from session metadata.
- Explicit request-level and config-level `prompt_cache_key` values are preserved.
- `prompt_cache_retention` can be forwarded explicitly.
- OpenAI-compatible non-GPT model names do not receive an automatic session-derived key.
- OpenAI Responses replay aliases are stripped from the final body.

### Real validation after routing hardening

A fresh real session using the configured `~/.starweaver/config.toml` default profile (`homelab@openai-responses:gpt-5.5` with `openai_responses_high`) completed four headless turns successfully, proving the current gateway accepts the final request shape with `prompt_cache_key`.

Validation session: `session_0cfe24ef-482f-4e56-ae21-d36c705c406b`.

Observed usage:

| Turn | Input tokens | Cached tokens | Hit rate |
| ---: | -----------: | ------------: | -------: |
|    1 |       18,603 |         2,560 |    13.8% |
|    2 |       18,659 |        16,384 |    87.8% |
|    3 |       18,692 |        16,384 |    87.7% |
|    4 |       18,725 |        18,432 |    98.4% |

This does not make prompt caching deterministic, because OpenAI caching remains best-effort and load-balanced. It does, however, align Starweaver with OpenAI's documented cache-routing control and removes the avoidable dependence on implicit routing for multi-turn CLI sessions.
