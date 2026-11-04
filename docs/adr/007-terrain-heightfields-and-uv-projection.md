# ADR 007: 2D Tiled Heightfield Terrain Engine (`pcc-terrain`), Shallow-Water Hydraulic/Thermal Erosion, and UV Projection

- **Status:** Accepted
- **Date:** 2026-10-31
- **Supersedes / Extends:** [ADR 006](006-topological-modeling-and-subdivision.md)

## Context

Completing Phase 1 M1.4 (`docs/PLAN.md` and `docs/research/07-geometry-processing.md`) required introducing a 2D Copy-on-Write tiled heightfield terrain subsystem, physically based hydraulic and thermal erosion, UV projection parameterization, and seamless DAG integration with `GeoDetail`:

1. **2D Copy-on-Write Tiled Heightfields (`pcc-terrain`)**: Terrain heightfields (`height`, `mask`, `water`, `sediment`, `bedrock`) backed by $64 \times 64$ (`TILE_DIM = 64`) Copy-on-Write `Tile2D` blocks with BLAKE3 per-tile and per-layer content hashing.
2. **Derivative-Warped Multi-Fractal fBm Noise (`apply_fbm_noise_cancellable`)**: Deterministic `Philox4x32`-backed quintic-Hermite 2D value/gradient fBm noise with analytical slope-derivative damping $\frac{1}{1 + \|\sum \nabla n\|^2}$ (Musgrave / Inigo Quilez ridge/erosion fBm) and optional `mask` layer modulation.
3. **Shallow-Water Virtual-Pipe Hydraulic & Thermal Erosion (`erode_cancellable`)**: Physically based shallow-water pipe solver (Mei, Decaudin & Hu 2007; Št'ava et al. 2008) simulating rainfall, 4-neighbor hydrostatic pipe flux, velocity fields, sediment transport capacity $C = K_c \sin(\alpha) \|\mathbf{v}\|$, erosion/deposition rates, thermal talus slumping above critical slope angles, semi-Lagrangian sediment advection, and evaporation.
4. **UV Parameterization (`pcc-geo::uv_project_cancellable`)**: Normalized point UV projection (`v@uv` in $[0, 1]^2 \times \{0\}$) supporting `PlanarY`, `PlanarZ`, `PlanarX`, `Cylindrical`, `Spherical`, and `Box` (triplanar) modes while preserving CoW page sharing for `positions` and untouched columns.
5. **DAG Operator Catalog & Evaluator Wiring (`pcc-graph`, `pcc-eval`)**: Added `uvproject`, `heightfield`, `hf_noise`, and `hf_erode` to the 17-operator catalog, `.ofxs` serialization, and content-addressed evaluator.

## Decisions

### 1. Tier-1 Crate `pcc-terrain` (`Tile2D`, `TiledLayer`, `HeightField`)

- Added crate `pcc-terrain` at Tier 1 in `xtask`:
  - `TiledLayer` partitions a `rows x cols` grid into $64 \times 64$ `Tile2D` blocks of `FiniteF32` samples. `TiledLayer::with_sample` copies only the single affected tile while sharing all untouched tiles via `Arc`.
  - `HeightField::flat` initializes `height`, `mask`, `water`, and `sediment` layers.
  - `HeightField::to_geo_detail_cancellable` and `HeightField::from_geo_detail_cancellable` losslessly convert between `HeightField` and a manifold quad-grid `GeoDetail` (storing `height`, `mask`, `water`, `sediment` on `AttrDomain::Point` and `hf_rows`, `hf_cols`, `hf_size_x`, `hf_size_z` on `AttrDomain::Detail`, or inferring grid dimensions from `CreateGrid` topology).

### 2. UV Projection (`pcc-geo`)

- Added `UvProjectMode` (`PlanarY`, `PlanarZ`, `PlanarX`, `Cylindrical`, `Spherical`, `Box`) and `GeoDetail::uv_project_cancellable`, computing `v@uv` on `AttrDomain::Point` without copying `positions`.

### 3. DAG Operators (`pcc-graph`, `pcc-eval`)

- Extended `Node`, `.ofxs`, `node_catalog()` (17 operators), `CookCache::node_key`, and `eval_single_node` with:
  - `Node::UvProject { id, input, mode }` (`uvproject`)
  - `Node::HeightField { id, rows, cols, size_x, size_z }` (`heightfield`)
  - `Node::HeightFieldNoise { id, input, amplitude, frequency, octaves, seed }` (`hf_noise`)
  - `Node::HeightFieldErode { id, input, iterations, rain_rate, talus_angle, seed }` (`hf_erode`)

## Verification

- `pcc-geo`: `uv_project_modes_normalize_coordinates_and_preserve_cow`.
- `pcc-terrain`: `tiled_layer_cow_shares_unmodified_64x64_tiles_and_hashes` and `heightfield_noise_and_shallow_water_erosion_round_trip_to_geo_detail`.
- `pcc-graph`: `.ofxs` canonical round-trip and malformed input rejection for `uvproject`, `heightfield`, `hf_noise`, and `hf_erode`.
- `pcc-eval`: `heightfield_noise_erode_and_uv_project_dag_cook`.
- `pcc-mcp-stdio`: end-to-end stdio integration test verifying `HeightField -> HeightFieldNoise -> HeightFieldErode -> UvProject -> Normal` and inspecting all emitted terrain and UV point attributes (`N`, `height`, `mask`, `sediment`, `uv`, `water`).
