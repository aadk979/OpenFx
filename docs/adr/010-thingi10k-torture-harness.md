# ADR 010: Thingi10K Torture Harness and What It Found

- **Status:** Accepted
- **Date:** 2026-11-05
- **Supersedes / Extends:** [ADR 009](009-surface-operator-crate.md). Corrects claims in [ADR 008](008-sparse-vdb-volumes-and-manifold-booleans.md) and ADR 009.

## Context

M1.3 requires zero panics and manifold, watertight booleans across 200 Thingi10K meshes. Until now, every operator had only been tested on clean, procedurally generated geometry.

## Decisions

### 1. Corpus

`tests/corpus/thingi10k-200.csv` pins 200 models to a dataset revision by SHA-256, stratified across 15 labels from the dataset's own metadata: 14 defect classes and a `clean` control group. Half of each stratum is its most severe examples, and half is an unbiased sample.

- **Licensing:** each model has its own license, so meshes are never committed. They download into the git-ignored `corpus/` directory. The subset is limited to CC-BY, CC-BY-SA, public domain, and BSD models, because the harness transforms them.
- **Size:** models over 150,000 faces and non-STL models (0.4% of the dataset) are excluded.

### 2. Harness (`pcc-torture`)

Each `(mesh, operator)` pair runs in its own process, so a panic, abort, or hang is classified instead of ending the run. Meshes are centred, rescaled to a bounding-box diagonal of 10, and welded first.

| Outcome | Meaning | Fails the run |
|---|---|---|
| `ok`, `ok_empty` | Finished; invariants hold | No |
| `rejected`, `over_budget` | Structured error: input outside the contract, or over the 2 GiB budget | No |
| `timed_out` | Observed cancellation at the soft limit | No |
| `invariant`, `nondeterministic` | Wrong output, or differing output on a repeat run | **Yes** |
| `panic`, `crash`, `hang` | Process failure, or cancellation ignored past the grace period | **Yes** |

Fault-injection operators (`fault:panic`, `fault:abort`, `fault:hang`, `fault:slow`) prove the supervisor detects each outcome. Oracles include closure, bounds, UV inversion, volume on simple solids, and vertex manifoldness.

### 3. Volumetric pipeline changes

- **Fast winding numbers.** Voxelization was voxels times triangles, twice. It now uses a BVH with a truncated far-field expansion of the winding kernel (Barill et al., 2018), and evaluates Z slices on all cores, merged in slice order so that output does not depend on scheduling.
- **A real manifold guarantee.** Surface Nets is manifold exactly when each grid cell's inside corners form one edge-connected group, and so do its outside corners. Regularization now enforces that to a fixed point. It is tested on all 254 single-cell configurations and on random voxel noise.

### 4. Other contract changes

- `remesh` budgets count locked edge length, not just area. A sliver has almost no area but very long edges that can only be split.
- `remesh`, `polyreduce`, and `voronoifracture` reject back-to-back faces, which pass every edge-based manifold test but enclose nothing.
- `clip` and `voronoifracture` cap every opening, including the clockwise loops produced by inside-out solids.
- Voxelization treats inside-out shells as solid by using winding magnitude.
- The dev profile builds at `opt-level = 1`, so geometry tests are not needlessly slow.

## Results

2026-11-05, on one Windows machine with 22 cores and 8 concurrent jobs. "Finished" counts runs that returned output, including those that then failed an invariant. The baseline used a 30 s limit and the final run used 60 s; a 30 s run had already reached the same `boolean` and `vdb_roundtrip` results. The baseline had no volume or vertex-manifold oracles, and `polyreduce_half` was added afterwards.

| Operator | Baseline finished | Final `ok` | Final `ok_empty` | `rejected` | Unfinished |
|---|---|---|---|---|---|
| `boolean` | 68 | 187 | 13 | 0 | 0 |
| `vdb_roundtrip` | 83 | 184 | 16 | 0 | 0 |
| `remesh` | 115 | 113 | 0 | 87 | 0 |
| `polyreduce` | 114 | 113 | 0 | 87 | 0 |
| `polyreduce_half` | n/a | 113 | 0 | 87 | 0 |
| `uvflatten` | 191 | 193 | 0 | 0 | 7 |
| `polybevel` | 115 | 115 | 0 | 85 | 0 |
| `clip` | 200 | 200 | 0 | 0 | 0 |
| `voronoifracture` | 105 | 103 | 0 | 97 | 0 |

`scatter_surface`, `triangulate`, and `normal` finished all 200. `subdivide` and `extrude` finished 115, rejecting 85.

**Final totals:** 2,800 runs with 0 panics, crashes, hangs, invariant violations, or nondeterministic results. The longest cancellation took 165 ms, down from about 14 s. The baseline's 62 invariant failures were 52 `uvflatten`, 6 `voronoifracture`, and 4 `clip`. The 52 `uvflatten` failures came mostly from an oracle that was too strict rather than from the operator: they persisted after the placeholder fix below and cleared when the oracle began to allow `f32` rounding. On six probed models, every non-positive area was below the rounding bound.

### Bugs found

| Bug | Effect | Found by |
|---|---|---|
| Voxelization cost voxels times triangles | `boolean` unfinished on 132 of 200 | Corpus |
| Inside-out solids lost their caps | `clip` and `voronoifracture` opened closed solids | Corpus |
| Surface Nets regularization was incomplete | A non-manifold edge, despite ADR 008's "guarantee" | Corpus |
| Back-to-back faces broke edge splitting | 24,384 non-manifold edges from a two-triangle input | Synthetic suite |
| `remesh` budget ignored locked edges | Effectively unbounded work on slivers | Synthetic suite |
| The placeholder UVs for faces that fail to flatten ran clockwise | Inverted UVs on any such face | Code reading during the corpus investigation |

## Corrections to earlier ADRs

- **ADR 008** said Surface Nets was "mathematically guaranteed" manifold. It was not, as found on model 145065. The guarantee now rests on the cell condition above.
- **ADR 009** said zero inverted triangles holds for any input. Charts are flip-free in `f64`, but UVs are stored as `f32`, so a sliver can round to a slightly negative area. The oracle allows only that rounding error.

## Known limitations

This satisfies the plan's wording of zero panics and no infinite loops, and every non-empty `boolean` output is watertight. It does not mean the engine handles all of this data well:

- **Nearly half of the corpus cannot be remeshed or decimated.** `remesh`, `polyreduce`, `polybevel`, `subdivide`, and `extrude` reject the 85–87 non-manifold models with a clean error. Real workflows need a mesh repair operator, which does not exist yet.
- **`boolean` drops thin geometry.** 13 models produce nothing at 48 voxels across the diagonal because their walls fall between voxel centres; the five measured have a mean wall of 0.02–0.11 against a 0.208 voxel. This happens silently, and there is no adaptive resolution. `boolean` is also still voxel-based, so it is approximate and is not the exact `manifold-rust` boolean in the plan.
- **`uvflatten` did not finish 7 models.** All are over 70,000 faces: six hit the 60 s limit and one exceeded the budget. They cancel cleanly, but this is the solver limit from ADR 009.
- **`uvflatten` can shatter charts.** Model 51354 produced 26,112 islands for 26,112 triangles, which is unusable for texturing. No test bounds fragmentation.
- **Aggressive decimation can spend volume.** Model 98006 (high genus) keeps its volume at 50% but loses 29% at 25%, flattening a plate to reach the target. No error bound stops it.
- **Coverage is limited to this one run.** It covers one machine, one voxel size, models under 150,000 faces, and fixed parameters. CI does not run the corpus; only the synthetic reductions in `crates/pcc-torture/tests/` run there.

## Verification

- `cargo xtask ci`: 145 tests.
- Mutation check: 29 hand-picked mutations of the new code, by a one-off script. The first run missed 10, including mirrored voxel slices and a pinched-vertex count that had never reported above zero. All 29 are now caught.
- Reproduce the corpus run with `python tools/thingi10k/fetch.py`, then `cargo run --release -p pcc-torture -- run --timeout 60 --jobs 8`.
