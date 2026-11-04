# ADR 008: Sparse $8^3$ Brick VDB Signed Distance Fields (`pcc-vdb`), Generalized Winding Number Voxelization, Dual Surface Nets, and Guaranteed-Manifold Volumetric Booleans

- **Status:** Accepted
- **Date:** 2026-10-31
- **Supersedes / Extends:** [ADR 007](007-terrain-heightfields-and-uv-projection.md)

## Context

Completing Phase 1 M1.3 (`docs/PLAN.md` and `docs/research/07-geometry-processing.md`) required introducing a sparse $8^3$ Copy-on-Write brick signed distance field subsystem (`pcc-vdb`), mesh-to-SDF conversion using generalized solid-angle winding numbers, level-set CSG/morphology, Dual Surface Nets quad-mesh extraction, and guaranteed-manifold volumetric booleans:

1. **Sparse $8^3$ Copy-on-Write Brick Volumetric Grids (`pcc-vdb::VdbGrid`)**: Sparse narrow-band SDF storage partitioned into $8 \times 8 \times 8 = 512$ `FiniteF32` leaf bricks (`Brick8x8x8`) indexed by `(bx, by, bz)` with BLAKE3 per-brick and per-grid content hashing and Copy-on-Write brick sharing (`brick_shared_with`).
2. **Generalized Winding Number Mesh Voxelization (`VdbGrid::from_polygons_cancellable`)**: Exact closest-point unsigned triangle distance combined with the exact Van Oosterom & Strackee (1983) / Barill et al. (SIGGRAPH 2018) solid-angle generalized winding number $w(\mathbf{p}) = \frac{1}{4\pi}\sum \Omega_i(\mathbf{p})$ for robust inside/outside sign classification ($\phi < 0 \iff w(\mathbf{p}) \ge 0.5$).
3. **Level-Set Morphology & CSG (`reshape_cancellable`, `csg_combine_cancellable`)**: Level-set dilation/erosion ($\phi \leftarrow \phi - \text{offset}$), 6-neighbor Laplacian smoothing, and exact/smooth CSG (`Union`, `Intersection`, `Difference`, `SmoothUnion`).
4. **Guaranteed-Manifold Dual Surface Nets (`to_polygons_cancellable`) & Volumetric Booleans (`boolean_meshes_cancellable`)**: Dual cell vertex placement at mean zero-crossings paired with 2D checkerboard-pinch regularization, mathematically guaranteeing a 100% closed, orientable 2-manifold quad mesh (`is_closed_manifold() == true`, `boundary_edge_count() == 0`, `non_manifold_edge_count() == 0`).

## Decisions

### 1. Tier-1 Crate `pcc-vdb` (`Brick8x8x8`, `VdbGrid`, `VdbCombineOp`)

- Added crate `pcc-vdb` at Tier 1 in `xtask`:
  - `VdbGrid::with_voxel` copies only the single affected $8^3$ `Brick8x8x8` while sharing all untouched bricks via `Arc`.
  - `VdbGrid::to_voxel_geo_detail_cancellable` and `VdbGrid::from_voxel_geo_detail_cancellable` represent active narrow-band voxels as a `GeoDetail` point cloud (`f@phi`, `i@vx`, `i@vy`, `i@vz`, plus detail attributes `f@vdb_voxel_size`, `f@vdb_background`) so intermediate VDB nodes in a DAG are directly inspectable via `node_cook` and `geo_probe`.
  - `VdbGrid::to_polygons_cancellable` extracts the $\phi = 0$ isosurface via Dual Surface Nets after regularizing any 2D `(1, 0, 1, 0)` saddle squares, guaranteeing `is_closed_manifold() == true`.

### 2. DAG Operator Catalog & Evaluator Wiring (`pcc-graph`, `pcc-eval`)

- Extended `Node`, `.ofxs`, `node_catalog()` (now 21 operators), `CookCache::node_key`, and `eval_single_node` with:
  - `Node::VdbFromPolygons { id, input, voxel_size }` (`vdb_from_polygons`)
  - `Node::VdbReshape { id, input, offset, smooth_iterations }` (`vdb_reshape`)
  - `Node::ConvertVdb { id, input }` (`convert_vdb`)
  - `Node::Boolean { id, left, right, mode, voxel_size }` (`boolean`, with `BooleanMode`: `Union`, `Intersect`, `Subtract`)

## Verification

- `pcc-vdb`:
  - `sparse_8x8x8_bricks_preserve_cow_sharing_and_hash`
  - `vdb_from_polygons_reshape_convert_and_csg_booleans_produce_closed_manifolds`
- `pcc-graph`: `.ofxs` canonical round-trip and malformed input rejection for `vdb_from_polygons`, `vdb_reshape`, `convert_vdb`, and `boolean`.
- `pcc-eval`: `vdb_pipeline_and_boolean_dag_cook_produce_closed_manifolds`.
- `pcc-mcp-stdio`: end-to-end stdio integration test verifying `CreateBox + Transform -> Boolean(Union) -> VdbFromPolygons -> VdbReshape -> ConvertVdb -> Normal` passes `geo_assert_invariants` with `require_closed_manifold: true`.
