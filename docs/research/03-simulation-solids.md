# 03 — Solid, Deformable, Granular, and Rigid-Body Simulation

> **Scope:** Unified position-based dynamics (XPBD / Vellum-equivalent), Vertex Block Descent (VBD / Otis-equivalent muscle & tissue), Finite Elements (FEM), Material Point Method (MLS-MPM), Rigid-Body Destruction (RBD), guaranteed non-penetrating contact (IPC), and physical validation tests.

---

## 1. Solver Taxonomy & Why Multiple Solvers Coexist

Just as Houdini separates **Vellum**, **Otis**, **Bullet RBD**, **FEM**, and **MPM**, no single numerical discretization handles every solid regime optimally:

| Regime | Best-Fit Architecture | Why |
|---|---|---|
| **Cloth, Hair, Feathers, Thin Shells, Dry Grains, Soft Inflation** | **Small-Steps XPBD** (Macklin et al. 2016/2019) | Extremely fast per iteration on GPU, handles complex constraint graphs (stitching, tearing, pins, sliding curves), unconditionally stable. |
| **Anatomical Muscles, Fascia, Fat, High-Stiffness Tetrahedral Soft Bodies, High Mass Ratios** | **Vertex Block Descent (VBD)** (Chen et al., SIGGRAPH 2024) + **Augmented VBD** (2025) | Minimizes exact variational implicit Euler energy $\sum E_i(\mathbf{x})$ via per-vertex $3\times 3$ block Newton steps; handles $10^4:1$ stiffness/mass ratios where XPBD becomes stretchy. |
| **Multi-Phase Elastoplastic Continuum (Snow, Mud, Wet Sand, Lava, Melting, Post-Fracture Chunks)** | **MLS-MPM** (Hu et al., SIGGRAPH 2018) | Topology-free large deformation and self-collision on a background grid without mesh tangling or remeshing. |
| **Large-Scale Architectural Destruction (100k+ Rigid Fragments, Hinge/Glue/Cone-Twist Constraints, Metal Plasticity)** | **Rigid Body Dynamics (RBD)** (`Rapier` / XPBD rigid islands + impulse-based contact) | $6$ DoF per rigid chunk is $100\times$ cheaper than continuum grids until fracture threshold is exceeded. |
| **Guaranteed Intersection-Free Hero Cloth / Knots / High-Velocity Thin-Shell Impact** | **IPC / GIPC barrier contact** (Li et al. 2020; Huang et al. 2024) | Logarithmic barrier potential $b(d)$ with continuous collision detection (CCD) line search guarantees zero self-intersections under arbitrary compression. |

---

## 2. Unified Position-Based Dynamics (`ofx-sim-xpbd` — Vellum Equivalent)

### 2.1 Small-Steps XPBD Formulation

Following **Macklin, Müller, Chentanez (MIG 2016), *XPBD: Position-Based Simulation of Compliant Constrained Dynamics*** and **Macklin et al. (SCA 2019), *Small Steps in Physics Simulation***:
- Instead of 1 large timestep with 40 Gauss-Seidel iterations, we divide each frame step into $N_{\text{sub}} \in [8, 32]$ substeps with $1\text{--}2$ constraint iterations per substep. Because compliance scales as $\tilde{\alpha} = \frac{\alpha}{\Delta t^2}$, smaller $\Delta t$ quadratically improves convergence and eliminates iteration-count-dependent stiffness:

$$\Delta \lambda_j = \frac{-C_j(\mathbf{x}) - \tilde{\alpha}_j \lambda_j}{\nabla C_j \, \mathbf{M}^{-1} \nabla C_j^T + \tilde{\alpha}_j + \tilde{\gamma}_j}, \qquad \Delta \mathbf{x} = \mathbf{M}^{-1} \nabla C_j^T \Delta \lambda_j$$

### 2.2 Constraint Catalog

| Constraint Type | Arity | Purpose |
|---|---|---|
| `Distance` / `EdgeStretch` | 2 pts | Cloth warp/weft/shear, hair segment stretch, strut constraints |
| `TriangleARAP` / `FEM2D` | 3 pts | Anisotropic Continuum Triangle (Baraff-Witkin / Continuum cloth) independent of mesh triangulation bias |
| `DihedralBend` / `IsometricBend` | 4 pts (hinge) | Cloth & sheet bending, with plastic yield threshold $\theta_{\text{yield}}$ for crumpled paper/sheet metal |
| `CosseratBendTwist` | 2 pts + 1 `Quatf` edge frame | **Discrete Elastic Rods / Hair / Fur / Feathers** (Kugelstadt & Schömer 2016) with H22-equivalent quasistatic twist solve |
| `TetVolume` / `TetNeoHookean` | 4 pts | Soft-body volume preservation and deviatoric elasticity |
| `ShapeMatch` / `BalloonPressure` | $N$ pts | Closed-surface global volume pressure $C(\mathbf{x}) = \sum (\mathbf{x}_i \times \mathbf{x}_j) \cdot \mathbf{x}_k - 6 V_0$ |
| `PinToTarget` / `AttachToGeo` | 1 pt + target prim UV | Sliding cloth-on-body or belt-loop constraints with breakable force threshold $F_{\text{break}}$ |
| `GranularContact` | 2 spheres | Dry sand/pebble friction & cohesion |

**Parallel Graph Coloring:** Constraint batches are partitioned at setup time via greedy Welsh-Powell / Luby parallel graph coloring so constraints within a color share no vertices and execute in parallel across SIMD lanes and GPU threads with zero atomic conflicts (falling back to Jacobi accumulation for high-valence dynamic contacts).

---

## 3. Vertex Block Descent (`ofx-sim-vbd` — Otis Muscle & Tissue Equivalent)

Houdini's Otis solver replaced multi-pass Vellum muscles with **Vertex Block Descent** (**Chen, Chitalu, Lin, Komura, SIGGRAPH 2024**, [*Vertex Block Descent*](https://ankachan.github.io/VBD/)).

### 3.1 Core Mathematical Mechanism
VBD minimizes the global Backward Euler incremental potential:

$$E(\mathbf{x}) = \frac{1}{2 \Delta t^2} \|\mathbf{x} - \mathbf{y}\|_{\mathbf{M}}^2 + \sum_{e \in \mathcal{E}} \Psi_e(\mathbf{x}) + \sum_{c \in \mathcal{C}} B_c(\mathbf{x})$$

by updating each vertex $\mathbf{x}_i \in \mathbb{R}^3$ in parallel within a vertex-colored Gauss-Seidel sweep using only its local $3\times 3$ Hessian block $\mathbf{H}_i \in \mathbb{R}^{3\times 3}$ and force gradient $\mathbf{g}_i \in \mathbb{R}^3$:

$$\mathbf{H}_i = \frac{m_i}{\Delta t^2} \mathbf{I}_3 + \sum_{e \ni i} \frac{\partial^2 \Psi_e}{\partial \mathbf{x}_i^2}, \qquad \mathbf{g}_i = -\frac{m_i}{\Delta t^2}(\mathbf{x}_i - \mathbf{y}_i) - \sum_{e \ni i} \frac{\partial \Psi_e}{\partial \mathbf{x}_i}, \qquad \Delta \mathbf{x}_i = \mathbf{H}_i^{-1} \mathbf{g}_i$$

- **Why VBD excels:** Every vertex solve is an analytic $3\times 3$ linear system ($\det \mathbf{H}_i$ closed-form Cramer's rule in registers—no global matrix assembly), guaranteed to monotonically decrease $E(\mathbf{x})$ (with SPD Hessian clamping + Chebyshev acceleration).
- **Muscle Fiber Activation (Stable Neo-Hookean + Anisotropic Hill-Type Fiber):**
  $$\Psi_{\text{muscle}}(\mathbf{F}, a) = \Psi_{\text{SNH}}(\mathbf{F}) + \frac{k_{\text{fiber}}}{2} \left( \|\mathbf{F} \mathbf{d}_0\| - (1 - a(t) \, \epsilon_{\max}) \right)^2$$
  where $\mathbf{d}_0 \in \mathbb{R}^3$ is the rest muscle fiber tangent field and $a(t) \in [0, 1]$ is the activation channel driven by KineFX/APEX skeletal joint angles or tension lines.
- **Unified Multi-Layer Solve:** Simulates tetrahedral muscles, tetrahedral fat/tissue core, and triangular fascia/skin shell simultaneously in one coupled VBD graph.

---

## 4. Material Point Method (`ofx-sim-mpm` — Snow, Sand, Mud, Metals)

Implemented using **Moving Least Squares MPM (MLS-MPM)** (**Hu et al., SIGGRAPH 2018**) over the same $8^3$ sparse brick infrastructure (`ofx-vdb`) used by FLIP/Pyro:

1. **Constitutive Models per Particle:**
   - **Elastoplastic Snow (Stomakhin et al., SIGGRAPH 2013):** Singular values of elastic deformation gradient $\mathbf{F}_E = \mathbf{U} \boldsymbol{\Sigma} \mathbf{V}^T$ clamped to $[\,1 - \theta_c, \, 1 + \theta_s\,]$ with exponential strain hardening $\mu(\xi_p), \lambda(\xi_p) = \mu_0 e^{\xi \det \mathbf{F}_P}$.
   - **Drucker-Prager Granular Sand & Soil (Klar et al., SIGGRAPH 2016):** Projects logarithmic Hencky strain $\boldsymbol{\epsilon} = \ln \boldsymbol{\Sigma}$ onto the conical Drucker-Prager yield surface with friction angle $\phi_f$ and cohesion $c$.
   - **Von Mises Ductile Metal & Herschel-Bulkley Mud/Lava:** Deviatoric trial stress $\mathbf{s}^{\text{trial}}$ radial return mapping.
2. **Heightfield & VDB Collider Modes:** Supports static, rigid-transformed, and deforming SDF colliders with sticky, slip, and Coulomb friction boundary projections.

---

## 5. Rigid-Body Destruction (`ofx-sim-rbd`) & Voronoi Fracture

- **Solver Backbone:** Integrate **`rapier3d`** (Apache-2.0, Dimforge) for standard rigid-body dynamics and broad/narrow-phase collision (`parry3d`), augmented with a high-throughput **XPBD Rigid Constraint Network Solver** for destruction glue/hinge/spring/plastic-metal-bend constraints between 100,000+ packed Voronoi fragments.
- **Plastic Metal Bending & Tearing (H22 Equivalent):** When angular or linear impulse across an RBD constraint exceeds `yield_threshold`, the constraint's rest relative transform $\mathbf{T}_{\text{rest}}$ updates plastically toward the deformed pose until total plastic strain exceeds `tear_threshold`, at which point the constraint breaks and optionally triggers dynamic sub-fracturing.
- **Tetrahedral Meshing & Convex Decomposition Licensing Alert:**
  - **TetGen is AGPLv3 — STRICTLY FORBIDDEN.**
  - **CGAL is GPLv3+ — STRICTLY FORBIDDEN.**
  - **Approved Permissive Alternatives:** **`fTetWild`** (MPL-2.0) / pure-Rust Delaunay refinement for tetrahedralization; **`coacd`** (MIT) and **`vhacd`** (BSD-3) for approximate convex decomposition; **`voro++`** (BSD-3) / pure-Rust clipped 3D Voronoi cells for Voronoi & boolean wood/concrete/glass material fracture.

---

## 6. Ecosystem & Reference Library Assessment (2026)

| Library / Crate | URL | License | Status (2026) | Recommendation |
|---|---|---|---|---|
| **`rapier3d` / `parry3d`** | [rapier.rs](https://rapier.rs/) | Apache-2.0 | Actively maintained, pure Rust, SIMD & deterministic mode (`enhanced-determinism` feature) | **Adopt** for RBD base solver, BVH, spatial queries, and convex contact manifolds. |
| **Newton / NVIDIA Warp** | [github.com/newton-physics/newton](https://github.com/newton-physics/newton) | Apache-2.0 | Linux Foundation hosted (2025–2026) | **Reference** for GPU XPBD/VBD/MuJoCo-style kernels and differentiable contact formulations; port core kernels to Rust + `CubeCL`. |
| **`libuipc`** | [github.com/spiriMirror/libuipc](https://github.com/spiriMirror/libuipc) | Apache-2.0 | Modern C++ Unified Incremental Potential Contact | **Reference** for GIPC / offset geometric contact barrier formulations. |
| **Jolt Physics** (`joltc` / Rust bindings) | [github.com/jrouwe/JoltPhysics](https://github.com/jrouwe/JoltPhysics) | MIT | Production AAA standard | Optional C-FFI benchmark baseline for RBD throughput comparisons. |

---

## 7. Mandatory Physical Validation & Benchmark Suite

1. **Cantilever Beam Deflection (FEM & VBD):** 3D beam under gravity compared against Euler-Bernoulli / Timoshenko analytical tip deflection $\delta = \frac{q L^4}{8 E I}$ across mesh refinements ($<2\%$ relative error).
2. **Cook's Skewed Membrane & Locking Patch Test:** Verify Stable Neo-Hookean tetrahedral elements do not suffer volumetric locking as Poisson's ratio $\nu \rightarrow 0.49$.
3. **Cusick Cloth Drape & Analytical Catenary Curve:** Hanging cable/ribbon under gravity must match exact hyperbolic cosine catenary $y(x) = a \cosh(x/a)$ to $<0.1\%$ RMSE regardless of substep count ($8, 16, 32$).
4. **CCD & Barrier Non-Penetration Stress Test:** Twist a double-layered cloth ribbon $1080^\circ$ and fire a $100\text{ m/s}$ sphere through a 1 mm cloth mesh; verify **strictly zero self-intersecting triangles** (`parry3d` triangle-triangle intersection count $= 0$).
5. **Granular Angle of Repose (MPM & XPBD Grains):** Column collapse of cohesionless sand with internal friction angle $\phi_f = 30^\circ$ must settle to a final pile slope within $30^\circ \pm 2^\circ$ matching Lube et al. (2004) scaling laws.
6. **RBD 20-Tier Pyramid & Cross-Thread Determinism:** 20-high box pyramid remains stationary for 300 frames with zero jitter; 1,000-shard fracture explosion produces bit-identical transforms across runs (`enhanced-determinism`).

---

## 8. Recommendations, Open Risks, and Milestones

### Recommendations
1. **Build `ofx-collision` as a shared contact substrate** (BVH + spatial hash + cubic root-finder CCD + barrier potentials) consumed by XPBD, VBD, RBD, and MPM.
2. **Prioritize Small-Steps XPBD and RBD first** (highest artist utility), followed immediately by **VBD** (shared tetrahedral energy code with FEM, plus powers Otis-class muscles) and **MLS-MPM**.

### Open Risks
- **CCD Root-Finder Degeneracies in Floating Point:** Coplanar point-triangle and collinear edge-edge impacts can cause cubic solver misses in `f32`. *Mitigation:* Execute narrow-phase CCD inclusion root-finding (Wang et al. 2021 *Tight-Inclusion CCD*) in `f64` with conservative interval arithmetic bounds.

### Suggested Milestones

---

## Audit addendum — robust constitutive evaluation and coupling boundary

XPBD uses `Delta lambda = -(C + alpha_tilde*lambda + gradC·M^-1 f_ext*dt^2) /(sum_i w_i |grad_i C|^2 + alpha_tilde)` and `alpha_tilde=compliance/dt^2`; zero denominator means no correction and a diagnostic, never division by zero. Constraint colors are computed from stable constraint IDs and recomputed after tearing. CCD failure, self-intersection, or a non-finite position aborts that substep and restores its checkpoint.

For VBD/MPM, all 3x3 inversions/SVDs must use a deterministic robust routine: reject non-finite input; clamp singular values to `[sigma_min, sigma_max]` with documented material-scale thresholds; enforce proper rotations (`det(UV^T)>0` by flipping the least-significant singular vector); if factorisation fails, use the previous valid deformation gradient for that element, mark it invalid, and stop after a per-tick invalid-element threshold. Zero-volume tets are refused at ingest. The solver API exchanges only `CouplingWorld` ports described in `02`; it must not take mutable references to FLIP, pyro, XPBD, or rig state.
- **M3.1 — `ofx-sim-rbd` + Voronoi Fracture:** Integrate `rapier3d` (`enhanced-determinism`), glue/hinge/plastic-bend constraint networks, and 3D Voronoi cell cutter.
- **M3.2 — `ofx-sim-xpbd` Unified Solver:** Small-steps XPBD for cloth, Cosserat hair/rods, soft bodies, balloons, and dry grains with graph-colored SIMD & `CubeCL` GPU execution.
- **M3.3 — `ofx-sim-vbd` Muscle & Tetrahedral Soft-Body Solver:** Vertex Block Descent with Stable Neo-Hookean + anisotropic Hill muscle fibers, fascia sliding, and Tight-Inclusion CCD.
- **M3.4 — `ofx-sim-mpm` Multi-Material Continuum Solver:** Sparse-brick MLS-MPM for snow, Drucker-Prager sand, viscoplastic mud, and ductile fracture.
