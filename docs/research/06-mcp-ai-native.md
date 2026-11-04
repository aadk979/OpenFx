# 06 — Model Context Protocol (MCP) as a Root-Level Architectural Layer

> **Scope:** Designing an AI-native DCC where the **Model Context Protocol (MCP 2026-07-28 spec)** is the foundational control, introspection, and observation layer of the engine—not a fragile Python/RPC wrapper bolted onto a GUI.

---

## 1. Why Existing DCC "MCP Wrappers" Fail in Production

Existing community MCP integrations for Blender, Houdini, Unreal, and Maya typically expose an `execute_python_code(script: str)` tool over a socket connected to the GUI main thread. This fails for serious production engineering because:
1. **Unstructured Blind Execution:** Agents guess API signatures, hallucinate parameter names, and parse raw `stdout` tracebacks instead of receiving machine-checkable JSON Schemas and structured diagnostics.
2. **Main-Thread UI Blocking & Crash Coupling:** Running a 30-second simulation cook inside a GUI socket callback freezes the application or triggers race conditions against the viewport.
3. **Context Window Explosion:** Dumping a 2-million-point mesh as JSON crashes the LLM context window; conversely, returning only `"Success"` leaves the agent blind to whether its boolean or fluid simulation actually worked or produced an empty bounding box.
4. **No Transactional Rollback or Diffing:** If step 7 of an agent's 10-step rig build fails, steps 1–6 leave partial garbage nodes scattered across the scene.

---

## 2. MCP 2026-07-28 Specification & `rmcp` Rust SDK Foundation

As verified against the current **MCP 2026-07-28 specification** (backward-compatible with `2025-11-25`) and the official **`rmcp` v3.8+ Rust SDK** ([crates.io/crates/rmcp](https://crates.io/crates/rmcp)):

| MCP Capability | `rmcp` 3.8 Feature | How Our Engine Exploits It at the Root |
|---|---|---|
| **Tools with Input & Output JSON Schemas (2020-12)** | `#[tool]` + `schemars` + `structured_content` | Every `EngineCommand` in `ofx-cmd` automatically registers its strongly-typed input and output schema (`JsonSchema`). Agents receive both human-readable summaries and machine-validated `structuredContent`. |
| **Resources & URI Templates (`ofx://...`)** | `ResourceHandler` + URI templates | The entire scene graph, node catalog, cooked geometry summaries, attribute histograms, simulation diagnostics, and rendered AOV frames are addressable as zero-side-effect read-only `ofx://` URIs. |
| **Transport-Neutral Subscriptions** | `resources/subscribe`, `notifications/resources/updated` | Agents subscribe to `ofx://scene/nodes/{path}/cook_state` or `ofx://sim/{job_id}/telemetry` and receive push notifications when cooks complete or divergence spikes. |
| **Long-Running Async Tasks & Progress** | `ProgressToken`, `notifications/progress`, `notifications/cancelled` | Multi-frame FLIP/Pyro simulations, TOP wedges, and Karma renders execute asynchronously on background worker pools, streaming per-frame progress (`frame`, `substep`, `vram_mb`, `eta_ms`) and honouring instant cancellation (`CancellationToken`). |
| **Multimodal Image Content (`ImageContent`)** | `Content::image(base64_png_or_webp, mime)` | Viewport snapshots, multi-angle turntables, attribute heatmaps, and rendered AOV passes return directly as compressed `image/webp` / `image/png` payloads alongside 3D bounding/metric metadata so vision-capable agents can visually inspect their work! |
| **Transports** | `transport-io` (stdio), `transport-streamable-http-server` | Supports `stdio` for local CLI/IDE agent spawning (`ofx-mcp --stdio`) and **Streamable HTTP** (`Mcp-Method`, `Mcp-Name` headers) for multi-agent studio/cloud collaboration against a live workstation or farm session. |

---

## 3. Root-Level Architecture: One Engine, Four Equal Peers

```
                        ┌──────────────────────────────────────────────┐
                        │            AI Agents (Claude, Gemini, etc.)  │
                        └──────────────────────┬───────────────────────┘
                                               │ MCP 2026-07-28 (stdio / Streamable HTTP)
┌─────────────────┐     ┌──────────────────────▼───────────────────────┐     ┌─────────────────┐
│   Native GUI    │     │              ofx-mcp Server Core             │     │ Python 3.13 API │
│ (ofx-ui Client) │     │  • Tool Dispatcher  • Resource URI Resolver  │     │  (ofx-py PyO3)  │
└────────┬────────┘     │  • Visual Probe     • Transaction Sandbox    │     └────────┬────────┘
         │              └──────────────────────┬───────────────────────┘              │
         └──────────────────────────┐          │          ┌───────────────────────────┘
                                    ▼          ▼          ▼
                        ┌──────────────────────────────────────────────┐
                        │      Root Command & Query Bus (ofx-cmd)      │
                        │  • Single source of truth for all mutations  │
                        │  • Atomic Transactions + Dry-Run Forking     │
                        │  • Deterministic Journal + Access Control    │
                        └──────────────────────┬───────────────────────┘
                                               ▼
                        ┌──────────────────────────────────────────────┐
                        │     SceneState (ArcSwap Immutable Roots)     │
                        └──────────────────────────────────────────────┘
```

### 3.1 Solving the Tool-Count Explosion (Progressive Discovery Pattern)

A production DCC has **500+ node types** (`scatter`, `vdb_from_polygons`, `pyro_solver`, `uv_flatten`, etc.). Exposing 500 separate MCP tools bloats the LLM system prompt by $>150,000$ tokens.

Instead, `ofx-mcp` exposes **24 orthogonal, composable Core Tools** paired with **searchable Node Catalog Resources**:

| Category | Core MCP Tools | Purpose |
|---|---|---|
| **Discovery & Docs** | `node_catalog_search`, `node_type_inspect` | Semantic/keyword/category search over all registered operator types; returns exact pin topology, parameter schemas, defaults, and usage examples for only the requested node types. |
| **Graph Authoring** | `graph_batch_edit`, `node_create`, `node_delete`, `node_connect`, `node_set_params`, `node_set_flags` | Single or transactional batch edits (`graph_batch_edit` applies an entire subgraph of nodes, parameters, and wires in one atomic round-trip with automatic topological layout). |
| **Transactions & Diff** | `transaction_begin`, `transaction_commit`, `transaction_rollback`, `scene_diff` | Let an agent experiment on a copy-on-write fork of `SceneState`, inspect the cooked results, and either commit or cleanly roll back if validation fails. |
| **Execution & Sim** | `node_cook`, `sim_step`, `top_execute` | Trigger synchronous or async-task evaluation with frame ranges, returning cook timing, memory usage, and structured warnings/errors. |
| **Geometry & Field Inspection** | `geo_probe`, `geo_attribute_stats`, `geo_spatial_query`, `geo_assert_invariants` | Token-efficient telemetry over cooked `GeoDetail` without dumping raw megabytes (see §3.2). |
| **Code & Shaders** | `vex_compile_and_check`, `expr_evaluate` | Compiles VEX-equivalent snippets or MaterialX shaders and returns line/column structured diagnostics (`miette` JSON) before running a cook. |
| **Visual Feedback** | `viewport_capture`, `render_preview` | Renders single or 4-view orthographic/perspective contact sheets (with optional wireframe, point numbers, normal vectors, or scalar attribute heatmaps) and returns `ImageContent`. |

### 3.2 Token-Efficient Geometry Telemetry (`geo_probe` & `ofx://` Resources)

When an agent calls `geo_probe(node_path="/obj/geo1/vdb_reshape1", frame=24.0)`, it never receives 50 MB of raw float arrays. It receives a compact, deterministic **Geometry Telemetry Digest (~400 tokens)**:

```json
{
  "node_path": "/obj/geo1/vdb_reshape1",
  "cook_status": "ok",
  "cook_Duration_ms": 14.2,
  "content_hash": "b3:9f8e7d6c...",
  "counts": {
    "points": 148290,
    "vertices": 593160,
    "primitives": 148290,
    "prim_breakdown": { "Quad": 148289, "VdbVolume": 1 }
  },
  "bounds_world": {
    "min": [-4.501, 0.000, -4.498],
    "max": [ 4.502, 8.120,  4.501],
    "centroid": [0.000, 3.841, 0.001]
  },
  "topology_health": {
    "is_manifold": true,
    "is_watertight": true,
    "open_boundary_edges": 0,
    "non_manifold_edges": 0,
    "degenerate_zero_area_prims": 0,
    "connected_components": 1,
    "nan_or_inf_attributes": []
  },
  "attributes": {
    "point": [
      { "name": "P", "type": "Vec3f", "min": [-4.5, 0.0, -4.5], "max": [4.5, 8.12, 4.5], "mean": [0.0, 3.84, 0.0] },
      { "name": "v", "type": "Vec3f", "mag_min": 0.0, "mag_max": 12.4, "mag_p95": 6.1 }
    ]
  },
  "volumes": [
    { "name": "surface", "class": "LevelSet", "voxel_size": 0.025, "active_voxels": 1842100, "background": 0.1 }
  ]
}
```

If the agent needs specific element values, it calls `geo_spatial_query` (e.g., *"sample 16 points nearest `[0, 5, 0]`"* or *"list first 10 primitives failing `area > 1e-6`"*).

### 3.3 Built-In Geometric & Physical Assertions (`geo_assert_invariants`)

Agents can attach declarative **Verification Nodes (`Assert SOP`)** or run `geo_assert_invariants` directly via MCP:
- `assert_watertight == true`
- `assert_point_count_in_range(10_000, 500_000)`
- `assert_bbox_contains([-5, 0, -5], [5, 10, 5])`
- `assert_no_self_intersections == true`
- `assert_volume_conservation(reference_node="/obj/geo1/source", tolerance_pct=1.0)`

This turns procedural 3D creation from "blind open-loop generation" into **closed-loop test-driven 3D engineering**.

---

## 4. Multi-Client Concurrency & Security Model

1. **Lock-Free Snapshot Reads (`ArcSwap<SceneState>`):** Any number of MCP agent readers, viewport threads, and background cooks can inspect the scene simultaneously without blocking.
2. **Optimistic Concurrency Control (`base_revision`):** Mutating MCP tool calls optionally specify `expected_revision: u64`. If another agent or human artist modified the same subnet concurrently, the command bus either cleanly merges non-overlapping node paths or returns a structured `RevisionConflict` diff.
3. **Capability-Based Sandbox:**
   - File I/O nodes (`File SOP`, `ROP Export`) validate paths against the MCP client's declared **MCP Roots** (`roots/list`), preventing path-traversal writes outside the project workspace.
   - Custom scripts/plugins execute inside resource-limited WASM sandboxes or timeout-guarded VEX kernels.

---

## 5. Recommendations, Open Risks, and Milestones

### Recommendations
1. **Use official `rmcp` v3.8+ (`server`, `macros`, `schemars`, `transport-io`, `transport-streamable-http-server`)** directly on top of `ofx-cmd`.
2. **Require every new operator node to register a `NodeSpec` alongside its implementation:** The CI pipeline will fail to compile a node that lacks parameter descriptions, pin types, and at least one automated MCP self-test recipe.

### Open Risks
- **Visual Hallucination on 2D Projections:** A single perspective viewport capture can hide back-face self-intersections or floating geometry. *Mitigation:* `viewport_capture` defaults to a **4-panel Quad View (Perspective + Top + Front + Right + Coordinate Grid + Bounding Dimensions Overlay)** plus the structured `topology_health` JSON block.

### Suggested Milestones

---
## Audit addendum — protocol and concurrency hardening

MCP `2026-07-28` has a stateless request core; correctness must not depend on a connection session. Each mutation takes `base_revision`, idempotency key, authenticated principal and declared resource budget, returning a committed revision or structured conflict. Long cooks are engine jobs on immutable snapshots, not open HTTP requests. Pin `rmcp` 3.8.0 and test stdio/HTTP independently.

If Agent A cooks revision 42 while Agent B publishes 43 and a human scrubs, A owns immutable 42 pages, B cannot mutate them, and presentation selects the newest requested revision/time. Cancellation reaches CPU, GPU fences and spill I/O; stale completion cannot overwrite a cache key that includes snapshot/time. Invalid VEX, cycle or breaker trip commits nothing. Expose read-only catalogues first; mutation, plugin loading and farm submission need separate scopes. Treat resources and generated labels as untrusted data. Sources: [MCP release](https://blog.modelcontextprotocol.io/posts/2026-07-28/) and [transport specification](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/).
- **M6.1 — `ofx-mcp` Core Server & `ofx://` Resource Tree:** Implement `rmcp` stdio + Streamable HTTP server exposing `node_catalog_search`, `node_type_inspect`, `graph_batch_edit`, and transactions.
- **M6.2 — `geo_probe` Telemetry & `Assert SOP`:** Implement $O(1)$ page-summary statistics, topology health checker (manifold/watertight/NaN checks), and declarative assertion nodes.
- **M6.3 — Visual Probe (`viewport_capture`) & Async Task Streaming:** Integrate headless `wgpu` Lavapipe/GPU 4-view contact-sheet rendering and MCP progress/cancellation streaming for sims/renders.
- **M6.4 — Agent-in-the-Loop Benchmark Suite:** Automated eval harness where an MCP client builds 20 canonical procedural assets from natural-language specs and verifies every output via `geo_assert_invariants`.
