# Starweaver Implementation TODO

This memo is the detailed working roadmap for implementing the architecture in `spec/`. It replaces the earlier SDK implementation roadmap and tracks landed work, missing replay coverage, Pydantic AI feature coverage, ya-agent-sdk integration, SessionStore readiness, observability, MCP direction, and validation gates.

## Current Validation Commands

Use these commands while executing TODO items:

```bash
make replay-check
make fmt-check
make check
make test
python3 scripts/check-docs-examples.py
make ci
```

`make replay-check` is the focused model compatibility gate:

```bash
cargo test -p starweaver-model --test fixture_schema --test replay --test replay_tooling --test request_parameters --test stream_replay --locked
```

## Landed Replay Coverage

Current fixture-driven replay coverage:

| Provider family   | Fixture                          | Status |
| ----------------- | -------------------------------- | ------ |
| OpenAI Chat       | text response                    | landed |
| OpenAI Chat       | tool call response               | landed |
| OpenAI Chat       | tool return history              | landed |
| OpenAI Responses  | text response                    | landed |
| OpenAI Responses  | function call response           | landed |
| OpenAI Responses  | native web search request        | landed |
| OpenAI Responses  | native MCP request               | landed |
| Anthropic         | text response                    | landed |
| Anthropic         | tool use response                | landed |
| Anthropic         | tool result history              | landed |
| Gemini            | text response                    | landed |
| Gemini            | function call response           | landed |
| Gemini            | function response history        | landed |
| Bedrock           | text response                    | landed |
| Bedrock           | tool use response                | landed |
| Bedrock           | tool result history              | landed |
| Model parameters  | serialization round trip         | landed |
| Model settings    | merge precedence                 | landed |
| Model profiles    | provider capability contracts    | landed |
| Structured output | OpenAI Responses request mapping | landed |
| Structured output | Gemini request mapping           | landed |

Current replay test count: 24 in `crates/starweaver-model/tests/replay.rs` plus 1 replay tooling test in `crates/starweaver-model/tests/replay_tooling.rs` plus 6 in `crates/starweaver-model/tests/request_parameters.rs` plus 4 in `crates/starweaver-model/tests/stream_replay.rs` plus 1 fixture schema validation test in `crates/starweaver-model/tests/fixture_schema.rs`.

## Unmigrated Replay TODO

Provider replay items to migrate from Pydantic AI-style coverage:

### OpenAI Chat

- structured output request fixture through `response_format` — landed
- JSON object mode fixture — landed
- tool choice fixture: auto, none, required, named tool — landed
- parallel tool calls setting fixture — landed
- refusal/content-filter response fixture — landed
- malformed choices fixture — landed
- streaming text delta fixture — landed
- streaming tool-call argument delta fixture — landed
- usage-at-stream-end fixture — landed
- multimodal user input fixture — landed

### OpenAI Responses

- structured output response fixture — landed
- reasoning item fixture — landed
- thinking/summary item fixture — landed
- native web search response fixture — landed
- native MCP call/approval response fixture — landed
- file/image output fixture — landed
- tool choice fixture — landed
- provider refusal fixture — landed
- streaming output text delta fixture — landed
- streaming function-call delta fixture — landed
- status error fixture — landed

### Anthropic Messages

- thinking block fixture — landed
- thinking signature fixture — landed
- tool use with text preamble fixture — landed
- tool result error fixture — landed
- image input fixture — landed
- cache control/provider metadata fixture — landed
- max token stop fixture — landed
- refusal/safety-style fixture — landed
- stream delta fixture — landed

### Gemini generateContent

- safety block fixture — landed
- finish reason safety and max token fixtures — landed
- function call with missing id fixture — landed
- tool config / function calling mode fixture — landed
- code execution native tool fixture — landed
- Google search native tool fixture — landed
- multimodal input fixture — landed
- stream delta fixture — landed
- malformed candidate fixture — landed

### Bedrock Converse

- strict tool call fixture — landed
- tool result error fixture — landed
- max token stop fixture — landed
- content block variants fixture — landed
- Converse additional model response fields fixture — landed
- provider status error fixture — landed
- stream delta fixture — landed
- SigV4/gateway metadata fixture — landed

### Cross-provider

- cassette import utility — landed
- cassette scrub utility — landed
- fixture schema validator — landed
- snapshot summary generator — landed
- provider error and retry fixtures — landed
- request parameter merge precedence across defaults, agent, run, and protocol client — landed
- model alias/profile resolution fixtures — landed
- native tool serialization for OpenAI Responses and Gemini native tool types — landed

## Pydantic AI Core Coverage TODO

The core layer should cover the important concepts documented by Pydantic AI.

| Pydantic AI docs area | Starweaver target                                   | Status  | Next work                                                                                                           |
| --------------------- | --------------------------------------------------- | ------- | ------------------------------------------------------------------------------------------------------------------- |
| Agents                | runtime agent and SDK builder                       | partial | align run APIs, iter/graph inspection, stream events                                                                |
| Dependencies          | `AgentContext` typed/named dependencies             | partial | dependency-aware hooks and output validators docs/tests                                                             |
| Output                | schemas, typed output, validators, output functions | partial | SDK `OutputPolicy` ergonomics                                                                                       |
| Capabilities          | capability bundles                                  | partial | configuration loading and ordered hooks                                                                             |
| Hooks                 | lifecycle hooks                                     | partial | complete hook taxonomy and event evidence                                                                           |
| Agent Specs           | serializable app/subagent specs                     | partial | agent spec loader and validation                                                                                    |
| Message History       | canonical messages and processors                   | partial | docs parity and additional processors                                                                               |
| Direct                | direct model/tool execution                         | planned | direct run APIs over model/tools                                                                                    |
| Models overview       | model adapters/profiles                             | partial | more profiles and provider aliases                                                                                  |
| OpenAI                | Chat/Responses support                              | partial | finish replay TODOs and docs                                                                                        |
| Anthropic             | Messages support                                    | partial | thinking and stream replay                                                                                          |
| Google/Gemini         | generateContent support                             | partial | native tools and safety replay                                                                                      |
| Bedrock               | Converse support                                    | partial | strict tools and gateway docs                                                                                       |
| Tools                 | function tools                                      | partial | advanced schema/function signature ergonomics                                                                       |
| Advanced tools        | retries, prepare, context                           | partial | complete docs and test matrix                                                                                       |
| Toolsets              | toolset composition                                 | partial | dynamic/live toolsets and search                                                                                    |
| Deferred tools        | control-flow metadata                               | partial | durable approval/deferred resume                                                                                    |
| Native tools          | provider native tools                               | partial | response parsing and more request fixtures                                                                          |
| Common tools          | first-party bundles                                 | planned | filesystem, shell, search, media, task, notes                                                                       |
| Third-party tools     | external toolsets                                   | planned | proxy and MCP integration                                                                                           |
| Input                 | multimodal input                                    | partial | canonical media input and replay fixtures                                                                           |
| Thinking              | thinking parts                                      | partial | provider replay and stream handling                                                                                 |
| Retries               | model/tool/output retries                           | partial | provider retry fixtures and SDK policy                                                                              |
| Extensibility         | custom models/tools/hooks                           | partial | public extension docs                                                                                               |
| Multi-agent patterns  | subagents                                           | partial | unified delegation and inherited tools                                                                              |
| Web/UI                | service stream adapters                             | planned | SSE and CLI renderer tests                                                                                          |
| Observability         | OTel GenAI spans and Langfuse-friendly OTLP export  | planned | trace propagation and span snapshot tests                                                                           |
| Embeddings            | embeddings APIs                                     | planned | defer until core agent loop stable                                                                                  |
| Testing               | test models/request guard                           | partial | replay fixture tooling and snapshots                                                                                |
| MCP                   | static foundations                                  | partial | official `rmcp` live client, transports, resources, prompts, sampling, roots, notifications, and long-running tasks |

## ya-agent-sdk Integration TODO

Reference modules and Starweaver targets:

### Agents

- migrate compaction behavior into history processors and context state
- migrate guards into capability hooks and policy presets
- migrate lifecycle extensions into ordered runtime hooks
- migrate stream cancel/resume tests into durable stream tests
- migrate streamer behavior into `AgentSession::run_stream` and service streams
- migrate usage snapshot behavior into context/exported evidence tests

### Context

- complete message bus parity
- complete note tool parity
- add task manager state/tool parity
- define context serialization versioning
- define dependency rehydration contract
- add trace context export/restore
- keep `StateStore` domains ready for ya-claw-style `SessionStore` projection

### Filters as Capabilities

- auto-load files capability
- background shell capability
- bus message capability
- cold start capability
- environment instructions capability
- handoff capability
- image/media upload capability
- model switch capability
- reasoning normalize capability
- runtime instructions capability
- system prompt capability
- tool args capability

### Environment

- design and implement `EnvironmentProvider`
- local provider
- process provider
- sandbox provider
- composite provider
- virtual file operator
- shell sandbox integration
- background process state export/restore
- environment state domain in `AgentContext`
- environment provider ids stored in execution records and trace attributes

### Toolsets

- base toolset parity
- instruction grouping parity
- skill toolsets
- tool search
- tool proxy
- official `rmcp` live MCP client wrapper
- MCP stdio and streamable HTTP deterministic tests
- deterministic eval-style tests for tool search

### Subagents

- complete `SubagentSpec` frontmatter fields
- subagent factory and builtin registry
- unified delegation tool
- inherited tool policy
- required vs optional tools
- lifecycle event propagation
- nested delegation guardrails
- durable subagent polling extension
- nested OpenTelemetry span propagation

### Media

- canonical media input/output model
- image compression capability
- media upload capability
- file/resource references through environment provider

### Observability

- accept external root trace or parent span context in SDK/session run APIs
- create OTel GenAI `invoke_agent` spans for agent loops
- create OTel GenAI `inference` spans for model requests
- create OTel GenAI `execute_tool` spans for tool calls
- recursively nest subagent spans under parent agent spans
- map usage and model settings into OTel GenAI attributes
- store trace id and span id on execution records
- add compact run trace projection for session tools and UI
- add Langfuse-friendly OTLP metadata adapter
- add redaction/truncation policy for content attributes

### Config and Presets

- SDK config model
- model/provider presets
- tool bundle presets
- approval presets
- environment presets
- project/global config loading

## Architecture Implementation Order

### Batch A: Replay Completion and CI

Status: current batch.

- maintain `make replay-check`
- keep CI replay check before full tests
- add fixture schema validation
- add missing replay categories from unmigrated replay TODO
- add cassette import/scrub tooling

### Batch B: Core Agent Loop Solidification

- fill Pydantic AI feature coverage gaps in runtime loop
- add graph/iter inspection API
- complete stream event model for provider deltas
- harden output policy and validator retry semantics
- complete direct run APIs
- extend checkpoint shape for resume

### Batch C: SDK Ergonomics

- design `OutputPolicy`
- add SDK presets
- expand `AgentSession`
- improve public re-exports
- add agent spec loader
- update user-facing docs

### Batch D: Environment Provider

- implement trait shapes after spec review
- add virtual provider first
- add local provider
- add shell provider fake
- add state export/restore
- bind filesystem and shell tools through capabilities

### Batch E: First-Party Tool Bundles

- filesystem bundle
- shell bundle
- note/task bundle
- search/web bundle
- media/resource bundle
- skill bundle
- tool search/proxy bundle

### Batch F: Subagents, Skills, and Observability

- inherited tool policy
- unified delegation tool
- lifecycle failure propagation
- nested delegation guardrails
- skill parser and registry
- durable subagent extension points
- trace parent propagation for subagents
- OTel GenAI span snapshots for model/tool/subagent paths

### Batch G: Durable Service Runtime

- ya-claw-inspired `SessionStore` contract
- session storage contract
- checkpoint store
- event replay
- approval/deferred resume
- environment provider restoration
- trace id/span id persistence
- compact run trace projection
- SSE stream replay

### Batch G2: Platform Adapter Layer

- A2A adapter over service/session contracts
- AGUI adapter over service/session/event contracts
- use Pydantic AI A2A and AGUI demos as reference adapters
- adapter conformance tests after core SDK and service runtime stabilize

### Batch H: CLI Product

- local run
- model/profile config
- environment binding
- session inspect/resume
- approval prompts
- diagnostics
- replay-check command

## Documentation and Project UX TODO

Project-facing surfaces to revise after spec review:

- `README.md`: concise product introduction, docs site link, quick start, status, and validation commands
- `CONTRIBUTING.md`: contributor workflow, spec workflow, replay workflow, docs workflow, and external protocol boundaries
- `AGENTS.md`: repository memory aligned with current crate responsibilities, planned layers, validation gates, and design decisions
- `docs/SUMMARY.md` and `docs/nav.json`: layered information architecture from overview to core concepts to SDK tasks to operations
- `book.toml` and docs deployment metadata: site title, canonical URL, sitemap, robots, and edit links

Docs site surfaces to add or revise after spec review:

- layered docs UX from overview to core concepts to SDK tasks to operations
- site navigation that clearly separates core foundation, SDK layer, environment/tool bundles, durability, observability, and CLI
- docs landing page that routes users by goal: build an agent, add tools, test providers, persist sessions, inspect traces, and operate the CLI

Core docs to add or revise after spec review:

- `docs/core-agent-loop.md`
- `docs/models.md`
- `docs/tools.md`
- `docs/output.md`
- `docs/capabilities.md`
- `docs/message-history.md`
- `docs/testing.md`

SDK docs to add or revise after spec review:

- `docs/sdk-app.md`
- `docs/environment.md`
- `docs/filesystem-tools.md`
- `docs/shell-tools.md`
- `docs/subagents.md`
- `docs/skills.md`
- `docs/durability.md`
- `docs/cli.md`
- `docs/observability.md`

## Open Design Questions

- exact `EnvironmentProvider` trait split: one trait with optional capabilities or separate file/shell/resource provider traits
- state domain schema for environment resources and background shell handles
- checkpoint granularity for model streaming deltas
- unified delegation schema for subagent selection and task metadata
- typed output ergonomics in Rust with manageable generic complexity
- skill package format and precedence across project/global/builtin scopes
- durable resume semantics for already-started external shell processes
- exact trace parent API shape across `AgentApp`, `AgentSession`, runtime, and model requests
- Langfuse extension attribute names and redaction defaults
- compact run trace projection schema for session tools

## Review Checklist

Before implementation resumes:

- review `spec/core/*`
- review `spec/sdk/*`
- review `spec/ops/*`
- confirm `EnvironmentProvider` direction
- confirm SDK/core crate split
- confirm replay TODO priority order
- confirm docs plan
- confirm official `rmcp` MCP integration plan
- confirm OTel/Langfuse observability plan
- confirm A2A/AGUI platform adapter timing
