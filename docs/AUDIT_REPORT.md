# Architecture Audit Report — 2026-10-31

## Verdict

The initial suite is ambitious and useful, but was not implementable as written without creating high-risk false promises. It treated several crates as mature, pure-Rust interoperability solutions based on their names and blurred protocol, state and determinism boundaries. The plan is now explicitly a staged qualification programme rather than a promise of Houdini parity from unproven dependencies.

## Corrections applied

- Verified registry reality at audit date: `rmcp` 3.8.0; `cubecl` 0.12.0-pre.1; `burn` 0.23.0-pre.1; `ort` 2.0.0-rc.15; `rapier3d` 0.36.1; `parry3d` 0.31.1; `faer` 0.24.4; `kiddo` 6.3.0; `i_overlay` 9.1.0; `exr` 1.74.2; `ufbx` 0.11.5; `egui` 0.37.0; `egui_tiles` 0.18.0; `nucleo` 0.5.0; and `cranelift-jit` 0.137.1. Versions are discovery facts, not lockfile recommendations.
- `openusd` 0.8.0, `vdb` 0.4.0 and `nanovdb-rs` 0.0.7 are no longer described as production-complete pure-Rust parity. OpenUSD/OpenVDB/Alembic/FBX sit behind opt-in, pinned native adapters until conformance and Windows/Linux/macOS builds pass.
- Marked `manifold-rust`, `boolmesh`, `i_overlay`, `shader-slang`, pre-release CubeCL/Burn, ONNX Runtime and JIT as qualified dependencies with concrete fallback/containment requirements. License and native transitive dependencies must be evaluated from the locked source, not only crates.io metadata.
- Corrected MCP framing: 2026-07-28 is a stateless-core revision with extensions; engine transactions and job lifetime are application semantics, not protocol sessions.
- Clarified VFX Platform: CY2026 is final and CY2027 is published/targeted; exact dependencies remain a platform matrix, not one generic claim.

## Architectural gaps closed in the documents

- Added concrete core structs, hashing/alignment/byte accounting, invalid-data behavior, revisioned snapshots, conflict semantics, cancellation and hard resource circuit breakers.
- Specified deterministic lowering for for-each, feedback and Solver SOP state zones; compile blocks now have a no-hidden-side-effect contract.
- Added global SI/right-handed/Y-up convention, rational time, shutter samples, USD metadata and end-to-end OCIO/ACEScg/UI/EXR color flow.
- Defined a fixed-order `CouplingWorld` protocol, APIC/FLIP and MGPCG formulas/pseudocode parameters, robust singular-matrix handling, and an atomic checkpoint/domain-slicing contract.
- Defined packed-instance SOP→RBD→USD PointInstancer→TLAS/BLAS path without mesh explosion.
- Added root-MCP concurrent-agent scenario, idempotent revisioned mutation, scoped tool authority and untrusted-content rule.
- Replaced the project-name ambiguity with three provisional candidates and a mandatory trademark/legal gate; documented the distinction from ASWF OFX.
- Strengthened `xtask` requirements against stub, test-only and fake-benchmark loopholes.

## Mandatory Phase 0/1 prototypes

1. **Data/evaluator spike:** 16K-page CoW geometry, canonical hashing, state-zone checkpoint replay, cancellation and memory-breaker tests. Do not start broad node work first.
2. **Native-adapter matrix:** minimal OpenUSD/OpenVDB/Alembic/ufbx builds and round trips on Windows, macOS and Linux using pinned source/toolchains. Decide supported feature tiers from evidence.
3. **Numerics kernel:** one CPU reference plus GPU comparison for MAC projection/MGPCG, APIC transfer and deterministic reductions before FLIP/Pyro scope.
4. **Coupling experiment:** fluid vs moving rigid/cloth proxy with fixed iteration ledger, conservation measurements and recovery from failed substeps.
5. **MCP harness:** protocol conformance, revision conflicts, cancellation and authorization/capability tests for stdio and HTTP.
6. **Color/instance path:** ACES/OCIO texture-to-EXR golden scenes and a million-instance packed path that proves no implicit unpack.

## Sources consulted

- [MCP 2026-07-28 release](https://blog.modelcontextprotocol.io/posts/2026-07-28/) and [official specification release](https://github.com/modelcontextprotocol/modelcontextprotocol/releases/tag/2026-07-28)
- [VFX Reference Platform](https://vfxplatform.com/) and [CY2026 alignment table](https://aps.autodesk.com/developer/overview/me-vfx-platform-alignment)
- [OpenUSD documentation](https://openusd.org/release/index.html), [OpenVDB](https://www.openvdb.org/), [MaterialX](https://materialx.org/), [OpenColorIO](https://opencolorio.org/)
- crates.io API registry records queried on 2026-10-31 for every crate named above, including `openusd`, `vdb`, `nanovdb-rs`, `manifold-rust`, `boolmesh` and `shader-slang`.
