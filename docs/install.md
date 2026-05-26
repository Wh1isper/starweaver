# Install

Starweaver is currently a workspace crate set. Add the SDK facade from the workspace or consume the lower-level crates directly while APIs stabilize.

## Workspace development

```bash
git clone https://github.com/Wh1isper/starweaver
cd starweaver
make ci
```

## Crate layers

| Crate                | Use for                                                         |
| -------------------- | --------------------------------------------------------------- |
| `starweaver-agent`   | application-facing builder and SDK helpers                      |
| `starweaver-runtime` | core agent loop and checkpointable runtime                      |
| `starweaver-model`   | model messages, settings, profiles, and provider clients        |
| `starweaver-tools`   | function tool schema, toolsets, registries, and MCP foundations |
| `starweaver-context` | lifecycle context, state, events, message bus, and dependencies |

## Local validation

```bash
make fmt-check
make check
make test
```

`make ci` runs the full local validation sequence.
