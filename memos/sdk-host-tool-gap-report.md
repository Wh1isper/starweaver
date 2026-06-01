# SDK Host Tool Gap Report

This memo reports the remaining work to turn Starweaver's host-operation envelopes into executable SDK built-in tools. It focuses on web search/scrape, media URL understanding, and downloads after reviewing ya-agent-sdk and pydantic-ai reference implementations. Document conversion remains tracked here as a skill workflow because the practical implementation relies on shell-installed converters.

## Direction

Starweaver should expose a compact model-facing host tool surface for web work:

- `search`: one general web search tool
- `scrape`: one page-to-Markdown tool

The SDK should keep media understanding, direct URL media loading, and downloads as built-in host-backed tools:

- `download`
- `load_media_url`
- `read_image`
- `read_video`
- `read_audio`

Raw HTTP `fetch` behavior should become an internal adapter for `scrape`, `download`, and media probing. PDF and Office conversion should be provided by document-conversion skills that run shell commands in the active environment.

## Reference findings

### ya-agent-sdk behavior

Reviewed reference files under `packages/ya-agent-sdk/ya_agent_sdk/toolsets/core`:

- `web/search.py`
  - `search(query, num)`: Google Custom Search first, Brave Search second, Tavily third.
  - The image-specific reference tools are useful implementation evidence for URL accessibility validation, while Starweaver's public surface keeps image search under `search` result typing when needed.
- `web/scrape.py`
  - `scrape(url)`: Firecrawl when configured, then MarkItDown fallback.
  - URL verification, Markdown result, 60K truncation, total length, and usage tips.
- `web/fetch.py`
  - `fetch(url, head_only)`: HEAD/GET, fallback from HEAD to GET on 405, SSRF-safe request helpers, streaming text truncation, inline image `BinaryContent` with size limits.
- `web/download.py`
  - `download(urls, save_dir)`: parallel streaming downloads into `file_operator`, UUID filenames, content-type extension inference, size and content-type metadata.
- `web/_http_client.py`
  - Shared safe HTTP helpers: URL validation, redirect validation, request/stream wrappers, and URL accessibility checks.
- `content/load_media_url.py` and `content/_url_helper.py`
  - `load_media_url(url)`: HTTP/HTTPS validation, MIME/extension category detection, model capability checks, provider-ready image/video/audio/document URL values, fallback messages to media understanding tools or download plus conversion.
- `multimodal/image.py`, `multimodal/video.py`, `multimodal/audio.py`
  - `read_image`, `read_video`, `read_audio`: fallback understanding agents for models without matching native media capabilities, with usage accounting into parent context.
- `document/pdf.py`
  - The reference PDF conversion tool provides provider-scoped file read, size gate, private local tempdir, PyMuPDF page count, `pymupdf4llm.to_markdown(write_images=true)`, exported Markdown plus images next to source, and page range metadata.
- `document/office.py`
  - `office_to_markdown(file_path)`: provider-scoped file read, size gate, private local tempdir, MarkItDown conversion with `keep_data_uris`, data-URI image extraction, exported Markdown plus images next to source.

### External documentation evidence

Checked current public documentation for the main candidate adapters:

- MarkItDown: Microsoft describes it as a lightweight Python utility that converts files to Markdown for LLM pipelines. Its documented supported inputs include PDF, PowerPoint, Word, Excel, images, audio, HTML, text formats, ZIP, YouTube URLs, EPUB, and more. Its security guidance recommends sanitizing untrusted inputs, restricting URI schemes and destinations, and using the narrowest conversion API such as `convert_local`, `convert_response`, or `convert_stream`.
- PyMuPDF4LLM: the project documents Markdown, JSON, and TXT extraction from PDFs, layout analysis, multi-column support, image and vector extraction, page selection, OCR support, and `pymupdf4llm.to_markdown("input.pdf")` as the basic API.
- Cloudflare Browser Run: Cloudflare exposes headless Chrome through Quick Actions and browser sessions. The Quick Actions list includes Markdown, screenshots, PDFs, snapshots, links, HTML element scraping, structured JSON, and crawled content, making it a strong later adapter for JavaScript-rendered pages.
- Firecrawl: the scrape endpoint returns Markdown and metadata, handles dynamic and JavaScript-rendered sites, and documents automatic parsing for PDF, DOCX, and other document URLs. It also supports HTML, screenshots, links, JSON, images, audio, and video formats.

### pydantic-ai patterns to preserve

Reviewed pydantic-ai `AbstractToolset` and tool APIs:

- Toolsets own tool discovery, tool calls, per-run setup, per-step adaptation, lifecycle, and instructions.
- Toolset instructions are first-class and can be dynamic per run.
- Tool definitions include validators and retry budgets at the toolset boundary.
- Dependencies are provided through `RunContext`, which makes host services injectable and testable.

Starweaver already has matching foundations: `Toolset`, `ToolContext`, typed `AgentContext` dependencies, capability bundles, tool metadata, retries, instructions, and deterministic tests. Host adapters should use these seams rather than adding provider-specific behavior to the runtime kernel.

## Starweaver current state

Current executable foundations:

- `EnvironmentProvider` supports provider-scoped file read/write/list/glob/grep, policies, resources in state snapshots, virtual provider, and local provider.
- `host_operation_tools()` exposes web/media/download/document tools with typed JSON Schema and descriptions.
- The current host-operation tools return operation envelopes.
- Provider-native web/search tools are represented through `NativeToolDefinition` and replay tests in `starweaver-model`.
- `ToolProxyToolset` gives a separate `search_tools` and `call_tool` surface for tool discovery.

Current structural gap:

- The SDK has public tool schemas, executable host-backed web/media/download adapters, injectable client traits, and deterministic bundle tests.
- Remaining implementation deepening centers on binary/resource write extensions, richer streaming downloads, and first-party fallback media model clients.

## Tool implementation gap count

Target executable SDK built-ins in this area: 7 tools.

| Tool             | Current Starweaver state                                                                                                | Target state                                                                                                                                | Gap status                                    |
| ---------------- | ----------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------- |
| `search`         | Executable via injectable `HostSearchClient` or Brave Search env adapter                                                | Executable general search via `SearchClient`, with Brave Search as the required first adapter and Google/Tavily as optional adapters        | First slice landed                            |
| `scrape`         | Executable via injectable `HostScrapeClient`, Firecrawl env adapter, Cloudflare env seam, or local static-HTML fallback | Executable Markdown scrape via `ScrapeClient`: Firecrawl, Cloudflare Browser Run, local static-HTML fallback, and document-resource handoff | First slice landed                            |
| `download`       | Executable text download into `EnvironmentProvider`; binary handoff returns structured resource-extension message       | Executable streaming download into `EnvironmentProvider` or resource store                                                                  | Text path landed; binary resource API remains |
| `load_media_url` | Executable URL classification plus provider-ready media/document content values tied to `ModelProfile` capabilities     | Executable URL classification plus provider-ready media/document content values                                                             | First slice landed                            |
| `read_image`     | Executable through injectable fallback `HostMediaUnderstandingClient`                                                   | Executable fallback image understanding agent/tool                                                                                          | Adapter seam landed                           |
| `read_video`     | Executable through injectable fallback `HostMediaUnderstandingClient`                                                   | Executable fallback video understanding agent/tool                                                                                          | Adapter seam landed                           |
| `read_audio`     | Executable through injectable fallback `HostMediaUnderstandingClient`                                                   | Executable fallback audio transcription/understanding agent/tool                                                                            | Adapter seam landed                           |

Public-surface cleanup: the default host-operation bundle exposes one web search tool and raw HTTP `fetch` behavior as an internal helper for `scrape`, `download`, and media probing. Document conversion is handled by skills.

Remaining executable implementation gap: binary/resource download extensions and concrete first-party fallback media model clients.

## Proposed Starweaver design

### Host service traits

Add small SDK-level traits behind first-party bundles after the spec stabilizes:

```rust
trait SearchClient {
    async fn search(&self, request: SearchRequest) -> Result<SearchResponse, SearchError>;
}

trait ScrapeClient {
    async fn scrape(&self, request: ScrapeRequest) -> Result<ScrapeResponse, ScrapeError>;
}

trait DownloadClient {
    async fn download(&self, request: DownloadRequest, sink: DownloadSink) -> Result<DownloadRecord, DownloadError>;
}

trait MediaUrlResolver {
    async fn resolve(&self, request: MediaUrlRequest) -> Result<MediaUrlResolution, MediaUrlError>;
}

trait MediaUnderstandingClient {
    async fn understand(&self, request: MediaUnderstandingRequest) -> Result<MediaUnderstandingResponse, MediaUnderstandingError>;
}

```

These traits should be injectable through `AgentContext` typed dependencies or SDK capability bundles. Tests should use deterministic fakes. Concrete network clients live in SDK/bundle code, while network reachability restrictions remain delegated to the user's environment and runtime network policy.

### Shared HTTP substrate

Create one HTTP substrate for `search`, `scrape`, `download`, `load_media_url`, and compatibility `fetch`:

- URL parser with `http` and `https` allowlist.
- Host runtime redirect handling with final URL recording.
- Timeout, response size, body streaming chunk size, and content-type sniffing settings.
- Header redaction in traces and errors.
- Deterministic fake HTTP client for fixture tests.

Implementation crates likely fit:

- `reqwest` for async HTTP and streaming bodies.
- `url` for parsing and normalization.
- `mime_guess` and response headers for extension/content-type inference.
- `sha2` for optional checksums.

### Search

`search` should ship Brave Search as the required first executable adapter. Google Custom Search and Tavily can be added as optional adapters through the same `SearchClient` response shape.

Response shape should normalize provider results:

- `success`
- `query`
- `results[]`
  - `title`
  - `url`
  - `description`
  - `provider`
  - `rank`
  - `content_type`
  - `published_at`
  - `citation`
- `errors[]` for skipped or failed providers
- `truncated`
- `metadata`

Implementation notes:

- Keep provider credentials in SDK configuration or typed dependencies.
- Return provider errors in `errors[]` while falling through to the next configured provider.
- Clamp `num` by provider limits: Brave up to 20, Google 1-10, Tavily based on adapter settings.
- Keep image result support as typed `search` result categories when needed.

### Scrape

`scrape` should prefer:

1. Firecrawl when configured.
2. Cloudflare Browser Run when configured for JavaScript-rendered pages that need headless browser execution.
3. Local fallback for reachable static HTML. Firecrawl can parse remote PDF/DOCX/document URLs when configured, while the local fallback should hand document-like resources to `download` plus document conversion.

Response shape should include:

- `success`
- `url`
- `final_url`
- `title`
- `markdown_content`
- `adapter`
- `truncated`
- `total_length`
- `content_type`
- `citation`
- `metadata`

Concrete implementation scheme:

- `FirecrawlScrapeClient` calls Firecrawl scrape with Markdown format and maps the response into Starweaver's normalized shape. It can handle dynamic pages and remote document URLs such as PDF and DOCX when configured.
- `CloudflareScrapeClient` calls a configured Cloudflare Browser Run Markdown Quick Action endpoint for JavaScript-rendered pages that need headless browser execution.
- `LocalScrapeClient` uses safe HTTP GET for static pages, extracts title and readable body, converts HTML to Markdown, and truncates at the configured character threshold.
- For PDF/Office/EPUB URLs, `scrape` should return a structured handoff that recommends `download` followed by a document-conversion skill workflow, preserving `download` as the full-resource path.
- Compatibility `fetch` should call the shared HTTP substrate and stay hidden from the default public bundle once `scrape` and `download` are executable.

Implementation crates likely fit local fallback:

- `scraper` or `lol_html` for title/body extraction.
- `html2md` or a small internal Markdown renderer for HTML-to-Markdown conversion.
- `ammonia` for conservative sanitization before Markdown conversion when needed.

### Download

`download` should stream to the active environment or resource store:

- HTTP/HTTPS URL validation with network reachability policy delegated to the user's environment.
- Max redirects, max file size, bounded chunk size, and configurable concurrency.
- UUID filename by default.
- Extension inferred from original URL or content type.
- Optional checksum.
- Saved provider path or resource id.
- Content type and byte count.

This needs an `EnvironmentProvider` extension for binary writes/resources. The immediate slice can add `write_bytes` and `write_bytes_stream`-style operations to the environment layer, then implement streaming download without buffering full files in memory.

### Media URL understanding

`load_media_url` should classify URLs by headers, extension, and lightweight probing:

- image
- video
- audio
- document
- text
- unknown

It should inspect current model capabilities and return provider-ready content when supported. When the current model lacks support, it should return a structured fallback instruction pointing to `read_image`, `read_video`, `read_audio`, or `download` plus a document-conversion skill workflow.

### Media fallback tools

`read_image`, `read_video`, and `read_audio` should call configured fallback model adapters or subagents. Results should include:

- source URL
- model id
- textual analysis/transcript
- usage absorbed into the parent `AgentContext`
- trace parent correlation
- truncation or sampling metadata when large media is summarized

This mirrors ya-agent-sdk usage ledger behavior and fits Starweaver's existing usage and trace context primitives.

## Document conversion skill workflow

PDF and Office conversion should live in skills that execute shell commands in the active environment. This keeps heavy and format-specific converter dependencies outside the SDK built-in host tool surface while preserving the proven ya-agent-sdk behavior as reusable workflow guidance.

Skill targets:

- PDF to Markdown skill: use PyMuPDF4LLM or a compatible CLI/Python script.
- Office/EPUB to Markdown skill: use Microsoft MarkItDown or a compatible CLI/Python script.

Skill workflow requirements:

- Validate provider-scoped file existence before invoking shell commands.
- Enforce file size limits in the skill instructions or wrapper script.
- Run commands from the active workspace with explicit input and output paths.
- Write outputs beside the source file under `export_{stem}/`.
- Store Markdown at `export_{stem}/{stem}.md`.
- Store extracted images under `export_{stem}/images/` when the converter produces images.
- Return clear shell output that names the markdown path, export path, converter, page range when applicable, and warnings.

Recommended shell-backed implementations:

```bash
python -m pymupdf4llm input.pdf > export_input/input.md
markitdown input.docx -o export_input/input.md
```

A richer skill can wrap those commands with a Python helper that mirrors the reference behavior: page range validation for PDF, image extraction, data-URI image extraction for Office outputs, relative image paths, and structured JSON output.

Skill validation should focus on workflow shape:

- Markdown file exists.
- Export directory exists.
- Image directory exists when images are extracted.
- Markdown contains expected headings/text snippets.
- PDF page range arguments are passed to the converter helper.
- Unsupported extension messages point to the matching skill workflow.

## Validation plan

Spec/memo validation for this research update:

```bash
git diff --check
```

Implementation validation when adapters land:

```bash
cargo test -p starweaver-agent --test bundles --locked
cargo test -p starweaver-environment --locked
cargo test -p starweaver-model --test native_tool_coverage --locked
make docs-check
make test
```

Add focused tests for:

- `search` fallback ordering and normalized result shape.
- `scrape` adapter preference: Firecrawl, Cloudflare, local fallback.
- HTTP/HTTPS protocol validation and host network policy delegation.
- Text truncation and binary guards.
- Parallel downloads and safe filenames.
- Media capability routing and fallback messages.
- Fallback media usage accounting into parent context.
- Document conversion skill workflow fixture guidance for PDF and Office shell commands.

## Immediate next implementation slices

1. Add binary/resource write extension traits to `starweaver-environment` for non-text downloads.
2. Implement streaming binary `download` records with checksum metadata.
3. Add concrete first-party fallback media understanding clients and usage accounting.
4. Keep document conversion as a skill workflow powered by shell commands.
5. Keep raw HTTP `fetch` behavior internal to scrape/download/media probing in the public SDK surface.
