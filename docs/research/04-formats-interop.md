# 04 — Data Formats, VFX Industry Standards, and Interoperability

> **Scope:** OpenUSD, OpenVDB/NanoVDB, Alembic, OpenEXR, OpenColorIO, MaterialX, OpenPBR, glTF 2.0, FBX, Houdini `.geo`/`.bgeo` compatibility, VFX Reference Platform CY2026/CY2027 compliance, and our Houdini-Engine-equivalent C ABI (`ofx_engine_api`).

---

## 1. Critical Naming Collision Notice: "OpenFX"

> [!WARNING]
> **Project Name Collision:** "OpenFX" (OFX) is an existing Academy Software Foundation (ASWF) open standard for 2D visual effects / compositing plugins ([openeffects.org](https://openeffects.org/), `ofxCore.h`). In fact, Houdini 22's Copernicus (COP) hosts OpenFX image plugins!
>
> **Resolution:** We must **host** standard ASWF OpenFX image plugins inside our COP context while choosing a distinct public brand/binary prefix before our first public release (e.g., ** Tapestry / Vexel / Crucible / Lattice / Nodal** — see `PLAN.md`). Internally, workspace crates use a single clean prefix configurable in `Cargo.toml`.

---

## 2. VFX Reference Platform Alignment (CY2026 & CY2027)

To be adopted in studio pipelines alongside Houdini, Maya, Nuke, and Katana, our compiled libraries, Python bindings, and C/C++ FFI bridges target the finalized **VFX Reference Platform CY2026 / CY2027** specifications ([vfxplatform.com](https://vfxplatform.com/)):

| Component | CY2026 Target | CY2027 Target | Impact on Our Rust Build Pipeline |
|---|---|---|---|
| **Linux Baseline** | `glibc 2.28` (EL8), `GCC 14.2` | **`glibc 2.34` (Rocky/AlmaLinux 9)**, `GCC 14.2` | Build official Linux binaries inside a Rocky Linux 9 container (`glibc 2.34`). |
| **Windows Toolset** | Visual Studio 2022 (`v14.3x`) | Visual Studio (`v14.44+`) | `x86_64-pc-windows-msvc` on latest MSVC CRT. |
| **macOS Target** | `macOS 14.0` (Sonoma) | `macOS 14.0` (`aarch64-apple-darwin`) | Native Apple Silicon Metal/wgpu builds. |
| **C++ Standard** | **C++20** | **C++20** | All C++ FFI bridges (`cxx`) compile with `-std=c++20`. |
| **Python** | **Python 3.13** | **Python 3.13** | Target `PyO3` against CPython 3.13 ABI (including free-threaded `3.13t` support). |
| **OpenEXR / Imath** | `3.3.x` / `3.1.x` | `3.3.x+` | Pure-Rust `exr` crate + optional C++ `OpenEXRCore` for Deep EXR. |
| **OpenVDB** | `12.x` | `12.x+` | Native `.vdb` & `.nvdb` (NanoVDB) read/write. |
| **OpenUSD** | `25.x` | `26.x` | Pure-Rust `openusd` for fast stage I/O + optional `pxr` C++ bridge for Hydra delegates. |
| **MaterialX** | `1.39.x` | `1.39.x+` | XML/USD `UsdShade` MaterialX graph import/export + OpenPBR surface schema. |
| **OpenColorIO** | `2.4.x` | `2.5.x` | OCIOv2 config parser + GPU LUT/shader generation in viewport & COPs. |

---

## 3. Format-by-Format Strategy: Pure Rust vs C++ FFI

Building 15 millions lines of C++ (Boost, TBB, IlmBase, USD, VDB, OIIO) from `cargo build` breaks cross-platform compilation speed and agent ergonomics. We enforce a **Pure-Rust First, Isolated C-FFI Second** rule:

| Standard / Format | Tier | Pure-Rust Crate (2026) | License | C/C++ Fallback | Decision & Rationale |
|---|---|---|---|---|---|
| **OpenUSD (`.usda`, `.usdc`, `.usdz`)** | **Tier 1** | **`openusd` + `openusd-schemas`** ([crates.io/crates/openusd](https://crates.io/crates/openusd)) | MIT / Apache-2.0 | `rust-usd` / `hydra-rs` (`pxr` C++, Apache-2.0) | **Use pure-Rust `openusd` by default** for `.usda`/`.usdc`/`.usdz`, LIVRPS composition, `UsdGeom`, `UsdSkel`, `UsdShade`, `UsdLux`, and `ParticleField3DGaussianSplat`. Provide optional feature `pxr-bridge` for studios needing external C++ Hydra render delegates (RenderMan, Arnold). |
| **OpenVDB / NanoVDB (`.vdb`, `.nvdb`)** | **Tier 1** | **`vdb`** + **`nanovdb-rs`** ([crates.io/crates/vdb](https://crates.io/crates/vdb)) | MPL-2.0 / MIT | ASWF `openvdb` (MPL-2.0) | **Adopt `vdb` and `nanovdb-rs`** directly into `ofx-vdb` for zero-C++-dependency `.vdb` and `.nvdb` serialization and tree traversal. |
| **OpenEXR (`.exr`)** | **Tier 1** | **`exr`** ([crates.io/crates/exr](https://crates.io/crates/exr)) | BSD-3 | ASWF `OpenEXRCore` C API (BSD-3) | **Adopt pure-Rust `exr`** for multi-layer/multi-channel scanline & tiled 16-bit/32-bit float EXR and Cryptomatte AOVs; extend or wrap `OpenEXRCore` C API for Deep EXR (`deep scanline` / `deep tile`) support. |
| **glTF 2.0 (`.gltf`, `.glb`)** | **Tier 1** | **`gltf`** ([crates.io/crates/gltf](https://crates.io/crates/gltf)) | MIT / Apache-2.0 | — | **Adopt pure-Rust `gltf`**, extended with H22-equivalent `KHR_materials_*` (OpenPBR/transmission/volume/iridescence) and multi-clip skeletal/morph animations. |
| **FBX (`.fbx` ASCII & Binary)** | **Tier 1** | **`ufbx`** (Rust bindings to single-file C `ufbx.c`, [github.com/ufbx/ufbx](https://github.com/ufbx/ufbx)) | MIT | — | **Adopt `ufbx`:** zero-dependency, fuzz-hardened single-file C library with safe Rust bindings; handles all FBX versions, skinning, blendshapes, cameras, and animation curves cleanly. |
| **Houdini Geometry (`.geo`, `.bgeo`)** | **Tier 1** | Custom `ofx-io-bgeo` (`serde_json` + binary JBinary parser) | MIT / Apache-2.0 | — | SideFX publicly documents the JSON-based `.geo` schema and binary `.bgeo` token stream in the HDK (`GA_SaveMap`). Implementing a clean-room reader/writer for `.geo` / `.bgeo.sc` (Blosc/Zstd) allows drop-in cache exchange with Houdini! (`.hip` is proprietary and out of scope). |
| **Alembic (`.abc` Ogawa)** | **Tier 1** | Pure-Rust Ogawa container parser + schema layer (or `alembic-ogawa` FFI) | BSD-3 | ASWF `alembic` (BSD-3, Ogawa is ~1,500 LoC C++) | Ogawa (Alembic's modern binary container) is a simple, documented little-endian tree of byte streams and index tables. Implement a **pure-Rust Ogawa reader/writer** for `AbcGeom` (`IPolyMesh`, `ICurves`, `IPoints`, `IXform`, `ICamera`, `ISubD`). |
| **MaterialX (`.mtlx`) & OpenPBR** | **Tier 1** | Custom typed XML/graph AST (`quick-xml` + `serde`) + WGSL/Slang codegen | MIT / Apache-2.0 | ASWF `MaterialX` (Apache-2.0) | MaterialX `.mtlx` files are strictly structured XML graphs of standard library nodedefs (`standard_surface`, `open_pbr_surface`, `gltf_pbr`). Parse natively in Rust and lower to our unified Shader IR. |
| **Gaussian Splats (`.ply`, `.spz`, `.ksplat`)** | **Tier 1** | Native in `ofx-splat` (`ply-rs` + Niantic `.spz` gzipped fixed-point format) | MIT / Apache-2.0 | — | Support standard 3DGS `.ply`, compact `.spz` ($10\times$ smaller), and USD `ParticleField3DGaussianSplat`. |
| **OpenColorIO v2 (`.ocio`)** | **Tier 1** | OCIO config parser + LUT (`1D`/`3D` `.cube`/`.spi3d`) evaluator | BSD-3 | ASWF `OpenColorIO` C++ | Parse `.ocio` YAML configs and evaluate ACEScg / AgX / Filmic transforms directly in WGSL/CubeCL compute shaders. |
| **ASWF OpenFX Image Plugins (`.ofx`)** | **Tier 2** | **`ofx-host`** (`#[repr(C)]` bindings to `ofxCore.h`, `ofxImageEffect.h`) | BSD-3 | — | Pure C header ABI (zero C++ dependencies); allows loading third-party denoising/grading OpenFX plugins inside our COP context. |
| **OpenTimelineIO (`.otio`)** | **Tier 2** | Native JSON serde schema (`ofx-otio`) | Apache-2.0 | — | `.otio` is a documented JSON schema; trivial native Rust support. |

---

## 4. Embedding Engine & Stable C ABI (Houdini Engine / HAPI Equivalent)

To embed our procedural assets (`.ofxa`) inside **Unreal Engine 5, Blender, Unity, Maya, and Godot**, we expose **`libofx_engine`** via a rock-solid C ABI:

### 4.1 Design Principles (Lessons from SideFX `HAPI`)
1. **Session Abstraction (`OfxSession`):** Every API call takes `const OfxSession* session`. A session can run either:
   - **In-Process:** Direct function calls into the Rust engine inside the host process, or
   - **Out-of-Process (Thrift/IPC/Shared-Memory or MCP Streamable HTTP):** Isolates crashes, memory pressure, and GPU contexts from Unreal/Maya, while sharing geometry attribute pages via OS shared memory (`shmem` / `memmap2`) for zero-copy mesh transfer!
2. **Flat Handle-Based C API Generated via `cbindgen`:**
   - All structs across the boundary are `#[repr(C)]` (`OfxNodeId`, `OfxPartInfo`, `OfxAttributeInfo`, `OfxParmInfo`, `OfxTransformEuler`).
   - Bulk array transfers (`ofx_get_attribute_float_data`, `ofx_set_attribute_float_data`, `ofx_get_vertex_list`) operate directly on contiguous `PagedColumn<T>` slices.
3. **Host Attribute Mapping Conventions:**
   - Support Houdini-Engine-compatible special attributes (`unreal_material`, `unreal_instance`, `unity_tag`, `maya_shading_group`) alongside native asset parameters so technical artists migrating from Houdini Engine can reuse their mental model immediately.

---

## 5. Recommendations, Open Risks, and Milestones

### Recommendations
1. **Zero mandatory C/C++ build dependencies in default `cargo build --workspace`:** By using `openusd` (pure Rust), `vdb` / `nanovdb-rs` (pure Rust), `exr` (pure Rust), `gltf` (pure Rust), and `ufbx` (single C file), any developer or AI agent on Windows, Linux, or macOS can run `cargo test` in seconds without installing a 40 GB C++ VFX build tree.
2. **Isolate optional C++ bridges (`pxr-sys`, `oidn-sys`) behind opt-in Cargo features** tested in dedicated CI containers.

### Open Risks
- **USD Composition Edge Cases in Pure Rust:** Complex studio USD assets using nested Payloads + Clips + Specialized arcs. *Mitigation:* Run NIST / ASWF OpenUSD Working Group conformance test assets against `openusd` in CI and contribute upstream fixes or fallback to `pxr-bridge`.

### Suggested Milestones

---

## Audit addendum — interoperability reality check and naming decision

### No false "pure Rust" promise

The registry packages called `openusd` (0.8.0), `vdb` (0.4.0), and `nanovdb-rs` (0.0.7) are not evidence of production-complete OpenUSD/OpenVDB/NanoVDB authoring interoperability. Treat their APIs as **research-only** until format conformance and platform builds are demonstrated. Production USD composition, OpenVDB and Alembic support must be isolated behind optional ABI-stable adapters using the vendor C++ SDKs, with a pure-Rust transport/model layer that does not claim semantic parity. The adapter CI matrix must build cleanly on Windows, macOS, and Linux and explicitly records CMake/compiler/CRT/ABI requirements. `ufbx` is a Rust wrapper around the native ufbx library, so it likewise belongs in an opt-in native adapter with license notice, pinned source hash and smoke imports—not the no-native-dependency core.

OpenUSD schemas are not a blanket guarantee: `UsdSkel` and `UsdShade` are stable core schema families; a 3D Gaussian schema must be capability-negotiated. Do not hard-code `ParticleField3DGaussianSplat` as a universally available schema; use a project-owned applied API/fallback `UsdGeomPoints` representation until the target OpenUSD release and schema package are validated. MaterialX and OpenPBR are separate evolving specifications; retain the authored MaterialX document/version and lower it through a tested renderer profile rather than assuming every MaterialX graph is an OpenPBR graph.

### Packed primitives without expansion

`PackedPrim` contains `{prototype: AssetKey, local_xform, attrs, bounds, stable_id}`; it never embeds a copied mesh. SOP transforms compose local transforms, RBD simulates a proxy hull plus transform and keeps the prototype immutable, LOP interns `(prototype, material, primvars)` into a `UsdGeomPointInstancer` prototype table, and the renderer uploads one prototype BLAS per geometry/material revision plus instance transforms/custom attributes to the TLAS. Deformation or a topology-affecting SOP explicitly materializes a new prototype key; only `Unpack` creates per-face geometry. Bounds are conservative over shutter samples.

### Candidate public identities

Trademark clearance is legal work, not an assertion this audit can make. Reserve domains, search target territories/classes, and obtain counsel before selection. The candidates below deliberately avoid `OpenFX`/`OFX`; they are provisional: **Kinetara** / `kin`, **Asterforge** / `ast`, **FormaFlux** / `ffx`. Until approved, use neutral internal `pcc-*` crates in examples and state everywhere that **ASWF OpenFX/OFX** means the `ofxCore.h` / `ofxImageEffect.h` 2D image-plugin standard hosted by COP, not this engine.

### Color-management contract

Scene-linear rendering and COP computation is ACEScg unless a node declares another `ColorEncoding`. OCIO v2 configuration is versioned in the scene and its hash participates in cook/cache keys. UI chrome is display-referred sRGB; image widgets transform a linear image through the selected OCIO display/view/LUT at presentation only. Texture inputs require an explicit role: `color` (declared source → ACEScg), `data`/normal/displacement (Raw, no transfer), or already-scene-linear. EXR writes linear scene-referred pixels plus chromaticities/OCIO metadata; integer previews are explicitly display encoded. This prevents the common error of treating every EXR as ACEScg or every PNG as raw.

### Sources

- [VFX Reference Platform](https://vfxplatform.com/) — CY2026 final and CY2027 status.
- [OpenUSD](https://openusd.org/release/index.html), [OpenVDB](https://www.openvdb.org/), and [MaterialX](https://materialx.org/) — upstream APIs/specifications, not crate-name proxies.
- **M4.1 — Core Mesh & Volume I/O (`ofx-io`):** Implement `.ofxg` native cache, `.obj`, `.ply`, `.gltf`/`.glb`, `.vdb`/`.nvdb`, `.exr`, and `.geo`/`.bgeo.sc` round-trip readers/writers with property tests.
- **M4.2 — OpenUSD & MaterialX Stage Bridge (`ofx-usd`):** Integrate `openusd` + `openusd-schemas` for LOP stage authoring, `UsdGeomMesh`, `UsdGeomPoints`, `UsdGeomCurves`, `UsdVolOpenVDBAsset`, `UsdSkel`, and MaterialX `UsdShade` networks.
- **M4.3 — `ufbx` & Alembic Ogawa Interop:** Full skeletal/animated FBX import/export and Alembic `.abc` geometry cache reader/writer.
- **M4.4 — `libofx_engine` C ABI & Host Plugins:** Publish `cbindgen` C header, shared-memory out-of-process session server, and reference Blender & Unreal Engine 5 integration plugins.
