# ADR 004: Geometry I/O (`pcc-io`), Transactional Sandboxes, and Mesh/Scatter SOPs

- **Status:** Accepted
- **Date:** 2026-10-31
- **Supersedes / Extends:** [ADR 003](003-phase0-dag-domains-and-introspection.md)

## Context

To complete the Phase 0 headless core and bridge into early Phase 1 procedural geometry workflows, the engine needed:

1. **Deterministic geometry I/O (`pcc-io`, M0.3)**: A native binary geometry cache format (`.ofxg`) that preserves positions, `PrimRun` topology (`Polygon` and `Polyline`), `Point`/`Vertex`/`Prim`/`Detail` typed attributes, and `Point`/`Prim` element groups with cryptographic integrity verification, alongside Wavefront `.obj` mesh import/export.
2. **Polygon mesh & deterministic scatter operators (`pcc-graph`, `pcc-eval`)**: Surface operators beyond raw point clouds (`CreateGrid` for Y-up XZ-plane quad polygon meshes and `Scatter` using counter-based `Philox4x32` over upstream bounds), plus intermediate node cooking (`cook_node_cached` / `node_cook`) so agents and tools can inspect any node in a DAG.
3. **Copy-on-Write transactional sandboxes (`pcc-mcp`, `pcc-mcp-stdio`, M0.4)**: Isolated speculative editing sessions (`tx_begin`, `tx_stage_edits`, `tx_probe`, `tx_commit`, `tx_rollback`) that allow AI agents to stage graph mutations, cook and verify the resulting geometry in isolation, and atomically commit or discard without polluting the live scene or undo history.

## Decisions

### 1. Native `.ofxg` Binary Cache and Wavefront `.obj` I/O (`pcc-io`)

- Added crate `pcc-io` (Tier 2 in the workspace dependency DAG, depending on `pcc-geo` and `pcc-math`).
- `to_ofxg_bytes` / `from_ofxg_bytes` / `from_ofxg_bytes_with_limit`:
  - Uses explicit magic header `OFXG\x00\x00\x00\x01`, followed by a 32-byte BLAKE3 checksum of the payload, and a 32-byte expected `GeoDetail::hash()` digest.
  - Serializes all positions, `PrimRun` descriptors, `vertex_points`, `Point`/`Vertex`/`Prim`/`Detail` attribute columns, and `Point`/`Prim` element groups in little-endian canonical order.
  - Rejects truncated, corrupted, non-finite, or over-budget payloads with typed `IoError` variants before or during reconstruction, and verifies that the reconstructed `GeoDetail::hash()` matches the recorded content digest.
- `to_obj` / `from_obj`:
  - Exports and imports Wavefront `.obj` `v` (positions), `f` (1-based polygon faces), and `l` (1-based polylines), coalescing consecutive primitives of identical kind and arity into compact `PrimRun`s.

### 2. `CreateGrid`, `Scatter`, and Intermediate Node Cooking (`pcc-graph`, `pcc-eval`)

- Extended `Node` in `pcc-graph` with:
  - `Node::CreateGrid { id, rows, cols, size_x, size_z }` (`grid` in `.ofxs` and the operator catalog), generating a Y-up XZ-plane quad polygon mesh (`rows * cols` points, `(rows - 1) * (cols - 1)` 4-vertex polygon primitives).
  - `Node::Scatter { id, input, count, seed }` (`scatter` in `.ofxs` and the operator catalog), deterministically scattering `count` points across the upstream geometry's bounding box using `pcc_math::Philox4x32`.
- Added `cook_node_cached` in `pcc-eval` and `RootService::probe_node` / `node_cook` in `pcc-mcp` / `pcc-mcp-stdio` to evaluate and probe any specific `NodeId` in the DAG while sharing content-addressed per-node cache entries.

### 3. Copy-on-Write Transactional Sandboxes (`pcc-mcp`, `pcc-mcp-stdio`)

- Added `TransactionId` to `pcc-id` and `SandboxRecord` management to `pcc-mcp::RootService`.
- Exposed five MCP tools for speculative multi-step workflows:
  - `tx_begin`: Captures the current immutable `GraphSnapshot` and `base_revision` under a fresh `TransactionId`.
  - `tx_stage_edits`: Applies `GraphEdit` batches to the sandbox's staged snapshot in isolation without mutating the live `SceneStore`.
  - `tx_probe`: Cooks and probes the sandbox's staged graph (warming content-addressed per-node cache entries) without publishing to the live store.
  - `tx_commit`: Atomically commits the staged snapshot to `SceneStore` against `base_revision` (failing with `ToolErrorCode::RevisionConflict` if the live store moved ahead).
  - `tx_rollback`: Discards the sandbox transaction cleanly.

## Verification

- `pcc-io` unit, property, and corruption-fuzz tests (`ofxg_and_obj_round_trip_preserve_hash_and_topology`, `property_and_fuzz_ofxg_rejects_corrupted_bytes_without_panic`).
- `pcc-eval` and `pcc-mcp` tests verifying `CreateGrid -> Scatter`, intermediate `cook_node_cached` / `probe_node`, and full sandbox lifecycle (`tx_begin` -> `tx_stage_edits` -> `tx_probe` -> `tx_commit` / `tx_rollback`).
- Subprocess stdio integration tests in `crates/pcc-mcp-stdio/tests/stdio_integration.rs`.
