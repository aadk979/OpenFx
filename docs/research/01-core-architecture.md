# 01 — Core Engine Architecture

> **Scope:** Node-graph execution engine, in-memory geometry representation (`GeoDetail`), multithreading & determinism, out-of-core caching, undo/redo, scene serialization, plugin ABI, and the root command bus that unifies UI, CLI, Python, and MCP.

---

## 1. Procedural Node-Graph Execution Model

### 1.1 Comparative Analysis of Existing Engines

| System | Evaluation Model | Caching & Dirty Propagation | Strengths | Weaknesses / Lessons |
|---|---|---|---|---|
| **SideFX Houdini** (`OP_Network` / `SOP_Node`) | Pull-based (lazy evaluation from display/output flag) with upward dirty propagation when parameters or upstream inputs mutate. Context-specialized (`SOP`, `DOP`, `LOP`, `COP`, `TOP`, `CHOP`, `APEX`). [SideFX HDK Docs](https://www.sidefx.com/docs/hdk/_h_d_k__op_basics__overview.html) | Per-node output cache keyed by cook serial + time dependency flag (`OP_Parameters::isTimeDependent`). Copy-on-Write (`GA_Attribute` page sharing) avoids duplicating unmodified attribute arrays across SOP cooks. | Battle-tested on 100M+ element scenes; explicit separation of geometry (`SOP`), stateful simulation (`DOP`), USD (`LOP`), and task graphs (`TOP`). | Legacy `OBJ`/`SOP` split creates transform confusion; global locks in older HDK paths; stateful Python/HScript side effects can bypass cache invalidation. |
| **Gaffer** (`Gaffer::ValuePlug`, Cortex) | Pure pull-based functional evaluation keyed by cryptographic content hashes (`MurmurHash3` over upstream plug hashes + context variables). [Gaffer Architecture](https://www.gafferhq.org/documentation/1.4.0.0/WorkingWithTheScriptingAPI/TheNodeGraph/index.html) | Global LRU memory pool of immutable `IECore::Object` instances indexed by 128-bit `IECore::MurmurHash`. | Identical subgraphs or repeated frames hit cache automatically without explicit node state; thread-safe concurrent evaluation across contexts/frames. | Pure pull without stateful zones makes iterative time-stepped simulations awkward without external cache files. |
| **Blender Geometry Nodes** | Hybrid lazy-function graph + **Fields** (deferred closures evaluated in the context of a target geometry domain) + explicit **Simulation Zones** & **Repeat Zones**. [Blender Developer Docs](https://developer.blender.org/docs/features/objects/geometry_nodes/) | Socket value logging and reference-counted `GeometrySet` instances (`Mesh`, `Curves`, `PointCloud`, `Volume`, `Instances`). | Fields eliminate temporary attribute clutter for simple math; unified `GeometrySet` container cleanly holds mixed types. | Lack of a dedicated USD authoring or farm-orchestration graph inside the same paradigm. |
| **Graphite** (`graphene` Rust engine) | Compile-time & runtime node graph lowered to composable functional pipelines in Rust. [Graphite Repo](https://github.com/GraphiteEditor/Graphite) | Type-erased node registry with memoization footprints. | Clean Rust type-driven node definitions via procedural macros. | 2D-focused; lacks 3D attribute page sharing and massive out-of-core volume caches. |
| **Salsa** (`salsa-rs`) | Demand-driven incremental computation with automatic dependency tracking, durability tiers, and early-cutoff optimization (if a recomputed node produces an output equal to its previous revision, downstream dependents are not re-evaluated). [Salsa Book](https://salsa-rs.github.io/salsa/) | Revision-numbered memo tables with red-green verification algorithm. | Eliminates manual dirty-flag bugs; early cutoff prevents cascading cooks when an upstream edit doesn't change resulting geometry hashes. | Built for compiler-sized values; large geometry payloads must be stored out-of-band as content-addressed handles (`Arc<GeoDetail>` with precomputed `blake3` hashes) so Salsa compares 32-byte digests in $O(1)$. |

### 1.2 Recommended Execution Architecture: Content-Addressed Pull Graph + Explicit Stateful Zones

We adopt a **Gaffer + Salsa hybrid** for stateless contexts (`SOP`, `LOP`, `COP`) combined with **explicit stateful solver zones** (`DOP` / Simulation Zones) and **work-item DAGs** (`TOP`):

1. **Evaluation Context (`EvalContext`):** An immutable key passed down during pull evaluation containing `(frame: f64, sample_rate: f64, substep: u32, lod: LodTier, custom_vars: SmallVec<[(Symbol, Value); 4]>)`.
2. **Two-Phase Cook (Hash $\rightarrow$ Compute):**
   - **Phase 1 (`compute_hash`):** Recursively computes a 256-bit `blake3` hash from `(node_type_id, node_version, evaluated_parameter_values, input_output_hashes, relevant_context_keys)`. If a node does not read `@Frame` or `@Time`, `frame` is omitted from its hash key (automatic time-independence detection).
   - **Phase 2 (`cook`):** Queries the global `CacheManager` by the Phase 1 digest. On cache hit, returns `Arc<CookOutput>` in $O(1)$. On miss, acquires a task slot on the scheduler, executes the operator's `cook(&CookInput, &CancellationToken) -> Result<CookOutput, CookError>`, verifies invariants in debug/CI builds, inserts into the LRU/memory-budgeted cache, and returns.
3. **Early Cutoff:** After `cook` produces a `GeoDetail`, its structural digest (computed from the per-attribute page digests) is compared against the previous revision's output digest. If identical (e.g., a `Delete` node whose bounding box touched zero points), downstream nodes skip cooking entirely.
4. **Progressive & Interruptible Cooking:** Every `cook` receives a cooperative `CancellationToken` (checked per attribute page / solver substep). If a user or agent scrubs the timeline or mutates a parameter mid-cook, in-flight tasks abort within $<5\text{ ms}$ without corrupting cached state (since outputs are immutable until committed to the cache).

---

## 2. In-Memory Geometry Data Model (`GeoDetail`)

To match Houdini's `GA_Detail` / `GU_Detail` scalability while exploiting Rust's ownership and SIMD ecosystem, `GeoDetail` uses a **paged, copy-on-write Structure-of-Arrays (SoA)** layout.

### 2.1 Topological Domains & Element Indexing

A `GeoDetail` has four attribute domains:

| Domain | Houdini Equivalent | Description | Indexing |
|---|---|---|---|
| **Point (`Pt`)** | `GA_ATTRIB_POINT` | 3D spatial locations (`P: Vec3f` required) shared by one or more vertices. | `PointIdx(u32)` mapped via sparse/dense `IndexMap` |
| **Vertex (`Vtx`)** | `GA_ATTRIB_VERTEX` | Corners of primitives; each vertex references exactly one `PointIdx`. | `VertexIdx(u32)` + CSR offset table per primitive |
| **Primitive (`Prim`)** | `GA_ATTRIB_PRIMITIVE` | Polygons, PolySoup, Bezier/NURBS curves, Volumes (`VdbGrid`), Packed Primitives / Instances, Tetrahedra, Sphere/Tube primitives, Agent primitives. | `PrimIdx(u32)` |
| **Detail (`Det`)** | `GA_ATTRIB_DETAIL` (Global) | Single tuple per `GeoDetail` (metadata, solver state, bounding metrics). | Singleton (`0`) |

### 2.2 Paged Copy-on-Write Attribute Columns (`PagedColumn<T>`)

Copying a 50-million-point mesh to add a single `Cd` (color) attribute or move 1,000 points in a soft-selection must not clone 600 MB of `P`, `N`, and `uv` buffers.

```
AttributeColumn<T>
 └── Arc<Vec<Arc<Page<T>>>>     (Page size = 16,384 elements = 2^14; 192 KB for Vec3f)
       ├── Page 0: [T; 16384] + AtomicU64(blake3_page_hash) + PageFlags(CONSTANT | UNIFORM)
       ├── Page 1: [T; 16384]   <-- shared via Arc across upstream & downstream GeoDetails!
       └── Page K: [T; N]
```

- **Page-Level CoW (`Arc::make_mut`):** Modifying a single point's position only clones the 192 KB page containing that point; all other pages remain shared across the node graph.
- **Constant-Page Compression:** If all 16,384 elements in a page share the same value (e.g., default `pscale = 1.0` or `Alpha = 1.0`), the page stores a single scalar `T` with `PageFlags::CONSTANT` and zero heap allocation for the array.
- **Content Hashing per Page:** Each page caches a lazy 64-bit `blake3` digest. The digest of an entire `AttributeColumn` is the hash of its page digests, making whole-geometry hashing after a localized edit essentially instantaneous.
- **Supported Storage Types (`AttrType`):** `F16`, `F32`, `F64`, `I8`, `I16`, `I32`, `I64`, `U8`, `U32`, `Bool`, fixed-size tuples (`Vec2f`, `Vec3f`, `Vec4f`, `Quatf`, `Mat3f`, `Mat4f`), `Symbol` (32-bit interned string handle backed by a lock-free `lasso::ThreadedRodeo` interner), and variable-length per-element arrays (`ArrayAttr<T>` stored as a flat values column + CSR `u64` offset column, identical to Apache Arrow's `ListArray`).
- **Zero-Copy Interop:** Page buffers are 64-byte aligned (cache-line & AVX-512 aligned) and implement `bytemuck::Pod`, allowing zero-copy upload to `wgpu` storage buffers, zero-copy views into Python `numpy.ndarray` via buffer protocol, and zero-copy conversion to Apache Arrow record batches.

### 2.3 Primitive Soup & Heterogeneous Containers

Rather than forcing every polygon into a heavy heap object, `GeoDetail` groups primitives into homogeneous **Primitive Runs** (`PrimRun`):
- **`PolyRun`:** Contiguous range of polygons stored as `(vertex_offsets: PagedColumn<u32>, vertex_to_point: PagedColumn<PointIdx>)`. Fixed-valence runs (pure triangles or pure quads) omit the `vertex_offsets` array entirely and set a constant stride (`3` or `4`), cutting topology memory by 25% and enabling direct index-buffer binding in the viewport.
- **`CurveRun`:** Polyline, Bezier, Catmull-Rom, or NURBS curves with basis metadata and optional knot vectors.
- **`TetRun`:** 4-point tetrahedral elements for FEM and Otis-equivalent muscle solvers.
- **`VolumePrim`:** Holds an `Arc<VdbGrid>` (sparse OpenVDB/NanoVDB hierarchy) or `Arc<HeightfieldLayer2D>` plus an affine index-to-world transform.
- **`PackedPrim` (Instancing):** Lightweight reference `(child_detail: Arc<GeoDetail>, transform: Affine3A, pivot: Vec3, lod_display: PackedDisplayMode)` enabling millions of nested instances with $O(1)$ memory per instance.

### 2.4 Groups (`ElementGroup`)

Named selections on Points, Vertices, Primitives, or Edges:
- Point/Vertex/Prim groups use a **paged roaring bitmap** (`roaring::RoaringBitmap` or custom per-page `[u64; 256]` bitblocks aligned with the 16,384-element attribute pages) so SIMD kernels can mask active elements per page using a single `u64` bitmask load.
- Edge groups store sorted pairs `(min(PtA, PtB), max(PtA, PtB))` in a compact hash set / sorted slice.

---

## 3. Multithreading, SIMD, and Determinism

Production pipelines require that **cooking the same scene on a 16-core workstation and a 128-core farm blade produces bit-identical caches** (or bounded tolerance on heterogeneous GPUs).

1. **Two-Level Parallelism (`rayon` + DAG Scheduler):**
   - **Graph-level:** Independent branches of the node graph and independent TOP work items execute on an async work-stealing task coordinator.
   - **Data-level:** Inside a single operator cook, element-wise loops iterate over the 16,384-element pages via `rayon::prelude::IndexedParallelIterator` with fixed chunking per page (`par_iter_mut` over pages). Because page boundaries are independent of thread count, any per-page work is invariant to the number of worker threads.
2. **Deterministic Reductions & Spatial Hashing:**
   - Parallel reductions (bounding boxes, attribute sums, centroids) reduce within each fixed 16,384-element page first, then combine page results in strict deterministic page-index order (`0..num_pages`). This guarantees **bit-identical IEEE-754 floating-point results regardless of thread scheduling**.
   - Parallel spatial grids and hash tables sort keys by `(cell_id, element_id)` via deterministic radix sort before building cell ranges—never relying on non-deterministic atomic insertion order.
   - Random number generation in nodes/solvers uses stateless counter-based PRNGs (`Philox4x32-10` or `ChaCha8` keyed by `(node_seed, frame_bits, element_id)`), never thread-local stateful RNGs.
3. **Portable SIMD:** Use the `pulp` (MIT/Apache-2.0) and `wide` (Zlib/Apache-2.0/MIT) crates with runtime CPU feature dispatch (`AVX2`, `AVX-512`, `NEON`) over the contiguous 16,384-element pages.

---

## 4. Memory Budgeting & Out-of-Core Caching

A single 500-frame FLIP or Pyro simulation easily exceeds 128 GB of RAM.

- **Unified `MemoryArena` & Budget Enforcer:** Every `Page<T>`, `VdbGrid`, and image tile registers its byte footprint with a central `MemoryBudgetManager` (default limit: 75% of physical RAM).
- **Spill-to-Disk Tier:** When cache pressure exceeds the high-water mark, least-recently-used pages not currently pinned by an active cook are compressed with **`lz4_flex`** (for fast warm spill, $>4\text{ GB/s}$) or **`zstd`** (for cold frame caches) and written to an append-only memory-mapped scratch file (`memmap2`). Evicted pages replace their `Arc<Page<T>>` payload with a lightweight `SpilledPageHandle(file_offset, compressed_len, blake3_hash)` that transparently faults back into RAM on demand.
- **Native Geometry Cache Format (`.ofxg` / `.bgeo.sc` interop):** Frame caches write the paged SoA columns directly with a FlatBuffers/rkyv header + `zstd`-compressed 64-byte-aligned attribute pages, allowing `mmap` zero-copy reads when uncompressed.

---

## 5. Scene Format, Parameters, Digital Assets, and Undo/Redo

### 5.1 Text-Diffable Scene Format (`.ofxs`)

Binary `.hip` files are notorious in production because they cannot be diffed or merged in Git. Our native scene file (`.ofxs`) is a **deterministic, human-readable RON / TOML-like declarative format** (or directory bundle `.ofxs.d/` for multi-department shots) where:
- Every node, subnet, parameter override, and wire connection is serialized in canonical sorted order by stable `NodeId` / path (`/obj/geo1/scatter1`).
- Non-default parameter values are stored sparsely (defaults come from the versioned Node Definition schema).
- Heavy binary payloads (painted attribute strokes, locked geometry stashes) live in a sibling `assets/<blake3_hash>.ofxg` content-addressed blob directory so the graph file stays small and Git-mergeable.
- Schema version migrations are pure functions `migrate_v1_to_v2(SceneAst) -> Result<SceneAst>` tested in CI against golden scene corpora.

### 5.2 Parameter System & Digital Assets (`.ofxa` / HDAs)

- **Parameter Types:** `Float(1..4)`, `Int(1..4)`, `Bool`, `String`, `FilePath`, `NodePath`, `ColorRgba`, `Ramp(Spline/Linear)`, `Menu/Enum`, `Button/Action`, `MultiparmList` (dynamic repeating parameter blocks).
- **Evaluation Stack:** A parameter slot holds either a literal value, an animation curve (`ChannelSpline`), a fast parameter expression, or a link (`ch("../other_node/parm")`). All references register directed dependency edges in the graph engine.
- **Digital Assets (`.ofxa`):** Any subnetwork can be packaged into a versioned asset definition (`namespace::asset_name::2.1.0`) containing:
  1. Exported parameter interface schema (JSON Schema + UI layout hints),
  2. Internal immutable node graph template,
  3. Embedded VEX-equivalent kernels / scripts,
  4. Embedded self-test cases (input fixtures + expected output invariants).

---

## 6. Plugin & Extension Architecture

Rust has no stable ABI across compiler versions. We support two complementary extension tiers:

1. **Tier 1 — Sandboxed Portable Plugins (WebAssembly Component Model via `wasmtime`):**
   - Custom nodes, procedural generators, and third-party studio tools compile to **WASM Components** against a WIT (WebAssembly Interface Types) definition of the `GeoDetail` and `EvalContext` APIs.
   - **Why:** Deterministic, cross-platform single binary (`.wasm`), immune to `rustc` version mismatches, and completely sandboxed (cannot segfault the host or exfiltrate studio assets unless granted capability handles).
2. **Tier 2 — Native High-Performance Plugins (`abi_stable` / C ABI):**
   - For studio plugins requiring direct GPU device handles or custom C++ solver libraries, `ofx-plugin-abi` exposes a `#[repr(C)]` vtable verified at load time via `abi_stable` (MIT/Apache-2.0) with strict ABI version handshakes.

---

## 7. The Root Command Bus (Foundation for MCP, UI, CLI, and Python)

To satisfy the requirement that **MCP is integrated at the root rather than a surface wrapper**, **no subsystem (including the native GUI) is permitted to mutate the `SceneGraph` directly**.

```
┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐
│ Desktop GUI │  │  CLI / Batch│  │ Python (HOM)│  │   MCP Server (rmcp)     │
└──────┬──────┘  └──────┬──────┘  └──────┬──────┘  └────────────┬────────────┘
       │                │                │                      │
       ▼                ▼                ▼                      ▼
┌────────────────────────────────────────────────────────────────────────────┐
│                     Typed Command & Query Bus (ofx-cmd)                    │
│  • Every command implements: Command { Input, Output, Error, inverse() }   │
│  • Auto-derives JSON Schema (schemars 2020-12) for MCP Tools & Docs        │
│  • Transactional batching, Event Log, Undo/Redo stack, Dry-Run validation  │
└────────────────────────────────────┬───────────────────────────────────────┘
                                     ▼
┌────────────────────────────────────────────────────────────────────────────┐
│               Core Engine (ofx-graph, ofx-geo, ofx-eval)                   │
└────────────────────────────────────────────────────────────────────────────┘
```

Every state mutation (`CreateNode`, `DeleteNode`, `ConnectPins`, `SetParamValue`, `SetKeyframe`, `PackageAsset`, `TriggerCook`) is a struct implementing:

```rust
pub trait EngineCommand: Serialize + DeserializeOwned + JsonSchema + Send + Sync + 'static {
    const ID: &'static str;
    const SUMMARY: &'static str;
    type Output: Serialize + DeserializeOwned + JsonSchema + Send + Sync;
    
    fn validate(&self, state: &SceneState) -> Result<(), CommandError>;
    fn apply(&self, state: &mut SceneState) -> Result<(Self::Output, UndoOp), CommandError>;
}
```

- **Undo/Redo for Free:** Every `apply` returns a reversible `UndoOp` (command-pattern delta). Compound user/agent actions wrap multiple commands in a `TransactionId` so an entire multi-step agent operation can be rolled back atomically with a single Undo or if any step fails validation.
- **Deterministic Replay & Bug Reproduction:** Because every mutation flows through `EngineCommand`, any session can record an append-only `.ofxj` command journal that reproduces crashes or simulation discrepancies deterministically in CI.

---

## 8. Recommended Workspace Crate Decomposition (Core Layer)

| Crate | Responsibility | Unsafe Allowed? |
|---|---|---|
| `ofx-id` | Interned symbols (`lasso`), generational arenas, deterministic UUIDs, node paths | `#![forbid(unsafe_code)]` |
| `ofx-math` | SIMD math types (`glam`, `wide`), deterministic reductions, bounding volumes, counter PRNG (`Philox`) | `#![forbid(unsafe_code)]` |
| `ofx-geo` | `GeoDetail`, `PagedColumn<T>`, `PrimRun`, `ElementGroup`, topology indices, attribute hashing | Isolated to SIMD/aligned page alloc; Miri-verified |
| `ofx-vdb` | Sparse volumetric grids, NanoVDB interop, level-set primitives, 2D heightfield layers | Isolated to zero-copy buffer casts |
| `ofx-graph` | Node, Pin, Subnet, Parameter types, channel splines, `.ofxs` serializer/deserializer | `#![forbid(unsafe_code)]` |
| `ofx-eval` | Pull evaluator, `blake3` cook cache, `MemoryBudgetManager`, spill-to-disk, `CancellationToken` | `#![forbid(unsafe_code)]` |
| `ofx-cmd` | Root `EngineCommand` trait, registry, transaction manager, undo/redo log, schema generator | `#![forbid(unsafe_code)]` |

---

## 9. Recommendations, Open Risks, and Milestones

### Recommendations
1. **Adopt `glam` + `wide` + `pulp`** for math and SIMD, **`blake3`** for page and cook-cache hashing, **`roaring`** for sparse element groups, and **`schemars`** for root command schema generation.
2. **Enforce page-granularity (16,384 elements) everywhere** in `ofx-geo` from commit #1 so copy-on-write, hashing, out-of-core spilling, and deterministic parallel reductions all share one unit of work.
3. **Forbid direct `SceneState` mutation outside `ofx-cmd`** via Rust module visibility rules.

### Open Risks
- **Topology Mutation Overhead:** While modifying point attributes on a 50M-point mesh is $O(\text{modified pages})$, operations that insert/delete arbitrary polygons require rebuilding `PrimRun` offset tables. *Mitigation:* Support lazy tombstoning (`deleted_pts: RoaringBitmap`) compacted only at explicit `Clean` boundaries or before export/rendering.
- **Fine-Grained Locking vs Throughput:** High-frequency viewport scrubbing while an agent edits upstream nodes. *Mitigation:* `SceneState` uses `ArcSwap` / persistent data structures so readers (cooking threads, viewport, MCP resource queries) snapshot an immutable root pointer without blocking writers.

### Suggested Milestones

---

## Audit addendum — normative kernel, stateful evaluation, and safety limits (2026-10-31)

This addendum is normative where it conflicts with earlier aspirational prose. `ofx-*` remains the **temporary** crate prefix pending the naming decision in `04`; it is not related to the ASWF OpenFX image-effect ABI.

### Canonical data types

```rust
pub const PAGE_LEN: usize = 16_384;
pub const PAGE_ALIGN: usize = 64;

pub struct Page<T: Pod + Zeroable> {
    pub len: u16,                    // 0..=PAGE_LEN; trailing bytes are zeroed
    pub generation: u64,
    pub hash: blake3::Hash,
    pub bytes: Arc<AlignedBytes<T>>, // allocation alignment >= max(64, align_of::<T>())
}
pub struct PagedColumn<T: Pod + Zeroable> {
    pub default: T,
    pub len: u64,
    pub pages: Arc<[Arc<Page<T>>]>,
}
pub struct PrimRun { pub kind: PrimKind, pub first: u64, pub count: u64,
                     pub index_offset: u64, pub index_count: u64, pub flags: u32 }
pub struct GeoDetail {
    pub points: Domain, pub vertices: Domain, pub primitives: Arc<[PrimRun]>,
    pub corner_point: PagedColumn<u64>, pub prim_indices: PagedColumn<u64>,
    pub attrs: AttributeStore, pub groups: GroupStore, pub metadata: DetailMetadata,
    pub revision: RevisionId,
}
pub struct TopologyIndex { pub point_to_corners: Csr<u64>, pub point_to_prims: Csr<u64>,
                           pub edge_to_corners: RobinHoodMap<EdgeKey, SmallVec<[u64;2]>>,
                           pub source_revision: RevisionId }
pub struct EvalContext { pub snapshot: Arc<GraphSnapshot>, pub time: FrameTime,
    pub shutter: ShutterInterval, pub cancel: CancellationToken, pub budget: CookBudget,
    pub seed: u64, pub cache: Arc<Cache>, pub diagnostics: DiagnosticSink }
pub struct CookOutput { pub value: Arc<Value>, pub cache_key: blake3::Hash,
    pub deps: Arc<[DependencyFingerprint]>, pub stats: CookStats, pub complete: bool }
pub enum EngineCommand { Graph(GraphEdit), Parameter(ParameterEdit), Asset(AssetEdit),
    Timeline(TimelineEdit), Transaction(TransactionCommand) }
pub struct WorkItem { pub id: WorkId, pub graph_revision: RevisionId, pub node: NodeId,
    pub inputs: Arc<[CacheKey]>, pub context: EvalContext, pub priority: Priority }
```

`Pod` inputs must reject NaN, infinity, invalid UTF-8 and denorm-sensitive nondeterministic reductions at ingress. Empty domains are valid. A polygon with fewer than three distinct points, a zero-area triangle, or an index outside its domain is represented but rejected by algorithms that require a surface; it must never panic or silently fabricate a normal. Page hashes cover canonical little-endian bytes, `len`, type tag and schema version—not pointer values. A 16,384-element `f32x3` position column is 196,608 B/page (plus allocation metadata); a `u64` column is 131,072 B/page. Do not promise 64-byte *size* alignment: only the allocation start is 64-byte aligned, and SIMD loads must use unaligned-safe paths at page tails.

### Evaluation semantics: loops, feedback, compile blocks

The graph is a DAG **except inside an explicitly typed `StateZone`**. A regular cycle is a validation error with the complete node/port cycle reported before it can commit. `ForEach` lowers to deterministic child cooks keyed by `(body_graph_hash, iteration_domain_hash, ordinal, feedback_hash)`; pieces are sorted by canonical attribute bytes then original stable element id. `ForEachFeedback` and `SolverSOP` are StateZones with an initial-state input, monotonic integer tick, checkpoint key, and an explicit `prev_frame` output. They may only read committed tick `n-1`, never a live mutable value. Frame seeking restores the latest checkpoint `<= n` then replays fixed ticks.

A `CompileBlock` is an immutable, side-effect-free subgraph specialization. Its ABI is typed page/span inputs and outputs; compilation is invalid if it contains state, time-dependent hidden reads, topology-changing unknown bounds, Python, filesystem/network I/O, or nondeterministic atomics. It is an optimization, never a semantics change.

### Snapshots, cancellation, and circuit breakers

Commands atomically publish an immutable `GraphSnapshot` with a monotonically increasing revision. Cooks retain their starting snapshot; an upstream edit starts a new revision and cancels only obsolete, unobserved work. A UI scrub chooses the newest requested `(revision,time)` and drops stale presentation, not a partially computed result. Transaction commit compares read-set revisions; disjoint edits merge, otherwise return a machine-readable conflict (`object, field, expected, actual`)—no last-writer-wins for graph topology or parameters.

Every cook receives enforced defaults: `max_cook_memory_bytes`, `max_cook_time_ms`, `max_element_expansion_ratio`, `max_recursion_depth`, and `max_state_ticks`. Reserve memory before allocating; exceedance returns `ResourceLimit` with partial diagnostics, triggers cancellation through every child work item, and does not cache output. The limit must be inherited by VEX, plugins, GPU submission, and decompression. Phase 0 must make values configurable per trusted local project, but MCP clients cannot raise them beyond server policy.

### Shared world convention

All engine-space data is right-handed, Y-up, metres, kilograms and seconds; rotations are active right-handed transforms. USD export writes `upAxis=Y` and `metersPerUnit=1`. `FrameTime` is rational, not `f32`. Motion samples use the authored shutter interval (default `[-0.25,+0.25]` frames), with velocity in m/s and explicit time offsets. Cameras/lights carry world transforms and time samples through SOP, LOP and renderer without unit conversion; importers must declare conversion in provenance.
- **M1.1 — `ofx-geo` Paged SoA Kernel:** Create `GeoDetail` with `Point`, `Vertex`, `Prim`, `Detail` attributes, 16K pages, CoW sharing, `blake3` page hashing, and proptest suite verifying zero-copy page sharing across clones.
- **M1.2 — `ofx-graph` & `.ofxs` Round-Trip:** Implement node/subnet/parameter graph, expression dependency edges, and canonical text serialization with 100% round-trip property tests.
- **M1.3 — `ofx-eval` Content-Addressed Cook Engine:** Implement pull evaluator with early cutoff, cancellation tokens, deterministic per-page parallel reductions, and disk-spill LRU cache.
- **M1.4 — `ofx-cmd` Transactional Command Bus:** Implement the root command registry with automatic JSON Schema emission, undo/redo transactions, and journal replay verification.
