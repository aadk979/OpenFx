# Project Documentation

Working name: **OpenFX** (placeholder — collides with the ASWF/OpenFX image-effects plugin standard; see the naming decision in [`PLAN.md`](PLAN.md)).

Goal: an open-source, production-grade (feature-film quality) procedural content-creation platform comparable to SideFX Houdini, written in Rust, with the Model Context Protocol (MCP) integrated at the root of the architecture so AI agents can introspect and drive every capability — built by AI agents under a strict, test-gated engineering process.

## Layout

| Path | Purpose |
|---|---|
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | Normative: what each subsystem does today and what it must become |
| [`PLAN.md`](PLAN.md) | Current state, phases with exit evidence, and what is unscheduled |
| [`adr/`](adr/) | Decision records. The source of truth for what was built and measured |
| [`AUDIT_REPORT.md`](AUDIT_REPORT.md) | Audits of 2026-10-31 (plan, ecosystem) and 2026-11-11 (architecture, code) |
| [`research/`](research/) | Background research. Not a specification: it uses the old `ofx-*` crate names, describes many things that are not built, and its external facts have not been re-verified |

## Research index

| # | Document | Domain |
|---|---|---|
| 00 | [Houdini capability reference](research/00-houdini-capability-reference.md) | What we are matching (owner-supplied preliminary research) |
| 01 | [Core architecture](research/01-core-architecture.md) | Node graph engine, geometry model, cooking, commands, plugins |
| 02 | [Fluids simulation](research/02-simulation-fluids.md) | FLIP, Pyro, oceans, pressure solvers, validation |
| 03 | [Solids simulation](research/03-simulation-solids.md) | XPBD/Vellum, VBD, FEM, MPM, rigid bodies, contact |
| 04 | [Formats & interop](research/04-formats-interop.md) | USD, VDB, Alembic, EXR, MaterialX, host integrations, C ABI |
| 05 | [Rendering & GPU](research/05-rendering-gpu.md) | GPU backends, path tracer, viewport, image processing |
| 06 | [MCP / AI-native](research/06-mcp-ai-native.md) | MCP spec, rmcp, MCP-at-the-root architecture |
| 07 | [Geometry processing](research/07-geometry-processing.md) | Booleans, remeshing, UVs, SDFs, terrain, SOP node list |
| 08 | [Language & scripting](research/08-language-scripting.md) | VEX-equivalent language, expressions, Python API |
| 09 | [Character & animation](research/09-character-animation.md) | Rigging, deformation, IK, channels, crowds, grooming |
| 10 | [ML & orchestration](research/10-ml-and-orchestration.md) | ONNX, splats, PDG/TOPs equivalent, farm integration |
| 11 | [Engineering pipeline](research/11-engineering-pipeline.md) | Agent guardrails, testing tiers, CI, supply chain, DoD |
| 12 | [Landscape & UI](research/12-landscape-and-ui.md) | OSS competitors, lessons, UI framework choice |

## Reading order for agents

1. `ARCHITECTURE.md` and `PLAN.md` (always)
2. The latest ADRs touching the area being worked on
3. The research document for that area, as background only. Where it conflicts with `ARCHITECTURE.md` or an ADR, it loses
