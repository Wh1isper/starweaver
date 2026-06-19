# Repository Automation

Repository automation is implemented in the Rust `xtask` crate.

Run commands through Cargo from the repository root:

```bash
cargo run -p xtask -- check-docs-examples
cargo run -p xtask -- summarize-model-fixtures
cargo run -p xtask -- check-repository-scripts
cargo run -p xtask -- coverage-gate agent
cargo run -p xtask -- upversion 0.0.1
cargo run -p xtask -- record-model-cassette request.json --provider openai_chat --output cassette.json
```

The Makefile wraps these commands so local and CI workflows use the same Rust automation entry point.
