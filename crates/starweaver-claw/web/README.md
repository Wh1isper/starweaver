# Starweaver Claw Web

Vite + React + TypeScript web console for Starweaver Claw.

## Focus

- workspace browser
- profile selection
- session and run inspection
- backend handshake for the local runtime

## Development

From the repository root:

```bash
make claw-dev
```

This starts the API at `http://127.0.0.1:9042`, applies pending SQLite migrations, and starts the web console at `http://127.0.0.1:5173`.

For split terminals:

```bash
make claw-dev-api
make claw-dev-web
```

Set `VITE_CLAW_BASE_URL` when the backend runs on a different origin.
