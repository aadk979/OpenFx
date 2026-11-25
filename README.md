# OpenFX (working name)

A procedural content-creation engine in Rust, in the class of Houdini, driven by people and AI agents through one typed command surface. The name is a placeholder; see [`docs/PLAN.md`](docs/PLAN.md).

## State

Early. There is a geometry kernel, a content-addressed node evaluator, about 50 surface, volume, and terrain operators, a small wrangle language, and an MCP server over stdio. There is no viewport, UI, simulation, USD, or rendering yet. [`docs/PLAN.md`](docs/PLAN.md) says what exists and what comes next.

## Use

```
cargo xtask ci        # format check, clippy, source audit, tests
cargo run -p pcc-mcp-stdio -- --workspace DIR   # MCP server; file tools confined to DIR
```

The real-mesh torture run needs the Thingi10K subset, which is downloaded rather than committed:

```
python tools/thingi10k/fetch.py
cargo run --release -p pcc-torture -- run --timeout 60 --jobs 8
```

## Documents

| Document | Purpose |
|---|---|
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | What the code does and what it must become. Normative |
| [`docs/PLAN.md`](docs/PLAN.md) | Phases, exit evidence, and what is unscheduled |
| [`docs/adr/`](docs/adr/) | Decisions, each with its measured results and limits |
| [`docs/AUDIT_REPORT.md`](docs/AUDIT_REPORT.md) | Audits of the plan and the code |
| [`docs/research/`](docs/research/) | Background research. Not a specification |

## License

`MIT OR Apache-2.0`.
