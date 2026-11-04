# Houdini Capability Reference (preliminary research, provided by project owner)

> Source: preliminary AI-generated research supplied at project start. Citation markers like 'sidefx+2' refer to multiple SideFX documentation pages. Treat claims as leads to verify, not ground truth.

Houdini: Capabilities and Technical Guide
SideFX Houdini is a procedural content-creation platform covering modeling, environments, animation, character effects, simulation, rendering, image processing, and pipeline automation. Its central idea is that operations become reusable node networks rather than a sequence of destructive edits.sidefx+2
This report maps those capabilities, explains their documented mechanisms, and links directly to technical documentation. It is a broad functional reference—not a claim that every parameter, historical node, third-party extension, or undocumented internal algorithm has been enumerated.
Scope and version
Product: SideFX Houdini, including Houdini Engine and the surrounding development interfaces.sidefx+1
Version basis: The inspected online help is labeled Houdini 22.0; H22-specific capabilities are identified where relevant.sidefx+1
Technical depth: Public architecture, data representations, solver approaches, node families, APIs, and production workflows—not a reconstruction of proprietary source code.
Availability caveat: A feature appearing in the documentation does not establish that it is available in every product edition; for example, the Otis solver is explicitly unavailable in Houdini Core.[sidefx](https://www.sidefx.com/docs/houdini/muscles/muscle_otismuscletissuesim.html)
How Houdini works
Procedural node networks
Houdini represents operations through nodes with parameters and connections; different network types operate on geometry, dynamics, images, animation channels, USD scenes, or executable tasks. These contexts are related, but they do not all execute the same kind of computation.[sidefx](https://www.sidefx.com/docs/houdini/basics/index.html)
Context
What it handles
How to understand it
OBJ — Objects
Scene objects, cameras, lights, geometry containers, and object-level relationships. [sidefx](https://www.sidefx.com/docs/houdini/basics/index.html)
Organizes objects in a scene
SOP — Surface operators
Geometry generation and modification. [sidefx](https://www.sidefx.com/docs/houdini/basics/index.html)
A geometry-processing pipeline
DOP — Dynamics operators
Simulation conditions, objects, relationships, and solver networks. [sidefx](https://www.sidefx.com/docs/houdini/basics/index.html)
A time-evolving simulation system
VOP — VEX operators
Graphical construction of programs, including shaders and geometry operations. [sidefx](https://www.sidefx.com/docs/houdini/basics/index.html)
Visual programming rather than ordinary scene assembly
LOP — Lighting operators
USD scene generation and modification for Solaris. [sidefx](https://www.sidefx.com/docs/houdini/solaris/usd.html)
A procedural USD-authoring pipeline
ROP — Render operators
Rendering, exports, and dependencies between output jobs. [sidefx](https://www.sidefx.com/docs/houdini/nodes/out/index.html)
An output and job-dependency system
CHOP — Channel operators
Creation, filtering, and manipulation of channel data. [sidefx](https://www.sidefx.com/docs/houdini/basics/index.html)
Signal and animation-channel processing
COP — Copernicus
GPU-based image processing in a framework supporting 2D and 3D workflows. [sidefx](https://www.sidefx.com/docs/houdini/copernicus/index.html)
A procedural image-processing environment
TOP — Task operators
Work items, dependencies, external processes, and local or farm execution. [sidefx](https://www.sidefx.com/docs/houdini/basics/index.html)
A data-processing and job-orchestration graph
APEX
Graph evaluation used for character rigs and other geometry manipulation. [sidefx](https://www.sidefx.com/docs/houdini/character/kinefx/apexgraphbasics.html)
A lower-level execution framework
Documentation: [Houdini basics](https://www.sidefx.com/docs/houdini/basics/index.html) · [Geometry nodes](https://www.sidefx.com/docs/houdini/nodes/sop/index.html) · [Dynamics nodes](https://www.sidefx.com/docs/houdini/nodes/dop/index.html) · [Task nodes](https://www.sidefx.com/docs/houdini/nodes/top/index.html)
Geometry is structured data
A Houdini geometry object contains points and primitives; primitives can include polygons, NURBS curves, and volumes, while vertices connect primitive corners to points. This distinction matters because multiple primitive vertices can reference the same point.sidefx+1
Attributes attach data to that geometry, allowing operations to work with information such as positions, colors, normals, UV coordinates, and simulation properties. Groups provide named selections that downstream operations can reuse.sidefx+2
Technical documentation: [Geometry API](https://www.sidefx.com/docs/houdini/hom/hou/Geometry.html) · [Point API](https://www.sidefx.com/docs/houdini/hom/hou/Point.html) · [Geometry functions in VEX](https://www.sidefx.com/docs/houdini/vex/geometry.html) · [Groups](https://www.sidefx.com/docs/houdini/model/groups.html)
Several execution layers coexist
Layer
Function
Important distinction
Built-in nodes
Expose predefined geometry, simulation, rendering, and pipeline operations. [sidefx](https://www.sidefx.com/docs/houdini/basics/index.html)
Using nodes does not require writing code
VEX snippets
Run operations over elements such as points, primitives, particles, or voxels. [sidefx](https://www.sidefx.com/docs/houdini//vex/snippets.html)
Designed for threaded data processing
VOP graphs
Build programs graphically. [sidefx](https://www.sidefx.com/docs/houdini/basics/index.html)
A program graph is not the same as a task graph
Python / HOM
Access Houdini objects, geometry, parameters, editors, and application behavior. sidefx+1
Useful for tooling and orchestration
OpenCL
Provides a separate programmable compute route with Houdini-specific data bindings. sidefx+1
VEX should not automatically be equated with GPU execution
APEX graphs
Evaluate rig logic, transforms, and geometry operations within a graph framework. [sidefx](https://www.sidefx.com/docs/houdini/character/kinefx/apexgraphbasics.html)
APEX sits below SOPs and is not tied to one network context
HDK
Provides the native development interface for compiled extensions. sidefx+1
Requires compiled-plugin development
Houdini Engine
Exposes Houdini assets through an integration API. [sidefx](https://www.sidefx.com/docs/houdini/ref/hengine.html)
Intended for embedding Houdini workflows into other applications
The practical consequence is that “Houdini runs on the GPU” is too broad a statement: different systems use different execution paths, and support must be checked per solver, renderer, or operator.sidefx+2
Geometry and modeling
Houdini can create and edit geometry interactively while retaining procedural operations that can be adjusted and reused. Its documented representations include polygonal geometry, curves, NURBS primitives, and volumetric data.sidefx+2
Capability
What it does
Documented mechanism
Component editing
Moves, rotates, and scales points, faces, and edges. [sidefx](https://www.sidefx.com/docs/houdini/model/edit_points.html)
Interactive component selections and transforms
Sculpt-style adjustments
Pushes, pulls, and smooths points using brushes. [sidefx](https://www.sidefx.com/docs/houdini/model/edit_points.html)
Brush operations on geometry
Attribute painting
Paints data onto a surface. [sidefx](https://www.sidefx.com/docs/houdini/model/edit_points.html)
Surface attributes become inputs to later operations
Selection management
Builds reusable collections of geometry elements. [sidefx](https://www.sidefx.com/docs/houdini/model/groups.html)
Named groups and selection criteria
Programmatic geometry
Reads, creates, and modifies geometry through code. sidefx+1
HOM geometry objects or element-wise VEX
Surface-to-volume conversion
Converts polygon surfaces into distance or density fields. [sidefx](https://www.sidefx.com/docs/houdini/nodes/sop/vdbfrompolygons.html)
VDB voxelization
Procedural deformation
Changes geometry by evaluating operations over attributes and elements. [sidefx](https://www.sidefx.com/docs/houdini//vex/snippets.html)
VEX/VOP processing and geometry nodes
Character deformation
Applies skeletal and blend-shape workflows. sidefx+1
KineFX/APEX character data and rig evaluation
A useful way to think about procedural modeling is “build a recipe that accepts geometry and parameters”, rather than “finish one fixed mesh.” Houdini Engine extends this idea by letting the same asset recipe be evaluated in another host application.[sidefx](https://www.sidefx.com/products/houdini-engine/)
Documentation: [Component editing](https://www.sidefx.com/docs/houdini/model/edit_points.html) · [Geometry node catalog](https://www.sidefx.com/docs/houdini/nodes/sop/index.html) · [VEX snippets](https://www.sidefx.com/docs/houdini//vex/snippets.html)
UV mapping
Houdini includes tools for creating UV coordinates, flattening surfaces, adjusting distortion, joining or separating seams, and packing islands into texture space. UV coordinates are stored as geometry attributes rather than existing only as an editor-specific layout.sidefx+2
Tool
Function
Technical detail
UV Texture
Assigns UV coordinates using projection methods. [sidefx](https://www.sidefx.com/docs/houdini/nodes/sop/texture.html)
Includes options such as camera-based projection
UV Flatten 3.0
Unwraps geometry into texture-space pieces. [sidefx](https://www.sidefx.com/docs/houdini/nodes/sop/uvflatten.html)
Supports seams, pins, and procedural or manual layout controls
UV Layout 3.0
Packs UV islands into target regions. [sidefx](https://www.sidefx.com/docs/houdini/nodes/sop/uvlayout.html)
Can pack 2D profiles into other 2D profiles
UV Relax
Reduces distortion in existing UV parameterizations. [sidefx](https://www.sidefx.com/docs/houdini/nodes/sop/uvrelax.html)
Can also resolve reversed UV orientation
UV Flatten from Points
Produces UV coordinates based on surface distance and direction from source points. [sidefx](http://www.sidefx.com/docs/houdini/nodes/sop/uvflattenfrompoints.html)
Useful for procedural masking and surface traversal
Labs Auto UV
Automates UV generation through multiple methods. [sidefx](https://www.sidefx.com/docs/houdini/nodes/sop/labs--autouv-1.0.html)
Uses surface characteristics to create seams and islands
Version caution: An older UV Flatten node explicitly documents Least Squares Conformal Mapping; that does not establish that every newer UV Flatten mode uses the same algorithm.sidefx+1
Technical documentation: [UV Flatten](https://www.sidefx.com/docs/houdini/nodes/sop/uvflatten.html) · [UV Layout](https://www.sidefx.com/docs/houdini/nodes/sop/uvlayout.html) · [UV Relax](https://www.sidefx.com/docs/houdini/nodes/sop/uvrelax.html) · [UV Flatten from Points](https://www.sidefx.com/docs/houdini/nodes/sop/uvflattenfrompoints.html) · [Labs Auto UV](https://www.sidefx.com/docs/houdini/nodes/sop/labs--autouv-1.0.html)
Volumes and clouds
Volumetric representations
Volumes store values over three-dimensional samples called voxels; those values can represent density, velocity, collision information, or distance from a surface. Houdini supports both conventional volume grids and sparse OpenVDB primitives.[sidefx](https://www.sidefx.com/docs/houdini/model/volumes.html)
Representation
What it stores
Typical purpose
Scalar field
One number per sample. [sidefx](https://www.sidefx.com/docs/houdini/model/volumes.html)
Smoke density or another scalar quantity
Vector field
A vector-valued quantity. [sidefx](https://www.sidefx.com/docs/houdini/model/volumes.html)
Velocity and directional information
Signed distance field
Distance from a surface, with sign indicating inside or outside. sidefx+1
Surface representation and collision processing
Fog volume
Density values across a region. [sidefx](https://www.sidefx.com/docs/houdini/nodes/sop/vdbfrompolygons.html)
Clouds, smoke, and other participating media
Sparse OpenVDB
Sparse voxel data that avoids allocating ordinary storage for background regions. [sidefx](https://www.sidefx.com/docs/houdini/model/volumes.html)
Efficient storage of spatially sparse phenomena
The VDB From Polygons node can generate signed or unsigned distance fields and fog fields; unsigned distance fields do not require the same inside/outside interpretation as signed fields. This makes representation choice important when working with open surfaces.[sidefx](https://www.sidefx.com/docs/houdini/nodes/sop/vdbfrompolygons.html)
Technical documentation: [Volumes](https://www.sidefx.com/docs/houdini/model/volumes.html) · [VDB From Polygons](https://www.sidefx.com/docs/houdini/nodes/sop/vdbfrompolygons.html) · [VDB Python API](https://www.sidefx.com/docs/houdini/hom/hou/VDB)
Cloud construction
Houdini’s cloud tools can generate individual clouds or larger sky arrangements, starting from shapes such as spheres, lines, and polygons. Additional operators control noise, replication, wispy detail, and density profiles before rendering.[sidefx](https://www.sidefx.com/products/houdini/world-building/clouds/)
The documented workflow connects geometric art direction to volumetric output: define the large shape, introduce structure and variation, adjust density, and shade the resulting volume.sidefx+1
Documentation: [Cloud toolset](https://www.sidefx.com/products/houdini/world-building/clouds/)
Terrains and environments
Houdini’s heightfield tools support procedural terrain generation, masking, erosion, terracing-related workflows, and material movement. Heightfields are not interchangeable with arbitrary polygonal surfaces; some vertical-detail requirements call for geometry conversion or a different representation.[sidefx](https://www.sidefx.com/docs/houdini/heightfields/)
Terrain processes
Capability
What it enables
How it works
Layered terrain shaping
Procedural landforms and selective effects. [sidefx](https://www.sidefx.com/docs/houdini/heightfields/)
Height and mask layers
Noise-driven detail
Multi-scale terrain variation. sidefx+1
Combined and distorted noise patterns
Hydraulic-style erosion
Channels, weathering, and sediment movement. [sidefx](https://www.sidefx.com/docs/houdini/nodes/sop/heightfield_erode_hydro.html)
Material transport over a heightfield
Slumping
Collapse and redistribution of terrain material. [sidefx](https://www.sidefx.com/docs/houdini/heightfields/slump.html)
Smooth or granular modes
Debris accumulation
Filled valleys and loose-material deposits. [sidefx](https://www.sidefx.com/docs/houdini/heightfields/slump.html)
Granular erosion and transported material
Terrain collision for MPM
Snow, soil, or other MPM materials interacting with terrain. [sidefx](http://www.sidefx.com/docs/houdini/mpm/heightfields.html)
Heightfields converted internally into collision distance fields
Copernicus terrains
Image-based terrain workflows. [sidefx](https://www.sidefx.com/docs/houdini/heightfields_cop/index.html)
2D maps rather than traditional SOP heightfield voxel grids
Neural terrain generation
Terrain generation using trained models. [sidefx](https://www.sidefx.com/docs/houdini/fluid/surfacing.html)
ONNX inference and a training recipe
For MPM collisions, the documentation distinguishes static, rigidly animated, and deforming terrain; selecting the wrong collider mode can produce penetration errors. Terrain resolution and collision resolution therefore remain technical choices, not just artistic settings.[sidefx](http://www.sidefx.com/docs/houdini/mpm/heightfields.html)
Technical documentation: [Heightfields](https://www.sidefx.com/docs/houdini/heightfields/) · [Hydraulic erosion node](https://www.sidefx.com/docs/houdini/nodes/sop/heightfield_erode_hydro.html) · [Slump](https://www.sidefx.com/docs/houdini/heightfields/slump.html) · [Terrain patterns](https://www.sidefx.com/docs/houdini/heightfields/patterns.html) · [Copernicus terrains](https://www.sidefx.com/docs/houdini/heightfields_cop/index.html) · [MPM heightfield collisions](https://www.sidefx.com/docs/houdini/mpm/heightfields.html)
Simulation architecture
Houdini has several simulation families because different representations and numerical approaches suit different effects. A particle liquid, a rigid-body destruction setup, a muscle simulation, and a smoke field should not be treated as variants of one universal solver.sidefx+3
System
Primary application
Documented approach
POP particles
Particle effects and attribute-driven motion. sidefx+1
Particle attributes and particle operators
Pyro
Smoke, flames, and explosions. [sidefx](https://www.sidefx.com/docs/houdini/pyro/pyro)
Grid-based gas simulation, including sparse modes
FLIP
Liquid simulation. sidefx+1
Particles and background grids
Vellum
Cloth, hair, grains, fluids, and soft bodies. [sidefx](http://www.sidefx.com/docs/houdini/unreal/)
Position-based dynamics; originally described as XPBD-inspired. [sidefx](https://www.sidefx.com/products/whats-new-17/)
MPM
Interacting deformable and elastoplastic materials. [sidefx](https://www.sidefx.com/products/houdini/vfx/mpm/)
Material Point Method using particles and grids
Bullet RBD
Rigid bodies and destruction. sidefx+1
Rigid-body simulation with geometry and constraints
Finite elements
Deformable solids and solid fracturing. sidefx+1
Tetrahedral simulation geometry
Otis
Muscles and soft tissue. [sidefx](https://www.sidefx.com/docs/houdini/muscles/overview)
Vertex Block Descent architecture
Crowds
Large groups of animated agents. [sidefx](https://www.sidefx.com/docs/houdini/crowds/index.html)
Agents, animation states, triggers, behaviors, and dynamics
A high-level SOP solver can also be a convenience interface over a lower-level network; the Pyro Solver SOP, for example, explicitly wraps a DOP network. Node placement alone therefore does not tell you the entire implementation or licensing story.sidefx+1
Particles and motion effects
Houdini particles support effects such as dust and flocking-style motion, with extensive behavior driven through attributes. POP VOP and POP Wrangle are documented routes for creating or modifying those attributes.sidefx+1
What the system provides
Particle-based effects: Build and evolve particle populations for visual-effects workflows.[sidefx](https://www.sidefx.com/docs/houdini/dopparticles/index.html)
Attribute-driven control: Read and write solver-recognized particle properties.[sidefx](https://www.sidefx.com/docs/houdini/dopparticles/attributes.html)
Visual programming: Define operations through VOP-based tools.[sidefx](https://www.sidefx.com/docs/houdini/dopparticles/index.html)
Code-based control: Apply custom element-wise logic through VEX snippets.[sidefx](https://www.sidefx.com/docs/houdini//vex/snippets.html)
Parallel execution: The documented particle environment is multithreaded.[sidefx](https://www.sidefx.com/docs/houdini/dopparticles/index.html)
The technical advantage is that particle behavior can be expressed as data transformations rather than only as fixed emitter presets.sidefx+1
Technical documentation: [Particles](https://www.sidefx.com/docs/houdini/dopparticles/index.html) · [Particle attributes](https://www.sidefx.com/docs/houdini/dopparticles/attributes.html) · [VEX snippets](https://www.sidefx.com/docs/houdini//vex/snippets.html)
Smoke, fire, and explosions
Pyro handles expanding-gas effects, including smoke, fire, and explosions. Its smoke foundation is grid-based, with additional fire-related behavior and art-direction controls in the Pyro system.[sidefx](https://www.sidefx.com/docs/houdini/pyro/pyro)
How Pyro operates
The Pyro Solver’s sparse mode concentrates calculations in active regions defined by reference fields; those regions are expanded with padding so the simulated phenomenon can move into surrounding space. Dense mode instead solves throughout the domain.[sidefx](https://www.sidefx.com/docs/houdini/nodes/sop/pyrosolver.html)
Capability
Practical function
Technical consideration
Sparse simulation
Concentrates work around relevant regions. [sidefx](https://www.sidefx.com/docs/houdini/nodes/sop/pyrosolver.html)
Activity thresholds and padding affect results
Dense simulation
Evaluates the full simulation domain. [sidefx](https://www.sidefx.com/docs/houdini/nodes/sop/pyrosolver.html)
Different memory and computation trade-offs
Minimal OpenCL mode
Provides a GPU-oriented dense solve. [sidefx](https://www.sidefx.com/docs/houdini/nodes/sop/pyrosolver.html)
Not simply another name for sparse Pyro
Sourcing
Adds smoke or heat into simulation objects. [sidefx](https://www.sidefx.com/docs/houdini/nodes/dop/smokeobject_sparse.html)
Source fields must match the intended simulation
Look development
Adjusts visualization and the appearance of smoke/fire fields. [sidefx](https://www.sidefx.com/docs/houdini/pyro/lookdev.html)
Simulation values and render appearance are separate concerns
Custom field logic
Allows VEX operations in dynamics contexts. [sidefx](https://www.sidefx.com/docs/houdini//vex/snippets.html)
Operates over field or geometry elements
Sparse simulation still needs careful memory management: the documentation discusses peak-memory effects from batched advection and provides controls for limiting batch size. Insufficient padding can also produce visible artifacts.[sidefx](https://www.sidefx.com/docs/houdini/pyro/lookdev.html)
H22 Copernicus simulation
The H22 product documentation describes GPU-native Pyro workflows in Copernicus using implicit-surface collisions, alongside effect recipes combining simulation, rendering, and compositing. This should be distinguished from assuming that all historical Pyro setups automatically execute through that newer path.[sidefx](https://www.sidefx.com/products/whats-new-in-h22/vfx/)
Technical documentation: [Pyro overview](https://www.sidefx.com/docs/houdini/pyro/pyro) · [Pyro Solver SOP](https://www.sidefx.com/docs/houdini/nodes/sop/pyrosolver.html) · [Pyro workflow and troubleshooting](https://www.sidefx.com/docs/houdini/pyro/lookdev.html) · [Sparse Smoke Object](https://www.sidefx.com/docs/houdini/nodes/dop/smokeobject_sparse.html) · [H22 VFX changes](https://www.sidefx.com/products/whats-new-in-h22/vfx/)
Liquids and oceans
FLIP liquids
Houdini’s FLIP workflows provide liquid containers, sources, collisions, waterlines, viscosity, surface tension, and conversion from simulated particles to renderable surfaces. SOP-level configure tools assemble working examples for tanks and other liquid setups.sidefx+3
Capability
What it does
Mechanism
Tanks and waterlines
Maintains a liquid region inside a domain. sidefx+1
Container and solver settings
Velocity sourcing
Introduces liquid with controlled motion. [sidefx](https://www.sidefx.com/docs/houdini/fluid/sopsourcing.html)
Source velocity and normal-direction emission
Pressure sourcing
Drives liquid from pressure-based boundaries. [sidefx](https://www.sidefx.com/docs/houdini/fluid/sopsourcing.html)
Pressure boundary conditions
Boundary flow
Sources and removes particles at domain boundaries. [sidefx](https://www.sidefx.com/docs/houdini/fluid/sopsourcing.html)
Maintains an equilibrium around the simulation region
Animated collisions
Handles moving or deforming collision geometry. [sidefx](https://www.sidefx.com/docs/houdini/fluid/sopminimalsetup.html)
FLIP collision operators
Viscosity
Produces thicker, more resistant liquid motion. [sidefx](https://www.sidefx.com/docs/houdini/fluid/flipviscosity.html)
Uniform or spatially varying viscosity
Bubbles and trapped air
Supports non-collapsing air pockets under a documented setup. [sidefx](https://www.sidefx.com/docs/houdini//fluid/bubbles.html)
Particle removal and air incompressibility
Surfacing
Converts liquid simulation output into a renderable form. [sidefx](https://www.sidefx.com/docs/houdini/fluid/surfacing.html)
Particle-fluid surface processing
Important limitations are documented: the referenced sourcing workflow expects closed volumetric source objects, and the described bubble workflow does not work with adaptive pressure solves.sidefx+1
Technical documentation: [Minimal FLIP setups](https://www.sidefx.com/docs/houdini/fluid/sopminimalsetup.html) · [Fluid sourcing](https://www.sidefx.com/docs/houdini/fluid/sopsourcing.html) · [Tank configuration](https://www.sidefx.com/docs/houdini/fluid/sopconfigtank.html) · [Viscosity](https://www.sidefx.com/docs/houdini/fluid/flipviscosity.html) · [Bubbles](https://www.sidefx.com/docs/houdini//fluid/bubbles.html) · [Fluid surfacing](https://www.sidefx.com/docs/houdini/fluid/surfacing.html)
Oceans and water surfaces
Houdini has dedicated ocean-spectrum and water-surface workflows, separate from treating every visible wave as a full liquid simulation. It also provides ocean-layer FLIP configurations and a render-time ocean procedural that can generate ocean detail and foam from lower-resolution input geometry.sidefx+2
This separation supports a common production strategy: use an appropriate large-scale surface representation, then reserve simulation detail for areas requiring actual interaction. The exact combination remains a shot-specific design decision.sidefx+1
Documentation: [Oceans and water surfaces](https://www.sidefx.com/docs/houdini/ocean/index.html) · [FLIP ocean-layer configuration](https://www.sidefx.com/docs/houdini/fluid/sopconfigocean.html)
Cloth, hair, and soft bodies
Vellum is Houdini’s unified position-based system for cloth, hair, grains, fluids, and soft-body objects. SideFX’s original description identifies it as XPBD-inspired and OpenCL-accelerated.sidefx+1
What Vellum supports
Cloth: Layered, panelled, draped, stitched, and tearing cloth workflows.[sidefx](https://www.sidefx.com/products/whats-new-17/)
Hair: Constraint-driven strand dynamics.sidefx+1
Grains: Particle-based granular effects, including cases where sparse granular simulation is preferable to grid-based MPM.[sidefx](https://www.sidefx.com/products/houdini/vfx/mpm/)
Soft bodies: Deformable objects within the Vellum framework.[sidefx](http://www.sidefx.com/docs/houdini/unreal/)
Small-scale fluids: Liquid effects suited to particle constraints and interactions with other Vellum materials.sidefx+1
Animated rest behavior: Nodes can blend constraint rest values toward an external rest state.[sidefx](https://www.sidefx.com/docs/houdini/nodes/sop/pyrosolver.html)
Feather proxies: Vellum can simulate simplified surfaces used to deform detailed feathers.[sidefx](https://www.sidefx.com/docs/houdini/feathers/simulating.html)
H22 adds a quasistatic option for hair bend/twist constraints and angular damping controls. Those additions have their own tuning requirements; the documentation notes that quasistatic solving requires different stiffness settings from the default dynamic method.[sidefx](https://www.sidefx.com/docs/houdini/fluid/surfacing.html)
Vellum versus FLIP
These are different solver systems, not interchangeable fluid modes: SideFX explicitly states that Vellum fluids and FLIP fluids cannot directly interact as one simulation. A workflow involving both needs a deliberate integration strategy rather than an assumption of automatic coupling.[sidefx](https://www.sidefx.com/docs/houdini/vellum/vellumvsflip.html)
Technical documentation: [Vellum versus FLIP](https://www.sidefx.com/docs/houdini/vellum/vellumvsflip.html) · [Original Vellum architecture description](https://www.sidefx.com/products/whats-new-17/) · [H22 Vellum changes](https://www.sidefx.com/docs/houdini/news/22/vellum.html)
MPM and material interaction
The Material Point Method system is aimed at interacting materials such as snow, soil, mud, sand, liquids, and deformable solids. Its representation combines particles with background grids.[sidefx](https://www.sidefx.com/products/houdini/vfx/mpm/)
What MPM is especially useful for
Situation
Why MPM is relevant
Clumping snow or soil
Well suited to elastoplastic materials that hold together in chunks. [sidefx](https://www.sidefx.com/products/houdini/vfx/mpm/)
Water interacting with deformable sand
Supports coupled interaction between those material types. [sidefx](https://www.sidefx.com/products/houdini/vfx/mpm/)
Mixed-material effects
Allows different material properties within the MPM simulation. [sidefx](https://www.sidefx.com/products/houdini/vfx/mpm/)
Force-driven breaking
Can avoid a conventional prefracturing step for appropriate effects. [sidefx](https://www.sidefx.com/products/houdini/vfx/mpm/)
Terrain interaction
Accepts heightfields through MPM collision tools. [sidefx](http://www.sidefx.com/docs/houdini/mpm/heightfields.html)
Production surfacing
Converts simulation output into VDBs or polygon meshes. [sidefx](https://www.sidefx.com/docs/houdini/mpm/surfacing)
Detailed asset deformation
Post-fracture workflows transfer simulation behavior onto textured assets. [sidefx](https://www.sidefx.com/products/houdini/vfx/mpm/)
Basic MPM pipeline
The documented starter configuration separates the source, collider, container, and solver. The container controls resolution through particle separation, while the solver combines those inputs to evolve the simulation.[sidefx](https://www.sidefx.com/docs/houdini/mpm/mpmconfig.html)
Resolution is expensive: the documentation explains that halving particle separation increases the three-dimensional resolution by a factor of eight. That is why source scale, domain design, and intended output detail matter early in setup.[sidefx](https://www.sidefx.com/docs/houdini/mpm/mpmconfig.html)
When another solver is preferable
Detailed splashing water: FLIP is more specialized and has an established whitewater workflow.[sidefx](https://www.sidefx.com/products/houdini/vfx/mpm/)
Rigid-body destruction: Bullet is usually more efficient for genuinely rigid materials.[sidefx](https://www.sidefx.com/products/houdini/vfx/mpm/)
Very stiff MPM materials: High substep counts can make simulations slow.[sidefx](https://www.sidefx.com/products/houdini/vfx/mpm/)
Dry, non-clumping grains: Vellum can be the better choice.[sidefx](https://www.sidefx.com/products/houdini/vfx/mpm/)
Small liquid drips: Vellum may be more efficient for appropriate small-scale effects.[sidefx](https://www.sidefx.com/products/houdini/vfx/mpm/)
Technical documentation: [MPM versus other solvers](https://www.sidefx.com/docs/houdini/mpm/mpmvsothers.html) · [MPM starter configuration](https://www.sidefx.com/docs/houdini/mpm/mpmconfig.html) · [MPM surfacing](https://www.sidefx.com/docs/houdini/mpm/surfacing) · [MPM terrain collisions](https://www.sidefx.com/docs/houdini/mpm/heightfields.html) · [MPM production workflows](https://www.sidefx.com/products/houdini/vfx/mpm/)
Destruction and deformable solids
Rigid-body destruction
Houdini’s Bullet workflows simulate rigid-body geometry together with constraint geometry, including dynamically emitted bodies. RBD preparation nodes and the RBD Bullet Solver SOP provide a higher-level workflow around that system.[sidefx](https://www.sidefx.com/docs/houdini/destruction/emit)
The destruction toolset also includes material-fracture workflows, while H22’s documented additions extend Bullet-based metal bending and tearing. This means metal deformation is not restricted to choosing MPM for every case.sidefx+1
Documentation: [Emitting rigid bodies](https://www.sidefx.com/docs/houdini/destruction/emit) · [H22 destruction improvements](https://www.sidefx.com/products/whats-new-in-h22/vfx/)
Finite-element workflows
Houdini supports tetrahedral simulation geometry for deformable solids and finite-element fracturing. A detailed render mesh can be embedded in a simpler tetrahedral mesh so that the simulation does not need to operate directly on the full render geometry.sidefx+1
The Tet Embed documentation also identifies a restriction: its input should consist of a single connected component. Such preparation requirements are important when deciding whether a solver is appropriate for an asset.[sidefx](https://www.sidefx.com/docs/houdini/nodes/sop/tetembed.html)
Technical documentation: [Tet Embed](https://www.sidefx.com/docs/houdini/nodes/sop/tetembed.html) · [Solid Fracture](https://www.sidefx.com/docs/houdini/nodes/sop/tetfracture.html)
Rigging and character animation
KineFX and APEX
KineFX provides character workflows around skeletons, capture weights, rigging, animation, and retargeting; APEX supplies the graph framework used to express and evaluate rig behavior. APEX graphs can combine numerical transform operations and geometry manipulation.sidefx+1
Capability
What it provides
Technical route
Skeleton preparation
Joint orientation, hierarchy preparation, and metadata. [sidefx](https://www.sidefx.com/docs/houdini/character/kinefx/apexupdates_H22.html)
Geometry-based character workflows
Skinning
Capture weights and skeletal deformation. sidefx+1
Character geometry and deformation operations
Rig construction
FK hierarchies, controls, and deformation logic. [sidefx](https://www.sidefx.com/docs/houdini/character/kinefx/apexgraphbasics.html)
APEX graphs
Procedural rig building
Reusable components and autorig workflows. [sidefx](https://www.sidefx.com/docs/houdini/character/kinefx/apexupdates_H22.html)
APEX rig components and graph-building tools
Scripted graph construction
Programmatic creation and manipulation of rig graphs. [sidefx](https://www.sidefx.com/docs/houdini/character/kinefx/apexupdates_H22.html)
APEX Script
Graph debugging
Inspection of graph values and behavior. [sidefx](https://www.sidefx.com/docs/houdini/character/kinefx/apexupdates_H22.html)
APEX debugging tools
Animation transfer
Retargeting between character skeletons and rigs. [sidefx](https://www.sidefx.com/docs/houdini/character/kinefx/apexupdates_H22.html)
Character mapping and retargeting workflows
Interactive animation
Posing, constraints, motion paths, and animation controls. [sidefx](https://www.sidefx.com/docs/houdini/character/kinefx/apexupdates_H22.html)
The animate state and related editors
H22 animation additions
The H22 documentation adds character pickers, revised animation interfaces, secondary-motion tools, expanded ragdoll controls, motion-mixer improvements, and biped retargeting workflows. It also documents parallel rig evaluation and animation caching.[sidefx](https://www.sidefx.com/docs/houdini/character/kinefx/apexupdates_H22.html)
A separate H22 workflow brings APEX rigging and animation into Solaris/USD stages, but the documentation explicitly marks this toolset as beta, unfinished, and subject to change. It should not be presented as having the same maturity guarantees as established workflows.[sidefx](https://www.sidefx.com/docs/houdini/solaris/usd.html)
Technical documentation: [APEX graph basics](https://www.sidefx.com/docs/houdini/character/kinefx/apexgraphbasics.html) · [KineFX/APEX documentation map](https://www.sidefx.com/docs/houdini/character/kinefx/apexupdates_H22.html) · [APEX selection patterns](https://www.sidefx.com/docs/houdini/character/kinefx/apexpathpattern.html) · [H22 character changes](https://www.sidefx.com/docs/houdini/news/22/kinefx.html) · [H22 Solaris animation status](https://www.sidefx.com/docs/houdini/news/22/solaris.html)
Muscles and tissue
The current Otis muscle-and-tissue system simulates anatomical layers together rather than using the earlier multi-pass muscle, tissue, and skin workflow. Its documented architecture is based on Vertex Block Descent.sidefx+1
What the system can do
Prepare muscle geometry: Convert muscle surfaces into tetrahedral simulation meshes.[sidefx](https://www.sidefx.com/docs/houdini/muscles/muscle_otismuscletissuesim.html)
Assign physical properties: Configure stiffness, damping-related behavior, and muscle-specific properties.sidefx+1
Drive contraction: Use muscle activation and tension-line workflows.[sidefx](https://www.sidefx.com/docs/houdini/muscles/musclesetup.html)
Build attachments: Configure muscle-to-muscle, muscle-to-bone, and muscle-to-tissue relationships.[sidefx](http://www.sidefx.com/docs/houdini/nodes/sop/muscleconstraintpropertiesotis.html)
Simulate anatomical layers: Evaluate muscles, tissue core, and tissue shell in a unified pass.[sidefx](https://www.sidefx.com/docs/houdini/muscles/overview)
Deform render skin: Transfer simulation results into the final visible character surface.[sidefx](https://www.sidefx.com/docs/houdini/muscles/overview)
Start from recipes: Use generic setups or Otto-based example assets.sidefx+1
Otis is specialized for tetrahedral soft-body constraints rather than being a general replacement for every Vellum use. Migration from the older Vellum muscle workflow is not automatic or backward-compatible.sidefx+1
Technical documentation: [Muscles and tissues overview](https://www.sidefx.com/docs/houdini/muscles/overview) · [Muscle setup](https://www.sidefx.com/docs/houdini/muscles/musclesetup.html) · [Otis simulation recipe](https://www.sidefx.com/docs/houdini/muscles/muscle_otismuscletissuesim.html) · [Constraint properties](https://www.sidefx.com/docs/houdini/nodes/sop/muscleconstraintpropertiesotis.html) · [Otis versus Vellum](https://www.sidefx.com/docs/houdini/muscles/differences.html)
Hair, fur, and feathers
Hair and fur
Houdini’s guide-based workflows separate a manageable set of control or simulation curves from the larger groom. Guide Reduce and Guide Deform support selecting simulation guides and transferring their motion to more detailed output.[sidefx](https://www.sidefx.com/docs/houdini/fur/guidedeform.html)
The guide-deformation workflow also has a Solaris counterpart that performs deformation at render time, including subframe sampling for motion blur. This reduces the need to treat every final hair as an independently simulated strand.[sidefx](https://www.sidefx.com/docs/houdini/fur/guidedeform.html)
Technical documentation: [Guide deformation](https://www.sidefx.com/docs/houdini/fur/guidedeform.html)
Feathers
Houdini’s feather simulation workflow creates simplified proxies, converts them into surfaces suitable for Vellum, simulates their motion, and uses the result to deform the original feathers. The shaft is treated separately so it can anchor the feather to the character.[sidefx](https://www.sidefx.com/docs/houdini/feathers/simulating.html)
This is a clear example of Houdini’s broader simulation philosophy: simulate an appropriate representation, then transfer the result to detailed render geometry.sidefx+1
Technical documentation: [Feather simulation and animation](https://www.sidefx.com/docs/houdini/feathers/simulating.html)
Crowd simulation
Houdini’s crowd system combines animated agents with state logic, triggers, behaviors, terrain adaptation, appearance variation, and dynamics interaction. Agents are more than duplicated meshes: they carry animation and behavior-related information.[sidefx](https://www.sidefx.com/docs/houdini/crowds/index.html)
Capability
What it enables
Agent preparation
Builds reusable animated crowd actors. sidefx+1
States and transitions
Changes animation and behavior according to conditions. [sidefx](https://www.sidefx.com/docs/houdini/crowds/index.html)
Layered clips
Adds animation layers to agent motion. [sidefx](https://www.sidefx.com/docs/houdini/crowds/index.html)
Terrain adaptation
Adjusts movement to terrain and reduces foot skating. [sidefx](https://www.sidefx.com/docs/houdini/crowds/index.html)
Obstacle avoidance
Defines geometry agents should avoid. [sidefx](https://www.sidefx.com/docs/houdini/crowds/index.html)
Appearance variation
Makes agents look and act differently. [sidefx](https://www.sidefx.com/docs/houdini/crowds/index.html)
Parent-child relationships
Defines relationships between agents. [sidefx](https://www.sidefx.com/docs/houdini/crowds/index.html)
Ragdolls
Transfers appropriate behavior into rigid-body motion. [sidefx](https://www.sidefx.com/products/houdini/characters/crowds/)
Clothing interaction
Simulates Vellum cloth associated with agent geometry. [sidefx](https://www.sidefx.com/docs/houdini/crowds/index.html)
Hero-agent adjustment
Refines selected agents without rebuilding an entire crowd simulation. sidefx+1
Render-time optimization
Uses the crowd procedural to improve scene handling. sidefx+1
The documentation includes both traditional SOP/DOP setup descriptions and newer SOP-oriented motion-path workflows. These should be distinguished when following tutorials from different Houdini generations.sidefx+1
Technical documentation: [Crowd documentation](https://www.sidefx.com/docs/houdini/crowds/index.html) · [Crowd setup](https://www.sidefx.com/docs/houdini/crowds/setup.html) · [Crowd production features](https://www.sidefx.com/products/houdini/characters/crowds/)
Solaris and USD scenes
Solaris is Houdini’s USD-based environment for scene assembly, look development, lighting, and rendering. LOP nodes author USD data through higher-level operations instead of requiring artists to edit every underlying property directly.sidefx+1
What Solaris provides
Scene assembly: Bring geometry and assets into a USD stage.[sidefx](https://www.sidefx.com/docs/houdini/solaris/usd.html)
USD structure: Work with prims, typed attributes, relationships, and geometry primvars.[sidefx](https://www.sidefx.com/docs/houdini/solaris/usd.html)
Asset packaging: Build components with materials, variants, payloads, and layering support.[sidefx](https://www.sidefx.com/docs/houdini/solaris/karma.html)
Material assignment: Build and associate shading networks with scene assets.sidefx+1
Lighting workflows: Use lighting tools and render-delegate previews.[sidefx](https://www.sidefx.com/docs/houdini/solaris/karma.html)
Instancing: Create and manipulate scene instances and point-instancer workflows.[sidefx](https://www.sidefx.com/docs/houdini/solaris/usd.html)
Render passes: Define output and visibility arrangements through USD render structures.[sidefx](https://www.sidefx.com/docs/houdini/solaris/usd.html)
Render-time procedurals: Generate or deform content during rendering rather than baking every final element beforehand.[sidefx](https://www.sidefx.com/docs/houdini/solaris/karma.html)
H22 reorganizes several instancing tools and introduces additional USD authoring and render-pass workflows. It also translates Gaussian splats into USD’s ParticleField3DGaussianSplat primitive type.[sidefx](https://www.sidefx.com/docs/houdini/solaris/usd.html)
Technical documentation: [USD basics](https://www.sidefx.com/docs/houdini/solaris/usd.html) · [Solaris and Karma guide](https://www.sidefx.com/docs/houdini/solaris/karma.html) · [H22 Solaris changes](https://www.sidefx.com/docs/houdini/news/22/solaris.html)
Rendering and materials
Karma
Karma is a physically based path tracer that uses USD for scene description and operates as a Hydra render delegate. It is a distinct renderer, not simply a renamed version of Mantra.[sidefx](https://www.sidefx.com/docs/houdini/solaris/karma.html)
Area
Karma CPU
Karma XPU
Execution
CPU rendering. [sidefx](https://www.sidefx.com/products/karma/)
Uses CPU and supported GPU resources. [sidefx](https://www.sidefx.com/products/karma/)
Materials
Supports MaterialX and a supported subset of legacy VEX shading. sidefx+1
MaterialX-oriented; VEX-based shaders are not supported. [sidefx](https://www.sidefx.com/docs/houdini/solaris/karma.html)
Compatibility
Retains access to some features absent from XPU. [sidefx](https://www.sidefx.com/products/karma/)
Has its own feature and limitation set. [sidefx](https://www.sidefx.com/docs/houdini/solaris/karma.html)
Scene framework
Integrated with Solaris/USD. [sidefx](https://www.sidefx.com/docs/houdini/solaris/karma.html)
Integrated with Solaris/USD. [sidefx](https://www.sidefx.com/docs/houdini/solaris/karma.html)
CPU and XPU are not guaranteed to produce identical results for every scene or feature. SideFX’s release documentation explicitly describes them as different render engines and warns about delegate mismatches.[sidefx](https://www.sidefx.com/docs/houdini/news/20/karma.html)
Rendering functionality
The Karma documentation covers AOVs, light-path expressions, Cryptomatte, image filters, light filters, subsurface scattering, texture baking, color management, instancing, and specialized render-time procedurals.[sidefx](https://www.sidefx.com/docs/houdini/solaris/karma.html)
H22 adds or extends render-pass execution, global material AOV workflows, geometry and volume-light handling, texture-baking options, and Gaussian-splat rendering. Some Gaussian-splat rendering modes have edge-case AOV and holdout limitations.[sidefx](https://www.sidefx.com/docs/houdini/solaris/usd.html)
MaterialX
Houdini provides VOP equivalents of MaterialX nodes so materials can be built as graphs or imported from existing MaterialX content. MaterialX support is still subject to renderer-specific restrictions rather than guaranteeing that every node behaves identically everywhere.[sidefx](https://www.sidefx.com/docs/houdini/solaris/materialx.html)
Examples of documented restrictions include unsupported MaterialX light shaders, limitations around string inputs, and restrictions on combining surface and volume materials on the same primitive.[sidefx](https://www.sidefx.com/docs/houdini/solaris/materialx.html)
Viewport versus final rendering
Viewport materials and final-render materials are related but separate systems; a viewport shader does not automatically define the final rendered result. Current documentation recommends MaterialX for custom materials in the Vulkan viewport and states that custom GLSL shaders are not supported there.sidefx+1
Technical documentation: [Karma overview](https://www.sidefx.com/docs/houdini/solaris/karma.html) · [Karma product guide](https://www.sidefx.com/products/karma/) · [Karma Render Settings](https://www.sidefx.com/docs/houdini/nodes/lop/karmarendersettings.html) · [MaterialX](https://www.sidefx.com/docs/houdini/solaris/materialx.html) · [XPU precompilation](https://www.sidefx.com/docs/houdini/solaris/support/karma_precompile.html) · [H22 Karma changes](https://www.sidefx.com/docs/houdini/news/22/karma.html) · [Vulkan shaders](https://www.sidefx.com/docs/houdini/shade/opengl.html)
Copernicus image processing
Copernicus is Houdini’s GPU image-processing framework, designed for image manipulation in a system that connects 2D and 3D workflows. It is also a meeting point for terrain processing, machine-learning inference, materials, and newer simulation workflows.sidefx+2
Documented capabilities
Procedural image networks: Build connected image-processing operations.sidefx+1
Image loading and output inspection: Bring images into networks and view processed results.[sidefx](https://www.sidefx.com/docs/houdini/copernicus/working_with_cops.html)
Camera-related workflows: Work with Copernicus camera concepts.[sidefx](https://www.sidefx.com/docs/houdini/copernicus/cameras)
OpenFX integration: Use the documented OpenFX workflow.[sidefx](https://www.sidefx.com/docs/houdini/copernicus/cameras)
Filter lists: Assemble image-filter operations.[sidefx](https://www.sidefx.com/docs/houdini/copernicus/cameras)
Terrain processing: Construct terrains using image-based height information.[sidefx](https://www.sidefx.com/docs/houdini/heightfields_cop/index.html)
ONNX inference: Apply trained models to image-processing tasks.[sidefx](https://www.sidefx.com/docs/houdini/copernicus/index.html)
Neural cellular automata: Generate evolving learned patterns.sidefx+1
Material integration: Connect COP-generated textures into USD material workflows.[sidefx](https://www.sidefx.com/docs/houdini/solaris/usd.html)
GPU-native effects: Use H22’s documented Copernicus Pyro and implicit-collision workflows.[sidefx](https://www.sidefx.com/products/whats-new-in-h22/vfx/)
The technical distinction is useful: Copernicus is not merely a final image viewer; it is an executable image-data network that can participate in broader Houdini workflows.sidefx+1
Technical documentation: [Copernicus](https://www.sidefx.com/docs/houdini/copernicus/index.html) · [Working with COPs](https://www.sidefx.com/docs/houdini/copernicus/working_with_cops.html) · [Copernicus cameras](https://www.sidefx.com/docs/houdini/copernicus/cameras) · [Node examples](https://www.sidefx.com/docs/houdini/examples/nodes/cop/index.html)
Machine learning and splats
Houdini’s ML functionality spans data preparation, model training, inference, and task orchestration; it is not limited to loading a single pretrained image model. The documented toolsets cover image, terrain, character, and volumetric use cases.sidefx+1
Capability
What it does
Documented route
ONNX inference
Executes supported trained models on geometry or image data. [sidefx](https://www.sidefx.com/docs/houdini/fluid/surfacing.html)
SOP and COP inference nodes
Regression training
Trains models for supported regression workflows. [sidefx](https://www.sidefx.com/docs/houdini/fluid/surfacing.html)
ML training nodes and TOP orchestration
Character deformation
Uses trained deformation models, including crowd-agent deformation. [sidefx](https://www.sidefx.com/docs/houdini/fluid/surfacing.html)
ML deformer workflows
Style transfer
Trains image style-transfer models. [sidefx](https://www.sidefx.com/docs/houdini/ml/train_solutions/nodes_image.html)
Training TOP and ONNX inference
Denoiser training
Prepares data and trains OIDN filters. [sidefx](https://www.sidefx.com/docs/houdini/ml/train_solutions/nodes_image.html)
OIDN preprocessing and training TOPs
Computer vision
Trains object detection, masking, or keypoint-related models. [sidefx](https://www.sidefx.com/docs/houdini/ml/train_solutions/nodes_image.html)
Preprocess, train, and inference nodes
Image segmentation
Creates masks from positive/negative clicks and optional bounding boxes. [sidefx](https://www.sidefx.com/docs/houdini/fluid/surfacing.html)
SAM2-based neural masking
Depth estimation
Estimates depth, normals, and positions from an image. [sidefx](https://www.sidefx.com/docs/houdini/fluid/surfacing.html)
MoGe-2-based node
Neural terrains
Runs terrain-generation models. [sidefx](https://www.sidefx.com/docs/houdini/fluid/surfacing.html)
Neural Terrain Generate SOP
Neural cellular automata
Learns and evolves tileable patterns. [sidefx](https://www.sidefx.com/docs/houdini/ml/train_solutions/nodes_image.html)
Training TOPs and COP execution
Gaussian-splat training
Trains splats from posed images. [sidefx](https://www.sidefx.com/docs/houdini/ml/train_solutions/nodes_image.html)
Preprocessing and GSplat training TOPs
Synthetic splat generation
Converts rendered scene observations into trained splat representations. [sidefx](https://www.sidefx.com/docs/houdini/ml/train_solutions/nodes_image.html)
Karma, TOPs, and Solaris recipes
Gaussian-splat workflow
The documented synthetic-splat pipeline uses Karma to generate image data, preprocesses images and camera information into a COLMAP-like dataset, and trains a Gaussian-splat model. H22 then provides additional rendering, animation, USD interchange, and relighting support.sidefx+1
This is a different representation from an ordinary polygon mesh or a smoke volume, even though Houdini can connect those representations within one project.sidefx+1
ML limitations
ONNX compatibility is model-dependent, not universal; H22 specifically documents extensions for tensor types, scalar tensors, larger external-data models, and device selection. The documentation also distinguishes some SOP-only behavior, such as content-dependent dynamic output sizes.[sidefx](https://www.sidefx.com/docs/houdini/fluid/surfacing.html)
The presence of an integration also does not establish unrestricted rights to every external model or weight package; deployment should include a separate review of those dependencies.
Technical documentation: [ML image tools](https://www.sidefx.com/docs/houdini/ml/train_solutions/nodes_image.html) · [H22 ML changes](https://www.sidefx.com/docs/houdini/news/22/ml.html) · [ML task orchestration](https://www.sidefx.com/docs/houdini/news/22/pdg.html) · [Gaussian-splat rendering changes](https://www.sidefx.com/docs/houdini/news/22/karma.html)
Automation and pipeline execution
PDG and TOPs
TOP networks define data-driven workflows by turning inputs into work items and processing them through dependencies. Tasks can represent external processes running locally or through a farm scheduler.[sidefx](https://www.sidefx.com/docs/houdini/basics/index.html)
Function
Production use
File discovery
Finds files and creates work items from matching patterns. [sidefx](https://www.sidefx.com/docs/houdini/solaris/usd.html)
Parameter variations
Applies wedge settings to scene-based tasks. [sidefx](https://www.sidefx.com/docs/houdini/solaris/usd.html)
Python execution
Runs scripts in process-related workflows, including out-of-process tasks. [sidefx](https://www.sidefx.com/docs/houdini/solaris/usd.html)
Scene loading
Loads a Houdini scene before running a task script. [sidefx](https://www.sidefx.com/docs/houdini/solaris/usd.html)
Render/output fetching
Executes ROP-related work with frame-range controls. [sidefx](https://www.sidefx.com/docs/houdini/solaris/usd.html)
Python environments
Creates and configures task-specific virtual environments. [sidefx](https://www.sidefx.com/docs/houdini/solaris/usd.html)
Persistent services
Uses scheduler services to process work. [sidefx](https://www.sidefx.com/docs/houdini/solaris/usd.html)
ML pipelines
Coordinates preprocessing and training jobs. sidefx+1
Texture conversion
Batch-converts images into efficient texture maps. [sidefx](https://www.sidefx.com/docs/houdini/solaris/usd.html)
USD packaging
Produces self-contained USDZ packages through usdzip. [sidefx](https://www.sidefx.com/docs/houdini/solaris/usd.html)
Documentation: [TOP node catalog](https://www.sidefx.com/docs/houdini/nodes/top/index.html) · [PDG changes and APIs](https://www.sidefx.com/docs/houdini/news/22/pdg.html)
Render and export dependencies
ROP networks can represent rendering jobs and dependency relationships between passes or outputs. They also include export-oriented operators, such as Alembic output.[sidefx](https://www.sidefx.com/docs/houdini/nodes/out/index.html)
The husk command-line workflow supports USD rendering outside the interactive viewport; H22 extends render-pass handling, frame-range behavior, output controls, and skipping existing frames.[sidefx](https://www.sidefx.com/docs/houdini/solaris/usd.html)
Technical documentation: [ROP reference](https://www.sidefx.com/docs/houdini/nodes/out/index.html) · [Husk-related changes](https://www.sidefx.com/docs/houdini/news/22/karma.html)
Reusable assets and integrations
Houdini Digital Assets
Houdini Digital Assets package node networks into reusable tools that expose selected parameters and interaction controls. Houdini Engine can load and evaluate those assets inside supported host applications.[sidefx](https://www.sidefx.com/products/houdini-engine/)
This lets a technical artist build a procedural tool once and expose a smaller, task-specific interface to other users. The host-side interaction can include parameter editing and viewport handles.[sidefx](https://www.sidefx.com/products/houdini-engine/)
Documentation: [Digital asset creation dialog](https://www.sidefx.com/docs/houdini/ref/windows/save_asset.html) · [Houdini Engine overview](https://www.sidefx.com/products/houdini-engine/)
Houdini Engine
Houdini Engine is a C API designed for binary compatibility and integration into native or managed hosts. SideFX distinguishes this relatively small integration interface from the broader C++ HDK.[sidefx](https://www.sidefx.com/docs/houdini/ref/hengine.html)
Integration area
What it enables
Unreal Engine
Evaluates procedural assets and translates inputs/outputs into Unreal workflows. sidefx+1
Unity
Runs Houdini asset workflows through the Unity plug-in. sidefx+1
Maya
Uses Houdini assets inside the supported Maya integration. sidefx+1
3ds Max
Instantiates assets as scene objects with generated geometry or instances. [sidefx](https://www.sidefx.com/docs/houdini/3dsmax/objectassets.html)
Custom applications
Creates, connects, queries, and evaluates nodes through HAPI. sidefx+1
Batch processing
Runs non-graphical tasks such as simulations and caching under appropriate licensing. [sidefx](https://www.sidefx.com/products/houdini-engine/)
H22 documents Copernicus-asset support and expanded texture workflows in Unreal, alongside PCG-related integration improvements. Host-version support is explicitly versioned and should be checked for the exact Houdini build being deployed.[sidefx](https://www.sidefx.com/docs/houdini/news/22/engine.html)
Interchange formats
The inspected documentation includes Alembic export, USD scene workflows, FBX character and camera handling, and glTF geometry/material/animation interchange. H22’s glTF work includes expanded material-extension support and multi-animation export.sidefx+2
Technical documentation: [Engine API overview](https://www.sidefx.com/docs/houdini/ref/hengine.html) · [HAPI node operations](https://www.sidefx.com/docs/hengine/_h_a_p_i__nodes.html) · [Unreal attributes](https://www.sidefx.com/docs/houdini/unreal/attributes.html) · [3ds Max object assets](https://www.sidefx.com/docs/houdini/3dsmax/objectassets.html) · [H22 plug-ins and interchange](https://www.sidefx.com/docs/houdini/news/22/engine.html)
Developer documentation
These are the principal technical interfaces for extending Houdini rather than only operating its user interface.sidefx+2
Interface
What to use it for
Technical entry point
HOM / Python
Application automation, geometry access, editor tooling, and object manipulation. sidefx+1
[Geometry API](https://www.sidefx.com/docs/houdini/hom/hou/Geometry.html)
VEX
Parallel operations over geometry and simulation elements. [sidefx](https://www.sidefx.com/docs/houdini//vex/snippets.html)
[VEX snippets](https://www.sidefx.com/docs/houdini//vex/snippets.html)
VEX geometry functions
Geometry traversal and geometry-specific operations. [sidefx](https://www.sidefx.com/docs/houdini/vex/geometry.html)
[Geometry functions](https://www.sidefx.com/docs/houdini/vex/geometry.html)
OpenCL
Custom compute kernels using Houdini’s bindings and helpers. [sidefx](https://www.sidefx.com/docs/houdini/fluid/surfacing.html)
[OpenCL documentation changes](https://www.sidefx.com/docs/houdini/news/22/vex.html)
APEX / APEX Script
Building and evaluating rig graphs. sidefx+1
[APEX basics](https://www.sidefx.com/docs/houdini/character/kinefx/apexgraphbasics.html)
HDK / C++
Compiled extensions and deeper native integration. sidefx+1
[HDK table of contents](https://www.sidefx.com/docs/hdk/)
Houdini Engine / HAPI
Embedding Houdini assets in host applications. sidefx+1
[HAPI node documentation](https://www.sidefx.com/docs/hengine/_h_a_p_i__nodes.html)
HScript
Existing script workflows and command integration. [sidefx](https://www.sidefx.com/docs/hdk/_h_d_k__call_c_m_d.html)
[Calling HScript from C++](https://www.sidefx.com/docs/hdk/_h_d_k__call_c_m_d.html)
USD / Solaris
Procedural scene-description authoring and rendering workflows. [sidefx](https://www.sidefx.com/docs/houdini/solaris/usd.html)
[USD basics](https://www.sidefx.com/docs/houdini/solaris/usd.html)
MaterialX
Portable material-graph authoring within supported limits. [sidefx](https://www.sidefx.com/docs/houdini/solaris/materialx.html)
[MaterialX in Solaris](https://www.sidefx.com/docs/houdini/solaris/materialx.html)
Native extension development
The HDK documentation provides compilation routes using hcustom, makefiles, and platform-specific compiler setups. It notes that hcustom is convenient for small examples but not suitable for larger multi-source projects.[sidefx](https://www.sidefx.com/docs/hdk/_h_d_k__intro__compiling.html)
Technical documentation: [Compiling HDK code](https://www.sidefx.com/docs/hdk/_h_d_k__intro__compiling.html)
Language-level constraints
VEX is powerful but not an unrestricted substitute for every general-purpose language feature; for example, its documented arrays do not support multidimensional arrays. API and language constraints should be checked before choosing an implementation layer.[sidefx](https://www.sidefx.com/docs/houdini/vex/arrays.html)
Technical documentation: [VEX arrays](https://www.sidefx.com/docs/houdini/vex/arrays.html)
Important boundaries
Features are version-dependent
Historical nodes can remain documented while being deprecated; examples include the old Point SOP and Particle Fluid Solver DOP. A documentation search should therefore distinguish current workflows from compatibility material.sidefx+1
Solver systems are not universal
MPM, FLIP, Vellum, Bullet, and Otis solve different problems with different trade-offs. Even systems that simulate apparently similar materials can differ in coupling, detail, performance, and preparation requirements.sidefx+2
GPU support is subsystem-specific
Karma XPU, Minimal OpenCL Pyro, Vellum acceleration, Copernicus processing, and Otis acceleration are different capabilities. Support in one subsystem does not establish support in another.sidefx+4
Licensing changes available functionality
Node context is not a reliable proxy for product entitlement: Otis is a SOP solver but is not available in Houdini Core. Houdini Engine also has distinct plug-in and batch-processing licensing arrangements.sidefx+1
Some new workflows are beta
H22’s APEX-in-Solaris tools are explicitly described as unfinished and subject to change. Production adoption should account for that status rather than treating every documented addition as equally mature.[sidefx](https://www.sidefx.com/docs/houdini/solaris/usd.html)
Public documentation has limits
This report establishes publicly documented capabilities and mechanisms. It does not establish complete internal solver implementations, exhaustive numerical-validation results, universal plug-in compatibility, or a benchmark ranking across hardware and software configurations.