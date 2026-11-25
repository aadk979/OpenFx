# Architecture

This document is normative. Where it conflicts with `research/*` or with an ADR older than [ADR 017](adr/017-architecture-revision.md), this document wins. It has two parts for every subsystem: what the code does today, and what it must become. Nothing in the "target" columns exists unless the "today" column says so.

Last verified against the code on 2026-11-11 (branch `feat/mesh-repair`).

## 1. Shape of the system

```
clients        pcc-mcp-stdio (rmcp, stdio)          [pcc-cli, UI, Python: not built]
                    │
service        pcc-mcp::RootService  ── session, jobs, sandboxes, asset store, probes
                    │
state          pcc-cmd (2 commands) · pcc-store (snapshot + undo stack)
                    │
evaluation     pcc-eval ── cook cache, state zones, parameter resolution, operator dispatch
                    │
operators      pcc-sop · pcc-vdb · pcc-terrain · pcc-vex · pcc-io
                    │
data           pcc-geo (GeoDetail) · pcc-graph (nodes, .ofxs) · pcc-expr · pcc-math · pcc-id
```

About 38,000 lines of Rust in 17 crates, around 50 operators, 20 MCP tools, 334 tests. External dependencies: `blake3`, `serde`, `schemars`, `rmcp`, `tokio`, `syn`. No `unsafe`.

What is sound and stays:

- **Immutable snapshots with revision-checked publication.** Cooks hold an `Arc<GraphSnapshot>`; edits publish a new one.
- **Content-addressed cooking.** A node's key is its operator, resolved parameter values, and input content hashes. Resolving expressions before keying (ADR 013) gives time-independence without dependency tracking.
- **State zones** (ADR 014). Sealed bodies, tick-indexed checkpoints, replay equals linear run.
- **Refusal by default** (ADRs 015, 016). An operator that does not declare support for a primitive kind refuses it rather than dropping or expanding it.
- **Hostile-input posture.** Fuel-metered VEX, bounded expressions, workspace-confined paths, budget checks before allocation.
- **Evidence over claims.** Thingi10K corpus, analytic oracles, mutation testing.

## 2. Geometry kernel (`pcc-geo`)

| Aspect | Today | Target |
|---|---|---|
| Page storage | `Arc<[T]>`, 16,384 elements, hash computed eagerly at construction, one `update` call per scalar | Same layout; hash computed lazily and memoized; one `update` over the page's bytes; constant pages stored as a single value |
| Detail hash | `GeoDetail::hash()` walks every position value on every call and is not memoized; the evaluator calls it once per input per node | Merkle root over column hashes, memoized per detail; O(1) after first use |
| Positions | A dedicated `positions` field, special-cased throughout | An ordinary `P` column in the point attribute map |
| Attribute types | `F32`, `I32`, `U32`, `Bool`, `Vec3` | Add `String` (dictionary-encoded), `I64`, `Vec2`, `Vec4`, `Quat`, `Mat3`, `Mat4`; later per-element arrays |
| Type semantics | None; `N` is recognised by name | A `TypeInfo` qualifier (point, vector, normal, colour, quaternion, matrix) that tells transforms how to treat a column |
| Topology | Runs of `(kind, arity, count)`; a mesh alternating triangles and quads produces one run per primitive | Per-primitive vertex offsets, with the fixed-arity run kept as a fast path |
| Groups | Point and primitive; one `bool` per element | Bit-packed pages; add vertex and edge groups |
| Index width | `u32` everywhere | Stays `u32`. The 4.29 billion element cap is accepted; the research's `u64` indices are dropped |
| Volumes | Single-level map of 8³ `f32` bricks, no transform; named `VdbGrid` | Add an index-to-world affine transform and `Vec3` grids. It is not an OpenVDB tree and must not be described as one; OpenVDB files go through an adapter |
| Curves | Polylines only | Bezier and NURBS runs with basis and knots |
| Alignment, `unsafe` | Default allocation; `unsafe` forbidden | Unchanged until a measured SIMD or GPU-upload need exists. The "64-byte aligned pages" claim is withdrawn |
| Element-level edits | `with_element` copies one page | Unchanged |

`f32` storage with `f64` computation in operators stays.

**Why strings come first.** Names, paths, material assignments, and piece identifiers are string attributes in every production pipeline. Without them there is no USD export, no material binding, and no name-based fracture or constraint workflow.

## 3. Graph and scene (`pcc-graph`)

| Aspect | Today | Target |
|---|---|---|
| Node identity | Numeric `NodeId` only | `NodeId` plus a unique name and a parent, giving paths |
| Hierarchy | Flat list | Subnetworks; digital assets are packaged subnetworks |
| Scene validity | At least two nodes; the first must be a generator and the last must not be | Any acyclic graph, including an empty one |
| Output | Whichever node sorts last | An explicit output flag per network; bypass and template flags |
| Parameters | Every parameter required; kinds are float, unsigned integer, boolean, vector, text, expression | Sparse non-default values against a versioned definition with defaults; add signed integer, menu, ramp, colour, `Vec2`/`Vec4`, multi-parameter lists, keyframed channels |
| Operator versions | None | A version per `NodeDef`, stored in the scene, with migration functions |
| `.ofxs` | Positional tokens per line (`grid 2 9 7 8e0 6e0`); adding a parameter to an operator invalidates every file; the session revision is stored in the file | Named sparse parameters, operator version tags, a scene settings block (frame rate, range), no revision. A v1 reader stays for migration |
| Snapshot storage | `Vec<Node>`; every edit clones all nodes, re-sorts, and re-validates the whole graph | Persistent map of `Arc<Node>` with a cached order; validation limited to what an edit can affect |
| Expressions | Compiled on every cook | Compiled when the edit is made and kept in the snapshot |
| Frame rate | Fixed at 24 | Scene setting |
| Presentation data | None | Node positions and colours, stored but excluded from cook keys |

The scene format must change before anything else depends on it. Positional parameters make every operator's parameter list a file-format commitment.

## 4. Evaluation (`pcc-eval`)

| Aspect | Today | Target |
|---|---|---|
| Traversal | Walks the node list in order and cooks every node until it reaches the target, including nodes the target does not depend on | Pull: cook only the ancestors of the requested outputs |
| Dispatch | One `match` on the operator name, about 1,100 lines; runtime operators use a separate table | One `Operator` trait and one registry for built-ins and plugins alike |
| Outputs | One geometry per node | Multiple named outputs |
| Parallelism | None in the evaluator. Only voxelization uses threads | `rayon`: independent branches concurrently, page-wise loops inside operators. Reductions combine in page order so results do not depend on thread count |
| Cache | Unbounded maps behind mutexes; no accounting, no eviction | A memory ledger that counts each shared page once; least-recently-used eviction; then disk spill |
| Staleness | Node entries are content-addressed with no revision check (fixed 2026-11-11). The whole-scene entry still records a revision, which the job layer uses to report a stale job | Revisions govern only which result is presented |
| Whole-scene key | Hash of the nodes, without the revision (fixed 2026-11-11) | Removed; the output node's key serves |
| Budget | Per-operator estimates; `max_elements`, `max_memory_bytes`, `max_state_ticks` | Add wall-clock limit; memory enforced by the ledger, not estimates |
| Time dependence | A static flag per operator | Recorded from what a cook actually read, so a wrangle that ignores `@Frame` is reused across frames |
| Zone execution | Clones the full output map and searches the node list linearly for each body node on each tick | Index lookups; nested zones; checkpoints counted by the ledger and spillable |
| Diagnostics | First error only | Per-node warnings and errors carried with the result |

Order of work matters here: memoized hashing first (every other cache improvement depends on hashes being cheap), then the operator trait, then pull traversal, then the ledger, then parallelism.

## 5. Engine service and commands

| Aspect | Today | Target |
|---|---|---|
| Location | `RootService` lives in `pcc-mcp`, so any other client would depend on the MCP crate | A `pcc-engine` crate holding the service, store, commands, jobs, and assets. `pcc-mcp` becomes a thin adapter |
| `pcc-cmd`, `pcc-store` | 89 and 169 lines; two commands; `Transaction::read_set` always holds one revision | Folded into `pcc-engine`. One serializable `Command` enum is the whole mutation API |
| Cook jobs | `cook_job_start` runs the cook on the request and returns when it is finished, so a caller never holds a job id while the job is running and cannot cancel it | A worker pool; start returns at once; status, wait, cancel, and progress notifications |
| Undo | Stack of snapshots, unbounded | Stays snapshot-based, which is cheap once nodes are shared. Bounded. The research's inverse-command `UndoOp` design is dropped |
| Journal | None | Append-only command log for replay |
| Conflicts | Whole-scene revision check | Unchanged until a second concurrent client exists; then per-node read and write sets |
| Idempotency keys, principals, scopes | None | Deferred to the HTTP transport. stdio has one trusted client |
| Transports | stdio | Streamable HTTP after async jobs |
| Probes | Counts, bounds, area, volume, manifoldness, attribute names | Attribute statistics, spatial queries, prim-kind breakdown, then image capture |

## 6. Languages

- **`pcc-expr`** is a bounded stack-bytecode evaluator and is adequate. Needs string values and path-based `ch`.
- **`pcc-vex`** is a tree-walking interpreter over points with `float`, `int`, and `vector`. ADR 005 calls it a VM; it is not bytecode. Target: all four domains, the kernel's full type list, strings, arrays, geometry queries (nearest point, point cloud lookups, primitive sampling), then a typed IR that both a batch interpreter and a JIT can consume. A JIT is justified only after the batch interpreter is measured. The plan's "10 M points in under 15 ms" is withdrawn: it is near memory bandwidth for position data alone and was never derived from a measurement.

## 7. Dependencies

The zero-dependency stance served the bootstrap and now costs more than it saves. `uvflatten`'s hand-written conjugate gradient is why it fails past 100,000 polygons. Policy from ADR 017:

- A dependency enters by ADR, with its license and transitive tree checked by `cargo-deny`, which must run in CI. It does not today, although `deny.toml` exists.
- Adopt now: `rayon` (parallelism), `bytemuck` (page bytes), `faer` (sparse direct solvers).
- `pcc-math` gains `f64` vectors, `Mat3`, `Mat4`, quaternions, and affine transforms, either by adopting `glam` or by writing them once here. Today `Trs` lives in `pcc-sop` and a 3×3 matrix lives in `pcc-geo::packed`.
- `Vec3`'s `*_checked` arithmetic returns a `Result` per operation. Finiteness is validated where data enters a column, not on every add.
- Native libraries (OpenUSD, OpenVDB, Alembic) stay behind opt-in adapter crates.

## 8. Engineering gates

| Gate | Enforced today | Notes |
|---|---|---|
| No stubs, `unwrap`, `panic!`, `allow`, `ignore`, empty tests | Yes, `xtask` AST audit | |
| `fmt`, `clippy -D warnings` | Yes | Without `--all-features` |
| Crate tier DAG | Yes | |
| Tests on Linux and Windows | Yes | |
| Mutation testing on changed lines | Workflow exists | Only the mutated crate's tests count |
| `cargo deny` | **No** | Configured, never run |
| Thread-count determinism | **No** | Nothing to compare until the evaluator is parallel; required from that change onward |
| Scaling exponent gate | **No** | Measured once by hand in ADR 009 |
| Test ratchet | **No** | |
| Rustdoc build | **No** | |
| Corpus run | Manual | Not in CI because meshes cannot be committed |
| Miri | Not applicable | No `unsafe` |

The Definition of Done is the "Yes" rows plus an analytic or corpus oracle for new geometry logic and an MCP round-trip test for new operators. Rows marked "No" become gates in the order listed in [`PLAN.md`](PLAN.md); until then no document may cite them as passing.

## 9. Conventions

Right-handed, Y-up, metres, kilograms, seconds. Rational frame time. Counter-based random numbers keyed by seed and element. Trigonometry in radians. These hold today and do not change.
