# Pydantic AI Feature Map

This spec maps Pydantic AI's documented feature surface to Starweaver's core architecture. The goal is feature awareness before implementation sequencing: every important concept has an owner, validation path, and planned Rust-native shape.

## Coverage Principles

- Pydantic AI concepts become Rust-native contracts in core crates.
- Provider behavior is validated through replay fixtures and request-parameter tests.
- Agent-loop semantics stay deterministic and checkpoint-friendly.
- SDK ergonomics wrap core contracts after runtime behavior is stable.

## Core Concept Map

| Pydantic AI feature | Starweaver owner                                               | Architecture target                                                                      | Validation path                                        |
| ------------------- | -------------------------------------------------------------- | ---------------------------------------------------------------------------------------- | ------------------------------------------------------ |
| Agents              | `starweaver-runtime`, `starweaver-agent`                       | reusable runtime agent, SDK builder, app/session facade                                  | builder, runtime, session tests                        |
| Dependencies        | `starweaver-context`                                           | typed and named dependencies available to tools, hooks, validators, dynamic instructions | context and runtime dependency tests                   |
| Output              | `starweaver-runtime`, `starweaver-model`                       | text, JSON schema, typed parse, validators, output functions                             | structured output, typed output, output function tests |
| Capabilities        | `starweaver-runtime`, `starweaver-agent`                       | composable tools, hooks, instructions, settings, validators, processors, usage limits    | capability bundle tests                                |
| Hooks               | `starweaver-runtime`                                           | ordered lifecycle hooks with context event evidence                                      | hook and capability tests                              |
| Agent Specs         | `starweaver-core`, `starweaver-agent`                          | serializable agent/subagent specs and loaders                                            | parser and loader tests                                |
| Message History     | `starweaver-model`, `starweaver-runtime`, `starweaver-context` | canonical history, continuation, processors, reinjection                                 | history tests                                          |
| Direct              | `starweaver-model`, `starweaver-tools`                         | direct model and tool invocation APIs for advanced composition                           | direct API tests                                       |
| Testing             | `starweaver-model`, all crates                                 | deterministic models, request guard, replay fixtures, docs examples                      | `make replay-check`, `make ci`                         |

## Model and Provider Map

| Pydantic AI provider topic | Starweaver owner                                             | Architecture target                                              | Replay status                           |
| -------------------------- | ------------------------------------------------------------ | ---------------------------------------------------------------- | --------------------------------------- |
| Models overview            | `starweaver-model`                                           | profiles, settings, aliases, protocol clients                    | partial                                 |
| OpenAI                     | `starweaver-model`                                           | Chat Completions and Responses families                          | broad fixture base landed               |
| Anthropic                  | `starweaver-model`                                           | Messages protocol, tools, thinking                               | text/tools landed, thinking planned     |
| Google/Gemini              | `starweaver-model`                                           | generateContent, tools, safety, native tools                     | text/tools landed, safety planned       |
| Bedrock                    | `starweaver-model`                                           | Converse protocol, tools, gateway patterns                       | text/tools landed, strict tools planned |
| Gateway                    | `starweaver-model`                                           | endpoint override, headers, extra body, alias registry           | client tests landed                     |
| Native tools               | `starweaver-model`, `starweaver-tools`                       | provider-native request definitions and canonical response parts | request fixtures partial                |
| Thinking                   | `starweaver-model`, `starweaver-runtime`                     | thinking parts and streaming deltas                              | canonical part partial                  |
| Retries                    | `starweaver-model`, `starweaver-runtime`, `starweaver-tools` | provider retry, tool retry, output retry                         | partial                                 |

## Tools and Toolsets Map

| Pydantic AI feature | Starweaver owner                                            | Architecture target                                                | Validation path                |
| ------------------- | ----------------------------------------------------------- | ------------------------------------------------------------------ | ------------------------------ |
| Function tools      | `starweaver-tools`                                          | schema, context, dynamic execution                                 | tool tests                     |
| Advanced tools      | `starweaver-tools`, `starweaver-runtime`                    | retries, metadata, prepare-tools, context access                   | tool retry and prepare tests   |
| Toolsets            | `starweaver-tools`, `starweaver-agent`                      | static, prefixed, dynamic, environment-backed, MCP-backed toolsets | toolset tests                  |
| Deferred tools      | `starweaver-tools`, `starweaver-runtime`, `starweaver-claw` | structured deferral metadata and durable resume                    | control-flow and service tests |
| Common tools        | `starweaver-agent`, `starweaver-environment`                | first-party filesystem, shell, search, media, task, note tools     | environment and bundle tests   |
| Third-party tools   | `starweaver-tools`, `starweaver-agent`                      | proxy, remote, MCP, external registries                            | proxy and MCP tests            |
| MCP                 | `starweaver-tools`, `starweaver-agent`                      | live client, tool discovery, resources, prompts, sampling          | MCP live client tests          |

## Advanced Feature Map

| Pydantic AI feature  | Starweaver architecture                              | Planned evidence                                 |
| -------------------- | ---------------------------------------------------- | ------------------------------------------------ |
| Input                | canonical text/media/file parts                      | multimodal replay fixtures and docs examples     |
| Streaming            | model deltas, runtime records, service stream replay | stream fixture tests and SSE tests               |
| Graph iteration      | runtime graph state inspection                       | graph/iter API tests                             |
| Multi-agent patterns | subagent specs, registry, unified delegation         | subagent lifecycle and inherited tool tests      |
| Web/UI               | service stream adapters and CLI renderer             | SSE, AGUI, and CLI tests                         |
| Embeddings           | future model-adjacent crate or module                | postponed until core agent loop review completes |
| Evals                | future evaluation layer                              | postponed until SDK surface stabilizes           |

## Agent Loop Requirements From Feature Map

The core loop must support:

- reusable agent construction
- per-run and default model settings
- dynamic instructions with dependencies
- model history continuation
- tool loop continuation
- output function termination
- structured output retry
- provider-neutral stream events
- graph state inspection
- checkpoint emission at model, tool, output, retry, suspend, and completion boundaries

## Review Gate

Before implementing the next core batch, verify each feature row has one of these statuses in `memos/implementation-todo.md`:

- landed
- partial
- planned
- postponed with owner and revisit trigger
