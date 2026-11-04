# ADR 002: Phase 0 Structured MCP Contracts, Cook Jobs, and Paged Attribute Columns

## Status

Accepted and implemented.

## Context

Following ADR 001, the Phase 0 state, evaluation, geometry, and MCP transport contracts needed to be hardened before any expansion into broader SOP catalogues, rendering, simulation, USD, GPU kernels, or UI:

1. Diagnostic string errors (`format!("{e:?}")`) in MCP tools were unsuitable for programmatic agent recovery (such as optimistic concurrency retry on revision conflicts or budget tuning).
2. Synchronous cooks needed a long-running job lifecycle with immutable job IDs, cooperative cancellation, progress reporting, strict inherited resource limits, and protection against stale job completion overwriting newer published revisions or cache entries.
3. `pcc-geo` only stored `P` (`Vec3`) positions rather than a general typed, paged copy-on-write attribute-column foundation.
4. `.ofxs` parsing and graph topology validation required strict canonical checks, deterministic property-based round-trip testing, and fuzz-style malformed input coverage.
5. `xtask` AST auditing needed to close concrete bypass vectors (path-qualified forbidden macros, UFCS unwrap/expect calls, `unwrap_err`/`unwrap_unchecked`/`expect_err`, `#[expect(...)]` lint suppression, and empty `#[test]` functions).

## Decision

### 1. Structured MCP Tool Results and Error Payloads (`pcc-mcp`, `pcc-mcp-stdio`)

All MCP tools on `PccMcpServer` return `Result<rmcp::Json<T>, rmcp::Json<ToolErrorPayload>>`:
- Successful calls emit `isError: false`, populate `structuredContent` with the typed payload (`GeoProbe`, `ReplaceSceneResponse`, `CookJobSnapshot`, `CancelJobResponse`), and publish JSON Schemas (`inputSchema` and `outputSchema`) via `tools/list`.
- Failed calls emit `isError: true` and populate `structuredContent` with `ToolErrorPayload`:
  - `code`: machine-readable `ToolErrorCode` (`revision_conflict`, `invalid_frame_denominator`, `scene_header_invalid`, `scene_field_invalid`, `scene_node_order_invalid`, `non_finite_input`, `invalid_graph`, `cook_cancelled`, `element_limit_exceeded`, `memory_limit_exceeded`, `index_out_of_bounds`, `store_poisoned`, `io_failure`, `job_not_found`).
  - `expected_revision` / `actual_revision`: populated on `revision_conflict` so agents can inspect the latest revision and retry safely.
  - `requested_elements` / `max_elements` / `max_memory_bytes` / `job_id`: contextual fields populated for resource-limit and job errors.

### 2. Long-Running Cook Job Infrastructure (`pcc-id`, `pcc-eval`, `pcc-mcp`, `pcc-mcp-stdio`)

- **Immutable Job IDs**: `JobId(u64)` is allocated monotonically per `RootService` instance.
- **Snapshot Capture & Strict Inherited Limits**: `enqueue_cook_job` / `start_cook_job` captures an immutable `Arc<GraphSnapshot>` at job creation time and clamps any caller-requested budget to the service's root `CookBudget` via `CookBudget::clamp_to`. A job can never widen memory or element limits beyond its parent service budget.
- **Cooperative Cancellation & Progress**: Jobs track `CookJobState` (`queued`, `running`, `completed`, `cancelled`, `failed`), `completed_steps`, and `total_steps`. Cancellation (`cancel_cook_job` / `cook_job_cancel`) sets an `AtomicBool` observed at node boundaries and 16,384-element (`PAGE_LEN`) page boundaries during both point creation and translation.
- **Stale Job Protection**: `CookCache` tracks the highest published `RevisionId` via `note_published_revision` and guards cache insertion with `insert_if_fresh`. If a newer scene revision is committed while a cook job is queued or running, the older job is marked `stale: true` and `cache_published: false`, never overwriting newer state or cache entries.

### 3. Paged Copy-on-Write Typed Attribute Columns (`pcc-geo`)

- `PagedColumn<T>` stores 16,384-element (`PAGE_LEN`) immutable `Page<T>` chunks wrapped in `Arc<[T]>`, each carrying a precomputed BLAKE3 page digest that incorporates the element type tag, page length, and canonical little-endian element bytes.
- `AttributeColumn` and `AttrType` support validated `F32` (via `FiniteF32`, which rejects NaN/infinity and canonicalizes `-0.0` to `0.0`), `I32`, `U32`, `Bool`, and `Vec3` columns.
- `GeoDetail` maintains the primary `P` position column alongside a deterministic `BTreeMap<String, AttributeColumn>` of named point attributes. Attribute names are validated (`[A-Za-z_][A-Za-z0-9_]*`, reserving `"P"` for the primary position column), column lengths must match `GeoDetail::len()`, and single-element edits (`with_point_position`, `with_f32_attribute_element`, `with_vec3_attribute_element`) copy only the affected 16,384-element page while sharing all unaffected pages across snapshots.

### 4. Strict `.ofxs` Validation and `xtask` AST Anti-Slop Enforcement (`pcc-graph`, `xtask`)

- `GraphSnapshot::validate` and `GraphSnapshot::from_ofxs` enforce non-zero revision and node IDs, distinct upstream/downstream node IDs, valid topological ordering, strict single-space ASCII tokenization, LF line endings, and finite float parameters.
- `xtask` AST auditing checks the last path segment of macros and attributes as well as UFCS function calls and `#[test]` items, rejecting `todo!`, `unimplemented!`, `panic!`, `dbg!`, `unreachable!` (including `std::panic!`, `core::todo!`, etc.), `unwrap`, `expect`, `unwrap_err`, `expect_err`, `unwrap_unchecked` (method and UFCS forms), `#[allow]`, `#[expect]`, `#[ignore]`, and empty `#[test]` functions.

## Acceptance Evidence

- `cargo xtask ci` passes (`cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `syn` AST audit across `crates/` and `xtask/`, and `cargo test --workspace`).
- Subprocess stdio integration tests in `crates/pcc-mcp-stdio/tests/stdio_integration.rs` spawn the `pcc-mcp-stdio` binary and verify MCP initialization, `tools/list` schema discovery, `geo_probe`, `replace_scene`, structured `revision_conflict` error payloads, and `cook_job_start` / `cook_job_status` / `cook_job_cancel`.
- Property and fuzz-style tests in `crates/pcc-graph/src/lib.rs` verify 1,000 generated valid graphs and 1,000 byte-mutated `.ofxs` payloads plus a malformed `.ofxs` corpus.
