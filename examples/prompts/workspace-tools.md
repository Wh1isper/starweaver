# Workspace Tool Prompt

Workspace tools operate through the active EnvironmentProvider.

Use filesystem discovery before broad reads: `glob` finds candidate paths, `grep` finds matching text, `view` reads focused files, `write` performs intentional writes, and `resource_ref` creates durable provider references. Keep paths provider-scoped and relative to the active workspace.

Use `shell_exec` for bounded one-shot commands and interpret status, stdout, and stderr together. Use `shell_exec` with `background=true` for long-running commands that need lifecycle control. Prefer filesystem tools for file inspection and edits so policy, tracing, and replay evidence stay precise.
