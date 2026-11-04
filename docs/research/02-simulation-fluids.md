# 02 — Fluid, Gas, and Ocean Simulation

> **Scope:** Feature-film liquid simulation (APIC/FLIP/Narrow-Band FLIP), sparse gas & combustion (Pyro), spectral and hybrid oceans, linear solvers (MGPCG), surfacing, and quantitative physical validation suites.

---

## 1. Liquid Simulation Architecture (FLIP / APIC / PolyPIC)

### 1.1 Particle-In-Cell Transfer Schemes

Houdini's FLIP solver blends PIC (dissipative, stable) with FLIP (noisier, energy-preserving) and optionally APIC. For feature-film fidelity with lower particle counts and minimal surface noise, our solver implements three selectable transfer kernels over a staggered **Marker-and-Cell (MAC)** sparse grid:

| Scheme | Canonical Reference | State per Particle | Characteristics |
|---|---|---|---|
| **FLIP / PIC blend** | Bridson, *Fluid Simulation for Computer Graphics* (2015); Zhu & Bridson (2005) | Position $\mathbf{x}_p$, velocity $\mathbf{v}_p$ | Classic VFX workhorse ($\alpha \approx 0.95$ FLIP + $0.05$ PIC); prone to ringing instability without PIC damping. |
| **APIC** (Affine Particle-In-Cell) | Jiang et al., *The Affine Particle-In-Cell Method*, SIGGRAPH 2015 ([Paper](https://www.math.ucla.edu/~jteran/papers/JSSTS15.pdf)) | $\mathbf{x}_p, \mathbf{v}_p$, affine matrix $\mathbf{C}_p \in \mathbb{R}^{3\times 3}$ | Locally conserves linear **and angular** momentum across P2G/G2P transfers; eliminates noise without artificial PIC viscosity. |
| **PolyPIC** | Fu et al., *A Polynomial Particle-In-Cell Method*, SIGGRAPH Asia 2017 | $\mathbf{x}_p, \mathbf{v}_p$, truncated polynomial modes | Higher-order vortical conservation for turbulent splashing liquids. |

**Recommendation:** Default to **APIC** with quadratic B-spline weights ($N(\mathbf{x})$ stencil width 3), while retaining a classic FLIP/PIC blend ratio slider for artistic backwards-compatibility with Houdini workflows.

### 1.2 Narrow-Band FLIP (Memory & Speed Scaling)

Simulating particles throughout the entire deep interior of a tank wastes $>80\%$ of particle memory and P2G/G2P bandwidth. Following **Ferstl et al. (Eurographics 2016), *Narrow Band FLIP for Liquid Simulations***:
- **Surface Band ($-\tau_{\text{in}} \le \phi(\mathbf{x}) \le 0$, typically 3–6 voxels deep):** Tracked with full Lagrangian APIC particles ($8$ particles per voxel).
- **Deep Interior ($\phi(\mathbf{x}) < -\tau_{\text{in}}$):** Tracked purely on the Eulerian MAC grid (advected via MacCormack/BFECC) with zero particles; particles entering below $-\tau_{\text{drop}}$ are deleted, and grid cells rising into the band reseed particles deterministically via stratified blue-noise templates.

### 1.3 Incompressibility & Pressure Projection

At each substep, we enforce $\nabla \cdot \mathbf{u}^{n+1} = 0$ by solving the variable-coefficient Poisson equation:

$$\nabla \cdot \left( \frac{\Delta t}{\rho} \mathbf{W} \nabla p \right) = \nabla \cdot \mathbf{u}^* - \phi_{\text{div\_source}}$$

where:
- **Cut-Cell Variational Solid Boundaries (Batty et al., SIGGRAPH 2007):** $\mathbf{W} = \text{diag}(w_x, w_y, w_z)$ stores face area fractions ($w_f \in [0, 1]$) open to fluid, computed from the collider signed distance field $\phi_{\text{solid}}$. This eliminates stair-stepping artifacts on sloped terrain or curved vessels without requiring body-fitted tetrahedral meshes.
- **Ghost Fluid Free Surface (Enright et al. 2003):** At the liquid-air interface ($\phi_{\text{liquid}} = 0$), sub-cell interface location $\theta = \frac{\phi_i}{\phi_i - \phi_j} \in (0, 1]$ modifies the diagonal entry by $\frac{1}{\theta}$ and incorporates capillary Laplace pressure jump $p_{\text{surf}} = \sigma \kappa$ (surface tension $\sigma$, mean curvature $\kappa = \nabla \cdot \frac{\nabla \phi}{\|\nabla \phi\|}$).
- **Solver — Matrix-Free Geometric Multigrid Preconditioned Conjugate Gradient (MGPCG):**
  - Based on **McAdams, Sifakis, Teran (SCA 2010)** and **Aanjaneya et al. (2017)** over sparse VDB block hierarchies ($8^3$ leaf bricks).
  - V-cycle uses red-black Gauss-Seidel or damped Jacobi ($w = 2/3$) smoothing (SIMD/GPU-friendly, deterministic), restriction via 8-cell averaging, prolongation via trilinear interpolation, and a direct Cholesky/PCG solve at the coarsest level.
  - **Convergence advantage:** Reduces iteration count from $O(N)$ (standard IC(0)-PCG) to $8\text{--}15$ V-cycles independent of grid resolution ($512^3+$ domains).

### 1.4 Implicit Variational Viscosity & Non-Newtonian Rheology

For honey, lava, mud, and melted chocolate with spatially varying viscosity $\mu(\mathbf{x})$ spanning $6+$ orders of magnitude:
- Explicit viscosity requires $\Delta t \propto \Delta x^2 / \nu$, which explodes substeps.
- We implement **Batty & Bridson (2008)** and **Larionov, Batty, Bridson (2017)** implicit variational Stokes solver coupling $(u, v, w)$ via the strain-rate tensor $\dot{\boldsymbol{\epsilon}} = \frac{1}{2}(\nabla \mathbf{u} + \nabla \mathbf{u}^T)$:

$$\rho \mathbf{u}^{n+1} - \nabla \cdot \left( 2 \Delta t \, \mu(\mathbf{x}) \dot{\boldsymbol{\epsilon}}(\mathbf{u}^{n+1}) \right) = \rho \mathbf{u}^*$$

solved via matrix-free block-Jacobi / multigrid PCG.

### 1.5 Secondary Whitewater (Spray, Foam, Bubbles) & Trapped Air

Following **Ihmsen et al., *Unified Spray, Foam and Air Bubbles for Particle-Based Fluids* (2012)**:
- **Emission Criteria:** Secondary particles spawn where trapped air potential $I_{\text{ta}} = \text{clamp}(\|\mathbf{v}_{\text{diff}}\|)$, wave crest curvature $I_{\text{wc}} = \max(-\nabla \cdot \hat{\mathbf{n}}, 0)$, and kinetic energy $I_k = \frac{1}{2} m \|\mathbf{v}\|^2$ exceed artistic thresholds.
- **Three-Regime Classification by Liquid SDF $\phi(\mathbf{x})$:**
  1. **Spray ($\phi > d_{\text{spray}}$):** Ballistic motion + aerodynamic drag + wind field from Pyro/air velocity.
  2. **Foam ($| \phi | \le d_{\text{foam}}$):** Advected along the liquid surface velocity with surface-tangent projection and repulsive density constraints (lifespan decay $\tau$).
  3. **Bubbles ($\phi < -d_{\text{bubble}}$):** Buoyancy force $\mathbf{f}_b = -b \mathbf{g}$ + liquid drag $\mathbf{f}_d = c_d (\mathbf{u}_{\text{liquid}} - \mathbf{v}_b)$.

### 1.6 Liquid Surfacing (Particles $\rightarrow$ Renderable SDF / Mesh)

1. **Anisotropic Kernel Reconstruction (Yu & Turk, *Reconstructing Surfaces of Particle-Based Fluids Using Anisotropic Kernels*, TOG 2013):** Stretches ellipsoidal kernels along the local particle covariance eigenvectors so thin sheets and splashes form smooth, non-blobby surfaces instead of "metaball" spheres.
2. **VDB Level-Set Filtering Pipeline:** Rasterize particles to a narrow-band `ofx-vdb` signed distance field $\rightarrow$ Dilate $\rightarrow$ Mean-Curvature / Gaussian Smooth (masked by high-curvature splash droplets to preserve crisp crown splashes) $\rightarrow$ Erode $\rightarrow$ Dual Contouring / Adaptive Marching Cubes with velocity (`v`), vorticity (`vorticity`), and UV attribute transfer.

---

## 2. Sparse Smoke, Fire, and Explosions (Pyro Architecture)

### 2.1 Sparse Block Grid Representation (`ofx-vdb` Active Tiles)

Matching Houdini's Sparse Pyro and GPU Copernicus Pyro:
- The simulation domain is unbound and divided into **$8^3$ voxel bricks** managed by a 2-level spatial hash / VDB tree.
- **Dynamic Topology Dilation:** Before each substep, active bricks where density $\rho > \epsilon$, temperature $T > \epsilon$, fuel $F > \epsilon$, or $\|\mathbf{u}\| > \epsilon$ are dilated by a padding radius $r_{\text{pad}} = \lceil \|\mathbf{u}\|_{\max} \Delta t / \Delta x \rceil + 2$ bricks so advected plumes never clip against unallocated boundaries.
- **Memory Pruning:** Bricks whose field norms fall below $\epsilon_{\text{prune}}$ for $k$ frames are deallocated back to the arena pool.

### 2.2 Advection, Vorticity, and Combustion Model

| Stage | Algorithm | Notes & Validation |
|---|---|---|
| **Velocity & Scalar Advection** | **BFECC** (Back and Forth Error Compensation and Correction) or **MacCormack** with **monotonicity clamping** (Selle et al. 2008) to the local $2\times 2\times 2$ source cell extrema. | Second-order accurate in space/time; clamping prevents negative densities or overshoot ringing around sharp shock fronts. |
| **Reflection / Advection-Projection** | Optional **Advection-Reflection** (Zehnder, Narain, Thomaszewski, SIGGRAPH 2018; $2\times$ projections per step) | Preserves kinetic energy and turbulent rolls dramatically better than standard Chorin splitting at half the grid resolution. |
| **Combustion & Thermodynamics** | Coupled fields: `density` ($\rho$), `temperature` ($T$), `fuel` ($F$), `flame` ($B$, burn progress), `divergence` ($\nabla \cdot \mathbf{u}$). | When $T > T_{\text{ignition}}$, burn rate $r = k_{\text{burn}} \min(F, O_2)$; releases heat $\Delta T = c_h r$, generates soot $\Delta \rho = c_s r$, and injects gas expansion divergence $\nabla \cdot \mathbf{u} = c_{\text{exp}} r$ into the MGPCG Poisson RHS. |
| **Buoyancy & Shredding** | Boussinesq buoyancy $\mathbf{f}_{\text{buoy}} = (\alpha \rho - \beta (T - T_{\text{amb}})) \mathbf{g}$ + **Vorticity Confinement** ($\mathbf{f}_{\text{vc}} = \epsilon \Delta x (\hat{\mathbf{N}} \times \boldsymbol{\omega})$ where $\boldsymbol{\omega} = \nabla \times \mathbf{u}$) + procedural curl-noise turbulence. | Restores sub-grid Kolmogorov cascade energy lost to numerical dissipation. |

---

## 3. Spectral Oceans & Hybrid FLIP Coupling

1. **Spectral Wave Synthesis (Tessendorf 2001; Horvath *Empirical Directional Wave Spectra for Computer Graphics*, DigiPro 2015):**
   - Supports **JONSWAP**, **TMA** (finite-depth shallow water attenuation), and **Pierson-Moskowitz** directional spectra with Hasselman directional spreading.
   - Evaluates 2D complex spectrum $\tilde{h}(\mathbf{k}, t)$ across 2–4 cascaded tile bands (e.g., $500\text{ m}, 80\text{ m}, 12\text{ m}$) via inverse FFT (`rustfft` on CPU / `CubeCL` Radix-2 FFT on GPU) to produce displacement $\mathbf{D}(\mathbf{x}, t)$, analytic velocity $\mathbf{u}_{\text{ocean}}(\mathbf{x}, t)$, and Jacobian determinant $J(\mathbf{x}, t) = J_{xx} J_{yy} - J_{xy}^2$ for cusp/foam detection.
2. **Ocean-Layer FLIP Boundary Coupling:**
   - A localized FLIP domain tracks only a thin animated band around a ship hull or creature, while boundary velocities and pressure Dirichlet conditions on the 5 outer faces of the FLIP box are driven analytically by the Tessendorf spectrum $\mathbf{u}_{\text{ocean}}(\mathbf{x}, t)$ through a sponge-absorption layer.
3. **Render-Time Ocean Procedural:**
   - The path tracer and viewport evaluate the multi-band FFT displacement shader directly on coarse adaptive camera-frustum grids at render time, avoiding multi-gigabyte mesh caches for open water.

---

## 4. Ecosystem & Reference Library Assessment (2026)

| Project / Crate | URL | License | Status (2026) | Recommendation |
|---|---|---|---|---|
| **OpenVDB / NanoVDB** | [openvdb.org](https://www.openvdb.org/) | MPL-2.0 | ASWF standard | **Adopt** file format & NanoVDB GPU layout; use `vdb` / `nanovdb-rs` + custom `ofx-vdb` $8^3$ brick compute allocator. |
| **Mantaflow** | [mantaflow.com](http://mantaflow.com/) | Apache-2.0 | Academic reference | **Reference only** for algorithm cross-checking; reimplement cleanly in Rust/CubeCL. |
| **SPlisHSPlasH** | [github.com/InteractiveComputerGraphics/SPlisHSPlasH](https://github.com/InteractiveComputerGraphics/SPlisHSPlasH) | MIT | Active C++ SPH reference | **Reference** for DFSPH/IISPH formulations and whitewater parameter defaults. |
| **`salva`** (Dimforge) | [github.com/dimforge/salva](https://github.com/dimforge/salva) | Apache-2.0 | Archived / maintenance | **Avoid direct dependency** (SPH-only, not sparse-grid APIC/MGPCG). |
| **`rustfft`** | [crates.io/crates/rustfft](https://crates.io/crates/rustfft) | MIT / Apache-2.0 | Mature, SIMD-accelerated | **Adopt** for CPU spectral ocean synthesis. |
| **`faer`** | [crates.io/crates/faer](https://crates.io/crates/faer) | MIT | Active high-perf Rust linear algebra | **Adopt** for coarse-grid direct Cholesky solves inside MGPCG. |

---

## 5. Mandatory Numerical Validation & Benchmark Suite

Every fluid/gas PR is gated in CI against quantitative physical benchmarks (no visual "eyeballing"):

1. **Zalesak Slotted Disk Rotation (Advection Accuracy):** 1 full revolution ($628$ steps) of a slotted SDF/density disk in a rigid-body vortex field. Must verify $L_1$ area error $< 1.5\%$ and strictly zero negative density under/overshoots.
2. **2D & 3D Taylor-Green Vortex (Inviscid & Viscous Decay):** Compare kinetic energy $E(t)$ against exact analytical decay $E(t) = E_0 e^{-2 \nu k^2 t}$; verify second-order spatial convergence rate ($p \ge 1.9$) under grid refinement ($32^3 \rightarrow 64^3 \rightarrow 128^3$).
3. **Hydrostatic Tank Equilibrium:** Stationary liquid in a sloped VDB collider bowl over 200 frames with gravity. Maximum parasitic velocity must satisfy $\|\mathbf{u}\|_{\infty} < 10^{-5}\text{ m/s}$ and volume drift $|\Delta V / V_0| < 0.05\%$.
4. **Martin & Moyce (1952) Dam Break Benchmark:** Dimensionless surge-front position $Z(T)$ and column height $H(T)$ must track experimental data points within $3\%$ RMSE.
5. **Rayleigh-Taylor & Rising Thermal Plume Symmetry:** Symmetric initial perturbation must preserve bilateral symmetry to $<10^{-6}$ on CPU across 1, 4, and 16 threads.

---

## 6. Recommendations, Open Risks, and Milestones

### Recommendations
1. **Unify FLIP and Pyro on a single `ofx-grid-solver` foundation:** Both systems share the same $8^3$ sparse brick allocator, MAC staggered velocity layout, MacCormack/BFECC advector, cut-cell variational collider weights, and matrix-free MGPCG Poisson solver.
2. **Execute MGPCG and P2G/G2P on both CPU (`rayon` + `pulp` SIMD) and GPU (`CubeCL`)** from the same algorithmic specification, using 32-bit floats (`f32`) with mixed-precision `f64` dot-product accumulators inside Conjugate Gradient to prevent stagnation on $10^8$-voxel grids.

### Open Risks
- **GPU P2G Atomic Contention:** Scattering APIC particle momentum to grid nodes on GPU causes atomic float-add contention in dense regions. *Mitigation:* Sort particles into $8^3$ bricks and use shared-memory (workgroup tile) reduction + register-level warp reductions before writing once to global memory.

### Suggested Milestones

---

## Audit addendum — coupling, numerics, checkpoints (normative)

### Deterministic staggered-grid contract

Cell centres hold scalars; MAC face velocities lie at their geometric face centres. With `h=(dx,dy,dz)`, trilinear particle-to-grid weights are `w_i(x)=prod_d max(0,1-|x_d-x_i,d|/h_d)`. APIC transfers use `v_i += w_i (v_p + C_p (x_i-x_p))`; grid-to-particle uses the same weights and `C_p = sum_i w_i v_i (x_i-x_p)^T D_p^{-1}`, with a condition-number guard: if `D_p` is singular or has `cond>1e8`, set `C_p=0` and emit a counted warning. FLIP is `v_p=(1-a)v_PIC+a(v_p+sum_i w_i(v_i^{new}-v_i^{old}))`, `a` clamped to `[0,1]`.

Projection solves `A p=b`, `b=(rho/dt) div(u*)`; the matrix-free seven-point operator uses face area fractions and `A_ii=sum_f a_f/(rho_f d_f)`, `A_ij=-a_f/(rho_f d_f)`. Closed components pin the lowest Morton-indexed fluid cell to zero pressure. MGPCG uses pre-smoothing weighted Jacobi (`omega=2/3`, 2 pre/2 post steps), full-weighting restriction, trilinear prolongation, a fixed V-cycle count, and a deterministic dot product (fixed page/Morton order, compensated `f64` accumulation). Stop only when relative residual is below the declared tolerance *and* max iterations is reached/recorded; never use an unordered GPU reduction as the authoritative CI path.

### Cross-solver coupling protocol

`CouplingWorld` owns an immutable collision snapshot at tick start, a shared SI unit system, and a contact/exchange ledger. Each global tick uses: (1) sample kinematic rigs at `t`; (2) broad phase; (3) solver predicted states; (4) fixed-order coupling iterations sorted by `(pair_kind, stable_object_id)`; (5) solve contacts/exchanges; (6) commit all states simultaneously. Fluid↔solid uses cut-cell boundary velocity and equal/opposite integrated pressure/viscous impulse; POP↔pyro samples gas velocity/scalars only at named ports; VBD→XPBD publishes a read-only deformed collider/attachment target; MPM↔animated character uses CCD against the tick-start/end transform sweep. Interfaces specify position, velocity, normal, signed distance, mass/temperature/material fields and impulse, with units. A pair may not directly mutate another solver's pages. Strong coupling repeats the declared fixed number of iterations; adaptive iteration counts are disallowed in deterministic mode.

### Recovery and distribution

`sim_checkpoint` is a content-addressed manifest: solver/version/schema, frame/tick/rational dt, world convention, parameter hash, RNG counter, input asset hashes, active-brick/page hashes, compressed blobs, and completed output hashes. Write blobs first, fsync, then atomically publish a manifest; resume validates all hashes and replays only after the latest complete manifest. Domain slices include `ghost_width >= stencil_radius + max_particle_travel_cells`; ownership is Morton ranges, particles migrate by stable id, and halo exchange has a fixed send/receive ordering. Distributed multi-GPU/machine mode is experimental until a one-vs-N-slice deterministic regression suite passes; it cannot be advertised as bit-identical merely because individual kernels are deterministic.
- **M2.1 — Sparse MAC Grid & Matrix-Free MGPCG:** Implement $8^3$ sparse brick grid, cut-cell boundary weights, and MGPCG solver; pass Taylor-Green vortex and hydrostatic equilibrium tests.
- **M2.2 — Sparse Pyro & Combustion Solver:** Add BFECC/MacCormack advection, Advection-Reflection, buoyancy, vorticity confinement, and temperature/fuel/flame thermodynamics.
- **M2.3 — APIC / Narrow-Band FLIP Liquid Solver:** Implement APIC P2G/G2P, narrow-band reseeding, ghost-fluid free surface, variational viscosity, whitewater (spray/foam/bubbles), and anisotropic VDB surfacing; pass Martin & Moyce dam break benchmark.
- **M2.4 — Spectral Ocean & Hybrid FLIP Boundary Layer:** Implement JONSWAP/TMA FFT ocean, foam Jacobian generator, and guided ocean-layer FLIP tank.
