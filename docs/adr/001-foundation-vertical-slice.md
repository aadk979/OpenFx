# ADR 001: Foundation vertical slice

## Status

Accepted for implementation; intentionally incomplete.

## Decision

The first executable milestone uses temporary `pcc-*` crate names and builds an immutable two-node graph: `CreatePoints → Transform`. Geometry is a 16,384-element logical page layout with copy-on-write page replacement; graph publication is revision-checked; cooks are cancellable and have a memory budget. Hashes are canonical BLAKE3 hashes of point values in deterministic order.

`pcc-store` owns immutable `Arc<GraphSnapshot>` values and publishes through optimistic transactions. `pcc-eval` keys its in-memory cache by canonical scene content plus rational frame time. `pcc-mcp::RootService` is the sole engine-facing façade for replacement and probing; a future MCP transport must adapt to it rather than mutate engine state.

## Acceptance evidence

`cargo xtask ci` must pass. Unit tests demonstrate finite-number rejection, stable cook hashes, cancellation, resource limits, unaffected-page sharing after a page-local edit, canonical `.ofxs` save/load, revision conflicts, cache reuse, and a revisioned root-service edit/probe. `xtask` parses workspace Rust source with `syn` and rejects common stub macros, `unwrap`/`expect`, and `allow`/`ignore` attributes.

## Explicit non-goals

This is not yet a network MCP server, 64-byte allocator, full typed attribute store, parallel evaluator, disk cache, or a complete semantic anti-slop auditor. The `.ofxs` serializer and typed command envelope support only the two-node foundation graph. Those restrictions must not be represented as general engine capability.
