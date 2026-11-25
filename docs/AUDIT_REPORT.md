# Audit Reports

## 2026-11-11: architecture and documentation audit

Scope: `docs/PLAN.md`, `docs/research/01`, `06`, `11`, ADRs 001 to 016, and the code of every crate's core types and control flow. Operator algorithms were not re-derived; their evidence is the corpus and mutation results already recorded in the ADRs. The audit itself was done by reading; a bug hunt that ran the system followed the same day and is recorded under "Bugs found by running the system" below. Line numbers refer to the working tree on `feat/mesh-repair`.

### Verdict

The ADRs are honest and the code they describe is careful: budgets, cancellation, refusal by default, and real-data testing are better than the plan around them. The problems are in two places. The plan and research documents describe a system that does not exist and assert numbers nobody derived. And the kernel under the operators has several structural defects that become more expensive with every operator added. Neither is fixed by adding operators, which is what the last ten ADRs did.

Outcome: [`ARCHITECTURE.md`](ARCHITECTURE.md) is new and normative, [`PLAN.md`](PLAN.md) is rewritten, and [ADR 017](adr/017-architecture-revision.md) records the decisions. The structural findings below are left for Phase A, apart from 9 and 10, which were fixed with the bugs.

### Findings in the code

Ordered by how much later work they block.

| # | Finding | Where | Consequence |
|---|---|---|---|
| 1 | `GeoDetail::hash()` re-reads every position on every call and is not memoized. Page hashes exist for positions but are not used. The evaluator hashes each input of each node on each cook | `pcc-geo/src/lib.rs` `hash` (about line 2254); `pcc-eval/src/lib.rs` `Pass::cook_node` | A cache hit on a 10 M-point input still costs a full pass over it, per downstream node. The "content hash is cheap" premise of the evaluator is false today |
| 2 | The evaluator is not pull-based. `eval_dag` cooks nodes in list order until it reaches the target, including nodes the target does not depend on | `pcc-eval/src/lib.rs` `eval_dag` | Probing one branch cooks every branch sorted before it |
| 3 | `.ofxs` is positional. A line is an operator name followed by values in declaration order; every parameter is required; the session revision is stored in the file | `pcc-graph` `write_ofxs_line`; golden file | Adding or reordering a parameter breaks every saved scene. There is no way to version an operator |
| 4 | A scene must have at least two nodes, start with a generator, and end with a non-generator. The output is whichever node is last | `GraphSnapshot::validate` | A scene holding one box is invalid. Leftover from the two-node slice in ADR 001 |
| 5 | `cook_job_start` runs the cook to completion inside the request. The job id is first returned when the job is over | `pcc-mcp/src/lib.rs` `start_cook_job`; `pcc-mcp-stdio` tool handler | Progress and cancellation cannot be used by the client that started the job. ADR 002 describes this as a long-running job lifecycle |
| 6 | The engine service lives in the MCP crate. `pcc-cmd` has two commands and `pcc-store`'s transaction read-set always holds exactly one revision | `pcc-mcp::RootService`, `pcc-cmd`, `pcc-store` | A CLI or UI would depend on `pcc-mcp`. The "root command bus" is a revision check |
| 7 | Five attribute types, no strings | `pcc-geo` `AttrType` | No names, paths, or material assignments; blocks USD and most pipeline conventions |
| 8 | Caches are unbounded maps with no accounting or eviction. Solver checkpoints are among them | `CookCache` | Memory grows until the process dies; the "memory budget" bounds single operators only |
| 9 | Content-addressed cache inserts are refused when their revision is older than the latest | `insert_node_if_fresh` | A correct result is discarded. Content addressing already makes staleness impossible |
| 10 | The whole-scene cache key hashes the `.ofxs` text, which includes the revision | `CookCache::key` | That cache cannot hit across revisions, including after undo |
| 11 | Operator dispatch is one `match` of about 1,100 lines; runtime-registered operators take a different path | `eval_single_node` | Built-ins and plugins are not the same kind of thing; every operator edit touches the evaluator |
| 12 | Every edit clones all nodes, sorts, and validates the whole graph. Zone bodies clone the full output map and search the node list linearly per body node per tick | `apply_edits`, `Pass::run_body` | Fine at 50 nodes; quadratic costs at thousands of nodes or ticks |
| 13 | No parallelism in the evaluator or in any operator except voxelization, which uses scoped threads | workspace-wide | The plan's thread-count determinism gate has nothing to test |
| 14 | Uniform-arity primitive runs only | `PrimRun` | A mixed triangle and quad mesh fragments into many runs |
| 15 | `pcc-vex` is a tree-walking interpreter, point domain only, three types | `pcc-vex` | Described as a VM in ADR 005 |
| 16 | `Vec3` arithmetic returns `Result` per operation; no matrix or quaternion type in `pcc-math`; transforms are defined separately in `pcc-sop` and `pcc-geo::packed` | `pcc-math` | Finiteness is checked in the hot path instead of at ingress |
| 17 | `deny.toml` exists; nothing runs `cargo deny` | `xtask`, CI | The license gate in the Definition of Done has never executed |

Findings 9 and 10 are fixed. Findings 1 to 6 are the content of Phase A.

### Bugs found by running the system

Reading found structure; running found bugs. Two probes did most of the work: every operator cooked against 19 degenerate inputs (now the permanent test `pcc-eval/tests/degenerate_inputs.rs`), and every operator cooked on a mesh carrying data in all four attribute domains and both group domains, to see what came out the other side.

| # | Bug | Effect before | Fix |
|---|---|---|---|
| 1 | The working tree failed its own gate | `cargo xtask ci` stopped at a debugging example that used `unwrap`, then at a catalog count that mesh repair had made stale | Example returns errors; count updated |
| 2 | Voxel indices overflowed | `vdb_from_polygons` and `boolean` panicked in checked builds, and wrapped silently in release, on geometry whose extent or position exceeds a 32-bit voxel index | Refused with `LimitExceeded` |
| 3 | Point-grid cell indices overflowed | `fuse` and spaced `scatter_surface` panicked on points far from the origin relative to the cell | Saturated cells have no neighbour on that side |
| 4 | Area and volume were measured in `f32` about the world origin | A unit cube 65,536 units out measured a volume of 65,536. Triangles whose products overflowed were skipped. Totals could be infinite. These are the values `geo_probe` and `geo_assert_invariants` report | `f64`, about the middle of the bounds, saturating |
| 5 | `blast` rebuilt geometry from points and primitives only | Vertex attributes, every group, and every point no primitive uses were lost | Carried; unused points stay |
| 6 | `foreach` cut pieces with `blast` | Each piece lost its vertex attributes and groups, so a UV-mapped mesh came out of a loop without UVs | Pieces are cut by a strict primitive subset that keeps them |
| 7 | `triangulate` and `fuse` dropped vertex and primitive attributes and groups | UVs read from an OBJ onto vertices did not survive either operator | Each triangle and surviving corner keeps its source's data |
| 8 | `clip`, `polyreduce`, `remesh`, `polybevel` ignored curves | Polylines beside the polygons vanished without an error | Refused, as ADR 016 does for tetrahedra and volumes |
| 9 | Heightfield recognition accepted any mesh whose point count divided evenly | `box` into `hf_noise` cooked: the box was read as a four-by-two terrain | The quads must be exactly the grid's |
| 10 | Whole-scene cache key held the revision; node cache refused entries from an older revision | An undone scene never found its earlier result; a sandbox behind the live scene recooked everything on every probe | Keyed by content; no revision check on content-addressed entries |
| 11 | `save_new` renamed onto its destination | An existing scene file was replaced despite the name | Refuses an existing destination |

Not fixed, and still losing data without an error:

- `subdivide`, `extrude`, `copytopoints`, `polybevel`, `polyreduce`, `remesh`, `clip`, and `voronoifracture` discard all attributes and groups. Carrying them needs interpolation rules, which is Phase B work.
- `clip`, `polyreduce`, `remesh`, and `polybevel` discard points that no polygon uses.
- `resample` passes polygons through but strips their attributes; `sweep` consumes polygons. Both are documented on the functions.
- `instance` does not carry the target points' attributes onto the instances.

Verification: `cargo xtask ci` passes with 334 tests. The Thingi10K corpus was rerun; its result is recorded in [ADR 017](adr/017-architecture-revision.md).

### Findings in the documents

- **`PLAN.md` presented direction as schedule.** 24 milestones and about 70 crates, including a path tracer, crowds, and a farm scheduler, each with numeric acceptance criteria. Section 5 still told the reader to initialise the repository.
- **Present-tense claims that are false.** 64-byte aligned pages, constant-page compression, roaring-bitmap groups, `lz4`/`zstd` disk spill, a unified GPU layer, Streamable HTTP, and five peer clients.
- **Underived numbers.** A 10 M-point wrangle in under 15 ms is at the memory bandwidth of the position data alone. The JIT, canvas, spreadsheet, viewport, and rig-evaluation figures have no stated basis either.
- **Names.** The plan and research use `ofx-*` crate names; the code uses `pcc-*`.
- **`research/01` and `research/11` call themselves normative** in their audit addenda and specify types (`u64` indices, `Page` with a generation, `RobinHoodMap` edge index, inverse-command undo) that the code deliberately does not follow.
- **`research/11`'s Definition of Done has ten gates; four are enforced.** See [`ARCHITECTURE.md` §8](ARCHITECTURE.md#8-engineering-gates).
- **Root `README.md`** described the Phase-0 slice, said the AST audit was pending, and listed ADRs only up to 015.
- **ADR status.** ADR 002's job lifecycle (finding 5) and ADR 005's "VM" (finding 15) overstate what was built. They are corrected by ADR 017 rather than edited, following the precedent of ADR 010.

External facts in the research (library versions, the MCP revision, VFX Platform contents, attributions of published techniques) were not re-verified in this audit. Treat them as unverified background.

### Status of the 2026-10-31 mandatory prototypes

| Prototype | Status |
|---|---|
| Data and evaluator spike | Partly done: paging, hashing, cancellation, state-zone replay. Memory breaker covers single operators only |
| Native-adapter matrix | Not started |
| Numerics kernel | Not started |
| Coupling experiment | Not started |
| MCP harness | stdio only; no HTTP, no authorization |
| Colour and instance path | Million-instance path done (ADR 015); colour not started |

## 2026-10-31: plan and ecosystem audit

Summary of the earlier audit, whose full text is in git history at `1f022fc`. It corrected the first draft of the plan and research:

- Checked crate versions on crates.io and stopped describing `openusd`, `vdb`, and `nanovdb-rs` as production-complete. OpenUSD, OpenVDB, Alembic, and FBX were moved behind opt-in native adapters.
- Marked `manifold-rust`, `boolmesh`, `i_overlay`, `shader-slang`, CubeCL, Burn, ONNX Runtime, and JIT compilation as dependencies needing qualification and a fallback.
- Corrected the MCP framing: transactions and job lifetime are engine semantics, not protocol sessions.
- Added the world convention (right-handed, Y-up, SI units, rational time) and the state-zone semantics that ADR 014 later implemented.
- Set the six mandatory prototypes tabulated above.

Its section "Architectural gaps closed in the documents" recorded additions to the research documents. Those additions were specifications, not implementations, and several are superseded by `ARCHITECTURE.md`.
