# 07 — Procedural Modeling and Geometry-Processing Algorithms (SOP Toolset)

> **Scope:** Topological editing structures, guaranteed-manifold mesh booleans, remeshing, subdivision, UV parameterization & packing, SDF/VDB volumetric conversions, terrain erosion, and the prioritized 60-node foundational SOP catalog.

---

## 1. Topological Accelerators over `GeoDetail`

While `GeoDetail` stores points, vertices, and primitives in flat, cache-friendly `PagedColumn<T>` arrays for fast cooking and GPU upload, local topological editing operators (`PolyBevel`, `PolyExtrude`, `EdgeLoop`, `Subdivide`, `Remesh`) need adjacency queries.

Rather than maintaining heavy pointer-chasing half-edge structs permanently in every `GeoDetail`, we build **lazy, immutable CSR Topological Indexes (`TopologyIndex`)** cached alongside the `PrimRun` hash:
- **Vertex-Twin / Half-Edge Array (`twin_he: Vec<u32>`):** Implicit array-based half-edge where half-edge ID `he` IS the `VertexIdx(u32)`! Next/prev around a face are computed directly from the polygon's `vertex_offsets` range, and `twin_he[he]` stores the opposite half-edge across a shared manifold edge (or a chained non-manifold edge list for non-manifold T-junctions).
- **Zero Pointer Overhead:** Uses exactly **4 bytes per vertex** (`u32`), supports non-manifold meshes gracefully without crashing (critical for procedural modeling where intermediate geometry is frequently non-manifold), and builds in parallel via radix sort on canonical edge keys `(min(pt_a, pt_b), max(pt_a, pt_b))`.

---

## 2. Core Algorithmic Subsystems & Crate Selection (2026)

### 2.1 Guaranteed-Manifold Mesh Booleans & Exact Predicates

Boolean operations (`Union`, `Subtract`, `Intersect`, `Shatter/Split`) are the #1 failure point in procedural modeling pipelines when implemented with naive floating-point intersection.

| Library / Crate | URL | License | Characteristics & Role |
|---|---|---|---|
| **`manifold-rust` / `manifold-csg`** | [crates.io/crates/manifold-rust](https://crates.io/crates/manifold-rust) | Apache-2.0 | Pure-Rust port / binding of Emmett Lalish's **Manifold** library. Guarantees topological manifold output on closed volume meshes with `rayon` parallelism and attribute interpolation. **Primary Closed-Volume Boolean Engine.** |
| **`boolmesh`** | [crates.io/crates/boolmesh](https://crates.io/crates/boolmesh) | MIT / Apache-2.0 | Pure-Rust boolean engine with first-class generic attribute interpolation (UV seams, normals, custom attributes). |
| **`robust` (Shewchuk Predicates)** | [crates.io/crates/robust](https://crates.io/crates/robust) | MIT / Apache-2.0 | Adaptive-precision floating-point `orient2d`, `orient3d`, `incircle`, `insphere` predicates. **Mandatory for all triangulation, Delaunay, and open-surface intersection code.** |
| **Fast Winding Numbers (Barill et al., SIGGRAPH 2018)** | Implemented in `ofx-sop-boolean` | — | Dipole hierarchical evaluation of generalized winding number $w(\mathbf{p}) \in \mathbb{R}$ in $O(\log N)$ per query; enables robust inside/outside classification even on dirty meshes with holes or self-intersections! |

### 2.2 Subdivision, Remeshing, Decimation, and PolyBevel

1. **Subdivision Surfaces (`Subdivide SOP`):**
   - **Catmull-Clark** (quad-dominant with semi-sharp `creaseweight` edge/vertex attributes matching OpenSubdiv specification) and **Loop** (triangle meshes), plus **OSD Adaptive Feature-Adaptive** limit-surface evaluation.
2. **Surface Simplification (`PolyReduce SOP`):**
   - **Garland-Heckbert Quadric Error Metrics (QEM)** extended with **Attribute-Aware Wedge Quadrics (Hoppe 1999)** so UV seams, sharp normal creases, and vertex color boundaries are preserved during decimation. Also integrates `meshopt` (MIT) for LOD cluster generation.
3. **Isotropic & Quad Remeshing (`Remesh SOP` / `QuadRemesh SOP`):**
   - **Isotropic Triangle Remeshing:** Botsch & Kobbelt (2004) iterative split $\rightarrow$ collapse $\rightarrow$ valence-equalizing flip $\rightarrow$ tangential smoothing projected onto the input BVH with sizing field driven by surface curvature or a point attribute `@targetmeshsize`.
   - **Field-Aligned Quad Dominant Remeshing:** Jakob et al. (SIGGRAPH 2015, *Instant Field-Aligned Meshes*, BSD-3 algorithm) orientation-field + position-field extrinsic smoothing.
4. **Robust Polygon Bevel & Extrude (`PolyBevel`, `PolyExtrude`):**
   - Implements **Howard Trickey's (Blender) / Houdini offset-miter profile solver** with loop-slide clamping and spherical/chamfer/custom ramp profiles.
   - For 2D profile inset/offset with self-intersection pruning, uses **`i_overlay`** (MIT, high-performance pure-Rust 2D boolean & polygon offsetting) and ** Straight Skeleton** (Felkel & Obdrzalek) to prevent miter spikes on acute angles.

### 2.3 UV Parameterization, Flattening, and Island Packing

UVs are stored as standard `Vec3f` (`uv`) attributes on either the `Vertex` or `Point` domain:

| UV Operator | Algorithm | Implementation |
|---|---|---|
| **`UV Project` / `UV Texture`** | Planar, Cylindrical, Spherical, Triplanar (Box), Camera Frustum | SIMD analytic projection with automatic seam vertex splitting. |
| **`UV Autoseam`** | Variational Shape Approximation (Cohen-Steiner et al. 2004) + dihedral visibility | Evolves chart proxies and cuts along high-curvature / occluded valleys. |
| **`UV Flatten` (Conformal & Isometric)** | **LSCM** (Lévy et al. 2002) for fast conformal initialization + **SLIM** (Scalable Locally Injective Maps, Rabinovich et al. 2017) or **BFF** (Boundary First Flattening, Sawhney & Crane 2017) | Assembled and solved using pure-Rust sparse Cholesky/LU (`faer`, MIT) with pin constraints and zero triangle flips. |
| **`UV Layout` / Packing** | Rasterized SDF / Horizon-line Skyline + No-Fit Polygon (NFP) + rotational sweep | Multi-threaded island packer supporting UDIM tile targets (`1001..1099`), texel-density equalization, and arbitrary concave hole packing. |

### 2.4 Volumetric & Signed Distance Field Modeling (`ofx-vdb`)

1. **`VDB From Polygons` / `VDB From Particles`:**
   - Exact narrow-band signed distance field ($\phi$) generation via triangle BVH closest-point queries + **Fast Winding Number sign determination** (so non-watertight meshes still voxelize cleanly!) + Fast Sweeping (`Eikonal` $|\nabla \phi| = 1$) dilation.
2. **`VDB Combine` & `VDB Smooth` / `VDB Reshape`:**
   - Level-set CSG (`min`, `max`, smooth-min polynomial blend), morphological dilate/erode (upwind HJ-WENO5 Hamilton-Jacobi step), mean-curvature flow, Laplacian, and median filtering.
3. **`Convert VDB` (Volume $\rightarrow$ Polygons):**
   - Dual Contouring / Adaptive Surface Nets (generating clean quad-dominant topology with feature preservation and adaptive polygon reduction in flat regions).

### 2.5 Procedural Terrains & Heightfields (`ofx-terrain`)

Heightfields are stored as collections of named 2D float layers (`height`, `mask`, `water`, `sediment`, `debris`, `bedrock`) backed by `ofx-vdb` 2D tiled grids and `CubeCL` GPU kernels:
- **Multi-Fractal & Warped Noise (`HeightField Noise`):** Simplex, Perlin, Worley cellular (F1, F2, F2-F1 ridges), Curl, and Swiss/Jordan analytical derivative-warped fBm (Musgrave / Inigo Quilez erosion-like noise).
- **Hydraulic & Thermal Erosion (`HeightField Erode`):**
  - **Shallow-Water Pipe Model (Mei, Decaudin, Hu 2007; Št'ava et al. 2008):** Simulates rainfall, virtual pipe flux between 4/8 neighbors, velocity field $\mathbf{v}$, sediment capacity $C = K_c \sin(\alpha) \|\mathbf{v}\|$, erosion/deposition rates ($K_s, K_d$), and thermal talus slumping when local slope exceeds critical angle $\alpha_{\text{talus}}$.
  - **Stream-Power Law Analytical Uplift & River Network Solver (Cordonnier et al., SIGGRAPH 2016):** Generates continental-scale dendritic drainage basins in $<50\text{ ms}$.

---

## 3. Prioritized Foundational 60-Node SOP Catalog

Every node below has a deterministic spec, schema, and validation test in Phase 1–2:

| Category | Nodes (60 Total) |
|---|---|
| **Primitives & Generators (8)** | `Grid`, `Box`, `Sphere` (Poly/Mesh/Geodesic), `Tube`, `Torus`, `Line`, `Circle`, `Curve` (Bezier/NURBS) |
| **Transform, Copy & Instancing (7)** | `Transform`, `MatchSize` (center/justify to bounds), `CopyToPoints` (orient via `@N`/`@up`/`@orient` quat + `@pscale`), `CopyAndTransform`, `Pack`, `Unpack`, `Instance` |
| **Attributes & Groups (9)** | `AttribCreate`, `AttribWrangle` (VEX kernel), `AttribPromote` (Pt $\leftrightarrow$ Vtx $\leftrightarrow$ Prim $\leftrightarrow$ Det with min/max/avg/sum), `AttribTransfer` (KD-tree spatial kernel blend), `AttribNoise`, `AttribBlur`, `GroupCreate` (bbox/normal/edge-angle), `GroupPromote`, `GroupCombine` |
| **Topological Modeling (10)** | `Merge`, `Blast` / `Delete`, `Split`, `PolyExtrude`, `PolyBevel`, `Subdivide`, `Remesh`, `PolyReduce`, `Fuse` / `Clean`, `Reverse` / `Normal` (area/angle-weighted) |
| **Booleans, Cuts & Fracture (5)** | `Boolean` (`manifold-rust`), `Clip`, `VoronoiFracture`, `BooleanFracture`, `ConvexDecomposition` |
| **Curves & Sweeps (5)** | `Resample` (arc-length parameterization), `Sweep` (ribbon/tube/cross-section along curve with parallel-transport RMF frames), `Skin` / `Loft`, `Carve`, `PolyWire` |
| **Sampling, Scatter & Distance (5)** | `Scatter` (bridson Poisson-disk / density-weighted blue noise), `Ray` (project along normals/minimum distance), `Measure` (area, volume, perimeter, curvature), `DistanceAlongGeometry` (Heat Method geodesics, Crane et al. 2013), `FindShortestPath` (Dijkstra/A*) |
| **UVs (4)** | `UVProject`, `UVAutoseam`, `UVFlatten`, `UVLayout` |
| **Volumes / VDB (4)** | `VDBFromPolygons`, `VDBFromParticles`, `VDBCombine` / `VDBReshape`, `ConvertVDB` |
| **Heightfields / Terrain (3)** | `HeightField`, `HeightFieldNoise`, `HeightFieldErode` / `HeightFieldScatter` |

---

## 4. Mandatory Geometric Validation & Robustness Suite

1. **Thingi10K Torture Corpus:** Run `Boolean`, `VDBFromPolygons`, `Remesh`, `PolyReduce`, and `UVFlatten` across a curated subset of 200 nasty real-world meshes from the **Thingi10K** dataset (self-intersecting, zero-area slivers, non-manifold edges). Zero panics, zero infinite loops, and `Boolean` output must pass `is_manifold && is_watertight`.
2. **Gauss-Bonnet & Divergence Theorem Invariants:** Verify `Measure SOP` surface area and enclosed volume ($V = \frac{1}{6} \sum \mathbf{p}_i \cdot (\mathbf{p}_j \times \mathbf{p}_k)$) match analytical sphere/torus/box formulas and remain invariant under arbitrary rigid `Transform` rotations.
3. **Geodesic & Scatter Convergence:** Verify `Scatter SOP` blue-noise minimum distance ratio and total expected point count $\int \rho \, dA$ within statistical bounds.

---

## 5. Recommendations, Open Risks, and Milestones

### Recommendations
1. **Use `manifold-rust` + `robust` + Fast Winding Numbers** as the non-negotiable foundation for all boolean, fracture, and SDF sign queries.
2. **Use `kiddo` v4+ (MIT/Apache-2.0)** bucketized KD-tree for `AttribTransfer`, `Fuse`, and point-cloud neighborhood queries.

### Open Risks
- **Attribute Interpolation Across Topological Splits:** Splitting polygons in `Boolean`, `Clip`, and `PolyBevel` must preserve vertex `uv` seams without bleeding across UV island boundaries. *Mitigation:* All topological split routines take a generic `VertexInterpolator` that checks per-half-edge discontinuity flags before barycentric blending.

### Suggested Milestones

---
## Audit addendum — dependency qualification

`manifold-rust` 0.19.1, `boolmesh` 0.2.2 and `i_overlay` 9.1.0 have materially different scopes; a crate name is not proof of watertight 3D booleans. `manifold-rust` has native/C++ build implications and requires Windows/Linux/macOS toolchain proof. Use an adapter trait, preserve a VDB/SDF fallback, and validate topology/volume/orientation on a corpus. `kiddo` 6.3.0 and `faer` 0.24.4 are utilities, not geometry-kernel guarantees. `TopologyIndex` is valid only for its exact `GeoDetail.revision` and must invalidate after every topology edit.
- **M7.1 — Core 25 Nodes (Primitives, Transforms, Attributes, Groups, Merge/Delete, CopyToPoints, Scatter):** Full SOP workflow capable of procedural scattering and instancing.
- **M7.2 — Topological Editing & Subdivision (`TopologyIndex`, `Normal`, `Fuse`, `PolyExtrude`, `PolyBevel`, `Subdivide`, `Sweep`, `Resample`):** Full hard-surface and organic procedural modeling.
- **M7.3 — Booleans, VDBs, Remesh, PolyReduce & Fracture:** `manifold-rust` booleans, VDB level-set suite, Voronoi fracture, QEM reducer, and isotropic remesher.
- **M7.4 — UV Suite & Heightfield Terrain Engine:** LSCM/SLIM `UVFlatten`, UDIM `UVLayout`, and GPU `HeightFieldErode`.
