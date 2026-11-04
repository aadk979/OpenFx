# 08 — VEX-Equivalent Data-Parallel Language, Expression Engine, and Python API

> **Scope:** Designing and implementing our VEX-compatible SPMD kernel language (`ofx-vex`), shared SSA Intermediate Representation for text snippets and VOP visual graphs, Cranelift SIMD JIT + GPU backend, parameter expressions, and the Python 3.13 (`PyO3`) scripting layer.

---

## 1. Why a VEX-Compatible SPMD Language is Essential

In Houdini, `Point Wrangle`, `Primitive Wrangle`, `Volume Wrangle`, and `VOP` graphs are the "universal glue" used in almost every production shot. Technical directors and AI agents alike can write a 5-line snippet:

```c
// AttribWrangle (Run Over: Points)
float dist = length(@P - chv("center"));
@Cd = chramp("color_ramp", fit(dist, 0.0, chf("radius"), 0.0, 1.0));
if (@Cd.r < 0.1) {
    removepoint(0, @ptnum);
}
```

and have it execute safely across **50 million points in parallel across all CPU SIMD lanes or GPU threads** in milliseconds.

---

## 2. `ofx-vex` Architecture: One Shared IR for Text Snippets & Visual VOP Graphs

Both textual `Wrangle` snippets and visual `VOP` node networks lower to a single **Static Single Assignment (SSA) Data-Parallel Intermediate Representation (`VexIr`)**:

```
┌───────────────────────────┐            ┌───────────────────────────┐
│  Textual Wrangle Snippet  │            │   Visual VOP Node Graph   │
│  (logos + chumsky parser) │            │  (Graph nodes + wires)    │
└─────────────┬─────────────┘            └─────────────┬─────────────┘
              │                                        │
              └───────────────────┬────────────────────┘
                                  ▼
              ┌────────────────────────────────────────┐
              │    Typed SSA Kernel IR (ofx-vex-ir)    │
              │  • Read/Write Attribute Bindings (@P)  │
              │  • Uniform Parameter Bindings (chf)    │
              │  • Deferred Topology Command Queues    │
              └─────────┬────────────────────┬─────────┘
                        │                    │
         ┌──────────────┘                    └──────────────┐
         ▼                                                  ▼
┌─────────────────────────────────┐      ┌─────────────────────────────────┐
│  CPU SPMD-on-SIMD JIT Backend   │      │    GPU Compute Shader Backend   │
│  • Cranelift 0.115+ (x86/ARM)   │      │  • WGSL / Slang / CubeCL        │
│  • 8-wide / 16-wide vector loop │      │  • For COPs, Volumes & GPU SOPs │
└─────────────────────────────────┘      └─────────────────────────────────┘
```

### 2.1 Language Semantics & High-Value Compatibility

1. **Familiar `@attr` Syntax with Explicit Type Inference:**
   - Standard built-in bindings automatically know their types: `v@P`, `v@N`, `v@v`, `v@Cd`, `f@pscale`, `f@Alpha`, `p@orient` (`Quatf`), `i@ptnum`, `i@primnum`, `i@vtxnum`, `i@numpt`, `f@Frame`, `f@Time`, `f@TimeInc`.
   - Custom attributes support explicit prefixes (`f@heat`, `v@rest`, `u@uv`, `i[]@neighbors`, `s@name`) or infer `float` by default.
2. **Automatic Read/Write Dependency Extraction:**
   - Before cooking a `Wrangle`, `VexIr` statically analyzes which attributes are **read-only** (`@P`), which are **mutated in-place** (`@Cd`), and which are **newly created** (`f@density`).
   - Only mutated/created attribute columns trigger `PagedColumn::make_mut` page clones! Unmodified columns are bound as zero-cost read-only slice pointers.
   - All `chf("parm")`, `chi("parm")`, `chv("parm")`, `chramp("parm", u)` calls are hoisted out of the element loop as **uniform constants** and registered with the graph dependency tracker.
3. **Deterministic Deferred Topology Mutations (`addpoint`, `addprim`, `removepoint`, `setpointattrib`):**
   - Because points execute in parallel across pages, calling `addpoint` or `removepoint` inside a kernel appends to a per-page **Thread-Local Topology Command Buffer** (`PageMutationLog`).
   - At the end of the parallel kernel sweep, page mutation logs are merged in strict deterministic page order (`0..num_pages`), assigning deterministic point/primitive IDs regardless of thread scheduling!
4. **Rich Built-In Standard Library:**
   - **Math & Shaping:** `fit`, `fit01`, `clamp`, `lerp`, `smooth`, `chramp`, `sin`, `cos`, `atan2`, `qmultiply`, `qrotate`, `slerp`, `eulertoquaternion`, `ident`, `invert`, `determinant`, `eigenvalues`.
   - **Procedural Noise (Deterministic SIMD):** `noise`, `snoise`, `anoise` (alligator), `curlnoise`, `voronoise`, `flownoise` in 1D/2D/3D/4D.
   - **Spatial & Geometry Queries:** `nearpoint`, `nearpoints` (via cached `kiddo` KD-tree on input 0..3), `xyzdist`, `primuv`, `intersect` (via cached `parry3d` BVH on input 0..3), `point`, `prim`, `vertex`, `detail`, `volumesample`, `volumegradient`.

---

## 3. Compiler & Execution Stack (2026 Crate Selection)

| Component | Selected Crate | License | Rationale |
|---|---|---|---|
| **Lexer** | **`logos`** | MIT / Apache-2.0 | Zero-copy DFA lexer generating $>1\text{ GB/s}$ token streams. |
| **Parser & Error Recovery** | **`chumsky`** | MIT | Combinator parser with rich error recovery so incomplete code in the editor or from an agent still produces a partial AST + types. |
| **Diagnostics (Human & MCP JSON)** | **`miette`** / **`ariadne`** | Apache-2.0 / MIT | Emits both labeled source spans for the UI and structured JSON diagnostics (`line`, `col`, `expected_type`, `found_type`, `fix_hint`) for the `vex_compile_and_check` MCP tool. |
| **CPU JIT Backend** | **`cranelift` (`cranelift-jit`, `cranelift-module`)** | Apache-2.0 with LLVM Exception | Compiles a Wrangle snippet to native `AVX2`/`AVX-512`/`NEON` machine code in **$<1.5\text{ ms}$** ($20\times$ faster compile latency than LLVM `inkwell`), cached globally by `blake3(source_ast)`. |
| **Fast Interpreter Fallback** | Register Bytecode VM in `ofx-vex-vm` | MIT / Apache-2.0 | Used for platforms disallowing JIT (`iOS` / strict W^X sandboxes) and for Miri/fuzzing verification against the Cranelift JIT. |
| **GPU Backend** | **`naga` / `CubeCL` / WGSL** | MIT / Apache-2.0 | Lowers `VexIr` (when free of topology-adding calls) directly to a GPU compute kernel for Copernicus `Pixel Wrangle` and GPU Volume Wrangles. |
| **LSP Server** | **`tower-lsp`** | MIT / Apache-2.0 | Provides autocomplete, hover documentation for all 200+ VEX built-ins, and live `@attribute` schema validation (inspecting upstream node's actual attributes!). |

---

## 4. Parameter Expression Language (`ofx-expr`)

Parameter fields (`tx`, `scale`, `file_path`, `switch_index`) need ultra-fast scalar/string expressions (`$F * 0.5`, `ch("../box1/sizex") * 2.0`, `` `chs("../name")`_v`padzero(3, $F)` ``):
- Implemented as a dedicated, zero-allocation **stack bytecode evaluator (`ofx-expr`)** (faster and more deterministic than embedding general-purpose Lua/Rhai per parameter).
- Supports Houdini-compatible channel functions (`ch`, `chf`, `chi`, `chs`, `chv`, `bbox`, `centroid`, `npoints`, `nprims`, `point`, `detail`) and context variables (`@Frame`, `@Time`, `@Fps`, `@ Wedgenum`), while recording every referenced node/parameter path into the `ofx-eval` dependency graph during bytecode compilation.

---

## 5. Python 3.13 Scripting & Pipeline API (`ofx-py` — `hou` Equivalent)

VFX Reference Platform CY2026/CY2027 standardizes on **Python 3.13**. Our Python package (`import ofx`) is built with **`PyO3` (v0.23+)** and **`maturin`**:

1. **Auto-Generated from `ofx-cmd`:** Because all scene mutations live in the root `ofx-cmd` registry, `ofx-py` automatically wraps every command with full **`.pyi` type stubs (`pyo3-stub-gen`)** so studio pipeline scripts get 100% IDE/mypy type checking.
2. **Zero-Copy `NumPy` & `Apache Arrow` Attribute Views:**
   - Calling `geo.point_float_attrib_pages("P")` returns a list of `numpy.ndarray` views (`shape=(16384, 3), dtype=float32`) pointing directly into `PagedColumn<Vec3f>` memory via Python's Buffer Protocol (`PyBuffer`)—or a single contiguous array via `geo.point_attrib_numpy("P")`.
3. **Free-Threaded Python 3.13 (`python3.13t` / No-GIL) Support:** Declared `gil_used = false` in `PyO3` module initialization so studio Python TOP tasks can execute genuinely in parallel across threads.

---

## 6. Mandatory Validation & Differential Testing Suite

1. **Differential Execution Fuzzing (Bytecode VM vs Cranelift SIMD JIT vs WGSL GPU):**
   - `cargo-fuzz` + `proptest` generates random type-valid `VexIr` programs (arithmetic, control flow, ramps, noise, matrix ops) and executes them on a 1,000-point test `GeoDetail` across all three backends (`VM`, `Cranelift JIT`, `Lavapipe GPU`). Outputs must agree within $\le 2\text{ ULP}$ (or bit-exact between scalar VM and SIMD JIT for basic ops).
2. **Sandbox & Infinite-Loop Protection:**
   - Every back-edge (loop iteration) in `VexIr` decrements an instruction fuel counter (`max_loop_iters = 100_000` configurable per node). An infinite `while (1) {}` loop written by a user or AI agent cleanly aborts the node cook with `CookError::KernelFuelExhausted { line, col }` in $<10\text{ ms}$ instead of hanging the process!

---

## 7. Recommendations, Open Risks, and Milestones

### Recommendations
1. **Build `VexIr` + Bytecode VM first (M8.1)** so `AttribWrangle` works on Day 1, then plug in **Cranelift SIMD JIT (M8.2)** and **WGSL GPU lowering (M8.3)** behind the exact same `VexIr` interface.
2. **Never allow unbounded loops or raw pointers** in `ofx-vex`.

### Open Risks
- **Transcendental Function Drift Between CPU SIMD and GPU:** `sin`, `cos`, `pow`, `exp` hardware approximations on GPU can differ by $10^{-5}$ from CPU `sleef`/`wide` polynomials. *Mitigation:* Use identical minimax polynomial approximations (` wide` / Estrin's scheme) in both Cranelift SIMD and WGSL kernels when deterministic mode is active.

### Suggested Milestones

---
## Audit addendum — sandbox and JIT boundary

`cranelift-jit` 0.137.1 generates code; it is not a sandbox. Untrusted VEX runs in a fuelled, bounds-checked VM first; JIT is for trusted local content and still gets bounds, cancellation and allocation-free ABI. Python is out-of-process or capability-scoped; PyO3/NumPy exports are read-only snapshots until a validated transaction imports a copy. Differential fuzzing, not claimed parity, decides VM/JIT/GPU compatibility.
- **M8.1 — `ofx-expr` & `ofx-vex` Parser + SSA IR + Bytecode VM:** Full lexer, parser, `@attr` type checker, structured `miette` diagnostics, fuel-metered VM, and `AttribWrangle` / `VolumeWrangle` SOPs.
- **M8.2 — Cranelift SIMD JIT Backend & Spatial Queries:** 8-wide AVX2 / 4-wide NEON JIT compilation ($<2\text{ ms}$ compile time) + `nearpoints` / `intersect` / `xyzdist` / `volumesample` standard library.
- **M8.3 — Visual VOP Graph Frontend & GPU Lowering:** Bidirectional VOP node graph $\leftrightarrow$ `VexIr` lowering + WGSL/CubeCL compute emission.
- **M8.4 — `ofx-py` Python 3.13 SDK:** `PyO3` bindings, `.pyi` stub generation, and zero-copy `NumPy` attribute access.
