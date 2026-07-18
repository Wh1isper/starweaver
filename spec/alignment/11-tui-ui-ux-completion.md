# TUI UI/UX Completion Evidence

## Purpose

This document records the completed implementation of the TUI UI/UX review. Every review item is mapped to code, tests, documentation, and validation evidence. It also records the main-agent-only `ask_user_question` boundary added during implementation.

## Design Principles

- The active user decision outranks passive status information.
- One modal interaction owns the keyboard at a time.
- Narrow and short terminals retain the current action, not internal identifiers.
- Commands, help, completion, and key hints share one source of truth.
- Persistent UI state is bounded, private, and atomically written.
- Rendering behavior is testable without entering a real alternate screen.
- `ask_user_question` is available only to a main agent because a subagent cannot answer host HITL.

## Completed Work

### 1. Interaction State and Panel Priority

- [x] Added explicit state for help, command completion, structured questions, task details, pickers, history search, and transcript selection.
- [x] Defined deterministic keyboard ownership and `Esc`, `Enter`, navigation, and cancellation behavior for every modal.
- [x] Applied panel priority: question/approval, command completion, picker/search/help, expanded tasks, task summary, status.
- [x] Replaced generic leading-line truncation with frame composition that preserves the active action under constrained height.

Evidence:

- `crates/starweaver-cli/src/tui/state.rs`
- `crates/starweaver-cli/src/tui/state/command_palette.rs`
- `crates/starweaver-cli/src/tui/state/tasks.rs`
- `crates/starweaver-cli/src/tui/terminal.rs`
- `crates/starweaver-cli/src/tui/render.rs`

### 2. Structured Ask Question

- [x] Parses clarifying requests into typed `ClarifyingQuestion` values.
- [x] Supports one to four questions and current-question progress.
- [x] Supports single-select and multi-select choices.
- [x] Renders header, question, option label, description, and selected preview.
- [x] Supports free-form answers with modal-owned Enter and `Ctrl+O` newline behavior.
- [x] Submits canonical question-keyed `ClarifyingQuestionAnswers` values.
- [x] Retains the answer state until durable approval persistence succeeds.
- [x] Prioritizes question, choice, and action over durable IDs in compact layouts.

Evidence:

- `crates/starweaver-agent/src/bundles/user_input.rs`
- `crates/starweaver-cli/src/tui/state.rs`
- `crates/starweaver-cli/src/tui/render/panels.rs`
- `crates/starweaver-cli/src/tui/terminal.rs`
- `crates/starweaver-cli/src/service/tui.rs`
- `crates/starweaver-cli/src/local_store/hitl.rs`
- Tests `clarifying_question_accepts_composer_answer`, `clarifying_question_supports_multiple_multi_select_and_free_form_answers`, and `compact_question_frame_preserves_action_and_hides_task_panel`.

### 3. Command Discovery and Completion

- [x] Added a shared built-in command descriptor registry.
- [x] Merges built-ins, configured commands, aliases, and loaded skills.
- [x] Shows bounded suggestions while editing a leading slash token.
- [x] Supports Up/Down, Tab, Shift-Tab, Enter, and Esc interactions.
- [x] Provides display mode, model profile, and recent-session argument candidates.
- [x] Provides near-miss hints for reserved commands without consuming valid skill prompts.
- [x] Reuses descriptors for startup help, transient help, `/help`, completion, and tests.

Evidence:

- `crates/starweaver-cli/src/command_catalog.rs`
- `crates/starweaver-cli/src/tui/state/command_palette.rs`
- `crates/starweaver-cli/src/tui/state/commands.rs`
- `crates/starweaver-cli/src/tui/render.rs`
- Tests `command_palette_completes_commands_aliases_skills_and_arguments`, `command_palette_owns_navigation_and_escape_without_changing_enter_behavior`, and `near_miss_builtin_shows_hint_but_exact_skill_remains_a_prompt`.

### 4. Status Bar Simplification

- [x] Replaced append-order composition with semantic priority.
- [x] Removed redundant state combinations such as `READY | State: IDLE`.
- [x] Uses one compact status line when content fits and wraps semantic segments on narrow terminals.
- [x] Keeps question, approval, error, waiting, running, paused output, and current action ahead of metadata.
- [x] Shows session-scoped estimated cost, current-run elapsed time, and context usage; elapsed time uses unit-suffixed formatting, freezes at terminal/waiting state, resets for the next run, and `/clear` or session changes reset the view-scoped metrics.
- [x] Keeps the status area above the composer and uses adaptive bottom padding to lift input without reversing the hierarchy.
- [x] Adds profile, model, session, and transport metadata only when relevant.
- [x] Keeps successful WebSocket-to-HTTP fallback as durable diagnostic evidence without surfacing a warning in the normal TUI; terminal transport failures remain visible.
- [x] Uses bounded transient notifications.
- [x] Preserves a current action when only one status row is available.

Evidence:

- `crates/starweaver-cli/src/tui/render/panels.rs`
- `crates/starweaver-cli/src/tui/state.rs`
- Status and frame tests in `crates/starweaver-cli/src/tui/tests.rs`.

### 5. Task Panel Redesign

- [x] Expands automatically when the first active task snapshot arrives and toggles to a one-line progress/current-task summary with `F2`.
- [x] Provides a read-only task list without selection, arrow navigation, Enter details, or Esc dismissal; `/tasks` remains the command equivalent without a permanent summary hint.
- [x] Shows `active_form` as current activity rather than truncating it to a status token.
- [x] Distinguishes completed, active, pending, and blocked tasks.
- [x] Adds `/tasks` and command discovery.
- [x] Hides passive task UI while question or approval UI owns the footer.
- [x] Collapses a completed board and emits a bounded completion notice.

Evidence:

- `crates/starweaver-cli/src/tui/state/tasks.rs`
- `crates/starweaver-cli/src/tui/render/panels.rs`
- `crates/starweaver-cli/src/tui/state/commands.rs`
- Task rendering and F2 toggle tests in `crates/starweaver-cli/src/tui/tests.rs`.

### 6. Help and Composer Discoverability

- [x] Activates transient `?`/F1 help without transcript mutation.
- [x] Uses modal-specific status actions; transient help is opened from the composer context.
- [x] Keeps `/help` as the persistent transcript form generated from shared descriptors.
- [x] Shows up/down overflow markers for hidden composer lines.
- [x] Wraps submitted and received steering feedback at terminal display width.
- [x] Drains accepted steering synchronously at request and final-output guard boundaries so late guidance is reinjected before completion; admission closure is linearized with the final drain, and rejected post-guard steering is queued as the next continuation instead of dropped.
- [x] Renders consecutive thinking segments without empty spacer rows while preserving boundaries around text, tools, and system events.
- [x] Keeps `Ctrl+O` as an explicit newline shortcut after assigning Tab to completion.

Evidence:

- `crates/starweaver-cli/src/command_catalog.rs`
- `crates/starweaver-cli/src/tui/render.rs`
- `crates/starweaver-cli/src/tui/render/panels.rs`
- `crates/starweaver-cli/src/tui/terminal.rs`
- Tests `help_history_search_and_empty_composer_history_navigation_are_modal` and `composer_overflow_markers_show_hidden_draft_directions`.

### 7. Prompt History

- [x] Persists bounded workspace-scoped prompt history in TUI client state.
- [x] Uses private directories/files, atomic replacement, and malformed-state recovery.
- [x] Excludes generated attachment placeholders and attachment-only submissions.
- [x] Supports incremental `Ctrl+R` reverse search.
- [x] Supports empty single-line Up/Down history without breaking multiline movement.
- [x] Applies the existing product policy that successful `/clear` removes current-workspace recall.

Evidence:

- `crates/starweaver-cli/src/tui/state/history.rs`
- `crates/starweaver-cli/src/prompt_input.rs`
- `crates/starweaver-cli/src/tui/terminal.rs`
- Tests cover workspace isolation, private permissions, bounds, malformed recovery, attachment filtering, draft/search behavior, and clear semantics.

### 8. Rendering and Interaction Regression Tests

- [x] Extracted pure `compose_frame` and `compose_frame_from_body` functions from terminal I/O.
- [x] Added width coverage at 20, 40, 80, and 120 columns and heights 4, 8, and 24.
- [x] Asserts that no rendered line exceeds the effective terminal width.
- [x] Covers multi-question, multi-select, preview, free-form, retained draft, and compact question rendering.
- [x] Covers built-in, configured command, alias, skill, and argument completion.
- [x] Covers semantic status wrapping, session cost, frozen/reset run timing, composer placement, default-expanded read-only tasks, F2 toggling, successful transport fallback suppression, terminal transport failure visibility, steering guard reinjection, compact consecutive thinking, and critical action visibility.
- [x] Covers transient help, composer overflow, persistent history, malformed recovery, and reverse search.
- [x] Uses deterministic full-frame smoke coverage because the test environment cannot retain alternate-screen PTY frames reliably.

Evidence:

- `crates/starweaver-cli/src/tui/terminal.rs`
- `crates/starweaver-cli/src/tui/tests.rs`
- `crates/starweaver-cli/src/tui/state/history.rs`
- `crates/starweaver-cli/src/prompt_input.rs`

### 9. Main-Agent-Only User Input Boundary

- [x] Rejects `ask_user_question` when declared as a required inherited child tool.
- [x] Strips it from optional, automatic, and inherit-all child inheritance.
- [x] Applies a final runtime deny after static, dynamic, and capability tools are prepared.
- [x] Applies the final deny to every delegated child, including child-owned and product-materialized agents.
- [x] Documents the invariant in SDK docs, the async-subagent spec, and repository guidance.
- [x] Verifies the child model's final request parameters omit the tool while preserving allowed child tools.

Evidence:

- `crates/starweaver-agent/src/subagent/inheritance.rs`
- `crates/starweaver-agent/src/subagent/registry.rs`
- `crates/starweaver-runtime/src/agent.rs`
- `crates/starweaver-runtime/src/agent/run_loop_helpers.rs`
- `crates/starweaver-agent/tests/subagent_inheritance.rs`
- `docs/subagents.md`
- `spec/sdk/06-async-subagent-execution.md`
- `AGENTS.md`

### 10. Documentation and Validation

- [x] Updated `docs/cli.md` with the final command, status, task, help, question, and history interaction model.
- [x] Updated subagent documentation and normative boundary guidance.
- [x] Updated the alignment index and this evidence document.
- [x] Added focused regression tests for every new interaction and security boundary.
- [x] Run `make fmt-check`.
- [x] Run `make check`.
- [x] Run `make test`.
- [x] Run `make docs-check`.

## Validation Evidence

Focused validation completed during implementation:

- `cargo test -p starweaver-agent --test subagent_inheritance -- --nocapture`: 7 passed.
- Structured-question focused tests: 2 passed.
- History store focused tests: 2 passed.
- Prompt input focused tests: 4 passed.
- Narrow startup rendering regression: passed.

Repository-wide validation completed:

- `make fmt-check`: passed.
- `make check`: passed, including API snapshot, architecture, capability registry, workspace check, and Clippy.
- `make test`: passed for the complete Rust workspace.
- `make docs-check`: passed with 90 compiled documentation examples.

## Completion Rule

This work is complete only when every checkbox above is checked, all repository validation gates pass, and the working tree contains only the intended reviewed changes.
