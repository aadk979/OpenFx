# ADR 015: Packed Instances and a Mutation Testing Gate

- **Status:** Accepted
- **Date:** 2026-11-05
- **Supersedes / Extends:** [ADR 014](014-state-zones.md)

## Decisions

### 1. Packed instances

`PrimKind::Packed` is an instance of a shared, immutable prototype: one point, one vertex, a prototype index, and a 3x3 matrix. New nodes are `pack`, `instance`, and `unpack`. Transforms compose into the matrix and never touch the prototype, and bounds come from stored prototype bounds.

- **No implicit unpack.** An operator must declare `accepts_packed`, currently `transform`, `xform`, `merge`, `pack`, `instance`, and `unpack`. Any other refuses packed input with an error naming `unpack`. `.obj` and `.stl` export refuse too; `.ofxg` stores instances losslessly.
- **Mirrored instances** reverse winding when unpacked, matching what `xform` does to real geometry.
- **Nesting is limited to 32 levels**, at creation and when reading a file, because hashing and files recurse through it.

### 2. Mutation testing in CI

`.github/workflows/mutants.yml` runs `cargo-mutants` on the changed lines of each pull request, where a survivor fails the job, and over everything weekly, where survivors are only reported. It has not run on GitHub yet.

Both jobs use 8 shards, because the pull request that introduces this workflow holds 1,849 mutants. A mutant costs a rebuild plus its own crate's tests: 529 mutants took 10 minutes with 6 jobs on 22 cores, about 7 s of job time each. GitHub's 4-core runners will be slower by an amount not yet measured. A shard with nothing to test exits at once, and a new push replaces a pull request's run in progress.

Because only the mutated crate's tests run, a test in another crate does not count, and a survivor can be a false alarm. So a crate's logic needs tests in that crate. An attempt with `--test-workspace=true` still ran only `pcc-geo`'s tests in its baseline, and the cause has not been investigated.

## Verification

- The Thingi10K corpus was rerun after these changes: 2,800 runs with 0 failures. Compared with the run recorded in [ADR 010](010-thingi10k-torture-harness.md), 2,799 geometry hashes are identical. The other is `uvflatten` on model 461110, which sits at the 60 s limit and finished this time instead of timing out.

- A million instances cook, transform, and report correct bounds under a 120 MB budget that refuses their 192 MB expansion.
- Transforming then unpacking matches unpacking then transforming, point for point and in winding, across rotation, non-uniform scale, and mirroring.
- `cargo-mutants` on `pcc-expr` found 18 survivors after hand-picked mutations had all been caught. Tests were added, and a rerun left 1 survivor and 5 timeouts, where the mutation makes the parser loop forever.
- `cargo-mutants` on `pcc-geo/src/packed.rs` tested 243 mutants and left 91 survivors. `determinant` and `multiply` were false alarms, caught only by tests in `pcc-eval`. The real gaps were normals under any non-diagonal matrix, and instances beside plain geometry. After new tests, a serial replay of those 91 and the `pcc-expr` survivor caught all 92. That replay reused the tool's list rather than rerunning the tool, because a full rerun was stopped by low system memory.
- **The other crates have not been through `cargo-mutants`.**

## Limits

- **Volumes are not primitives yet.** `VdbGrid` still passes between nodes as a point cloud, and tets do not exist, so M0.2's primitive list is unfinished.
- Most operators refuse instances rather than handle them. `blast`, `normal`, and `measure` are the obvious next ones.
- Groups on prototypes do not survive `unpack`, and instances have no per-instance attributes beyond their points.
