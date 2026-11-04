# 12 — Open-Source DCC Landscape, Lessons Learned, and Desktop UI Architecture

> **Scope:** Analysis of existing open-source and commercial procedural DCCs, why past "open Houdini" attempts stalled, strategic wedge positioning, and selection of the native Rust GUI & Node Editor stack (`egui` + `wgpu`).

---

## 1. Competitive & Open-Source Landscape Analysis (2026)

| Project | License | Focus | What Worked | Limitations / Gaps |
|---|---|---|---|---|
| **Blender (Geometry Nodes & Simulation Zones)** | GPLv2+ | Generalist 3D DCC | Massive community, fast C++ refactor of `GeometrySet` & Fields, tight viewport integration. | **GPLv2+ license** prevents studios from linking proprietary C++ solver plugins or embedding the engine in commercial hosts; lacks native sparse APIC/MPM/Otis-class solvers, LOP-grade non-destructive USD composition, and PDG farm orchestration. |
| **Gaffer** (Image Engine) | BSD-3 | Lookdev, Lighting, Automation | Content-addressed `ValuePlug` cache is rock solid; powers major VFX studios (*Dune*, *Spider-Verse*). | Weak interactive 3D modeling and no built-in multiphysics simulation solvers (relies on Houdini caches). |
| **Graphite** (`GraphiteEditor`) | Apache-2.0 / MIT | 2D Procedural Vector/Raster Editor in Rust | Proved that a Rust node-graph engine can be blazingly fast, memory-safe, and cleanly architected. | Strictly 2D graphic design; web-wrapper frontend rather than a 100M-polygon 3D workstation viewport. |
| **Rerun** (`rerun-io/rerun`) | MIT / Apache-2.0 | Multimodal 3D Spatial & Time-Series Visualizer in Rust | **Proof that `egui` + `wgpu` in Rust can deliver a world-class, 60–120 FPS professional technical UI** with docking, timelines, 3D viewports, and millions of data points. | Visualization/logging tool, not a procedural authoring or simulation DCC. |
| **NVIDIA Warp / Newton** | Apache-2.0 | GPU Simulation Kernels (Python/CUDA) | Clean kernel abstractions for XPBD, VBD, MPM, and FEM. | Library/SDK only—not a visual node-based artist DCC, and heavily CUDA-centric on the fast path. |
| **Historical "Open Houdini" Attempts** (*K-3D*, *Aqsis*, *Coral*, *Praxis*, *Bifröst-clones*) | Various | Procedural 3D | Ambition. | **Why they stalled:** Started by building giant C++/Qt GUI shells before having a working headless geometry kernel, deterministic test harness, or scripting/automation hook; choked on C++ build complexity. |

### Key Strategic Lessons
1. **Headless Kernel & Automation First, UI as a Client:** Studios and AI agents care first about a deterministic, fast, crash-free geometry & simulation engine that speaks OpenUSD, OpenVDB, Alembic, Python 3.13, and MCP.
2. **Permissive Licensing (MIT / Apache-2.0) is a Superpower:** Unlike GPLv2+ Blender, an MIT/Apache-2.0 Rust engine (`libofx_engine`) can be statically or dynamically embedded inside Unreal Engine, Unity, proprietary studio pipelines, and commercial cloud services without legal friction.
3. **AI-Native from Commit #1:** No existing DCC was designed with MCP at the root. Being the first procedural 3D & multiphysics platform where an AI agent has 100% structured introspection (`geo_probe`), atomic transactions, and visual/quantitative assertion loops gives this project an immediate, unmatched wedge.

---

## 2. Desktop UI & Node Editor Framework Evaluation in Rust (2026)

A Houdini-class workstation UI requires:
- **Sub-millisecond responsiveness** with **10,000+ nodes** in a network editor (panning, semantic zooming, minimap, wire cutting, network boxes, sticky notes, badge flags),
- **Virtual-scrolled Geometry Spreadsheet** displaying **50,000,000+ rows** of Point/Vertex/Prim attributes with instant sorting and filtering,
- **Multi-window tear-off docking panes** (Network View, 3D Viewport, Solaris Stage Tree, Parameter Inspector, Curve/Animation Dope Sheet, VEX Code Editor with syntax highlighting & inline diagnostics, MCP Agent Activity Console),
- **Zero-copy shared `wgpu::Device` and `wgpu::Texture`** with the 3D Viewport and Copernicus 2D viewer.

### 2.1 Comparison of Rust UI Toolkits (2026)

| Framework | License | Rendering Backend | Docking & Custom Canvas | 50M-Row Virtual Table | Assessment for Our DCC |
|---|---|---|---|---|---|
| **`egui` (v0.30+) + `egui_tiles` + Custom `wgpu` Viewport** | **MIT / Apache-2.0** | Native **`wgpu`** (single pass over shared device!) | **`egui_tiles`** (authored by Rerun team specifically for docking scientific/3D viewports, tabs, split panes, floating windows) + full `egui::Painter` custom 2D tessellation. | Built-in `egui_extras::TableBuilder` with $O(1)$ virtual row slicing (renders only the ~40 visible rows out of 50,000,000!). | **ADOPT (Winner).** Proven in production by **Rerun.io** and **Brush (3DGS)** for exact 3D DCC workloads. Zero C++/Qt dependencies, compiles in seconds on Windows/Linux/macOS/WASM, and shares `wgpu` buffers zero-copy. |
| **Qt 6 (`cxx-qt`)** | LGPLv3 / Commercial | OpenGL / RHI | Mature docking | `QAbstractTableModel` | **Avoid:** Pulls in a massive C++ Qt 6.8 dependency, complicates Vulkan/`wgpu` surface sharing across OSes, and introduces LGPLv3 dynamic-linking compliance burdens. |
| **`Slint`** | GPLv3 / Royalty-Free Desktop / Paid | Skia / FemtoVG / wgpu | Declarative DSL (`.slint`) | Limited for dynamic 10k-node spatial canvas | **Avoid:** Licensing complexity and declarative DSL is awkward for highly dynamic node graphs and spreadsheets. |
| **`iced` / `Xilem`** | MIT / Apache-2.0 | `wgpu` / Vello | Evolving docking support | Less mature than `egui` + `egui_tiles` for dense 3D pro apps | Secondary watchlist; `egui` + `egui_tiles` is substantially ahead for dense technical DCC tooling today. |
| **Tauri / Web UI** | MIT / Apache-2.0 | OS WebView (Chromium/WebKit) | HTML/Canvas | DOM overhead, IPC serialization bottleneck between WebView and `wgpu` viewport | **Avoid:** Serialization latency between WebView JS and native 3D `wgpu` surfaces kills 120 Hz viewport scrubbing. |

---

## 3. `ofx-ui` Workstation Architecture (`egui` + `egui_tiles` + `wgpu`)

### 3.1 UI as a Pure Client of `ofx-cmd`
The desktop UI (`ofx-ui`) holds **zero authoritative scene state**:
- Every user action (dragging a parameter slider, wiring two pins, pressing `Delete`, cutting wires with `Y`-drag, diving into a subnet) constructs an `EngineCommand` and submits it to `ofx-cmd`—the **exact same command bus** used by MCP agents.
- When an MCP agent creates or modifies nodes in the background, `ofx-ui` observes the new `ArcSwap<SceneState>` revision on the next 60/120 Hz frame and smoothly animates the new nodes into the network canvas with an "Agent Edit" highlight badge!

### 3.2 High-Density Network Editor (`ofx-ui-nodegraph`)
Rather than relying on a toy node-graph widget, `ofx-ui-nodegraph` implements a dedicated spatial index (**R-tree / Uniform Grid**) over nodes and wires on top of `egui::Painter`:
1. **Viewport Frustum Culling for 10,000+ Nodes:** Only nodes and cubic Bezier / orthogonal wires intersecting the visible 2D canvas rect are tessellated each frame ($<0.4\text{ ms}$ CPU time for a 20,000-node network).
2. **Semantic Level-of-Detail (LOD) Zoom:**
   - **Zoomed In ($>60\%$):** Full node body, pin labels, inline parameter badges, cook time bar (`2.4 ms`), and Houdini-style corner flags (**Bypass** yellow left, **Lock/Stash** red, **Template** pink, **Display/Render** blue right).
   - **Medium Zoom ($15\text{--}60\%$):** Node title, category color header, and primary wires.
   - **Overview / Minimap ($<15\%$):** Colored rectangles + bundled highway wires + heatmap overlay of cook time or memory footprint per node!
3. **Ergonomic Artist Workflows:**
   - **`Tab` Fuzzy Search Menu:** Instant fuzzy search (`nucleo` crate—the same matcher used by Helix editor) ranked by context compatibility and recent usage.
   - **Y-Key Wire Scissor & Shake-to-Disconnect:** Slice across wires to disconnect; drop a node onto an existing wire to auto-insert in-line.
   - **Radial Marking Menus (`C` / Right-Click Hold):** Context-sensitive 8-slice pie menu for rapid modeling/rigging mode switches.

### 3.3 Virtualized Geometry Spreadsheet (`ofx-ui-spreadsheet`)
- Inspects `Points`, `Vertices`, `Primitives`, or `Detail` attributes directly from the pinned or displayed node's `Arc<GeoDetail>`.
- Because `PagedColumn<T>` is O(1) random-access by `(page_idx = row >> 14, offset = row & 0x3FFF)`, scrolling to row `42,198,401` fetches only Page `2575` in $O(1)$ without formatting or allocating the other 49.99 million rows!

---

## 4. Strategic Wedge & Phased Parity Roadmap

Trying to ship all of Houdini 22 in a single monolithic release is a trap. We sequence delivery into **high-value, self-contained production wedges** where each phase is already a best-in-class tool:

1. **Wedge 1 (Phases 0–2): The AI-Native Procedural Geometry, VDB & Terrain Engine**
   - Unbeatable headless + GUI procedural modeling, VEX Wrangles (CPU SIMD + GPU), guaranteed-manifold booleans, UVs, VDBs, and GPU heightfield erosion with native MCP at the root and `.bgeo` / `.usd` / `.gltf` / `.fbx` I/O.
2. **Wedge 2 (Phase 3): Unified GPU/CPU Multiphysics (XPBD Vellum, RBD Destruction, APIC FLIP, Sparse Pyro, VBD Muscles, MLS-MPM)**
   - Feature-film simulation solvers validated against analytical physics benchmarks and controllable both interactively and by autonomous MCP wedging agents.
3. **Wedge 3 (Phases 4–5): Solaris-Class USD Lookdev, Wavefront XPU Path Tracer, Copernicus 2D/3D, Character Rigging/Crowds & TOP Farm Orchestration**
   - Full end-to-end shot assembly, lighting, rendering, 3D Gaussian Splatting, and OpenJD/OpenCue render farm scaling.

---

## 5. Recommendations, Open Risks, and Milestones

### Recommendations
1. **Standardize on `egui` + `egui_tiles` + `wgpu` + `nucleo` (fuzzy search)** for the desktop workstation application.
2. **Build the MCP Live Activity Inspector directly into the UI dock** so human artists and AI agents can pair-program on the same node graph in real time.

### Open Risks
- **Custom Widget Polish vs Native OS Chrome:** Default `egui` styling looks utilitarian. *Mitigation:* Define a custom dark low-contrast studio theme (`ofx-theme`, inspired by Houdini/Nuke/Blender dark neutral grays `#1e1e22` with semantic context accent colors) and custom vector icons in Phase 2.

### Suggested Milestones

---
## Audit addendum — UI correctness boundary

`egui` 0.37.0, `egui_tiles` 0.18.0 and `nucleo` 0.5.0 are versioned candidates; pin them and feature-gate native backends. UI renders immutable snapshots and emits revisioned commands, never mutable engine pointers. Chrome is display-referred sRGB; image presentation applies the OCIO transform from `04`. Picking, cache and selection carry snapshot revision to prevent stale-click edits.
- **M12.1 — `ofx-ui` Docking Shell & 3D Viewport Integration:** `winit` + `wgpu` + `egui_tiles` workspace with 3D Viewport, Network Editor, Parameter Inspector, and Geometry Spreadsheet panes.
- **M12.2 — 10k-Node Network Canvas & `Tab` Fuzzy Menu:** Spatial R-tree culling, semantic LOD zoom, Houdini-style node flags, wire cutting, network boxes, and `nucleo` search.
- **M12.3 — 50M-Row Virtual Geometry Spreadsheet & VEX Code Editor:** Zero-copy `PagedColumn` table inspector, inline diagnostic spans, and live MCP Agent Activity console.
