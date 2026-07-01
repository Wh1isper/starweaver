//! Tool instructions and registration for the filesystem bundle.

use std::sync::Arc;

use starweaver_tools::{DynToolset, StaticToolset, ToolInstruction};

use crate::bundles::helpers::{static_sequential_tool, static_tool};

use super::{
    copy_paths, delete_paths, edit_text, glob_files, grep_files, list_files, mkdir_paths,
    move_paths, multi_edit_text, read_text, resource_ref, write_text,
};

/// Create filesystem tools backed by the `EnvironmentHandle` stored in `AgentContext`.
#[must_use]
#[allow(clippy::needless_raw_string_hashes, clippy::too_many_lines)]
pub(super) fn filesystem_tools() -> DynToolset {
    Arc::new(
        StaticToolset::new("filesystem")
            .with_id("filesystem")
            .with_instruction(ToolInstruction::new(
                "filesystem",
                "Filesystem tools operate inside the active AgentContext environment. Prefer glob to discover candidate paths, grep to find matching text, view for focused reads, write for intentional writes, edit or multi_edit for precise replacements, and resource_ref when a durable provider-scoped reference is enough. Large glob, grep, and shell-style outputs are saved through the provider tmp file abstraction instead of bypassing the active environment.",
            ))
            .with_instruction(ToolInstruction::new(
                "view",
                r#"<view-tool>
Read files from the active environment. Supports text, images (PNG/JPEG/WebP), videos (MP4/WebM/MOV), and audio (MP3/WAV/OGG).

<best-practices>
- For large files: use line_offset to read in chunks.
- Increase line_limit if you need more context (default: 300).
- For PDF files: use `pdf_convert` instead when that provider-scoped conversion tool is available.
- For Office and EPUB files: use `office_to_markdown` instead when that provider-scoped conversion tool is available.
- For image, video, or audio files, pass `instructions` when you need focused analysis such as OCR, transcription, timestamped review, UI QA, speaker labels, or extracting specific details.
- Use multiple `view` calls with narrower `instructions` when a previous media result mentions unclear, low-confidence, omitted, summarized, or high-detail regions.
- Ask for the analyzer to name omitted details, uncertain observations, and useful follow-up focuses when you need complete media understanding.
- Video and audio files can use a fallback understanding adapter when the active model lacks the matching media capability.
</best-practices>
</view-tool>"#,
            ))
            .with_instruction(ToolInstruction::new(
                "ls",
                r#"<ls-tool>
List directory entries from the active environment.

<parameters>
- `path`: Directory root, default `.`.
- `ignore`: Entry name patterns to exclude.
- `max_entries`: Maximum entries to return, default `500`; use `-1` for unlimited only when the directory is known to be narrow.
</parameters>

<best-practices>
- Use the ignore parameter to filter out unwanted entries such as logs, caches, and dependency directories.
- Keep max_entries bounded for broad roots to avoid large tool responses.
- For recursive file search: use glob instead.
- For content search: use grep instead.
</best-practices>
</ls-tool>"#,
            ))
            .with_instruction(ToolInstruction::new(
                "write",
                r#"<write-tool>
Write or overwrite entire file content.

<best-practices>
- Always use `view` first to understand existing content before overwriting user-facing files.
- For new files, verify the parent directory exists with `ls`.
- Limit content to about 200 lines per call for large files.
- Use mode="a" for appending to existing files.
- For partial edits: use `edit` or `multi_edit` instead.
</best-practices>
</write-tool>"#,
            ))
            .with_instruction(ToolInstruction::new(
                "edit",
                r#"<edit-tool>
Performs exact string replacement in files.

<best-practices>
- old_string must match file content EXACTLY (including whitespace/indentation).
- Preserve exact indentation from view output (ignore line number prefixes).
- Include 3-5 lines of context to ensure unique matches.
- Use replace_all=true for renaming variables across the file.
- Use multi_edit instead of multiple edit calls when changing the same file, especially when changes could otherwise be issued concurrently.
- Empty old_string creates a new file (fails if the file exists).
</best-practices>
</edit-tool>"#,
            ))
            .with_instruction(ToolInstruction::new(
                "multi_edit",
                r#"<multi-edit-tool>
Perform multiple find-and-replace operations on a single file.

<best-practices>
- Prefer multi_edit over multiple single edits for the same file.
- When making multiple changes to the same file, including changes planned in parallel, do not issue concurrent edit calls; combine them into one multi_edit call.
- Each old_string must be unique (or use replace_all=true).
- Edits are applied sequentially; ensure earlier edits do not affect later ones.
- All edits must succeed or none are applied (atomic operation).
- Empty old_string in first edit creates a new file.
</best-practices>
</multi-edit-tool>"#,
            ))
            .with_instruction(ToolInstruction::new(
                "glob",
                r#"<glob-tool>
Fast environment-backed file discovery with ripgrep-style glob semantics. Results are sorted by modification time (newest first).

<semantics>
- Traversal stays inside the active environment boundary.
- Pattern matching uses ripgrep-style glob semantics.
- Bare file patterns like `*.py` match recursively at any depth.
- `**/*.py` matches root-level and nested Python files.
- A leading slash anchors the pattern to the environment root, e.g. `/*.py` matches only root-level Python files.
- Hidden dot paths and gitignored paths are excluded by default.
</semantics>

<parameters>
- `pattern`: ripgrep-style glob pattern to match files and directories.
- `root`: exactly one logical directory root to traverse from, default `.`. Do not put multiple paths in `root`.
- `include_hidden`: include hidden dot paths such as `.git`, `.venv`, and `.env`.
- `include_ignored`: include paths excluded by `.gitignore` and nested ignore files.
- `max_results`: maximum result count; use `-1` for unlimited when the pattern is narrow.
</parameters>

<best-practices>
- Use specific patterns to narrow results before reading file contents.
- Use `root` to limit traversal to one subdirectory when the search scope is known.
- For multiple directories, issue multiple glob tool calls in parallel, one root per call; if they share a parent, use that parent as `root` and narrow `pattern`.
- Prefer glob before grep when you need to inspect candidate file names first.
- Set `include_hidden=true` for dotfiles and hidden directories.
- Set `include_ignored=true` for generated, dependency, cache, and build outputs.
- Very large results are saved to a temp file with `output_file_path`; use view to read it.
</best-practices>
</glob-tool>"#,
            ))
            .with_instruction(ToolInstruction::new(
                "grep",
                r#"<grep-tool>
Environment-backed content search with ripgrep-backed regex and glob semantics. Returns matching lines with optional surrounding context.

<semantics>
- Traversal stays inside the active environment boundary.
- Regex validation and matching use ripgrep-style semantics.
- File selection uses ripgrep-style glob semantics shared with the glob tool.
- Bare include patterns like `*.py` match recursively at any depth.
- `**/*.py` matches root-level and nested Python files.
- A leading slash anchors the include pattern to the environment root, e.g. `/*.py` searches only root-level Python files.
- Hidden dot paths and gitignored paths are excluded by default.
- Directories, binary files, and files above the configured size limit are skipped.
</semantics>

<parameters>
- `pattern`: ripgrep-style regular expression pattern to search for.
- `include`: ripgrep-style glob used to select files, default `**/*`.
- `root`: exactly one logical directory root to traverse from, default `.`. Do not put multiple paths in `root`.
- `context_lines`: lines before and after each match, default `2`.
- `max_results`: maximum total matches; use `-1` for unlimited when scope is narrow.
- `max_matches_per_file`: maximum matches per file; use `-1` for unlimited.
- `max_files`: maximum files to search after filtering; use `-1` for unlimited.
- `include_hidden`: include hidden dot paths such as `.git`, `.venv`, and `.env`.
- `include_ignored`: include paths excluded by `.gitignore` and nested ignore files.
</parameters>

<best-practices>
- Use a specific `include` pattern or a single `root` for faster and cleaner results.
- For multiple directories, issue multiple grep tool calls in parallel, one root per call; if they share a parent, use that parent as `root` and narrow `include`.
- Use glob first when you need to inspect candidate file names.
- Keep `context_lines` low for broad scans and raise it for targeted inspection.
- Set `include_hidden=true` for dotfiles and hidden directories.
- Set `include_ignored=true` for generated, dependency, cache, and build outputs.
- Increase `max_files`, `max_results`, or `max_matches_per_file` deliberately after narrowing scope.
</best-practices>
</grep-tool>"#,
            ))
            .with_instruction(ToolInstruction::new(
                "mkdir",
                r#"<mkdir-tool>
Create directories within the active environment.

<best-practices>
- Use for asset directories and generated output directories, not broad project root restructuring.
- Set parents=true for nested directory creation.
- Existing directories are handled without error when supported by the environment provider.
</best-practices>
</mkdir-tool>"#,
            ))
            .with_instruction(ToolInstruction::new(
                "delete",
                r#"<delete-tool>
Delete files or directories within the active environment.

<parameters>
- `paths`: List of file or directory paths to delete.
- `recursive`: Delete directories and their contents, equivalent to `rm -r`.
- `force`: Ignore missing paths, equivalent to `rm -f`.
</parameters>

<best-practices>
- Use `paths` for multiple deletions in one call.
- Use recursive=true for non-empty directories only when you intend recursive removal.
- Use force=true when cleanup should succeed even if paths are already gone.
- Prefer specific file or directory paths over broad parent directories.
- Verify broad recursive targets with `ls` or `glob` before deleting.
</best-practices>
</delete-tool>"#,
            ))
            .with_instruction(ToolInstruction::new(
                "move",
                r#"<move-tool>
Move files or directories within the active environment.

<best-practices>
- Use pairs parameter for multiple moves in one call.
- Set overwrite=true only when intentionally replacing files.
- Verify source exists with ls or glob before moving.
</best-practices>
</move-tool>"#,
            ))
            .with_instruction(ToolInstruction::new(
                "copy",
                r#"<copy-tool>
Copy files within the active environment.

<best-practices>
- Use pairs parameter for multiple copies in one call.
- Set overwrite=true only when intentionally replacing files.
- For directory copy, create structure with mkdir and copy individual files when the provider only supports file copy.
</best-practices>
</copy-tool>"#,
            ))
            .with_instruction(ToolInstruction::new(
                "resource_ref",
                r#"<resource-ref-tool>
Create a stable provider-scoped resource reference for a path without reading the full file content.

<best-practices>
- Use when a downstream component needs a durable reference rather than inline content.
- Use view when you need to inspect file contents.
- Verify the path with ls, glob, or view first if existence is uncertain.
</best-practices>
</resource-ref-tool>"#,
            ))
            .with_tools([
                static_tool("view", "Read a provider-scoped file. Text reads support pagination and truncation metadata; image, video, and audio files are loaded through the active environment and either attached for native model media support or analyzed by the configured fallback client.", read_text),
                static_tool("ls", "List provider-scoped file entries.", list_files),
                static_sequential_tool("write", "Write a provider-scoped UTF-8 text file.", write_text),
                static_sequential_tool("edit", "Perform exact string replacement in files.", edit_text),
                static_sequential_tool("multi_edit", "Perform multiple exact replacements in one file.", multi_edit_text),
                static_tool("glob", "Find provider-scoped paths with ripgrep-style glob semantics.", glob_files),
                static_tool("grep", "Search provider-scoped text files with ripgrep regex semantics.", grep_files),
                static_sequential_tool("mkdir", "Create directories through a host/provider operation envelope.", mkdir_paths),
                static_sequential_tool("delete", "Delete files or directories through a host/provider operation envelope.", delete_paths),
                static_sequential_tool("move", "Move files or directories through a host/provider operation envelope.", move_paths),
                static_sequential_tool("copy", "Copy files through a host/provider operation envelope.", copy_paths),
                static_tool("resource_ref", "Create a stable provider-scoped resource reference for a path.", resource_ref),
            ]),
    )
}
