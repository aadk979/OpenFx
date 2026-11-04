# ADR 005: Half-Edge `TopologyIndex`, Fuel-Metered `pcc-vex` Kernel VM, and Phase 1 Modeling SOPs

- **Status:** Accepted
- **Date:** 2026-10-31
- **Supersedes / Extends:** [ADR 004](004-io-sandboxes-and-mesh-sops.md)

## Context

Advancing into Phase 1 (`M1.1` and `M1.2` in [`docs/PLAN.md`](../PLAN.md)) required three tightly integrated capabilities:

1. **Topological Query Acceleration & Geometric Oracles (`pcc-geo`)**: Implicit array-based half-edge adjacency (`TopologyIndex`), area-weighted smooth point normal computation (`compute_normals_cancellable`), and surface-area / divergence-theorem enclosed-volume evaluation (`surface_area`, `enclosed_volume`) for quantitative geometric invariant verification.
2. **Sandboxed VEX-Lite Data-Parallel Kernel Compiler & VM (`pcc-vex`, M1.1)**: A statically type-checked, fuel-metered point wrangle compiler and interpreter supporting `@P`, `@ptnum`, `@numpt`, `@Frame`, custom `f@`/`i@`/`v@` attributes, `.x`/`.y`/`.z` component access, local variables, `if`/`else` and bounded `while` loops, and deterministic math/noise built-ins (`Philox4x32`) while preserving Copy-on-Write page sharing for unmodified columns.
3. **Phase 1 Modeling & Instancing SOPs (`pcc-graph`, `pcc-eval`, `pcc-mcp`)**: Closed manifold `CreateBox` (`box`), area-weighted `Normal` (`normal`), VEX-lite `PointWrangle` (`wrangle`), and `CopyToPoints` (`copytopoints`, with `f@pscale` instancing support), plus the `vex_check` MCP tool and manifold/volume assertions in `geo_assert_invariants`.

## Decisions

### 1. Implicit CSR Half-Edge `TopologyIndex` & Geometric Oracles (`pcc-geo`)

- Added `TopologyIndex` to `pcc-geo`, where every vertex index `he in 0..vertex_count` serves as an implicit half-edge ID:
  - Builds `next_he`, `prev_he`, `vertex_prim`, and `twin_he` (`NO_HALF_EDGE` for boundary or non-manifold edges) deterministically by sorting canonical undirected edge keys `(min(u, v), max(u, v), he)`.
  - Exposes `is_manifold()`, `is_closed_manifold()`, `boundary_edge_count()`, `non_manifold_edge_count()`, `twin(he)`, `next(he)`, and `prev(he)`.
- Added `surface_area()`, `enclosed_volume()` (via the divergence theorem $V = \left|\frac{1}{6} \sum \mathbf{p}_0 \cdot (\mathbf{p}_k \times \mathbf{p}_{k+1})\right|$), and `compute_normals_cancellable()` (computing area-weighted `v@N` point normals) on `GeoDetail`.
- Upgraded `GeoDetail::merged_cancellable` to merge `PrimRun`s and offset `vertex_points` when combining polygon/polyline meshes.

### 2. Fuel-Metered `pcc-vex` Compiler & VM (`pcc-vex`)

- Added crate `pcc-vex` (Tier 2 in `xtask`):
  - Compiles VEX-lite point wrangle snippets (`compile_vex`, `check_vex`) with static type checking across `Float`, `Int`, and `Vec3` values and automatic read/write attribute dependency extraction (`read_attributes`, `written_attributes`).
  - Enforces a strict per-point instruction/loop fuel counter (`DEFAULT_MAX_FUEL_PER_POINT`), aborting unbounded `while` loops with `VexError::FuelExhausted { line, column }`.
  - Rejects any non-finite (`NaN`/`Inf`) output values at the point boundary with `VexError::NonFiniteOutput { point_index }`.
  - Only allocates/updates columns present in `written_attributes`, preserving `Arc` Copy-on-Write page sharing for all untouched position and attribute pages.

### 3. Phase 1 SOP Operators & MCP Introspection (`pcc-graph`, `pcc-eval`, `pcc-mcp`, `pcc-mcp-stdio`)

- Added `CreateBox` (`box`), `Normal` (`normal`), `PointWrangle` (`wrangle`, with canonical hex-encoded snippet serialization in `.ofxs`), and `CopyToPoints` (`copytopoints`) to `pcc-graph` and `pcc-eval`.
- Extended `GeoInvariantSpec` in `pcc-mcp` with `require_manifold`, `require_closed_manifold`, `expected_volume`, and `volume_tolerance`.
- Added the `vex_check` MCP tool to `pcc-mcp` and `pcc-mcp-stdio` so agents can validate VEX snippets and inspect read/write attribute bindings prior to cooking.

## Verification

- `pcc-geo`: `topology_index_normals_area_and_enclosed_volume` verifying closed manifold twin pairing, surface area, enclosed volume, and unit `v@N` normals on a box mesh.
- `pcc-vex`: `vex_wave_deformation_custom_attributes_and_cow_sharing` and `vex_sandbox_rejects_infinite_loops_non_finite_and_type_errors`.
- `pcc-eval`: `box_normal_wrangle_and_copy_to_points_cook` verifying `CreateBox -> Normal -> PointWrangle -> CopyToPoints` manifold and volume invariants.
- `pcc-mcp-stdio`: subprocess stdio integration test verifying `vex_check` alongside the full 18-tool MCP surface.
