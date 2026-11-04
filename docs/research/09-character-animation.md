# 09 — Rigging, Character Animation, Channels, Crowds, and Grooming

> **Scope:** KineFX/APEX-equivalent geometry-first skeletons and compiled rig graphs (`ofx-rig`), skinning & deformers, IK solvers, CHOP channel processing, crowd simulation (`ofx-crowd`), and guide-based hair/fur/feathers (`ofx-groom`).

---

## 1. Skeletons as Geometry & The Compiled Rig Graph (`ofx-rig` — KineFX / APEX Equivalent)

Houdini's KineFX and APEX revolutionized rigging by replacing opaque object-level bone hierarchies with **first-class `GeoDetail` point-and-line skeletons** and **data-driven rig evaluation graphs** stored inside geometry containers. We adopt and streamline this exact paradigm:

### 1.1 Skeleton Representation in `GeoDetail`
A skeleton is simply a `GeoDetail` containing:
- **Joints = Points (`Pt`):** With standard attributes:
  - `P: Vec3f` (world-space joint translation),
  - `transform: Mat3f` (world-space $3\times 3$ rotation/scale orientation matrix),
  - `localtransform: Mat4f` (parent-relative $4\times 4$ transform),
  - `name: Symbol` (unique joint identifier, e.g., `"pelvis"`, `"hand_l"`).
- **Hierarchy = 2-Point Polyline Primitives (`CurveRun`):** A directed segment from `pt_parent -> pt_child` defines parent-child bone hierarchy.
- **Why this is so powerful for Procedural & AI Workflows:** Any SOP node (`Transform`, `Mirror`, `Delete`, `AttribWrangle`, `Subdivide`/`Resample` to add twist joints) can procedurally generate, inspect, or modify a skeleton before a single rig constraint is attached!

### 1.2 Compiled APEX-Equivalent Rig Evaluation VM (`RigGraph`)

Evaluating a general SOP node network at 60 FPS for 10 rigged characters is too heavy if every joint transform goes through full geometry cook caching. Instead, `ofx-rig` compiles modular rig components (`fk_chain`, `two_bone_ik`, `spline_ik`, `aim_constraint`, `twist_driver`, `blendshape_eval`, `lbs_deform`) into a flat, zero-allocation **Static Execution Schedule (`CompiledRig`)**:
1. **Rig Graph as Serializable Data:** A `RigGraph` is stored as a lightweight DAG inside a packed primitive (`rig.graph`), making rigs procedural assets that can be authored, combined, and inspected via MCP (`ofx://scene/nodes/{path}/rig_graph`).
2. **Partial Dirty Evaluation:** When an animator or agent drags a hand IK control, only the downstream subgraph of affected `Mat4f` registers and skinning deformers recompute (typically $<0.15\text{ ms}$ per character on CPU SIMD or GPU).
3. **Parallel Multi-Character Evaluation:** All independent characters and crowd agents evaluate concurrently across `rayon` threads or a single batched `CubeCL` GPU skinning dispatch.

---

## 2. Skinning, Deformation, and Inverse Kinematics

### 2.1 Deformer Stack (`ofx-deform`)

| Deformer | Algorithm & Reference | Key Technical Details |
|---|---|---|
| **Capture Weights (`boneCapture`)** | Stored as a variable-length `ArrayAttr<(u32, f32)>` (or fixed4/8 top weights) per point | Automatic weight generation via **Geodesic Voxel Binding** (volumetric heat diffusion on `ofx-vdb`) or **Bounded Biharmonic Weights (BBW)** (Jacobson et al., SIGGRAPH 2011) solved via `faer` sparse Cholesky. |
| **Linear Blend Skinning (LBS) & Dual Quaternion Skinning (DQS)** | Kavan et al. (2008) Dual Quaternions with antipodal sign alignment | Eliminates "candy-wrapper" volume collapse on twisting wrists/shoulders; supports per-vertex `@dqs_blend` attribute mixing LBS and DQS. |
| **Delta Mush (Direct / Precomputed)** | Mancewicz et al. (SIGGRAPH 2014), *Delta Mush: Smoothing Deformations While Preserving Detail* | Stores tangent-space detail displacement vectors relative to a Laplacian-smoothed rest mesh; recovers crisp wrinkles after coarse skinning without tedious weight painting. |
| **Pose-Space Deformation (PSD) & BlendShapes** | Radial Basis Function (RBF) scattered data interpolation over joint quaternion deltas + sparse morph targets | Morph targets store only non-zero point deltas (`(PointIdx, Vec3f)` sparse pages) for $O(\text{active deltas})$ evaluation. |
| **Lattice / Cage / Wire Deformers** | Generalized Barycentric / Mean Value Coordinates (Ju et al. 2005) & Green Coordinates | Deforms high-res geometry via low-res cages or guide curves. |

### 2.2 Inverse Kinematics & Retargeting (`ofx-ik`)

1. **Analytical Two-Bone IK:** Exact law-of-cosines solver with pole-vector plane projection, soft-ik exponential distance clamping (prevents knee/elbow popping at full extension), and stretch scaling.
2. **FABRIK (Aristidou & Lasenby 2011) & Damped Least Squares (DLS) Jacobian IK:** For $N$-joint tentacles, spines, and multi-effector full-body IK (`FBIK`) with joint angular limits (swing-twist quaternion cones) and center-of-mass balance targets.
3. **Full-Body Retargeting (`RigMatchPose` & `FBIKTransfer`):** Maps arbitrary source skeletons (Mixamo, UE5 Mannequin, BVH mocap) to target character proportions using normalized hip-height scaling + spine/limb arc-length parameterization + IK foot-plant/hand-contact spatial locking.

---

## 3. Channel Operators (`ofx-chop` — Motion, Signals & Audio)

CHOPs process 1D time-series signals (`ChannelClip`: sample rate, start/end range, named `f32` tracks):
- **Procedural Motion Modifiers:** `Spring / Jiggle` (second-order harmonic oscillator $\ddot{x} + 2\zeta\omega_0 \dot{x} + \omega_0^2(x - x_{\text{target}}) = 0$ for secondary motion on ears, antennas, belly, props), `Noise`, `Lag`, `Filter` (Butterworth / Gaussian / Savitzky-Golay low-pass mocap smoothing), `Limit`, `Math`, `Warp`.
- **Audio-Driven Effects:** FFT spectrum analyzer (`rustfft`), envelope follower, and beat/onset detection to drive geometry, Pyro emission, or shader parameters from `.wav`/`.flac` audio tracks.
- **Motion Clip Blending & Foot-Skate Cleanup:** Quaternion spherical linear averaging, additive clip layering, and automatic contact-phase detection that locks ankle IK targets when foot velocity drops below threshold near terrain.

---

## 4. Crowd Simulation (`ofx-crowd`)

1. **Agent Primitive (`AgentPrim`):** A specialized `PackedPrim` carrying:
   - Reference to a shared `AgentDefinition` (skin mesh LODs, rig hierarchy, library of baked `ChannelClip` animations, collision capsule shapes),
   - Per-agent state: current `clip_names: SmallVec<[Symbol; 4]>`, `clip_times: SmallVec<[f32; 4]>`, `clip_weights: SmallVec<[f32; 4]>`, `state: Symbol`, `velocity: Vec3f`, and current evaluated joint palette `Arc<[Mat4f]>`.
2. **Behavior, Navigation & Collision Avoidance:**
   - **NavMesh Generation & Pathfinding:** Pure-Rust port/binding of **Recast/Detour** (Zlib license) voxel-based walkable navmesh builder + funnel A* path corridor.
   - **Reciprocal Velocity Obstacles (ORCA / RVO2):** **van den Berg et al. (2011)** Optimal Reciprocal Collision Avoidance via 2D/3D linear programming—scales to **100,000+ agents in real time** with zero agent-agent overlapping.
   - **Finite State Machine & Trigger Graph:** Declarative triggers (`dist_to_target < 2.0`, `impact_impulse > 50.0`, `timer_elapsed`) transition agents seamlessly from locomotion clips to **RBD Active Ragdoll** (`ofx-sim-rbd`) or **XPBD Vellum Cloth** (`ofx-sim-xpbd`).
   - **Terrain Adaptation & Render-Time Instancing:** Projects pelvis height and ankle IK targets onto heightfields/meshes; renders via `ofx-render` hardware instancing without unpacking geometry until ray intersection.

---

## 5. Hair, Fur, and Feathers (`ofx-groom`)

1. **Guide-Curve Grooming Operators:**
   - `GuideGroom` / `GuideProcess`: Procedural operators for **Length**, **Lift**, **Clump** (hierarchical Voronoi/KD-tree guide clustering with fractal sub-clumping), **Frizz**, **Bend**, **Wave/Curl**, **Part**, and **White-Hair Masking** driven by painted scalp skin attributes (`@density`, `@clumpid`, `@length`) or COP textures.
2. **Guide Interpolation & Render-Time Procedural (`GuideDeform`):**
   - Simulates only $2,000\text{--}10,000$ guide curves in `ofx-sim-xpbd` (Cosserat elastic rods with scalp pin & body SDF collision), then interpolates **1,000,000+ render hairs** using barycentric 3-guide triangle weights on the scalp UV/rest mesh at render time (with subframe velocity interpolation for curved motion blur).
3. **Procedural Feather System:**
   - Generates parametric rachis (central shaft) + left/right barbule vane curves (`FeatherTemplate`), converts to a lightweight 2D ribbon proxy for `ofx-sim-xpbd` aerodynamic/collision simulation, and deforms the high-detail barb geometry on the GPU.

---

## 6. Ecosystem & Reference Library Assessment (2026)

| Library / Crate | URL | License | Status (2026) | Recommendation |
|---|---|---|---|---|
| **`ozz-animation`** | [github.com/guillaumeblanc/ozz-animation](https://github.com/guillaumeblanc/ozz-animation) | MIT | Production reference for SIMD SoA skeletal evaluation & animation compression | **Reference** for SoA quaternion track compression & blending; implement natively in `ofx-rig` over `glam`/`wide`. |
| **RecastNavigation** (`rerecast` / bindings) | [github.com/recastnavigation/recastnavigation](https://github.com/recastnavigation/recastnavigation) | Zlib | Industry standard NavMesh generator | **Adopt** via pure-Rust port (`rerecast`) or thin C wrapper for Crowd navmeshes. |
| **RVO2** | [gamma.cs.unc.edu/RVO2](https://gamma.cs.unc.edu/RVO2/) | Apache-2.0 | Canonical ORCA implementation | **Reimplement cleanly in Rust (~600 LoC)** over `kiddo` spatial KD-tree for `ofx-crowd`. |

---

## 7. Mandatory Validation Suite

1. **Skinning Volume & Energy Invariants:** 180-degree forearm twist on cylinder mesh must maintain $>97\%$ enclosed volume under DQS vs LBS collapse; Delta Mush on zero-rotation rest pose must reproduce rest vertices to $<10^{-6}$.
2. **IK Analytical & Numerical Convergence:** Two-bone IK must reach exact target within $10^{-6}$ when inside reach radius $[|L_1 - L_2|, L_1 + L_2]$ and clamp smoothly on the boundary without NaN/flip when target passes through the shoulder origin.
3. **ORCA Crowd Non-Penetration:** 1,000 agents arranged on a circle walking to antipodal targets through the origin must complete transit with zero pair distances $< 0.98 \times (r_i + r_j)$ and zero deadlocks.

---

## 8. Recommendations, Open Risks, and Milestones

### Recommendations
1. **Treat Skeletons and Guides as ordinary `GeoDetail` point/curve primitives** so the entire 60-node SOP catalog and VEX Wrangles work on skeletons, mocap, and hair guides out of the box.
2. **Keep `CompiledRig` separate from general SOP cooking** for 60+ FPS interactive character posing.

### Open Risks
- **Rig Evaluation Cycles:** User-authored rig graphs can accidentally wire cycle dependencies. *Mitigation:* `RigGraph` performs Tarjan strongly-connected-component validation at wire-connection time and rejects cycle edges with a structured diagnostic.

### Suggested Milestones

---
## Audit addendum — coordinate and simulation interface

Skeleton transforms, bind matrices, weights and velocity use the SI/right-handed/Y-up convention in `01`. Validate finite matrices, parent acyclicity, normalized non-negative weights and non-zero totals. Animated collision exports shutter-sampled transforms through `CouplingWorld`; rigs never mutate simulation pages in place. Crowd/navmesh and grooming randomness uses stable IDs so a reorder cannot change a shot.
- **M9.1 — Skeletons, Capture Weights (Geodesic Voxel), LBS/DQS & Delta Mush:** Full skeletal skinning and `ufbx`/`UsdSkel` round-trip.
- **M9.2 — `ofx-rig` Compiled Rig VM, IK Suite & Retargeting:** Two-bone/FABRIK/FBIK solvers, modular autorig components (biped/quadruped), and mocap retargeter.
- **M9.3 — `ofx-chop` Channels & Procedural Secondary Motion:** Spring/Jiggle, filtering, clip blending, and foot-locking.
- **M9.4 — `ofx-groom` & `ofx-crowd`:** Guide grooming/clumping/deform + ORCA crowd steering and ragdoll transition.
