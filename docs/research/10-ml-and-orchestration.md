# 10 — Machine Learning Integration, 3D Gaussian Splatting, and TOP/PDG Pipeline Orchestration

> **Scope:** Native Rust ML inference & training (`Burn` + `ort`), 3D/4D Gaussian Splatting (`Brush` architecture + USD splats), neural deformers/terrains/segmentation, and distributed task orchestration (`ofx-top` / PDG equivalent + ASWF OpenCue & OpenJD farm integration).

---

## 1. Machine Learning Inference & Training Engine (`ofx-ml`)

Houdini 22 integrates ONNX inference across SOPs and Copernicus (COPs) alongside TOP-orchestrated training recipes for ML deformers, neural terrains, OIDN denoisers, SAM2 segmentation, and MoGe-2 depth/normal estimation.

### 1.1 Rust ML Stack Evaluation (2026)

| Crate / Framework | URL | License | Strengths | Role in Project |
|---|---|---|---|---|
| **`burn` (v0.16+)** | [burn.dev](https://burn.dev/) | MIT / Apache-2.0 | Pure-Rust deep learning framework built directly on **`CubeCL`**! Supports both **in-process training (autodiff)** and **ONNX model import** on `wgpu`, CUDA, ROCm, Metal, and CPU with zero external C++ runtime DLLs. | **Primary In-Engine Training & GPU-Resident Inference Engine** (shares the exact same `CubeCL` / `wgpu` device buffers as `ofx-cop` and `ofx-sim`!). |
| **`ort` (ONNX Runtime v2)** | [ort.pyke.io](https://ort.pyke.io/) | MIT / Apache-2.0 | Official Rust bindings to Microsoft ONNX Runtime (`TensorRT`, `CUDA`, `DirectML`, `CoreML`, `OpenVINO`, `XNNPACK`). Supports every exotic ONNX operator, external >2GB weights, and dynamic tensor shapes. | **Tier-1 Arbitrary External `.onnx` Model Executor (`ML Inference SOP` / `ML Inference COP`).** |
| **`safetensors`** | [crates.io/crates/safetensors](https://crates.io/crates/safetensors) | Apache-2.0 | Zero-copy `mmap` weight loading without arbitrary code execution vulnerabilities (unlike Python `pickle`/`.pt`). | **Mandatory format for all native model checkpoints.** |

### 1.2 Production ML Nodes & Model Licensing Guardrails

> [!CAUTION]
> **Model Weight Licensing Audit:** Code licenses and pretrained checkpoint licenses often differ! Every built-in model downloadable via our Model Manager must be strictly **Apache-2.0, MIT, or BSD** (never CC-BY-NC non-commercial weights in commercial studio builds).

| Capability | Node | Model / Architecture | Checkpoint License |
|---|---|---|---|
| **Neural Character & Crowd Deformer** | `ML Deformer Train TOP` / `ML Deformer SOP` | Residual MLP + PCA/Autoencoder over joint pose quaternions $\rightarrow$ vertex deltas (trained in-engine via `Burn` in $<2\text{ min}$ from 500 wedged VBD/FEM poses). | Trained from user's own asset (100% user-owned). |
| **Interactive Image Segmentation** | `COP Segment Mask` | **SAM 2.1 (Segment Anything 2)** (`tiny`/`base`/`large` ONNX) driven by positive/negative 2D points or bounding boxes. | **Apache-2.0** (Meta). |
| **Monocular Depth & Normal Estimation** | `COP Depth Estimate` | **MoGe-2** / **Depth-Anything-V2-Small** *(Warning: Depth-Anything-V2-Large uses DINOv2-g CC-BY-NC; only bundle Apache-2.0 licensed weights!)*. | **MIT / Apache-2.0** verified subset only. |
| **AI Ray-Tracing Denoiser** | `COP Denoise` / `Render Denoise` | **Intel OpenImageDenoise (OIDN 2.3+)** (`hdr_alb_nrm` UNet filter) + custom `OIDN Train TOP` for show-specific volumetric/fire fine-tuning. | **Apache-2.0** (ASWF / Intel). |
| **Neural Cellular Automata & Tileable Textures** | `COP Neural CA` | Differentiable Sobel/Laplacian perception + $1\times 1$ conv residual update (Mordvintsev et al. 2020) trained in seconds via `Burn`. | **MIT / Apache-2.0**. |
| **Neural Terrain Synthesis & Super-Resolution** | `HeightField Neural Upscale` | Conditional ResNet / Diffusion ONNX inference mapping coarse erosion masks + elevation to $4\times$ high-frequency cliff/scree heightfields. | **Apache-2.0**. |

---

## 2. 3D & 4D Gaussian Splatting (`ofx-splat`)

> [!IMPORTANT]
> **Original Inria 3DGS License Warning:** The original Kerbl et al. (2023) `graphdeco-inria/gaussian-splatting` repository uses a custom **Non-Commercial** license. We **strictly forbid** any code from the Inria repo and instead build on **Apache-2.0 / MIT** clean-room implementations (**`Brush`** in Rust/Burn/wgpu and **`gsplat`** mathematical formulations).

### 2.1 Representation in `GeoDetail` & OpenUSD

A Gaussian Splat cloud in `ofx-geo` is stored as standard Point (`Pt`) attributes on a `GeoDetail` (so every SOP node—`Transform`, `Delete`, `Clip`, `AttribWrangle`, `Vellum/XPBD` cage deform—works on splats automatically!):
- `P: Vec3f` (center $\boldsymbol{\mu}$),
- `pscale` / `scale: Vec3f` (log-space or linear 3D ellipsoid scales),
- `orient: Quatf` (3D covariance rotation $\mathbf{q}$),
- `Alpha` / `opacity: f32` (logit or linear opacity $\alpha$),
- `Cd: Vec3f` + `f_rest_0..44` (Spherical Harmonics degree 0–3 coefficients),
- **USD Interchange:** Maps 1:1 to OpenUSD's `UsdVol::ParticleField3DGaussianSplat` schema and Niantic's compressed `.spz` format.

### 2.2 In-Engine Splat Training & Synthetic Splat Baking (H22 Equivalent)

1. **Multi-View Camera & Synthetic Dataset Generator (`Splat Capture TOP`):** Renders $N$ stratified hemisphere/turntable views of a complex procedural asset (or heavy Pyro/FLIP sim) through `ofx-render` with exact camera intrinsics/extrinsics and depth initialization points.
2. **Native GPU Splat Trainer (`Splat Train TOP` — `Brush` / `Burn` + `CubeCL`):**
   - Tile-based radix-sorted rasterizer + analytical backward pass in `CubeCL` (`#[cube]`) with Adam optimizer, adaptive split/clone/prune densification, and $\mathcal{L}_1 + \lambda \mathcal{L}_{\text{D-SSIM}}$ loss.
   - Trains entirely inside our Rust binary on Vulkan/Metal/DX12/CUDA with zero Python/Conda setup required!
3. **Deforming & Relightable Splats:**
   - **Cage / Skeleton Deformation:** Covariance rotation and stretch updated via local deformation gradient $\mathbf{F}$ of an embedding tetrahedral cage (`TetEmbed`) or `LinearBlendSkinning`: $\boldsymbol{\Sigma}' = \mathbf{F} \boldsymbol{\Sigma} \mathbf{F}^T$.
   - **Ray-Traced Relighting (`ofx-render` 3DGRT):** Evaluates normal approximations and optional PBR roughness/albedo splat attributes inside the wavefront path tracer.

---

## 3. Task Orchestration & Farm Pipeline (`ofx-top` — PDG / TOPs Equivalent)

In production studios, Houdini's **PDG / TOPs (Procedural Dependency Graph)** is the backbone that runs parameter wedges, caches simulations in parallel, bakes textures, converts USDZ assets, and submits 10,000-task jobs to the render farm.

### 3.1 Core `ofx-top` Architecture

```
[Wedge TOP (25 variations)] ──► [ROP Sim Cache TOP (Frame 1..240)] ──► [Karma Render TOP] ──► [FFmpeg / ImageMagick Mosaic TOP]
```

1. **Work Items (`WorkItem`):** Each node in a TOP graph generates or transforms lightweight `WorkItem` records carrying:
   - `id: WorkItemId`, `index: u32`, `frame_range: Option<FrameRange>`,
   - `attributes: BTreeMap<Symbol, TopValue>` (typed floats, ints, strings, file paths, JSON blobs),
   - `expected_outputs: Vec<ArtifactPath>` with **content-addressed cache signatures (`blake3`)**.
2. **Static vs Dynamic Work-Item Generation:**
   - **Static Partitioning:** Known before execution (e.g., 5x5 parameter wedge $\times$ 100 frames).
   - **Dynamic Generation:** Discovered at runtime after an upstream work item finishes (e.g., `FilePattern TOP` scanning newly fractured chunk files on disk).
3. **Incremental Cache-Hit Skipping (`Skip Existing`):**
   - If a work item's input hash and expected output file digest match the content-addressed manifest on disk, the work item transitions immediately to `CachedComplete` in $0\text{ ms}$, allowing failed 1,000-item farm jobs to resume cleanly from the exact point of failure.

### 3.2 Pluggable Schedulers & Industry Farm Standards

| Scheduler Backend | Target Environment | Specification / Protocol |
|---|---|---|
| **`LocalScheduler`** | Artist Workstation / CI | Async `tokio` process/thread pool with CPU core, RAM (GB), and GPU VRAM slot semaphores so 4 concurrent GPU sims don't OOM a 24 GB card. |
| **`OpenJDScheduler`** | **AWS Deadline Cloud & Open-Standard Farms** | Emits **Open Job Description (OpenJD)** YAML/JSON templates ([github.com/OpenJobDescription](https://github.com/OpenJobDescription/openjd-specifications), Apache-2.0)—the vendor-neutral specification backed by AWS and ASWF. |
| **`OpenCueScheduler`** | **On-Prem Studio Render Farms** | Integrates with **ASWF OpenCue** ([opencue.io](https://www.opencue.io/), Apache-2.0) via gRPC (`cuebot` protobuf definitions compiled with `tonic`). |
| **`WorkerPoolScheduler`** | Lightweight Distributed Cluster | Built-in zero-config Rust worker daemon (`ofx-worker --connect <coordinator>`) over QUIC / mTLS for indie teams without a dedicated farm manager. |

### 3.3 MCP Integration for Autonomous Agent Wedging

Because `ofx-top` is wired directly into `ofx-cmd` and `ofx-mcp`:
- An AI agent can create a 9-variant `Wedge TOP` varying FLIP viscosity and surface tension, call `top_execute`, receive a single 3x3 mosaic contact-sheet image via `viewport_capture`, pick the best physical parameters visually + quantitatively, and promote that wedge to final high-res simulation!

---

## 4. Mandatory Validation Suite

1. **DAG Fault-Injection & Resumability Test:** Run a 50-item multi-stage TOP graph where 5 random worker processes are killed (`SIGKILL`); verify retry policy, checkpoint recovery, and that re-running the graph skips all 45 already-completed items with $100\%$ cache hits.
2. **Deterministic Inference & Splat Parity:** Verify `ML Inference` ONNX identity/Sobel filters match exact analytical tensors across CPU and GPU; verify 3DGS forward/backward pass gradients against finite-difference numerical gradients ($\|\nabla_{\text{analytic}} - \nabla_{\text{fd}}\| < 10^{-4}$).

---

## 5. Recommendations, Open Risks, and Milestones

### Recommendations
1. **Use `Burn` + `CubeCL` for native training/splats and `ort` for external ONNX inference.**
2. **Target OpenJD (Open Job Description) and ASWF OpenCue** as the primary farm specifications.

### Open Risks
- **VRAM Fragmentation Between Simulation, Rendering, and ML Runtimes:** Running `ort` (CUDA/DirectML) alongside `wgpu` can double-allocate GPU memory pools. *Mitigation:* Default ONNX execution provider to `wgpu` / `Burn` in-process, or run heavy external `ort` training inside isolated TOP worker subprocesses that release VRAM on task exit.

### Suggested Milestones

---
## Audit addendum — model and farm provenance

`ort` 2.0.0-rc.15, Burn and CubeCL require version/capability gates; ONNX Runtime's native binary and execution provider are reproducibility inputs. Record model hash, source/license, preprocessing, provider, precision and outputs; reject unknown/non-commercial assets from distributions. Queue immutable work items with graph/input hashes, container digest, schema version, limits and retry policy. Publish outputs by compare-and-swap content key; farm adapters never receive ambient MCP credentials.
- **M10.1 — `ofx-top` DAG Engine & `LocalScheduler`:** Static/dynamic work items, `Wedge`, `FilePattern`, `PythonScript`, `RopFetch`, `WaitForAll`, `PartitionByAttribute`, and content-addressed cache skipping.
- **M10.2 — `OpenJDScheduler` & `OpenCueScheduler`:** Export and live monitoring of OpenJD job bundles and OpenCue gRPC submissions.
- **M10.3 — `ofx-ml` ONNX Inference & OIDN Denoising:** `ML Inference SOP/COP`, OIDN 2.3 denoiser integration, and `ML Deformer` training recipe.
- **M10.4 — `ofx-splat` 3D Gaussian Splat Pipeline:** `.ply`/`.spz`/USD splat I/O, SOP attribute manipulation, and `Burn`/`CubeCL` native splat trainer.
