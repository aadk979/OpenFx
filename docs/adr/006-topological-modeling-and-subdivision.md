# ADR 006: Catmull-Clark Subdivision, Polygon Extrusion, Attribute Promotion, Blast, and Enriched `GeoProbe` Telemetry

- **Status:** Accepted
- **Date:** 2026-10-31
- **Supersedes / Extends:** [ADR 005](005-topology-vex-and-modeling-sops.md)

## Context

Completing Phase 1 M1.2 Part 2 (`docs/PLAN.md`) required extending `pcc-geo`, `pcc-graph`, `pcc-eval`, and `pcc-mcp` with half-edge-backed topological modeling operators, cross-domain attribute reduction, group/mask deletion, and richer geometry telemetry for AI agent introspection:

1. **Catmull-Clark Subdivision (`subdivide_cancellable`)**: Subdivide polygon meshes by $k$ iterations, computing face points, interior/boundary edge points via `TopologyIndex::twin`, and updated original vertex positions, outputting an all-quad `GeoDetail` that preserves `is_closed_manifold()` on closed meshes.
2. **Polygon Extrusion (`poly_extrude_cancellable`, `poly_extrude_group_cancellable`)**: Extrude polygon faces (all faces or a named `GroupDomain::Prim` face group) along their area-weighted face normals by `distance` with optional `inset`, stitching side-wall quads around each extruded face so closed manifolds remain closed manifolds.
3. **Cross-Domain Attribute Promotion (`promote_attribute`)**: Promote `F32` and `Vec3` attributes between `Point`, `Vertex`, `Prim`, and `Detail` domains using deterministic reductions (`Average`, `Min`, `Max`, `Sum`) while preserving Copy-on-Write page sharing for unmodified columns.
4. **Bounding-Box Grouping & Blast (`group_by_bounds`, `blast_group_cancellable`)**: Construct `Point` or `Prim` element groups by axis-aligned bounding-box inclusion and delete selected (or non-selected) primitives/points while compacting `positions`, point/primitive attributes, and remapping `vertex_points`.
5. **Enriched `GeoProbe` Telemetry (`pcc-mcp`)**: Expose `prim_count`, `vertex_count`, `surface_area`, `enclosed_volume`, `is_manifold`, `is_closed_manifold`, and `point_attributes` directly in `GeoProbe` across `geo_probe`, `node_cook`, `tx_probe`, and `cook_job_status`.

## Decisions

### 1. Topological Modeling & Attribute Operators (`pcc-geo`)

- **`GeoDetail::subdivide_cancellable(iterations, memory_limit, cancelled)`**:
  - Uses `TopologyIndex` to validate manifold polygon topology and pair opposite half-edges via `TopologyIndex::twin`.
  - Computes face centroids, interior edge points $\frac{\mathbf{p}_u + \mathbf{p}_v + \mathbf{f}_0 + \mathbf{f}_1}{4}$ (or midpoints $\frac{\mathbf{p}_u + \mathbf{p}_v}{2}$ on boundary edges), and updated original vertex positions $\frac{\mathbf{F} + 2\mathbf{R} + (n - 3)\mathbf{P}}{n}$ for interior vertices (or boundary crease weights $\frac{1}{2}\mathbf{P} + \frac{1}{2}\bar{\mathbf{M}}_{\text{boundary}}$ for boundary vertices).
  - Emits 4 consistently oriented sub-quads `[corner_pt, e_out_pt, f_pt_idx, e_in_pt]` per polygon corner, quadrupling quad count per iteration while preserving `is_closed_manifold()`.
- **`GeoDetail::poly_extrude_cancellable` & `poly_extrude_group_cancellable`**:
  - Computes per-face outward normals and centroids, displacing cap vertices by `normal * distance` and interpolating toward the face centroid by `inset.clamp(0.0, 1.0)`.
  - Stitches `arity` side-wall quads `[u, v, v_cap, u_cap]` per extruded face so that adjacent or single-face extrusions on a closed manifold remain closed manifolds.
- **`GeoDetail::promote_attribute(name, from_domain, to_domain, mode)`**:
  - Supports deterministic `Average`, `Min`, `Max`, and `Sum` reductions across `Point`, `Vertex`, `Prim`, and `Detail` domains for `F32` and `Vec3` columns, removing the source domain attribute when `from_domain != to_domain` and preserving CoW page sharing on `positions` and untouched attribute maps.
- **`GeoDetail::group_by_bounds` & `GeoDetail::blast_group_cancellable`**:
  - `group_by_bounds` creates `Point` (point inside AABB) or `Prim` (all primitive vertices inside AABB) `ElementGroup` masks.
  - `blast_group_cancellable` deletes or keeps (`negate: true`) elements matching a `Prim` group, a `Point` group, or a non-zero point attribute of the same name (enabling `PointWrangle -> Blast` pipelines directly in `.ofxs` graphs), compacting surviving points and remapping `vertex_points`.

### 2. DAG Operator Catalog & Evaluator Wiring (`pcc-graph`, `pcc-eval`)

- Extended `Node`, `.ofxs` serialization/parsing, `node_catalog()` (now 13 operators), `CookCache::node_key`, and `eval_single_node` with:
  - `Node::Subdivide { id, input, iterations }` (`subdivide`)
  - `Node::PolyExtrude { id, input, distance, inset }` (`extrude`)
  - `Node::AttribPromote { id, input, name, from_domain, to_domain, mode }` (`attribpromote`)
  - `Node::Blast { id, input, group_name, negate }` (`blast`)

### 3. Enriched `GeoProbe` Telemetry (`pcc-mcp`, `pcc-mcp-stdio`)

- Extended `GeoProbe` with `prim_count`, `vertex_count`, `surface_area`, `enclosed_volume`, `is_manifold`, `is_closed_manifold`, and `point_attributes: Vec<String>` so AI agents receive full topological and attribute telemetry from every probe call.

## Verification

- `pcc-geo`:
  - `catmull_clark_subdivide_preserves_closed_manifold_and_quadruples_faces`
  - `poly_extrude_group_and_box_preserve_manifold_and_increase_volume`
  - `promote_attribute_and_blast_group_preserve_topology_and_cow`
- `pcc-graph`: `.ofxs` canonical round-trip and malformed input rejection for `subdivide`, `extrude`, `attribpromote`, and `blast`.
- `pcc-eval`: `subdivide_extrude_promote_and_blast_dag_cook` verifying `CreateBox -> Subdivide (6 -> 24 -> 96 quads)`, `PolyExtrude`, `PointWrangle -> AttribPromote (Point -> Prim -> Detail)`, and `Blast`.
- `pcc-mcp-stdio`: end-to-end stdio integration test verifying `CreateBox -> Subdivide -> PolyExtrude -> Normal` and asserting the enriched `GeoProbe` fields.
