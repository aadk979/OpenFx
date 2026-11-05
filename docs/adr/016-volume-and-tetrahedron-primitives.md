# ADR 016: Volume and Tetrahedron Primitives

- **Status:** Accepted
- **Date:** 2026-11-05
- **Supersedes / Extends:** [ADR 015](015-packed-instances.md), whose limit that volumes are not primitives and tets do not exist no longer applies.

## Context

M0.2 lists polygons, curves, tets, packed instances, and VDB volumes as primitive runs. Tetrahedra did not exist, though soft-body solvers depend on them. A `VdbGrid` passed between nodes as a point cloud with one point per voxel, so any operator could read it as ordinary points, and each node rebuilt the grid from those points.

## Decisions

### 1. Grid storage moves to `pcc-geo`

`pcc-vdb` depends on `pcc-geo`, so a `GeoDetail` could not hold a grid. Bricks and `VdbGrid` now live in `pcc-geo`, and the algorithms stay in `pcc-vdb` as the `VdbOps` trait. Call sites keep their spelling once the trait is in scope.

### 2. Tetrahedra

`PrimKind::Tetrahedron` has exactly four corners. An element `(a, b, c, d)` is positive when `(b - a) x (c - a) . (d - a) > 0`.

- **`tetbox`** generates a solid box. Each cell is cut into six elements around a main diagonal that every cell shares, so neighbours agree on the triangles of a shared face.
- **`tet_surface`** extracts every face that exactly one element uses, and only the points those faces reach. It refuses other primitives beside the elements, inverted or flat elements, and elements that overlap across a face.
- **Mirroring** swaps two corners. Reversing all four is an even permutation and would leave the element inverted.

### 3. Volumes

`PrimKind::Volume` has one vertex, and its grid sits in a side table in primitive order, the same shape as packed instances. Grids are shared rather than copied.

- **Nodes.** `vdb_from_polygons` makes a volume, and `vdb_reshape` and `convert_vdb` read one. `merge` keeps each grid with its primitive, and the two readers work on every grid they are given. `vdb_to_points` replaces the old cloud for inspecting voxels.
- **Bounds** cover the active voxels. They are computed once, when a grid is attached, so `bounds()` scans no grid.
- **The point only anchors the primitive.** The grid's own coordinates say where its voxels are, so moving the point would not move them. `transform` and `xform` therefore refuse volumes, and volumes cannot be packed.
- **Budget.** Voxels still count as elements, as they did when they were points.

### 4. Refusal is the default

Surface operators skip primitives that are not polygons, so a solid could vanish without an error. As with instances, an operator must now declare `accepts_tets` or `accepts_volumes`, and any other names the node to use instead. Volume operators also refuse anything beside the volumes, which would otherwise be lost. `.obj` and `.stl` export refuse both kinds; `.ofxg` stores them losslessly.

### 5. Files

A kind tag and a grid section appear only in files that hold volumes, so other files and their hashes do not change. A reader refuses a grid that the writer could not have produced: bad settings, non-finite values, bricks out of order or repeated, and coordinates that could overflow a voxel index.

## Verification

- **Analytic oracles for tetrahedra.** Exact volumes and surface areas of `tetbox`, and the divergence theorem linking summed element volume to the enclosed volume of the extracted surface. Extraction refuses inverted and overlapping elements, so a clean surface also shows that every element is positive and that neighbours conform.
- **Solver state.** Three solver ticks of a reshape hash the same as three reshapes in a row, and a solver carries elements across ticks without changing their volume.
- **Corpus.** Thingi10K was rerun after these changes: 2,800 runs with 0 failures, and every status and geometry hash matches the previous run. Its `boolean` and `vdb_roundtrip` cases call the grid algorithms directly, so this shows that those algorithms did not change. It does not exercise volumes as primitives.
- **A real bug found on the way.** The file reader used `i32::abs` on brick coordinates, which panics on `i32::MIN`. The hostile-input test caught it before commit.
- **`cargo-mutants`** on the changed lines, with 2 jobs: 370 mutants, of which 253 were caught, 76 did not compile, and 41 survived. New tests address 39 of those, and a serial replay of that list confirmed it; the tool itself was not rerun. The other two are equivalent: flipping `face_key`'s winding convention consistently changes no comparison, and a face's corners are always distinct.
- `cargo xtask ci`, on Windows only.

## Limits

- **No affine transform.** `research/01-core-architecture.md` gives a volume an index-to-world transform. Here a grid's position is only its voxel size and coordinates, so volumes cannot be moved without resampling, and neither `transform` nor `xform` can act on them.
- **`tetbox` is the only source of elements.** There is no tetrahedralization of arbitrary meshes. The research names `fTetWild` or a pure-Rust Delaunay refinement.
- **Elements have no inversion check on creation.** Only `tet_surface` refuses inverted elements, because a solver may invert them in passing. `.ofxg` files and `with_primitives` can hold them.
- **`tet_surface` drops point groups**, and primitive attributes, since its faces are new primitives.
- **Grid-level operators are few.** `vdb_reshape` is the only operator that changes a grid in place. There is no sampling of a volume at points, no `HeightfieldLayer2D` volume, and no NanoVDB export.
- **Volumes and tets are not yet in the Thingi10K operator list.**
