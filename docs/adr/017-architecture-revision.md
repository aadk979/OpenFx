# ADR 017: Architecture Revision

- **Status:** Accepted. Decisions 1 to 9 are direction and are not implemented yet. The bug fixes in the section after them are implemented.
- **Date:** 2026-11-11
- **Supersedes / Extends:** [ADR 016](016-volume-and-tetrahedron-primitives.md). Corrects [ADR 002](002-phase0-mcp-jobs-and-attributes.md) and [ADR 005](005-topology-vex-and-modeling-sops.md). Replaces the normative addenda of `research/01`, `research/06`, and `research/11` with [`ARCHITECTURE.md`](../ARCHITECTURE.md).

## Context

An audit on 2026-11-11 ([`AUDIT_REPORT.md`](../AUDIT_REPORT.md)) compared the plan and research with the code. The ADRs were accurate. The plan was not: it scheduled about 70 crates with underived acceptance numbers and described features in the present tense that do not exist. The kernel also has defects whose cost rises with each operator: geometry hashing that is linear on every call, an evaluator that cooks nodes the target does not need, a positional scene format with no defaults or versions, and cook jobs that are synchronous.

## Decisions

### 1. Kernel revision comes before more breadth

Phase A in [`PLAN.md`](../PLAN.md) runs after mesh repair merges and before further operator work. Its nine items and their exit evidence are listed there.

### 2. One normative architecture document

`ARCHITECTURE.md` states, per subsystem, what the code does and what it must become. Research documents are background. An ADR records a decision and its measured result; it does not restate the target.

### 3. Hashing

A detail's hash becomes a memoized Merkle root over its column hashes, and `P` becomes an ordinary point column. This changes every geometry hash once. `.ofxg` gets a version bump, because its integrity digest is that hash.

### 4. Evaluation

The evaluator pulls from an explicit output and cooks only ancestors. Operators implement one trait and register in one table, built-in or not. Content-addressed cache entries are not checked against revisions. Memory is enforced by a ledger that counts shared pages once, with eviction before any disk spill.

### 5. Scene format v2

Named, sparse parameters against versioned operator definitions with defaults. Node names. A settings block for frame rate and range. No revision in the file. The v1 reader stays so that existing files migrate. The two-node minimum and the first-generator, last-non-generator rules are removed.

### 6. Engine crate

`RootService`, `pcc-cmd`, and `pcc-store` merge into `pcc-engine`, and `pcc-mcp` becomes an adapter. Cook jobs run on a worker pool and return their id immediately. Undo stays a stack of snapshots; the inverse-command design in `research/01` is dropped. Idempotency keys, principals, and scopes wait for the HTTP transport.

### 7. Things explicitly not adopted from the research

- `u64` element indices. `u32` stays.
- 64-byte aligned page allocation and `unsafe` in the kernel, until a measurement calls for them.
- Roaring bitmaps for groups. Bit-packed pages match the column layout and need no dependency.
- A JIT for wrangles, until a batch interpreter over a typed IR has been measured.
- An in-house path tracer as a scheduled milestone.

### 8. Dependency policy

The zero-dependency stance ends. A dependency enters by ADR, and `cargo deny` runs in CI. `rayon`, `bytemuck`, and `faer` are approved in principle for Phase A and B; each still needs its license and transitive tree checked when it is added. Whether `pcc-math` adopts `glam` or grows its own matrix and quaternion types is left to the ADR for A6.

### 9. Performance targets

Every performance figure in the former plan is withdrawn. A target is set only against an existing benchmark.

## Bug fixes made with this ADR

A bug hunt followed the audit. Its findings are tabulated in [`AUDIT_REPORT.md`](../AUDIT_REPORT.md); the changes in behaviour are:

- **Measurement.** `surface_area` and `enclosed_volume` sum in `f64` about the middle of the bounds and saturate at `f32::MAX`. In `f32` about the world origin, a unit cube 65,536 units out measured a volume of 65,536.
- **Index ranges.** Voxelization refuses geometry whose voxel indices would not fit in half the `i32` range. The point grids in `fuse` and spaced scattering no longer overflow on a saturated cell.
- **Data kept.** `blast`, `triangulate`, and `fuse` carry vertex and primitive attributes and groups for whatever survives. `blast` on a primitive group keeps points that no primitive uses. `foreach` cuts its pieces with `GeoDetail::with_only_prims_cancellable`, which keeps the same data and drops every other point.
- **Refusals.** `clip`, `polyreduce`, `remesh`, and `polybevel` refuse input holding anything but polygons. Terrain operators refuse geometry that is not exactly a grid's quads.
- **Cache.** The whole-scene key no longer holds the revision, and node entries are no longer refused for coming from an older revision.
- **`save_new`** refuses an existing destination.

A sweep of every operator over 19 degenerate inputs is now a test, `pcc-eval/tests/degenerate_inputs.rs`. Its first run found the two overflows.

## Corrections to earlier ADRs

- **ADR 006** says `blast` compacts points and attributes. It dropped vertex attributes and all groups, and removed points unrelated to the deleted primitives.
- **ADR 007** says a terrain's grid is inferred from `CreateGrid` topology. The inference read one vertex index and a point count, so a box passed as a terrain.
- **ADR 009**'s operator table lists `triangulate` and `fuse` as keeping point and detail attributes only. Both now keep vertex and primitive attributes and groups as well. Its `clip`, `polyreduce`, `remesh`, and `polybevel` rows omit that curves in the input were dropped; they are now refused.
- **ADR 002** describes a long-running job lifecycle with progress and cancellation. `start_cook_job` runs the cook inside the request and returns when it finishes, so the caller cannot use the job id while the job runs. `enqueue_cook_job` and `run_cook_job` exist as separate calls, but no worker runs them.
- **ADR 005** calls `pcc-vex` a VM. It is a tree-walking interpreter over the syntax tree, for points only.
- **ADR 003** describes per-node caching with early cutoff. The cache is correct, but each lookup re-hashes the node's inputs in full, and the evaluator cooks non-ancestors of the target.

## Limits

- The structural findings were read from the code, not measured: no benchmark shows what hashing or non-ancestor cooking costs on a real scene, and A1 and A3 should record a before-and-after figure.
- The bug hunt covered degenerate inputs, extreme parameter values, and data preservation. It did not cover concurrency, the file parsers, the workspace path checks, or the wrangle and expression languages beyond what their existing fuzz tests do.
- Eight operators still discard every attribute and group, and four still discard points that no polygon uses. They are listed in the audit report.
- `cargo-mutants` was not run on the changed lines.
- Operator algorithms were not audited. Their evidence remains the corpus and mutation results in ADRs 009 to 016.
- External facts in the research (crate versions, specification revisions, published techniques) were not re-checked.
- Phases C to F in the plan are ordered but not designed. Each needs its own ADR before work starts.
