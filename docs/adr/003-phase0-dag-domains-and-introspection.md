# ADR 003: Phase 0 General DAG Evaluation, Multi-Domain Geometry, and MCP Introspection

## Status

Accepted and implemented.

## Context

With the initial Phase 0 contracts established in ADR 001 and ADR 002, completing the Phase 0 kernel required lifting the two-node vertical-slice restrictions across `pcc-graph`, `pcc-cmd`, `pcc-store`, `pcc-eval`, `pcc-geo`, `pcc-math`, `pcc-mcp`, `pcc-mcp-stdio`, and `xtask`:

1. **General Acyclic DAGs & Fine-Grained Commands**: Agents and clients need to construct branching/converging DAGs (`CreatePoints`, `Transform`, `Merge`), apply atomic batch edits (`GraphEdit`) without rewriting whole `.ofxs` strings, and undo/redo scene mutations with optimistic revision safety.
2. **Per-Node Content-Addressed Caching & Early Cutoff**: Re-evaluating unchanged upstream branches when a downstream parameter changes wastes compute. Per-node BLAKE3 cache keys keyed on upstream geometry hashes provide automatic early cutoff.
3. **Multi-Domain Geometry, Topology, and Groups**: `GeoDetail` needed `Vertex`, `Prim`, and `Detail` attribute domains alongside `Point` attributes, uniform-arity primitive runs (`PrimRun`: polygons and polylines) with validated vertex-to-point indexing, and named `Point`/`Prim` selection groups (`ElementGroup`).
4. **MCP Catalog Introspection & Declarative Invariants**: Agents require `node_catalog_search`, `node_type_inspect`, `graph_batch_edit`, `scene_undo`, `scene_redo`, and `geo_assert_invariants` over MCP stdio.
5. **Deterministic Counter PRNG & Crate Tier DAG Gate**: Milestone M0.1 requires a counter-based `Philox4x32` PRNG in `pcc-math` and automated crate-tier dependency DAG enforcement in `xtask`.

## Decision

### 1. General Acyclic DAGs, `GraphEdit`, and Monotonic Undo/Redo (`pcc-graph`, `pcc-cmd`, `pcc-store`)

- `Node` supports `CreatePoints`, `Transform`, and `Merge` (`left`, `right`).
- `GraphSnapshot::apply_edits` applies a slice of `GraphEdit` operations (`AddNode`, `RemoveNode`, `SetCreateCount`, `SetTransformOffset`, `ConnectTransformInput`, `ConnectMergeInputs`), runs a deterministic Kahn's algorithm topological sort, and validates all DAG and finite-float invariants (`SceneError::NodeOrder` on cycles, dangling inputs, or duplicate node IDs).
- `SceneStore` maintains `undo_stack` and `redo_stack` of immutable `Arc<GraphSnapshot>` states. Both `undo(expected_revision)` and `redo(expected_revision)` verify the caller's expected revision and assign a strictly increasing `RevisionId` (`current.revision.next()`), ensuring in-flight cook jobs from superseded states are always recognized as stale.

### 2. Per-Node Content-Addressed Cache with Early Cutoff (`pcc-eval`)

- `CookCache::node_key` hashes `(node_op_and_params, upstream_geometry_blake3_hashes, frame_numerator, frame_denominator)`.
- During `cook_cached_with_progress`, each node in topological order checks `CookCache::get_node` before executing. Modifying one branch of a diamond DAG reuses cached `Arc<GeoDetail>` outputs for all unaffected upstream and sibling nodes without re-cooking.

### 3. Multi-Domain Attributes, Primitive Topology, and Selection Groups (`pcc-geo`)

- `AttrDomain` (`Point`, `Vertex`, `Prim`, `Detail`) validates column lengths against `GeoDetail::domain_len(domain)` (`Detail` has length `1`).
- `GeoDetail::with_primitives` validates `PrimRun` arities (`Polygon >= 3`, `Polyline >= 2`) and checks that all `vertex_points` indices are `< point_count`.
- `GeoDetail::with_group` builds paged `ElementGroup` masks on `GroupDomain::Point` or `GroupDomain::Prim`.
- `GeoDetail::hash` incorporates positions, topology, all four attribute domains, and selection groups deterministically.

### 4. MCP Introspection, Batch Edits, Undo/Redo, and Invariants (`pcc-mcp`, `pcc-mcp-stdio`)

- Added structured MCP tools with `schemars` JSON Schema generation:
  - `node_catalog_search` and `node_type_inspect`
  - `graph_batch_edit`
  - `scene_undo` and `scene_redo`
  - `geo_assert_invariants` (evaluating `GeoInvariantSpec` bounds and point/prim count constraints)

### 5. `Philox4x32` PRNG and `xtask` Crate-Tier Enforcement (`pcc-math`, `xtask`)

- Added 10-round `Philox4x32` counter-based PRNG in `pcc-math`.
- Extended `xtask` to audit `crates/*/Cargo.toml` against the 5-tier crate DAG (`Tier 0: pcc-id, pcc-math` → `Tier 1: pcc-geo, pcc-graph` → `Tier 2: pcc-store, pcc-cmd, pcc-eval` → `Tier 3: pcc-mcp` → `Tier 4: pcc-mcp-stdio`).
