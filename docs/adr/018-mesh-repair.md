# ADR 018: Mesh Repair

- **Status:** Accepted
- **Date:** 2026-11-25
- **Supersedes / Extends:** [ADR 010](010-thingi10k-torture-harness.md), [ADR 017](017-architecture-revision.md)

## Context

The Thingi10K torture run in ADR 010 showed that about 43% of the 200-model corpus (85 non-manifold models, 97 open models) was refused as non-manifold by the five topology-sensitive modeling operators (`remesh`, `polyreduce`, `polybevel`, `subdivide`, `extrude`), and more by `voronoifracture` which also requires closure. Refusal by default is the right contract for those operators, so users and agents need an explicit connectivity repair operator that turns polygon soup into an oriented manifold surface without moving existing points.

## Decisions

### 1. Connectivity-only repair pipeline (`mesh_repair`)

`pcc_sop::mesh_repair(geo, max_hole_edges, ctx)` repairs connectivity in stages without moving any input point:

1. **Refusal of non-polygons.** Inputs containing polylines, packed instances, tetrahedra, or volumes are refused with `GeoError::InvalidTopology`, matching ADR 016.
2. **Degenerate face removal.** Repeated consecutive corners are collapsed; faces with fewer than three distinct points or repeated non-adjacent corners are dropped.
3. **Duplicate and back-to-back face removal.** Faces sharing the same unordered point set reduce to a single face with the majority winding, or are all removed when opposite windings appear in equal counts (internal walls between touching solids).
4. **Consistent patch orientation.** Connected patches across 2-face edges are oriented consistently, keeping whichever direction covers more surface area so inside-out solids and inward-facing cavities preserve their direction.
5. **Non-manifold edge pairing and vertex splitting.** Edges shared by more than two faces are paired angularly around the solid they enclose, and pinched vertices are split into one vertex per manifold fan. Split points are appended after the original points and inherit their point attributes and groups; surviving faces retain their primitive and vertex attributes and groups.
6. **Bounded minimum-area hole filling.** Boundary loops up to `max_hole_edges` (capped at `MAX_FILLED_HOLE_EDGES = 512`) are triangulated by dynamic programming to minimize triangle area, placed in the `repair_filled` primitive group, and skipped if filling would create a repeated edge. `max_hole_edges = 0` fills nothing.

Clean inputs that require no changes return cloned with identical hash. Modified outputs record `repair_*` `i32` detail attributes (`repair_degenerate_faces`, `repair_duplicate_faces`, `repair_back_to_back_faces`, `repair_flipped_faces`, `repair_split_points`, `repair_filled_holes`, `repair_filled_faces`, `repair_open_holes`) and self-verify manifoldness before returning.

### 2. Node and operator integration

`mesh_repair` is registered in `pcc-graph` and `pcc-eval` with one parameter (`max_hole_edges: u32`) and exposed through the standard graph, `.ofxs`, and MCP surfaces.

### 3. Folded cap-triangle cancellation in `ear_clip`

Running the `repair+clip` pipeline across the corpus exposed an invariant failure on Thingi10K mesh `76966`: after repair closed its holes, `clip_halfspace`'s cap triangulation hit `ear_clip`'s degenerate fallback on a bridged loop and emitted an exact back-to-back triangle pair (`[a, b, c]` and `[a, c, b]`), leaving two 4-face non-manifold edges with zero boundary edges. `ear_clip` now drops degenerate triangles and cancels exact opposite pairs (`cancel_folded_triangles`), which leaves directed boundary edges and signed area unchanged while eliminating folded zero-volume fins.

## Verification

- **Analytic and defect-class unit tests:** 28 tests in `crates/pcc-sop/tests/mesh_repair.rs` covering clean passthrough, inside-out solids and cavities, degenerate/duplicate/back-to-back faces, non-manifold edges and pinched vertices, hole filling with area and volume oracles, attribute/group preservation, idempotence, and cancellation/budget checks.
- **Evaluator and MCP round-trip tests:** 3 graph tests in `crates/pcc-eval/tests/mesh_repair.rs` and an end-to-end `RootService` + `.ofxs` save/reload test (`mesh_repair_mcp_round_trip`) in `crates/pcc-mcp/src/lib.rs`, plus unit tests for `cancel_folded_triangles` in `crates/pcc-sop/src/earclip.rs`.
- **Thingi10K 200-model corpus** (`pcc-torture run`, 30 s per-run timeout, 1,600 runs in 267 s wall clock, **0 failures**):

| Operator | ok | rejected | timed_out | invariant | panic / crash / hang | max s |
|---|---:|---:|---:|---:|---:|---:|
| `mesh_repair` (`max_hole_edges = 0`) | 200 | 0 | 0 | 0 | 0 | 1.0 |
| `repair+polybevel` | 200 | 0 | 0 | 0 | 0 | 0.8 |
| `repair+subdivide` | 200 | 0 | 0 | 0 | 0 | 0.9 |
| `repair+extrude` | 200 | 0 | 0 | 0 | 0 | 0.8 |
| `repair+clip` | 200 | 0 | 0 | 0 | 0 | 0.6 |
| `repair+remesh` | 199 | 0 | 1 (`1368057`) | 0 | 0 | 30.6 |
| `repair+polyreduce` | 199 | 0 | 1 (`89266`) | 0 | 0 | 29.9 |
| `repair+voronoifracture` | 185 | 15 | 0 | 0 | 0 | 1.6 |

- `cargo xtask ci` passes on Windows (336 tests, `fmt`, `clippy -D warnings`, AST audit, tier DAG).

## Limits

- **Connectivity only.** Near-coincident points are not welded (`fuse` must run first), zero-area faces with three distinct points stay to avoid opening holes, and geometric self-intersections remain.
- **Hole-fill ceiling.** Minimum-area dynamic programming is $O(k^3)$ in boundary length $k$, so holes above `MAX_FILLED_HOLE_EDGES = 512` (or holes whose fill would duplicate an existing edge) stay open. Those account for the 15 `repair+voronoifracture` rejections on the corpus.
- **Mutation testing.** `cargo-mutants` has not yet been run on the `mesh_repair` or ADR 017 bug-fix lines in this environment (tool not installed locally; covered by the pull-request mutation workflow).
