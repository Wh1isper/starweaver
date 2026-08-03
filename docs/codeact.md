# CodeAct and Recipes

Starweaver can compose ordinary tools inside bounded synchronous JavaScript. CodeAct is an orchestration frontend over the runtime's canonical tool path; it does not provide a shell, Node.js, network access, filesystem access, credentials, or a second tool dispatcher.

## Execution model

The `run_code` tool accepts JavaScript source and JSON input. Source must define `function main(input)` and may call eligible tools by exact canonical name:

```javascript
function main(input) {
  const record = tools.call("lookup_record", { id: input.id });
  return { id: input.id, status: record.status };
}
```

The initial executor is synchronous and sequential. It starts a fresh QuickJS runtime for each call and enforces source, input, output, heap, stack, wall-clock, and child-call bounds. Source definitions are evaluated with an inert lexical `tools` object; any top-level `tools.call` attempt fails the whole execution without broker admission, even if source catches the JavaScript error. The bridge becomes active only immediately before the host invokes `main`. Async functions, promises, modules, timers, host clock, randomness, console access, dynamic evaluation, WebAssembly, filesystem, network, processes, environment variables, and arbitrary host globals are unavailable.

A child call still passes through the target's ordinary dependency projection, exact capability grant, hooks, retries, timeout, cancellation, usage accounting, tracing, and stream emission. Child evidence is attributed to the outer call but is not copied into ordinary model history. The sandbox receives only the child's public JSON value. Host-private result metadata remains inaccessible to JavaScript. The runtime transfers only a complete geometry-bound media bundle to the outer orchestration return; arbitrary child metadata and ordinary media cannot partially overwrite that atomic bundle. An effect-bearing child failure without a coherent post-effect screenshot clears earlier child geometry media. Such failures are terminal: the runtime-owned broker rejects later child calls before execution regardless of executor behavior, while the QuickJS latch provides defense in depth if JavaScript catches the thrown error. The outer error retains the structured result and receipt when its envelope fits `max_output_bytes`; otherwise it uses a bounded omission marker and does not duplicate the child payload into `app_value`. If the configured budget cannot hold even the minimum terminal marker, the runtime rejects every child request before identity allocation or execution. Approval, deferred execution, cancellation, exhausted limits, broker loss, and other terminal control flow cannot be caught by JavaScript and converted into a successful result. Starweaver never automatically reruns an interrupted program.

The current profile provides bounded in-process cancellation and deadline cleanup plus child stream and trace evidence. It does not checkpoint or restore a live JavaScript VM, and it does not yet provide dedicated nested-call checkpoint records or startup reconciliation for process loss after a child effect. A process restart must therefore treat an interrupted outer execution as non-resumable; durable ambiguous-effect reconciliation remains a planned service-runtime gate.

## Eligible tools

CodeAct sees only tools that survive the active run-step preparation and satisfy all of these conditions:

- the tool is ordinarily available in the current `AgentContext`;
- its prepared definition remains visible for the step;
- it declares `CodeActEligibility::Inherit` or `CodeActEligibility::Allow`;
- it uses validated `ToolDependencyProfile::Strict` requirements;
- the host has installed every requested exact per-tool capability grant;
- product hard policy does not deny it.

Tool authors can use `.with_codeact(bool)` or `.with_codeact_availability(...)` on `FunctionTool` and `TypedFunctionTool`. `Allow` never upgrades a Legacy or Filtered dependency profile and never creates authority. `Deny` always wins. Interaction, delegation, session-control, proxy, output, and nested orchestration tools are hard-denied from the initial code projection. The rendered catalog is limited to 32 KiB. If complete return schemas alone push the catalog over that bound, Starweaver renders every exact name, description, and argument schema with an explicit `return_schema_omitted` marker and keeps the same allowlist. If that compact catalog still exceeds the bound, the step fails closed to an empty nested allowlist rather than leaving unadvertised tools callable.

Generic SDK applications opt in explicitly with both toolsets:

- `codeact_tools(CodeActConfig::default())` installs `run_code`;
- `recipe_tools(CodeActConfig::default())` loads reviewed workspace recipes.

Use the same `CodeActConfig` clone for both when supplying a custom `CodeExecutor` or host limits. Attach the application's normal environment with `attach_environment`; recipe source is read through that provider rather than by the JavaScript runtime.

## File-backed recipes

`RecipeToolset` scans `.starweaver/recipes` by default. Each child directory can expose one generated high-level tool:

- `.starweaver/recipes/inspect_record/recipe.toml`
- `.starweaver/recipes/inspect_record/main.js`
- `.starweaver/recipes/inspect_record/input.schema.json`

Example manifest:

```toml
version = 1
name = "inspect_record"
description = "Look up and normalize one record"
source = "main.js"
source_digest = "sha256:0123456789abcdef..."
tools = ["lookup_record"]
input_schema = "input.schema.json"
```

`tools` is the complete maximum allowlist, not a capability grant. Recipe preparation validates contained relative paths, unique names, recursion, the optional source digest, and the JSON input schema, then pins the manifest-derived source, schema, allowlist, and digest for that prepared run step. Manifest, source, and schema bytes are read with explicit bounds and re-read for an exact comparison before publication, so a mixed provider snapshot fails closed. Changing files affects only a later preparation. Invocation validates input before source evaluation and rechecks that every declared target remains in the current code projection.

The initial loader accepts at most 512 inventory entries and 128 recipes per root. Each manifest is limited to 64 KiB, each input schema to 256 KiB, each description to 8 KiB, and each recipe to 64 declared targets. Preparation has a 30-second read lifecycle timeout. Source size remains bounded by the shared `CodeExecutionLimits::max_source_bytes` used by the executor.

Recipe tools cannot call `run_code`, another recipe, `ask_user_question`, delegation or session-control tools, MCP proxy tools, or any other nested-invoker tool. This prevents direct and indirect recipe recursion in the initial profile.

## CLI and standalone RPC defaults

The maintained CLI and standalone RPC host install both CodeAct and Recipes by default for every effective profile. Disable both explicitly when a product or deployment does not want local constrained orchestration:

```toml
[codeact]
enabled = false
```

CLI configuration uses the normal global/project layering rules. Standalone RPC stores the effective switch in each runtime configuration materialization so active and restored generations remain pinned. The supervised launch-envelope path used by the frozen Desktop reference remains disabled regardless of standalone defaults.

The JavaScript sandbox itself has no ambient authority. Enabling CodeAct does not make Legacy, plain Filtered, unavailable, ungranted, or hard-denied tools callable from code.
