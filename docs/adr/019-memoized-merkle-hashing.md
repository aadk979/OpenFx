# ADR 019: Memoized Merkle Hashing, Lazy Constant Pages, and `P` as a Column (Phase A1)

- **Status:** Accepted
- **Date:** 2026-11-25
- **Supersedes / Extends:** [ADR 017](017-architecture-revision.md) (implements Phase A1)

## Context

The 2026-11-11 audit ([`AUDIT_REPORT.md`](../AUDIT_REPORT.md), finding #1) showed that `GeoDetail::hash()` re-hashed every point position linearly on every call with no memoization, while `Page` hashed every element eagerly at construction using one `blake3::Hasher::update` call per scalar. Because the evaluator hashes every upstream input of every node on each cook, re-cooking a cached 10 M-point chain after a trivial downstream parameter edit took over a full second of redundant hashing. In addition, positions lived in a dedicated `GeoDetail::positions` field rather than in `point_attributes` alongside other point columns.

## Decisions

### 1. Three-level lazy, memoized Merkle tree

- **`Page<T>`** wraps `Arc<PageInner<T>>` with `data: PageData<T>` and `hash: OnceLock<blake3::Hash>`. Pages are not hashed at construction; `Page::hash()` hashes the page once on first use and shares the memoized digest across every CoW clone.
- **Chunked byte updates without `unsafe`.** `HashableValue` serializes elements into a 4 KiB stack buffer (4 BLAKE3 chunks) in little-endian order and feeds full chunks to `blake3::Hasher::update`, avoiding per-scalar update overhead without adding `unsafe` or new crate dependencies.
- **Constant-page storage.** `PageData::Constant { value, len }` stores any uniform 16,384-element page as a single value (detected by bit-identical equality in `Page::from_slice`/`Page::from_vec`, or constructed directly via `PagedColumn::constant`). Constant pages stream the exact same little-endian byte sequence into BLAKE3 as dense pages, so constant and dense representations hash identically.
- **`PagedColumn<T>`** wraps `Arc<ColumnInner<T>>` with `hash: OnceLock<blake3::Hash>` over its page hashes. Untouched columns copied across CoW mutations share their memoized column hash in O(1); editing one element re-hashes only the dirty page and the column root.
- **`GeoDetail`** memoizes its Merkle root (`geodetail-v2`) in `cached_hash: OnceLock<blake3::Hash>`. External `.clone()` preserves the memoized digest; all internal mutators call `GeoDetail::fork()`, which resets `cached_hash` to an empty `OnceLock`.

### 2. `P` as an ordinary point attribute column

`GeoDetail::positions` is removed as a dedicated field; point positions live under `"P"` in `point_attributes` as `AttributeColumn::Vec3(PagedColumn<Vec3>)`.

- `geo.attribute("P")` and `geo.attribute_names()` now include `"P"`.
- `with_attribute("P", col)` / `with_domain_attribute(AttrDomain::Point, "P", col)` accepts `Vec3` columns of length `self.len()`, rejects non-`Vec3` columns with `GeoError::AttributeTypeMismatch`, and rejects `"P"` on non-point domains (and in groups or `promote_attribute`) with `GeoError::ReservedAttributeName`.
- `geo.positions()`, `geo.point(i)`, `geo.with_positions(...)`, and `geo.with_point(...)` remain as typed convenience accessors over the `"P"` column.

### 3. `.ofxg` format version bump (v2)

Because the `geodetail-v2` Merkle root changes every geometry content hash once, `pcc-io` bumps `OFXG_MAGIC` from `OFXG\x01\x00\x00\x00` to `OFXG\x02\x00\x00\x00`. `"P"` continues to serialize in the positions section (section 1) and is skipped when writing section 3 to avoid duplicating position bytes.

### 4. Thread-local hash counters

`pcc_geo::hash_stats() -> HashStats` exposes thread-local cache-miss counters (`page_hashes`, `column_hashes`, `detail_hashes`) with `HashStats::delta(earlier)` so tests verify zero redundant hashing in parallel without cross-thread interference.

## Verification

- **Before/after benchmark (release mode, Windows, 10 M-point 5-node chain: `create(10_000_000)` -> `transform` -> `transform` -> `pack` -> `transform`):**

| Metric | Before A1 (`ef2a06d`) | After A1 | Ratio |
|---|---:|---:|---:|
| Cold cook (5 nodes, 10 M pts) | 2,645.80 ms | 497.69 ms | 5.3× faster |
| Re-cook after downstream edit | 1,092.21 ms | 0.017 ms | ~64,000× faster |
| Upstream hashes during re-cook | 30 M pts walked | 0 page, 0 column, 0 detail | — |

- **Exit evidence suite (`crates/pcc-eval/tests/merkle_hashing.rs`):**
  - 10 M-point chain re-cook after downstream parameter edit performs `0` page hashes, `0` column hashes, and `0` detail hashes.
  - Lazy construction performs `0` page hashes until `.hash()` is called; single-element edit across a 4-page column hashes exactly `1` page.
  - Constant pages (`PagedColumn::constant` and uniform `from_vec`) match dense hashes and survive perturb-and-restore with identical hashes.
  - `"P"` behaves as an ordinary `Vec3` point attribute column and every mutating `GeoDetail` method invalidates the memoized detail hash.
- `cargo xtask ci` passes on Windows (340 tests, `fmt`, `clippy -D warnings`, AST audit, tier DAG).

## Limits

- `HashStats` is thread-local, which is exact while evaluation is serial (A1–A7). When Phase A8 introduces `rayon` parallelism, worker-thread hash counts will need aggregation or an atomic counter scoped to parallel tests.
- `VdbGrid::hash()` still hashes its brick-hash list on each call, though `GeoDetail::hash()` memoizes the detail root around it.
- `bytemuck` was not added in A1 (deferred until `cargo-deny` is enforced in A9); 4 KiB stack-buffered little-endian chunking gives comparable SIMD chunking for BLAKE3 without `unsafe` or new dependencies.
- `cargo-mutants` has not yet been run locally on the A1 diff (covered by the pull-request mutation workflow).
