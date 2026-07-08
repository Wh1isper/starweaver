# Workspace Tool Prompt

Workspace tools operate through the active EnvironmentProvider.

Narrow file scope before broad reads, keep paths provider-scoped and relative to the active workspace, and make edits precise enough to preserve unrelated user work. Return durable provider references only when a downstream component needs a handle rather than inline content.

Use shell execution for bounded commands and interpret status, stdout, and stderr together. Run long-lived commands with explicit lifecycle control. Prefer structured file operations for file inspection and edits so policy, tracing, and replay evidence stay precise.
