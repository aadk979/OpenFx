# 05 — GPU Compute Strategy, Production Path Tracing, Viewport, and Copernicus Image Processing

> **Scope:** Unified cross-vendor GPU compute layer (`CubeCL` + `wgpu` + `Slang`), feature-film path tracer (`ofx-karma` equivalent), 100M+ element Vulkan/WebGPU viewport, and GPU-native 2D/3D procedural image processing (Copernicus equivalent).

---

## 1. Unified GPU Compute & Shading Strategy (2026 Landscape)

Houdini's GPU execution is fragmented across legacy OpenCL, Karma XPU (OptiX/Embree), Vulkan viewport shaders, and Copernicus compute kernels. Starting fresh in Rust in 2026 allows us to build **one unified GPU compute and shading substrate (`ofx-gpu`)** shared by Simulation Solvers, Copernicus (COPs), the Viewport, and the XPU Path Tracer:

### 1.1 Evaluation of Rust GPU Technologies (2026)

| Technology | License | Backends | Strengths | Limitations | Role in Project |
|---|---|---|---|---|---|
| **`wgpu` (v24+)** | MIT / Apache-2.0 | Vulkan, DirectX 12, Metal, WebGPU | Ubiquitous, safe, supports Hardware Ray-Query (`EXPERIMENTAL_RAY_QUERY` / `RAY_TRACING_ACCELERATION_STRUCTURE`), Subgroup operations, 64-bit atomics, timestamp queries. | Slight validation overhead if dispatching thousands of tiny kernels without command-buffer batching. | **Primary Graphics, Viewport, and Portable Compute Device Layer.** |
| **`CubeCL`** ([tracel-ai/cubecl](https://github.com/tracel-ai/cubecl)) | MIT / Apache-2.0 | **CUDA, ROCm/HIP, Metal, Vulkan/SPIR-V, WGSL (`wgpu`), CPU SIMD** | Write compute kernels directly in Rust (`#[cube]`); JIT-compiles with autotuning, shared-memory tiling, warp/subgroup intrinsics, and native CUDA/ROCm peak speed when available while falling back to `wgpu` everywhere! | Compute-focused (not for raster graphics pipelines). | **Primary Simulation, COP Image Processing, and ML Compute Kernel Language.** |
| **Slang (`shader-slang`)** ([shader-slang.org](https://shader-slang.org/)) | Apache-2.0 with LLVM Exception (Khronos) | SPIR-V, DXIL, Metal (MSL), CUDA/OptiX, WGSL, CPU | Industry standard for physically based rendering shaders; generics/interfaces, automatic differentiation (`[Differentiable]`), clean MaterialX target. | Requires Slang compiler library for shader compilation. | **Primary Shading & Ray-Tracing Kernel Language for the Path Tracer & MaterialX Codegen.** |
| **`rust-gpu`** | MIT / Apache-2.0 | SPIR-V | Compiles standard Rust crates to SPIR-V. | Tied to specific `rustc` nightly toolchains; less autotuning support than CubeCL for linear algebra/solvers. | Monitor; use `CubeCL` + `Slang` instead so workspace stays on **100% stable `rustc`**. |

### 1.2 Architectural Decision: `wgpu` + `CubeCL` + `Slang` on Stable Rust

1. **Zero Nightly Compiler Lock-in:** The entire project compiles on **stable `rustc`** (`1.93+`).
2. **Write Simulation Kernels Once in `CubeCL`:** APIC P2G/G2P, MGPCG V-cycles, XPBD constraint batches, VBD vertex sweeps, and MLS-MPM transfers are written in annotated Rust (`#[cube]`). On workstations with NVIDIA GPUs, `cubecl-cuda` executes native PTX/CUDA at 100% hardware roof-line; on AMD, `cubecl-hip` or `cubecl-wgpu` (Vulkan SPIR-V); on Apple Silicon, `cubecl-wgpu` (Metal); and on headless CPU-only farm blades, deterministic `rayon` + `pulp` SIMD!
3. **Cross-Vendor Determinism Mode:** GPU atomic float additions (`atomicAdd`) are non-deterministic due to warp scheduling. When `EvalContext::deterministic_gpu` is enabled (and in all CI tests), reductions and P2G scatter operations use **deterministic fixed-order block reductions** or integer fixed-point atomics (`i64` scaled accumulator) so GPU simulations are bit-reproducible across runs.

---

## 2. Production Path Tracer (`ofx-render` — Karma CPU/XPU Equivalent)

Rather than maintaining two incompatible renderers like Karma CPU and Karma XPU, `ofx-render` uses a **Wavefront Path Tracing Architecture** (**Laine, Karras, Aila, HPG 2013**) that executes the exact same queue-based integrator on both GPU (Hardware Ray-Query / OptiX / Vulkan RT) and CPU (SIMD BVH8 + `rayon`).

### 2.1 Wavefront Integrator Stages

```
[Generate Camera Rays] ──► [Intersect Scene BVH / NanoVDB] ──► [Sort Hit Queues by Material ID]
         ▲                                                                   │
         │                 ┌─────────────────────────────────────────────────┘
         │                 ▼
[Accumulate Film AOVs] ◄── [Evaluate MaterialX / OpenPBR BSDF + Next-Event Shadow Rays + Volume Tracking]
```

### 2.2 Core Production Rendering Capabilities

| Subsystem | Technical Implementation |
|---|---|
| **Two-Level Acceleration Structure (TLAS / BLAS)** | Bottom-Level BVHs built per unique `GeoDetail` / `PrimRun`; Top-Level BVH built over `PackedPrim` instances and USD point instancers—enabling **billions of instanced polygons** with minimal VRAM. Uses hardware RT (`VK_KHR_ray_query` / DXR / Metal RT via `wgpu`) when available, plus a wide SAH/PLOC++ BVH8 traversal kernel for CPU & non-RT GPUs. |
| **Shading System (OpenPBR & MaterialX)** | First-class support for **OpenPBR Surface** and **Autodesk Standard Surface** compiled from MaterialX graphs via Slang interfaces. Energy-conserving **turquin / multiscatter GGX** microfacet BSDFs, dielectric/conductor Fresnel, thin-film iridescence, sheen, and clearcoat. |
| **Subsurface Scattering (SSS)** | ** Walk-on-Spheres / Chiang et al. (SIGGRAPH 2016) Principled Random-Walk SSS** through closed manifold meshes with chromatic mean-free-path spectrum—no pre-baked irradiance point clouds required. |
| **Curve & Hair BSDF** | **Chiang et al. (Eurographics 2016) / Marschner** cylindrical fiber BSDF ($R, TT, TRT$ lobes with azimuthal roughness and melanin/pheomelanin absorption parameterization) intersected against analytic cubic ribbon/cylinder primitives. |
| **Participating Media & Volumes (`NanoVDB`)** | **Unbiased Null-Scattering / Ratio Tracking & Residual Ratio Tracking** (Miller et al. SIGGRAPH 2019; Novák et al. 2014) over heterogeneous overlapping `NanoVDB` grids (`density`, `temperature`, `flame`, `Cd`) with Henyey-Greenstein / Dual-Lobe phase functions and physical blackbody emission (Kelvin $\rightarrow$ CIE XYZ $\rightarrow$ ACEScg). |
| **Many-Light Sampling** | ** Estevez & Kulla (HPG 2018) Importance Sampling of Many Lights with Adaptive Tree Splitting (Light BVH)** + Equi-angular / Environment Map Hierarchical Warping + Multiple Importance Sampling (MIS power heuristic $\beta = 2$). |
| **3D Gaussian Splat Ray Tracing (3DGRT)** |Following **Moenne-Loccoz et al. (SIGGRAPH Asia 2024), *3D Gaussian Ray Tracing***: wraps ellipsoidal 3D Gaussians (`ParticleField3DGaussianSplat`) in bounding icosahedra inside the BLAS, collecting sorted $k$-buffer transmittance hits along any ray so **Gaussian splats cast shadows, appear in reflections/refractions of polygonal glass, and illuminate Pyro volumes!** |
| **Production AOVs, LPEs, Cryptomatte & Deep Output** | Regular-expression **Light Path Expressions (LPE)** (e.g., `C<RD>[<L.>O]` for diffuse direct, `C<RS.*>L` for specular reflections), **Cryptomatte** (`crypto_object`, `crypto_material`, `crypto_asset` murmur3 hash-rank pairs), and **OpenImageDenoise (OIDN)** albedo/normal auxiliary buffers. |

---

## 3. High-Performance Interactive Viewport (`ofx-viewport`)

Built on `wgpu` (Vulkan / Metal / DX12) to stay fluid at 60+ FPS on scenes with **100M+ points or polygons**:

1. **Zero-Copy Attribute Page Binding:** Because `ofx-geo` stores `P`, `N`, `Cd`, `uv`, and `pscale` in 64-byte-aligned 16,384-element pages, the viewport uploads only dirty pages (tracked via `Page::blake3_hash`) directly into persistent GPU storage buffers (`StorageBuffer`) and draws via **GPU-Driven Indirect Rendering (`multi_draw_indexed_indirect_count`)** with per-page frustum and Hi-Z occlusion culling in a compute pre-pass!
2. **Visibility Buffer & Sub-Pixel Picking:** Renders a 64-bit Visibility Buffer (`(InstanceId: u24, PrimRunId: u16, PrimIdx: u24)`) enabling:
   - Instant $O(1)$ GPU mouse picking of points, edges, faces, and instances without CPU raycasts,
   - Rectangular/lasso marquee selection evaluated via a parallel compute shader directly into a `RoaringBitmap` selection group.
3. **Real-Time Viewport Modes:**
   - **Wireframe / Hidden-Line / MatCap / Flat Attribute Heatmap** (visualizing any scalar/vector attribute with customizable color ramps and vector barb/trail overlays),
   - **Lit PBR (MaterialX Realtime)** with screen-space reflections, cascaded shadow maps, and single-pass ray-marched NanoVDB volume overlays,
   - **Interactive Karma/XPU Progressive Viewport** (swaps the raster pass for the progressive Wavefront Path Tracer right inside the viewport dock).

---

## 4. GPU-Native Image Processing & Compositing (`ofx-cop` — Copernicus Equivalent)

Houdini 20.5–22 introduced **Copernicus (COPs)** to replace legacy CPU compositing with a real-time GPU 2D/3D procedural texture, slap-comp, heightfield, and Continuum simulation environment.

### 4.1 `ofx-cop` Architecture
- **Tiled Lazy GPU Graph (`512x512` tiles):** Images and 2D fields (`f16`, `f32`, `Vec2f`, `Vec4f`, `SDF2D`) are divided into $512\times 512$ tiles with explicit **Domain of Definition (DoD)** and **Region of Interest (RoI)** pull propagation.
- **Automatic Kernel Fusion:** Adjacent point-wise COP nodes (e.g., `Levels` $\rightarrow$ `ColorCorrect` $\rightarrow$ `Over` $\rightarrow$ `Remap`) are fused by `ofx-cop` into a single `CubeCL` / WGSL compute dispatch so intermediate tiles remain in GPU registers rather than round-tripping through VRAM.
- **Seamless 2D $\leftrightarrow$ 3D Bridge:** Zero-copy sharing of GPU buffers between `ofx-cop` textures, MaterialX viewport/render textures, `ofx-vdb` 2D heightfields, and ONNX ML inference nodes.

---

## 5. Mandatory Rendering & GPU Validation Suite

1. **White Furnace Energy Conservation Test:** Every BSDF lobe (`OpenPBR`, `StandardSurface`, `HairChiang`, `VolumeHenyeyGreenstein`) placed inside a uniform white environment ($L_e = 1.0$) with albedo $= 1.0$ must render to $1.000 \pm 0.002$ across all roughness values $\alpha \in [0.001, 1.0]$ (verifies zero energy loss or gain).
2. **Analytical Radiometric & Cornell Box Reference Suite:** Compare rendered spectral/RGB irradiance against analytical spherical area-light form factors and `Mitsuba 3` / `pbrt-v4` reference renders using **NVIDIA FLIP** error metric (mean FLIP $< 0.015$, PSNR $> 42\text{ dB}$).
3. **Headless Software-Vulkan GPU CI:** All `wgpu` viewport, `ofx-cop`, and path-tracer tests run on every PR in GitHub Actions on Linux/Windows using **Mesa Lavapipe (`llvmpipe` Vulkan 1.3 software ICD)** so GPU pipelines are tested even on CPU-only CI runners!

---

## 6. Recommendations, Open Risks, and Milestones

### Recommendations
1. **Standardize on `wgpu` + `CubeCL` + `Slang`** across the entire repository.
2. **Build the Wavefront Path Tracer (`ofx-render`) natively in Rust + Slang/CubeCL** while simultaneously supporting **USD Hydra Scene Index output** (`ofx-usd`) so studios can also render scenes through RenderMan, Arnold, or Cycles.

### Open Risks
- **Shader Compilation Latency:** Complex MaterialX graphs can take seconds to compile on first frame. *Mitigation:* Implement an **Uber-Shader interpreter mode** for instant $<16\text{ ms}$ interactive lookdev feedback while background threads asynchronously compile specialized Slang/WGSL PSOs keyed by material hash.

### Suggested Milestones

---
## Audit addendum — GPU claims are hypotheses, not compatibility promises

`CubeCL` 0.12.0-pre.1 and `Burn` 0.23.0-pre.1 are pre-release at audit time; pin exact revisions and put them behind a backend capability table. `wgpu` is the portable graphics/compute layer, but does not itself supply CUDA/ROCm compatibility or hardware-ray-tracing semantics. `shader-slang` 0.1.0 is an experimental binding: use a versioned external Slang toolchain and WGSL fallback. Every GPU result needs a CPU-reference tolerance test, device-loss recovery, explicit resource limits, and no floating-point atomic order in deterministic mode. Motion bounds span the canonical shutter interval in `01`; render/COP textures carry the explicit `ColorEncoding` from `04`.
- **M5.1 — `ofx-gpu` & `ofx-viewport` Core:** `wgpu` device manager, zero-copy `PagedColumn` GPU uploader, indirect page culling, visibility-buffer picking, and attribute heatmap overlays tested on Mesa Lavapipe in CI.
- **M5.2 — `ofx-cop` GPU Image & Texture Graph:** Tiled $512\times 512$ lazy evaluator, kernel fusion for point-wise ops, blurs/filters, noise generators, OCIOv2 color transforms, and EXR output.
- **M5.3 — `ofx-render` Wavefront Path Tracer (Meshes, Instances, Hair, SSS):** TLAS/BLAS builder, OpenPBR/MaterialX shading, Light BVH, Chiang hair, random-walk SSS, LPEs, and Cryptomatte; pass White Furnace & Cornell Box benchmarks.
- **M5.4 — Volumetric NanoVDB & 3D Gaussian Splat Ray Tracing:** Add null-scattering heterogeneous volume integrator, blackbody fire emission, and 3DGRT Gaussian splat intersection to `ofx-render`.
