# 11 — Production Engineering Process, Anti-Slop Guardrails, and Build Pipeline

> **Scope:** Designing a zero-compromise software engineering pipeline for a 50–100 crate Rust DCC codebase authored by AI coding agents—ensuring every module is mathematically verified, benchmarked, audited, and free of "AI slop" (stubs, hallucinated APIs, tautological tests, or silent regressions).

---

## 1. Anatomy of "AI Slop" in Complex Systems & How We Engineer It Out

Published experience reports on large-scale agentic coding (2024–2026) identify seven recurring failure modes when AI agents write systems software without structural enforcement:

| Failure Mode ("AI Slop") | What Agents Do When Unchecked | Our Mechanical Enforcement Gate (`cargo xtask audit`) |
|---|---|---|
| **1. Stubs & Placeholder Logic** | Leave `todo!()`, `unimplemented!()`, `// TODO: implement full multigrid`, or return dummy input geometry unchanged. | **Compile-time & AST Ban:** `clippy::todo`, `clippy::unimplemented`, `clippy::dbg_macro`, `clippy::panic_in_result_fn` set to **`deny`** workspace-wide, plus custom `xtask check-no-stubs` regex/AST scanner banning placeholder comments on `main`. |
| **2. Tautological / Vacuous Tests** | Write `assert!(result.is_ok())` without checking output geometry values, conservation laws, or topological invariants. | **Mutation Testing Gate (`cargo-mutants`):** CI injects AST mutations (flipping `+` to `-`, `<` to `<=`, deleting statements) into modified functions. If the test suite still passes when the math is mutated, **the PR is rejected**. |
| **3. Test Weakening to Force Green CI** | Delete failing tests, add `#[ignore]`, or widen numerical tolerance from `1e-5` to `10.0` when a refactor breaks a solver. | **Test Ratchet Gate (`xtask check-test-ratchet`):** Diffs PR against `main`: any removed `#[test]`, newly added `#[ignore]`, or loosened tolerance constant in `tests/` fails CI unless explicitly signed off by a human checkpoint tag. |
| **4. Architectural Spaghetti & Layer Violations** | Import UI types into the geometry kernel or bypass the root `ofx-cmd` command bus to mutate scene state directly. | **Strict Crate DAG Enforcement (`cargo-deny` + `xtask check-layers`):** Acyclic 6-tier crate hierarchy where lower tiers cannot depend on higher tiers. |
| **5. Hallucinated or Bloated Dependencies** | Pull in 5 different linear algebra or random number crates across different subagents. | **Locked Dependency Allowlist (`deny.toml`):** Adding any new external crate to `Cargo.toml` fails `cargo deny check bans` until added to the central architecture allowlist. |
| **6. Unchecked `unsafe` Memory Bugs** | Use raw pointer casts that violate alignment or aliasing rules. | **`#![forbid(unsafe_code)]` by Default:** 90% of crates forbid `unsafe`. The 4 low-level crates (`ofx-geo`, `ofx-vdb`, `ofx-vex`, `ofx-plugin-abi`) require `#[expect(unsafe_code)]` with mandatory `// SAFETY:` proofs and pass **Miri (`cargo +nightly miri test`)** in CI. |
| **7. Silent Performance Regressions** | Replace an $O(N)$ spatial hash lookup with an $O(N^2)$ nested point loop that passes small unit tests (`N=20`) but freezes on `N=1,000,000`. | **Asymptotic Complexity & Instruction-Count Gate:** Every operator and solver benchmark runs at two scales ($N_1 = 10^3, N_2 = 10^5$) to assert empirical scaling exponent $\frac{\log(T_2/T_1)}{\log(N_2/N_1)} \le 1.15$, backed by deterministic instruction counting (`gungraun`/`iai-callgrind`). |

---

## 2. Workspace Layout & Enforced Layer Hierarchy

```
openfx/
├── Cargo.toml                  # Workspace root: [workspace.dependencies], [workspace.lints]
├── deny.toml                   # cargo-deny license, advisory, and dependency bans
├── rust-toolchain.toml         # Pinned stable rustc (1.93+) + rustfmt + clippy + miri components
├── AGENTS.md                   # Mandatory rules & workflow contract read by every coding agent
├── xtask/                      # Rust automation binary: `cargo xtask ci`, `audit`, `gen-schemas`
├── docs/                       # Research, ADRs (docs/adr/), and Master Plan (docs/PLAN.md)
└── crates/
    ├── tier0-foundation/       # ofx-id, ofx-math, ofx-arena, ofx-diagnostics (Zero internal deps)
    ├── tier1-data/             # ofx-geo, ofx-vdb, ofx-usd, ofx-io (Depends only on tier0)
    ├── tier2-execution/        # ofx-graph, ofx-eval, ofx-expr, ofx-vex, ofx-gpu (Depends on tier0..1)
    ├── tier3-domains/          # ofx-sop, ofx-sim-*, ofx-rig, ofx-groom, ofx-crowd, ofx-cop, ofx-render, ofx-ml, ofx-top
    ├── tier4-control/          # ofx-cmd (Root Command Bus), ofx-mcp, ofx-py, ofx-engine-abi
    └── tier5-apps/             # ofx-cli, ofx-ui (Desktop App), ofx-worker, ofx-test-harness
```

### 2.1 Workspace-Wide Compiler & Clippy Lints (`Cargo.toml`)

```toml
[workspace.lints.rust]
unsafe_code = "forbid"
missing_docs = "warn"
unused_must_use = "deny"
unreachable_pub = "warn"

[workspace.lints.clippy]
all = { level = "deny", priority = -1 }
pedantic = { level = "warn", priority = -1 }
todo = "deny"
unimplemented = "deny"
dbg_macro = "deny"
unwrap_used = "deny"
expect_used = "warn"
panic = "deny"
float_cmp = "deny"
large_stack_arrays = "deny"
rc_buffer = "deny"
```
*(Note: `unwrap_used = "deny"` in library crates forces explicit typed error propagation via `thiserror` across all operators and solvers).*

---

## 3. Seven-Tier Verification Pyramid

Every feature must include tests across the applicable tiers before its PR can merge:

| Tier | Tooling | What It Verifies |
|---|---|---|
| **T1: Unit & Doc Tests** | `cargo nextest run` + `cargo test --doc` | Every public function, edge case, and documentation code example compiles and passes. |
| **T2: Property-Based Invariant Tests** | **`proptest`** | Generates thousands of random `GeoDetail` topologies, parameter combinations, and graph edits; checks algebraic invariants (e.g., `Unpack(Pack(geo)) == geo`, `Serialize(Deserialize(scene)) == scene`, `Page::ref_count` CoW sharing). |
| **T3: Undefined Behavior & Concurrency** | **`cargo miri test`** + **`loom`** | Runs page allocators, SIMD slices, and lock-free cache structures under Miri (detecting alignment, aliasing, or uninitialized reads) and Loom (exhaustive thread-interleaving permutation check). |
| **T4: Differential & Fuzz Testing** | **`cargo-fuzz` / `bolero`** | Fuzzes file parsers (`.ofxs`, `.bgeo`, `.vdb`, `.usd`), VEX compiler (`VM` vs `Cranelift JIT`), and `manifold-rust` booleans on malformed inputs. |
| **T5: Quantitative Physical & Mathematical Oracles** | `ofx-test-harness` | Analytical PDE solutions (Taylor-Green vortex, Euler-Bernoulli cantilever beam, catenary drape, White Furnace energy conservation, Gauss-Bonnet curvature). |
| **T6: Visual & Perceptual Regression** | Headless `wgpu` Lavapipe + **NVIDIA FLIP** | Renders golden scenes and computes perceptual image error against baseline EXR/PNGs (must achieve mean FLIP $< 0.015$). |
| **T7: End-to-End MCP Agent Evals** | `ofx-mcp` test client | Drives the entire engine purely through MCP JSON-RPC calls (creating networks, cooking, wedging, asserting geometry telemetry) exactly as an external AI agent does. |

---

## 4. Multi-Agent Workflow Protocol (Preventing Cross-Agent Conflicts)

Because this project is built by autonomous and semi-autonomous AI coding agents:

1. **Spec-First ADR Gate:** Before implementing any new subsystem or solver, an Architecture Decision Record (`docs/adr/NNN-title.md`) defining the exact Rust traits, mathematical equations, and validation oracle must be committed.
2. **Isolated Worktrees / Branched Workspaces per Subagent:** Parallel coding agents work on disjoint crate slices (or isolated git worktrees) with explicit interface contracts in `tier0`–`tier2`.
3. **Independent Reviewer Agent Audit:** Before any milestone is marked complete, a separate **Auditor Subagent** (prompted specifically to find numerical bugs, stubbed branches, missing edge cases, or test weaknesses) reviews the diff and runs `cargo xtask audit` + `cargo mutants --in-diff`.
4. **Fast Incremental Compile Times (<3 seconds):**
   - Enable **`lld` / `mold`** linker and split-debuginfo in `.cargo/config.toml`.
   - Use **`cargo-hakari` (`workspace-hack` crate)** so shared dependencies (`serde`, `glam`, `tokio`, `wgpu`) compile once across all 60+ workspace crates with unified feature flags.

---

## 5. Licensing & Supply-Chain Policy

- **Project License:** Dual **MIT OR Apache-2.0** (matching the standard Rust ecosystem and ASWF compatibility).
- **Permitted Dependency Licenses (`deny.toml`):** `MIT`, `Apache-2.0`, `Apache-2.0 WITH LLVM-exception`, `BSD-2-Clause`, `BSD-3-Clause`, `Zlib`, `ISC`, `Unicode-3.0`, `MPL-2.0` (file-level copyleft, safe for unmodified crate dependencies like `vdb`).
- **Strictly Forbidden Licenses (`deny = ["GPL-2.0", "GPL-3.0", "AGPL-3.0", "LGPL-2.1", "LGPL-3.0", "CC-BY-NC-4.0"]`):** Enforced on every build via `cargo deny check licenses`.

---

## 6. Verbatim Definition of Done (DoD) Checklist

No task, node, or milestone is complete until **all 10 checks pass with zero warnings**:

- [ ] **1. Zero Stubs:** No `todo!()`, `unimplemented!()`, `dbg!()`, `.unwrap()` (in library code), or placeholder comments.
- [ ] **2. Clean Static Analysis:** `cargo fmt --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings` pass with 0 warnings.
- [ ] **3. License & Supply Chain:** `cargo deny check` passes with 0 advisories, 0 banned crates, and 0 unapproved licenses.
- [ ] **4. Unit, Doc & Property Tests:** `cargo nextest run --workspace` and `cargo test --workspace --doc` pass 100%.
- [ ] **5. Determinism Check:** Multi-threaded cook/simulation test runs with `RAYON_NUM_THREADS=1` and `RAYON_NUM_THREADS=8` and asserts bit-identical output hashes.
- [ ] **6. Miri Safety (if touching `unsafe`):** `cargo +nightly miri test` passes on the modified crate.
- [ ] **7. Quantitative Domain Oracle:** Physical/geometric output verified against an analytical formula or reference conservation law (not merely `is_ok()`).
- [ ] **8. Mutation Resilience:** `cargo mutants --in-diff` achieves $\ge 85\%$ mutation kill score on new algorithmic logic.
- [ ] **9. Root MCP Exposure:** New functionality is registered in `ofx-cmd` / `ofx-mcp` with complete JSON Schema (`schemars`), description strings, and an end-to-end MCP tool call test.
- [ ] **10. Documentation Sync:** Rustdoc comments (`///`) and user/agent catalog docs build cleanly via `cargo doc --no-deps` with `-D rustdoc::broken_intra_doc_links`.

---

## Audit addendum — close anti-slop loopholes

`cargo xtask audit` must parse Rust AST/HIR (not grep alone) and fail production paths containing `todo!`, `unimplemented!`, `panic!`, `unwrap/expect`, empty error branches, `#[ignore]`, unconditional test substitutes, broad `allow`, disabled assertions, fabricated fixtures, or mock-only implementations. Allowlists need owner, issue, expiry and a release-path proof. CI must use clean checkouts; run dependency/license/SBOM checks, native adapter smoke builds on all three target OSes, mutation thresholds per critical crate, fuzz-corpus replay, thread-count determinism, and negative tests proving every resource breaker fires. Coverage is a floor, not behavioral evidence. Review includes generated code and test mutation, not merely a green workflow.
