# SDK Host Tool Gap Report

This memo records the executable state of Starweaver host-backed SDK tools for search, scrape, media URL understanding, downloads, and document-conversion workflows.

## Current Direction

The default model-facing web surface should stay compact:

- `search`: normalized web search
- `scrape`: page-to-Markdown extraction

The host-operation bundle currently also exposes `fetch` for compatibility and shared HTTP probing. The long-term default surface can hide `fetch` behind `scrape`, `download`, and media URL probing once migration callers are covered.

Built-in media and download tools remain first-party SDK tools:

- `download`
- `load_media_url`
- `read_image`
- `read_video`
- `read_audio`

PDF, Office, EPUB, and richer document conversion remain skill workflows backed by shell-installed converters such as PyMuPDF4LLM and MarkItDown.

## Reference Findings

### ya-agent-sdk

The ya-agent-sdk reference supplies concrete behavior targets:

- `search(query, num)`: Google Custom Search, Brave Search, Tavily fallback, normalized results.
- `scrape(url)`: Firecrawl, local/MarkItDown fallback, Markdown output, truncation metadata.
- `fetch(url, head_only)`: SSRF-aware HTTP helper, HEAD-to-GET fallback, streaming text truncation, inline image handling with size guards.
- `download(urls, save_dir)`: parallel downloads, UUID filenames, content-type extension inference, provider-scoped writes.
- `load_media_url(url)`: URL validation, MIME/extension detection, model capability checks, provider-ready media/document URL values.
- `read_image`, `read_video`, `read_audio`: fallback understanding agents with parent usage accounting.
- `pdf_convert` and `office_to_markdown`: provider-scoped conversion flows with exported Markdown and assets next to source files.

### Pydantic AI

Useful patterns:

- Toolsets own discovery, call behavior, instructions, setup, lifecycle, and per-step adaptation.
- `RunContext` makes host services injectable and testable.
- Capabilities can add tools, instructions, hooks, and settings as composable middleware.
- Deferred tools and approval wrappers are first-class host-interaction mechanisms.

Starweaver already has matching seams through `Toolset`, `ToolContext`, `AgentContext` typed dependencies, capability bundles, retry metadata, instructions, and deterministic tests.

## Current Starweaver State

| Tool             | Current state                                                                                                      | Remaining depth                                                                                         |
| ---------------- | ------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------- |
| `search`         | executable through `HostSearchClientHandle` or Brave Search env adapter                                            | Google/Tavily optional adapters, richer citations, quota metadata, provider error aggregation           |
| `scrape`         | executable through `HostScrapeClientHandle`, Firecrawl env adapter, Cloudflare seam, or local static-HTML fallback | JavaScript-rendered page adapter depth, document handoff metadata, richer sanitization                  |
| `fetch`          | executable compatibility tool over shared HTTP substrate                                                           | migrate default model-facing use toward `scrape`/`download`; keep adapter behavior for internal callers |
| `download`       | executable text-like downloads into `EnvironmentProvider`                                                          | binary/resource provider extension, streaming checksums, concurrent writes, resumable records           |
| `load_media_url` | executable classification and capability checks for image/video/audio/document/text URLs                           | richer HEAD/GET fallback metadata, provider resource store integration                                  |
| `read_image`     | executable with injectable `HostMediaUnderstandingClientHandle`                                                    | first-party fallback model client, usage accounting integration, fixtures                               |
| `read_video`     | executable with injectable `HostMediaUnderstandingClientHandle`                                                    | first-party fallback model client, S3/resource upload integration, fixtures                             |
| `read_audio`     | executable with injectable `HostMediaUnderstandingClientHandle`                                                    | first-party fallback transcription/understanding client, fixtures                                       |

## Shared HTTP Substrate Requirements

The current shared HTTP helper should continue toward:

- HTTP/HTTPS-only validation.
- Redirect handling with final URL recording.
- Timeouts, bounded response size, bounded chunk size, and content-type sniffing.
- Header redaction in traces and errors.
- Deterministic fake HTTP clients for fixture tests.
- Binary guardrails that return resource-extension requirements when the active environment cannot write binary resources.

## Download and Resource Store Requirements

`download` should grow into a streaming resource writer:

- Write text and binary content through environment/resource provider traits.
- Record original URL, final URL, content type, filename, byte size, checksum, storage path or resource id, adapter, and truncation/download status.
- Support concurrency limits and per-file max size.
- Support stable UUID filenames with safe extension inference.
- Provide deterministic fixtures for text, binary, redirect, content-type mismatch, failed URL, and partial failure cases.

## Media Understanding Requirements

`load_media_url` and fallback media tools should coordinate with model profiles and SDK media processors:

- Detect media category from headers, file extension, and optional lightweight probe.
- Return provider-ready `ContentPart` values when URL media is supported.
- Return precise fallback instructions when native support is unavailable.
- Route large video/audio through S3/resource upload settings when configured.
- Account fallback model usage into the parent `AgentContext`.
- Preserve trace correlation from parent run to fallback media client.

CLI and product-layer gap:

- Agent-level media preflight/upload seams exist, while YAACLI-compatible CLI config for `[media.s3]` still needs import, validation, and upload hook wiring.
- TUI clipboard image support should attach binary media as `ContentPart::Binary` or a provider `ResourceRef`, then run the same SDK media preflight processors.
- Browser/media configuration should distinguish SDK host-operation tool availability from CLI product workflows, so docs state the exact layer that has parity.
- Fallback image/video/audio usage should be merged into the parent usage ledger and exposed through CLI `/cost`, session traces, and replay display events.

## Document Conversion Skill Requirements

Document conversion should stay skill-driven and environment-backed:

- Skill declares required commands/packages and supported formats.
- Tool workflow reads provider-scoped input files and writes exported Markdown/assets next to source files.
- Conversion records page ranges, output files, image extraction, truncation, and converter version.
- CLI and service hosts can install or surface conversion skills through normal skill discovery.

## Focused Validation

```bash
cargo test -p starweaver-agent --test bundles --locked
cargo test -p starweaver-agent --test skills --locked
cargo test -p starweaver-environment --locked
cargo test -p starweaver-model --test media_preflight --locked
cargo test -p starweaver-model --test multimodal_mapping --locked
```
