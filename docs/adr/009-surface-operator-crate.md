# ADR 009: Surface Operator Crate (`pcc-sop`): Primitives, Modeling, Fracture, and UV

- **Status:** Accepted
- **Date:** 2026-11-04
- **Supersedes / Extends:** [ADR 008](008-sparse-vdb-volumes-and-manifold-booleans.md)
- **Corrected by:** [ADR 010](010-thingi10k-torture-harness.md), on UV inversion and on real-world behaviour generally.

## Context

The catalog held 21 operators and lacked named M1.2–M1.4 deliverables: `Fuse`, `PolyBevel`, `Sweep`, `Resample`, `AttribTransfer`, `Remesh`, `PolyReduce`, `VoronoiFracture`, `UVFlatten`, and `UVLayout`. `Transform` was translate-only, no sphere, tube, torus, or curve generator existed, and `Blast` could not be driven by a graph-authored group because no node created one.

## Decisions

### 1. Tier-1 crate `pcc-sop`

Operators live in a new crate rather than growing `pcc-geo`. Each is a pure function from immutable `GeoDetail` inputs to a new `GeoDetail`, taking a `SopContext` (memory budget and cancellation flag). Computation runs in `f64` and rounds to `f32` on output. `pcc-geo` gained only the primitives that need private fields: `with_positions`, `with_reversed_winding`, and `AttributeColumn::selected`.

No external dependency was added. The plan names `faer` and `kiddo`; a Jacobi-preconditioned CG and a uniform point hash serve for now, and the cost of that choice is measured below.

### 2. Operators (catalog is now 42)

| Token | Method | Attributes after the operator |
|---|---|---|
| `sphere`, `tube`, `torus`, `line` | Parametric, Y-up, outward-facing | None |
| `xform` | Scale, Euler XYZ rotation, translation; `N` by inverse transpose; mirroring reverses winding | All kept |
| `reverse` | Reverses polygon winding | All kept; vertex attributes follow their vertices |
| `triangulate` | Ear clipping in the Newell plane | Point and detail |
| `fuse` | Uniform grid; lowest-indexed point wins | Point and detail |
| `group_bounds` | Existing `group_by_bounds`, now reachable from the graph | All kept |
| `resample`, `sweep` | Arc length; parallel-transport frames | None |
| `attribtransfer` | Normalized Wendland C2 kernel | All kept |
| `measure` | Prim `area`/`perimeter`, detail `total_area`/`total_volume` | All kept |
| `polybevel` | Corner offsetting with dihedral threshold | None |
| `polyreduce` | Garland-Heckbert QEM, link condition, no-flip check, boundary planes | None |
| `remesh` | Botsch-Kobbelt split/collapse/flip/smooth/project; features and boundaries locked | None |
| `clip` | Half-space clip; caps with hole bridging | None |
| `voronoifracture` | Repeated bisector clipping; closed shells tagged `i@piece` | `piece` only |
| `scatter_surface` | Area-weighted, optional dart-throwing minimum distance | `N` only |
| `uvflatten` | LSCM on disk charts, split until no flips | All kept, plus `uv`, `uv_island` |
| `uvlayout` | Bounding-box skyline packing with UDIM tiles | All kept, plus `uv`, `uv_island` |

### 3. Chart splitting instead of SLIM for flip-free UVs

`uvflatten` bisects any chart that is not a disk, has an inverted triangle, or exceeds a 64:1 area magnification ratio. A single triangle always passes, so zero inverted triangles holds for any input without SLIM, at the cost of more islands on difficult shapes.

## Verification

- **Oracles:** analytic volume and area for primitives; exact chamfered-cube volume $s^3 - 6d^2(s-2d) - \tfrac{20}{3}d^3$ and inset-triangle area for bevel; volume conservation and the Voronoi distance property for fracture, including a torus with annular caps; Euler characteristic for decimation; isometric recovery of flat and developable patches for UVs.
- **Integration:** `pcc-eval` cooks full pipelines through the DAG and checks cache keys and budgets; a new `pcc-mcp-stdio` test has an agent discover operators, build a remesh, decimate, and fracture scene over real stdio, and verify it with `geo_assert_invariants`.
- **Mutation check:** 35 hand-picked formula mutations, run by a one-off script because `cargo-mutants` is not part of CI yet. The first run left 17 of 36 alive, which exposed real test gaps and one unreachable branch (since removed). Now 34 of 35 are caught. The survivor replaces tangential smoothing with full Laplacian smoothing in `remesh`; reprojection onto the input surface largely cancels the difference.

### Measured scaling

Release build, one thread, latitude-longitude sphere input, 2026-11-04. The exponent is $\log_{10}(T_{1M}/T_{100k})$, or the 10k to 100k step for `uvflatten`.

| Operator | 10k prims | 100k prims | 1M prims | Exponent |
|---|---|---|---|---|
| `triangulate` | 0.006 s | 0.067 s | 0.511 s | 0.88 |
| `clip` | 0.007 s | 0.060 s | 0.653 s | 1.04 |
| `polybevel` | 0.018 s | 0.145 s | 1.493 s | 1.01 |
| `fuse` | 0.013 s | 0.119 s | 1.491 s | 1.10 |
| `remesh` (1 iteration) | 0.124 s | 1.161 s | 15.254 s | 1.12 |
| `polyreduce` (50%) | 0.066 s | 0.881 s | 15.168 s | 1.24 |
| `uvflatten` | 0.440 s | 37.720 s | not run | 1.93 |

These are one-off measurements, not a CI gate.

## Known limitations

- **`uvflatten` does not scale.** It is impractical beyond roughly 100k polygons per surface, and the iterative solver is the likely cause. Sparse Cholesky is the planned fix. Relaxing the solver tolerance did not help.
- **`polyreduce` misses the plan's 1.15 scaling limit** at an exponent of about 1.24.
- **Islands can overlap themselves.** Only triangle inversion is excluded; a chart may still self-overlap globally. Packing separates island bounding boxes, not outlines, so it wastes space on concave islands.
- **`polyreduce` and `remesh` discard attributes.** Wedge quadrics and attribute-aware remeshing are not implemented, so UV seams and normals are not preserved.
- **`remesh` locks feature vertices**, so feature edges gain points but never lose them, and has no sizing field.
- **`sweep` does not miter**, so tubes narrow at sharp bends.
- **`.obj` export writes no `vt` records**, so UVs leave the engine only through `.ofxg`.

## Explicit non-goals

This does not complete M1.2–M1.4 as written. Not done: 200-mesh Thingi10K torture run, exact `manifold-rust` booleans (`boolean` remains voxel-based and approximate at voxel resolution), `Skin`, `Carve`, `PolyWire`, `Ray`, `Clip`-based `BooleanFracture`, `ConvexDecomposition`, heat-method geodesics, quad remeshing, SLIM, No-Fit Polygon packing, GPU `HeightFieldErode`, and the Python SDK. A catalog count of 42 is not parity with the plan's 60-node list.
