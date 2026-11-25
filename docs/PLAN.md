# Plan

> Revised 2026-11-11 by [ADR 017](adr/017-architecture-revision.md). The previous plan is in git history at `1f022fc`. It described about 70 crates and 24 milestones as if they were scheduled; most had no design behind them and several acceptance numbers were not derived from anything. This version separates what exists, what is committed next, and what is only direction.
>
> **Name.** "OpenFX" collides with the ASWF OFX image-effects standard and cannot be used for a release or a published package. The owner has chosen to keep it as a working name until then. Crates stay `pcc-*`.
>
> **License:** `MIT OR Apache-2.0`. **Toolchain:** Rust 2024, `rustc` 1.93.

## 1. Goal and what it means

The goal is a procedural content-creation engine in the class of Houdini, with the animation depth of Maya as a later aim, that AI agents and people drive through one typed command surface.

Houdini and Maya are each decades of work by hundreds of engineers. "Parity" is therefore not a milestone in this plan. The plan commits to two things instead:

1. **An architecture that does not have to be replaced to get there.** The data kernel, evaluator, scene format, and command layer must scale to production scenes and to every later context (simulation, USD, animation) without redesign. [`ARCHITECTURE.md`](ARCHITECTURE.md) defines it.
2. **Depth before breadth.** Each phase makes one area usable on real data before the next begins. An area counts as usable when it passes its corpus or analytic oracle, not when its operators exist.

Three properties distinguish the engine and are kept through every phase:

- **One command surface.** The UI, CLI, MCP server, and scripting are clients of the same engine service. No client mutates state directly.
- **Closed-loop work for agents.** Structured probes, declarative invariants, sandboxed transactions, and eventually image capture, so an agent can check its own result.
- **Evidence gates.** Analytic oracles, real-data corpora, and mutation testing decide what is done.

## 2. Where the build stands

Verified against the code on 2026-11-11.

| Area | State |
|---|---|
| Geometry kernel | Paged copy-on-write columns, four attribute domains, five attribute types, polygons, polylines, packed instances, tetrahedra, volumes. No strings, no curves beyond polylines |
| Scene graph | Flat DAG of data-described nodes, positional text format, expressions on parameters, state zones (solver, foreach) |
| Evaluator | Serial, content-addressed per-node cache, cancellation, budgets. Cooks every node in list order rather than only what the target needs; no eviction |
| Operators | About 50: primitives, transforms, topology edits, subdivision, bevel, remesh, decimate, clip, fracture, mesh repair, UV flatten and layout, terrain noise and erosion, voxel booleans, scatter, wrangle |
| Languages | Bounded parameter expressions; a point-only tree-walking wrangle interpreter with three types |
| I/O | `.ofxg` (lossless), OBJ, STL. Content-addressed asset import |
| Engine service | Snapshots, undo, sandboxed transactions, probes, invariants; 20 MCP tools over stdio. Cook jobs run synchronously |
| Verification | 334 tests, AST audit, tier DAG, a degenerate-input sweep over every operator, Thingi10K harness (manual), mutation testing workflow |
| Not started | Viewport, UI, CLI, Python, USD, simulation solvers, rendering, animation, GPU compute, plugins |

Known weak results on real data (ADR 010): 43% of the corpus is refused as non-manifold by five operators; booleans are voxel-based and drop thin walls silently; UV flattening does not scale past about 100,000 polygons and can shatter into one island per face.

## 3. Phases

Phases run in order. Items inside a phase are ordered where order matters. Each phase ends with an ADR that records what was measured.

### Phase A: Kernel revision

Everything here gets more expensive with each operator added, so it comes before more operators. Details and rationale are in [`ARCHITECTURE.md`](ARCHITECTURE.md).

| # | Work | Exit evidence |
|---|---|---|
| A1 | **Done ([ADR 019](adr/019-memoized-merkle-hashing.md)).** Memoized Merkle hashing; `P` as an ordinary column; lazy constant pages; `.ofxg` v2 | 10 M-point re-cook performs 0 page/column/detail hashes (`1,092 ms -> 0.017 ms`) |
| A2 | `Operator` trait and single registry; remove the dispatch `match` | Built-in and runtime-registered operators share one path; golden scene still cooks to identical hashes, apart from the documented A1 change |
| A3 | Pull evaluation; explicit output flag; lift the two-node and first/last scene rules | A test shows a node outside the target's ancestry is never cooked; empty and single-node scenes are valid |
| A4 | Scene format v2: named sparse parameters, defaults, operator versions, scene settings, node names; v1 migration | Every existing v1 golden file migrates and cooks to the same hashes; adding a defaulted parameter to an operator leaves old files valid |
| A5 | `pcc-engine` crate; real asynchronous cook jobs with a worker pool | An MCP test starts a long cook, receives the id at once, reads progress, cancels, and observes cancellation |
| A6 | Attribute types: strings, `I64`, `Vec2`, `Vec4`, `Quat`, `Mat3`, `Mat4`, type qualifiers; per-primitive vertex offsets; bit-packed, vertex, and edge groups | Round-trip through `.ofxg`; wrangle and transform respect qualifiers |
| A7 | Memory ledger and cache eviction | A cook sequence exceeding the budget evicts and completes instead of growing; shared pages are counted once |
| A8 | `rayon` in the evaluator and in page-wise operators; thread-count determinism gate | Corpus and test hashes are identical at 1 and 8 threads |
| A9 | CI: `cargo deny`, rustdoc, benchmark job with scaling exponents | Each fails a deliberately broken pull request |

A1 changes every geometry hash and therefore `.ofxg`'s integrity digest. That break happens once, here, with a format version bump.

### Phase B: Surface modeling depth

Goal: the modeling operators hold up on real meshes with real attributes.

- Mesh repair ([ADR 018](adr/018-mesh-repair.md), done: 200/200 on Thingi10K with zero non-manifold refusals on `repair+*`).
- Exact mesh booleans beside the voxel one. Candidate libraries are qualified against the corpus before adoption.
- Attribute-preserving decimation and remeshing (wedge quadrics, seam and feature preservation), and attribute interpolation for the other operators that still discard everything: `subdivide`, `extrude`, `copytopoints`, `polybevel`, `clip`, `voronoifracture`. The list is in [`AUDIT_REPORT.md`](AUDIT_REPORT.md).
- Sparse direct solver for UV flattening; a fragmentation bound; outline-based packing.
- Curves: Bezier and NURBS runs, carve, skin, polywire, mitred sweep.
- Group expressions and pattern syntax; operators accept a group parameter uniformly.
- Wrangle over all domains with the full type list, strings, arrays, and geometry queries.
- Volumes: affine transform, vector grids, sampling at points, OpenVDB file adapter.
- `pcc-cli`: cook, probe, export, and batch frame ranges from the command line.
- Operators for packed instances beyond the six that accept them.

Exit evidence: the Thingi10K run with repair shows no refusals attributable to fixable defects; a second corpus of production-style assets with UVs and normals survives decimate, remesh, and boolean with attributes intact.

### Phase C: Seeing it

Goal: a person can open a scene, see it, and edit it. An engine with no viewport is not comparable to a DCC, whatever its operators do.

- Headless rasterized capture first, exposed as an MCP tool and used by tests.
- Interactive viewport on `wgpu`: shaded, wireframe, attribute visualisation, selection.
- Desktop application as a client of `pcc-engine`: node editor, parameter panel, geometry spreadsheet, timeline.
- Keyframed parameter channels and a curve editor, since the parameter system must carry them.

The UI toolkit choice in `research/12` (`egui`) is a recommendation to be confirmed by a prototype, not a decision.

### Phase D: Interchange

- USD import and export through an opt-in native OpenUSD adapter; a stage-authoring context only after round-trips are proven.
- Alembic, glTF, PLY. FBX through `ufbx`.
- Materials as data (MaterialX references) carried on geometry.
- Rendering through existing renderers (Hydra delegates, or export) rather than a new path tracer.

### Phase E: Simulation

- Numerics prototype first, as the 2026-10 audit required: one CPU reference for a pressure projection and a particle-grid transfer, with convergence measured.
- Disk-backed checkpoints and substeps on state zones.
- One solver taken to production quality before a second begins. Position-based dynamics for cloth and grains is the proposed first, because it needs the least new infrastructure.
- Rigid bodies, then fluids, each with its analytic benchmarks from `research/02` and `research/03`.

### Phase F: Characters and animation

Skeletons as geometry, skinning, constraints, inverse kinematics, animation layers, retargeting. This is the Maya-class area and depends on Phase C's channels and curve editor.

### Unscheduled

These remain direction, with research but no committed design or dates: an in-house path tracer, a GPU image-compositing context, crowds, grooming, machine-learning operators, Gaussian splats, farm orchestration, host-application plugins, WASM plugins, GPU compute for solvers, a Python SDK. Each needs its own ADR before any work. Python scripting is the most likely to be pulled forward, because pipelines expect it.

## 4. Capability map

Where each Houdini context lands. "Research" points to background, not to a specification.

| Context | Status | Phase | Research |
|---|---|---|---|
| SOP geometry | Partial | A, B | [01](research/01-core-architecture.md), [07](research/07-geometry-processing.md) |
| VEX and expressions | Minimal | B | [08](research/08-language-scripting.md) |
| UVs | Partial, does not scale | B | [07](research/07-geometry-processing.md) |
| Volumes | Minimal | B | [07](research/07-geometry-processing.md) |
| Terrain | Partial, CPU | B | [07](research/07-geometry-processing.md) |
| Viewport and UI | None | C | [12](research/12-landscape-and-ui.md), [05](research/05-rendering-gpu.md) |
| Channels and keyframes | None | C | [09](research/09-character-animation.md) |
| USD and formats | OBJ, STL only | D | [04](research/04-formats-interop.md) |
| Particles, cloth, grains | State zones only | E | [03](research/03-simulation-solids.md) |
| Rigid bodies | None | E | [03](research/03-simulation-solids.md) |
| Fluids, smoke | None | E | [02](research/02-simulation-fluids.md) |
| Rigging and deformation | None | F | [09](research/09-character-animation.md) |
| Digital assets | None | A4 gives versions; packaging after subnetworks | [01](research/01-core-architecture.md) |
| Rendering, compositing, crowds, grooming, ML, farm, host plugins | None | Unscheduled | [05](research/05-rendering-gpu.md), [09](research/09-character-animation.md), [10](research/10-ml-and-orchestration.md) |
| Agent control (MCP) | Working over stdio | A5, then HTTP | [06](research/06-mcp-ai-native.md) |

## 5. Targets withdrawn

The previous plan stated these as acceptance criteria. None was derived from a measurement or a design, so none is a gate. Performance targets return when a benchmark exists to set them against.

- Wrangle over 10 M points in under 15 ms; JIT compile under 1.5 ms.
- 10,000-node canvas above 120 frames per second; 50 M-row spreadsheet.
- 100 M-element viewport.
- Character evaluation under 0.15 ms.
- "60+ core nodes" as a count. Operator counts are not a measure of capability.
- Every solver and renderer criterion in the former Phases 3 to 5. They stay in the research documents as candidate benchmarks.

Claims the previous plan made about the present that were false and are removed: 64-byte aligned pages, constant-page compression, roaring-bitmap groups, disk spill, a unified GPU compute layer, Streamable HTTP, and five peer clients.

## 6. How work is accepted

The enforced gates and the ones still missing are listed in [`ARCHITECTURE.md` §8](ARCHITECTURE.md#8-engineering-gates). In short: `cargo xtask ci` passes on Linux and Windows; new geometry logic has an analytic or corpus oracle; new operators have an MCP round-trip test; changed lines survive mutation testing; each decision is an ADR that states its limits.

## 7. Next step

Continue Phase A at A2 (`Operator` trait and single registry).
