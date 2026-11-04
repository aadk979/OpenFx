//! Immutable acyclic procedural node graph, canonical `.ofxs` serialization,
//! fine-grained graph edits, and operator catalog introspection.
use pcc_id::{NodeId, RevisionId};
use pcc_math::Vec3;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttrDomain {
    Point,
    Prim,
    Detail,
}

impl AttrDomain {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Point => "point",
            Self::Prim => "prim",
            Self::Detail => "detail",
        }
    }

    pub fn from_token(token: &str) -> Result<Self, SceneError> {
        match token {
            "point" => Ok(Self::Point),
            "prim" => Ok(Self::Prim),
            "detail" => Ok(Self::Detail),
            _ => Err(SceneError::Field),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PromoteMode {
    Average,
    Min,
    Max,
    Sum,
}

impl PromoteMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Average => "average",
            Self::Min => "min",
            Self::Max => "max",
            Self::Sum => "sum",
        }
    }

    pub fn from_token(token: &str) -> Result<Self, SceneError> {
        match token {
            "average" => Ok(Self::Average),
            "min" => Ok(Self::Min),
            "max" => Ok(Self::Max),
            "sum" => Ok(Self::Sum),
            _ => Err(SceneError::Field),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UvProjectMode {
    PlanarY,
    PlanarZ,
    PlanarX,
    Cylindrical,
    Spherical,
    Box,
}

impl UvProjectMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PlanarY => "planar_y",
            Self::PlanarZ => "planar_z",
            Self::PlanarX => "planar_x",
            Self::Cylindrical => "cylindrical",
            Self::Spherical => "spherical",
            Self::Box => "box",
        }
    }

    pub fn from_token(token: &str) -> Result<Self, SceneError> {
        match token {
            "planar_y" => Ok(Self::PlanarY),
            "planar_z" => Ok(Self::PlanarZ),
            "planar_x" => Ok(Self::PlanarX),
            "cylindrical" => Ok(Self::Cylindrical),
            "spherical" => Ok(Self::Spherical),
            "box" => Ok(Self::Box),
            _ => Err(SceneError::Field),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BooleanMode {
    Union,
    Intersect,
    Subtract,
}

impl BooleanMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Union => "union",
            Self::Intersect => "intersect",
            Self::Subtract => "subtract",
        }
    }

    pub fn from_token(token: &str) -> Result<Self, SceneError> {
        match token {
            "union" => Ok(Self::Union),
            "intersect" => Ok(Self::Intersect),
            "subtract" => Ok(Self::Subtract),
            _ => Err(SceneError::Field),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Node {
    CreatePoints {
        id: NodeId,
        count: u32,
    },
    CreateGrid {
        id: NodeId,
        rows: u32,
        cols: u32,
        size_x: f32,
        size_z: f32,
    },
    CreateBox {
        id: NodeId,
        size: Vec3,
    },
    Transform {
        id: NodeId,
        input: NodeId,
        offset: Vec3,
    },
    Normal {
        id: NodeId,
        input: NodeId,
    },
    PointWrangle {
        id: NodeId,
        input: NodeId,
        snippet: String,
    },
    Subdivide {
        id: NodeId,
        input: NodeId,
        iterations: u32,
    },
    PolyExtrude {
        id: NodeId,
        input: NodeId,
        distance: f32,
        inset: f32,
    },
    AttribPromote {
        id: NodeId,
        input: NodeId,
        name: String,
        from_domain: AttrDomain,
        to_domain: AttrDomain,
        mode: PromoteMode,
    },
    Blast {
        id: NodeId,
        input: NodeId,
        group_name: String,
        negate: bool,
    },
    UvProject {
        id: NodeId,
        input: NodeId,
        mode: UvProjectMode,
    },
    HeightField {
        id: NodeId,
        rows: u32,
        cols: u32,
        size_x: f32,
        size_z: f32,
    },
    HeightFieldNoise {
        id: NodeId,
        input: NodeId,
        amplitude: f32,
        frequency: f32,
        octaves: u32,
        seed: u64,
    },
    HeightFieldErode {
        id: NodeId,
        input: NodeId,
        iterations: u32,
        rain_rate: f32,
        talus_angle: f32,
        seed: u64,
    },
    VdbFromPolygons {
        id: NodeId,
        input: NodeId,
        voxel_size: f32,
    },
    VdbReshape {
        id: NodeId,
        input: NodeId,
        offset: f32,
        smooth_iterations: u32,
    },
    ConvertVdb {
        id: NodeId,
        input: NodeId,
    },
    Boolean {
        id: NodeId,
        left: NodeId,
        right: NodeId,
        mode: BooleanMode,
        voxel_size: f32,
    },
    Scatter {
        id: NodeId,
        input: NodeId,
        count: u32,
        seed: u64,
    },
    CopyToPoints {
        id: NodeId,
        source: NodeId,
        target: NodeId,
    },
    Merge {
        id: NodeId,
        left: NodeId,
        right: NodeId,
    },
}

impl Node {
    #[must_use]
    pub const fn id(&self) -> NodeId {
        match self {
            Self::CreatePoints { id, .. }
            | Self::CreateGrid { id, .. }
            | Self::CreateBox { id, .. }
            | Self::Transform { id, .. }
            | Self::Normal { id, .. }
            | Self::PointWrangle { id, .. }
            | Self::Subdivide { id, .. }
            | Self::PolyExtrude { id, .. }
            | Self::AttribPromote { id, .. }
            | Self::Blast { id, .. }
            | Self::UvProject { id, .. }
            | Self::HeightField { id, .. }
            | Self::HeightFieldNoise { id, .. }
            | Self::HeightFieldErode { id, .. }
            | Self::VdbFromPolygons { id, .. }
            | Self::VdbReshape { id, .. }
            | Self::ConvertVdb { id, .. }
            | Self::Boolean { id, .. }
            | Self::Scatter { id, .. }
            | Self::CopyToPoints { id, .. }
            | Self::Merge { id, .. } => *id,
        }
    }

    #[must_use]
    pub const fn is_generator(&self) -> bool {
        matches!(
            self,
            Self::CreatePoints { .. }
                | Self::CreateGrid { .. }
                | Self::CreateBox { .. }
                | Self::HeightField { .. }
        )
    }

    #[must_use]
    pub fn inputs(&self) -> Vec<NodeId> {
        match self {
            Self::CreatePoints { .. }
            | Self::CreateGrid { .. }
            | Self::CreateBox { .. }
            | Self::HeightField { .. } => Vec::new(),
            Self::Transform { input, .. }
            | Self::Normal { input, .. }
            | Self::PointWrangle { input, .. }
            | Self::Subdivide { input, .. }
            | Self::PolyExtrude { input, .. }
            | Self::AttribPromote { input, .. }
            | Self::Blast { input, .. }
            | Self::UvProject { input, .. }
            | Self::HeightFieldNoise { input, .. }
            | Self::HeightFieldErode { input, .. }
            | Self::VdbFromPolygons { input, .. }
            | Self::VdbReshape { input, .. }
            | Self::ConvertVdb { input, .. }
            | Self::Scatter { input, .. } => vec![*input],
            Self::CopyToPoints { source, target, .. } => vec![*source, *target],
            Self::Merge { left, right, .. } | Self::Boolean { left, right, .. } => {
                vec![*left, *right]
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "edit", rename_all = "snake_case")]
pub enum GraphEdit {
    AddNode {
        node: Node,
    },
    RemoveNode {
        id: NodeId,
    },
    SetCreateCount {
        id: NodeId,
        count: u32,
    },
    SetTransformOffset {
        id: NodeId,
        offset: Vec3,
    },
    ConnectTransformInput {
        id: NodeId,
        input: NodeId,
    },
    ConnectMergeInputs {
        id: NodeId,
        left: NodeId,
        right: NodeId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodePortSpec {
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeParamSpec {
    pub name: String,
    pub param_type: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeTypeSpec {
    pub type_name: String,
    pub category: String,
    pub summary: String,
    pub inputs: Vec<NodePortSpec>,
    pub outputs: Vec<NodePortSpec>,
    pub params: Vec<NodeParamSpec>,
}

#[must_use]
pub fn node_catalog() -> Vec<NodeTypeSpec> {
    vec![
        NodeTypeSpec {
            type_name: "create".to_owned(),
            category: "generator".to_owned(),
            summary: "Generates a deterministic sequence of points along the +X axis.".to_owned(),
            inputs: Vec::new(),
            outputs: vec![NodePortSpec {
                name: "out".to_owned(),
                description: "Generated point geometry stream.".to_owned(),
            }],
            params: vec![NodeParamSpec {
                name: "count".to_owned(),
                param_type: "u32".to_owned(),
                description: "Number of points to create.".to_owned(),
            }],
        },
        NodeTypeSpec {
            type_name: "grid".to_owned(),
            category: "generator".to_owned(),
            summary: "Generates a right-handed Y-up quad polygon mesh in the XZ plane.".to_owned(),
            inputs: Vec::new(),
            outputs: vec![NodePortSpec {
                name: "out".to_owned(),
                description: "Generated quad polygon mesh.".to_owned(),
            }],
            params: vec![
                NodeParamSpec {
                    name: "rows".to_owned(),
                    param_type: "u32".to_owned(),
                    description: "Vertex row count (>= 2).".to_owned(),
                },
                NodeParamSpec {
                    name: "cols".to_owned(),
                    param_type: "u32".to_owned(),
                    description: "Vertex column count (>= 2).".to_owned(),
                },
                NodeParamSpec {
                    name: "size_x".to_owned(),
                    param_type: "f32".to_owned(),
                    description: "Positive finite extent along X in metres.".to_owned(),
                },
                NodeParamSpec {
                    name: "size_z".to_owned(),
                    param_type: "f32".to_owned(),
                    description: "Positive finite extent along Z in metres.".to_owned(),
                },
            ],
        },
        NodeTypeSpec {
            type_name: "box".to_owned(),
            category: "generator".to_owned(),
            summary: "Generates a closed outward-oriented 8-point, 6-quad manifold box mesh.".to_owned(),
            inputs: Vec::new(),
            outputs: vec![NodePortSpec {
                name: "out".to_owned(),
                description: "Generated closed manifold box mesh.".to_owned(),
            }],
            params: vec![NodeParamSpec {
                name: "size".to_owned(),
                param_type: "Vec3".to_owned(),
                description: "Positive finite (x, y, z) dimensions in metres.".to_owned(),
            }],
        },
        NodeTypeSpec {
            type_name: "transform".to_owned(),
            category: "modifier".to_owned(),
            summary: "Translates all input points by a finite 3D offset in metres.".to_owned(),
            inputs: vec![NodePortSpec {
                name: "input".to_owned(),
                description: "Upstream geometry stream to translate.".to_owned(),
            }],
            outputs: vec![NodePortSpec {
                name: "out".to_owned(),
                description: "Translated geometry stream.".to_owned(),
            }],
            params: vec![NodeParamSpec {
                name: "offset".to_owned(),
                param_type: "Vec3".to_owned(),
                description: "Finite (x, y, z) translation offset in metres.".to_owned(),
            }],
        },
        NodeTypeSpec {
            type_name: "normal".to_owned(),
            category: "modifier".to_owned(),
            summary: "Computes area-weighted smooth point normals (`v@N`) across polygon faces.".to_owned(),
            inputs: vec![NodePortSpec {
                name: "input".to_owned(),
                description: "Upstream polygon mesh.".to_owned(),
            }],
            outputs: vec![NodePortSpec {
                name: "out".to_owned(),
                description: "Mesh with `v@N` point normal attribute.".to_owned(),
            }],
            params: Vec::new(),
        },
        NodeTypeSpec {
            type_name: "wrangle".to_owned(),
            category: "modifier".to_owned(),
            summary: "Executes a fuel-metered VEX-lite data-parallel snippet over all points.".to_owned(),
            inputs: vec![NodePortSpec {
                name: "input".to_owned(),
                description: "Upstream geometry stream.".to_owned(),
            }],
            outputs: vec![NodePortSpec {
                name: "out".to_owned(),
                description: "Modified geometry stream.".to_owned(),
            }],
            params: vec![NodeParamSpec {
                name: "snippet".to_owned(),
                param_type: "String".to_owned(),
                description: "VEX-lite point wrangle source code.".to_owned(),
            }],
        },
        NodeTypeSpec {
            type_name: "subdivide".to_owned(),
            category: "modifier".to_owned(),
            summary: "Subdivides manifold polygon meshes using Catmull-Clark subdivision.".to_owned(),
            inputs: vec![NodePortSpec {
                name: "input".to_owned(),
                description: "Upstream manifold polygon mesh.".to_owned(),
            }],
            outputs: vec![NodePortSpec {
                name: "out".to_owned(),
                description: "Subdivided all-quad polygon mesh.".to_owned(),
            }],
            params: vec![NodeParamSpec {
                name: "iterations".to_owned(),
                param_type: "u32".to_owned(),
                description: "Number of Catmull-Clark subdivision iterations (1..=6).".to_owned(),
            }],
        },
        NodeTypeSpec {
            type_name: "extrude".to_owned(),
            category: "modifier".to_owned(),
            summary: "Extrudes polygon faces along their face normals with optional inset and side-wall stitching.".to_owned(),
            inputs: vec![NodePortSpec {
                name: "input".to_owned(),
                description: "Upstream manifold polygon mesh.".to_owned(),
            }],
            outputs: vec![NodePortSpec {
                name: "out".to_owned(),
                description: "Extruded polygon mesh with stitched side-wall quads.".to_owned(),
            }],
            params: vec![
                NodeParamSpec {
                    name: "distance".to_owned(),
                    param_type: "f32".to_owned(),
                    description: "Finite extrusion distance along face normals in metres.".to_owned(),
                },
                NodeParamSpec {
                    name: "inset".to_owned(),
                    param_type: "f32".to_owned(),
                    description: "Non-negative finite inset toward face centroids in metres.".to_owned(),
                },
            ],
        },
        NodeTypeSpec {
            type_name: "attribpromote".to_owned(),
            category: "modifier".to_owned(),
            summary: "Promotes F32 or Vec3 attributes between point, prim, and detail domains.".to_owned(),
            inputs: vec![NodePortSpec {
                name: "input".to_owned(),
                description: "Upstream geometry stream.".to_owned(),
            }],
            outputs: vec![NodePortSpec {
                name: "out".to_owned(),
                description: "Geometry with promoted attribute on target domain.".to_owned(),
            }],
            params: vec![
                NodeParamSpec {
                    name: "name".to_owned(),
                    param_type: "String".to_owned(),
                    description: "Attribute name to promote.".to_owned(),
                },
                NodeParamSpec {
                    name: "from_domain".to_owned(),
                    param_type: "AttrDomain".to_owned(),
                    description: "Source domain (`point`, `prim`, `detail`).".to_owned(),
                },
                NodeParamSpec {
                    name: "to_domain".to_owned(),
                    param_type: "AttrDomain".to_owned(),
                    description: "Destination domain (`point`, `prim`, `detail`).".to_owned(),
                },
                NodeParamSpec {
                    name: "mode".to_owned(),
                    param_type: "PromoteMode".to_owned(),
                    description: "Reduction mode (`average`, `min`, `max`, `sum`).".to_owned(),
                },
            ],
        },
        NodeTypeSpec {
            type_name: "blast".to_owned(),
            category: "modifier".to_owned(),
            summary: "Deletes primitives or points matching a named group or attribute mask, compacting topology.".to_owned(),
            inputs: vec![NodePortSpec {
                name: "input".to_owned(),
                description: "Upstream geometry stream.".to_owned(),
            }],
            outputs: vec![NodePortSpec {
                name: "out".to_owned(),
                description: "Compacted geometry with selected (or non-selected) elements removed.".to_owned(),
            }],
            params: vec![
                NodeParamSpec {
                    name: "group_name".to_owned(),
                    param_type: "String".to_owned(),
                    description: "Name of the primitive/point group or mask attribute.".to_owned(),
                },
                NodeParamSpec {
                    name: "negate".to_owned(),
                    param_type: "bool".to_owned(),
                    description: "When true, deletes non-selected elements instead of selected ones.".to_owned(),
                },
            ],
        },
        NodeTypeSpec {
            type_name: "scatter".to_owned(),
            category: "generator_modifier".to_owned(),
            summary: "Deterministically scatters points across the input bounding volume using Philox4x32.".to_owned(),
            inputs: vec![NodePortSpec {
                name: "input".to_owned(),
                description: "Upstream geometry defining the bounding region.".to_owned(),
            }],
            outputs: vec![NodePortSpec {
                name: "out".to_owned(),
                description: "Scattered point cloud.".to_owned(),
            }],
            params: vec![
                NodeParamSpec {
                    name: "count".to_owned(),
                    param_type: "u32".to_owned(),
                    description: "Number of points to scatter.".to_owned(),
                },
                NodeParamSpec {
                    name: "seed".to_owned(),
                    param_type: "u64".to_owned(),
                    description: "Deterministic 64-bit Philox seed.".to_owned(),
                },
            ],
        },
        NodeTypeSpec {
            type_name: "copytopoints".to_owned(),
            category: "instancer".to_owned(),
            summary: "Copies the source geometry onto every point of the target geometry (scaling by `f@pscale` if present).".to_owned(),
            inputs: vec![
                NodePortSpec {
                    name: "source".to_owned(),
                    description: "Prototype geometry to copy.".to_owned(),
                },
                NodePortSpec {
                    name: "target".to_owned(),
                    description: "Target point cloud onto which copies are placed.".to_owned(),
                },
            ],
            outputs: vec![NodePortSpec {
                name: "out".to_owned(),
                description: "Instanced/merged geometry.".to_owned(),
            }],
            params: Vec::new(),
        },
        NodeTypeSpec {
            type_name: "merge".to_owned(),
            category: "utility".to_owned(),
            summary:
                "Concatenates two upstream geometry streams in deterministic left-then-right order."
                    .to_owned(),
            inputs: vec![
                NodePortSpec {
                    name: "left".to_owned(),
                    description: "First upstream geometry stream.".to_owned(),
                },
                NodePortSpec {
                    name: "right".to_owned(),
                    description: "Second upstream geometry stream.".to_owned(),
                },
            ],
            outputs: vec![NodePortSpec {
                name: "out".to_owned(),
                description: "Merged geometry stream.".to_owned(),
            }],
            params: Vec::new(),
        },
        NodeTypeSpec {
            type_name: "uvproject".to_owned(),
            category: "uv".to_owned(),
            summary: "Projects normalized point UV coordinates (`v@uv` in [0, 1]^2) using planar, cylindrical, spherical, or box projection."
                .to_owned(),
            inputs: vec![NodePortSpec {
                name: "in".to_owned(),
                description: "Upstream geometry stream.".to_owned(),
            }],
            outputs: vec![NodePortSpec {
                name: "out".to_owned(),
                description: "Geometry with projected `v@uv` point attribute.".to_owned(),
            }],
            params: vec![NodeParamSpec {
                name: "mode".to_owned(),
                param_type: "string".to_owned(),
                description: "Projection mode (`planar_y`, `planar_z`, `planar_x`, `cylindrical`, `spherical`, `box`).".to_owned(),
            }],
        },
        NodeTypeSpec {
            type_name: "heightfield".to_owned(),
            category: "terrain".to_owned(),
            summary: "Generates a 2D Copy-on-Write tiled heightfield terrain grid with `height`, `mask`, `water`, and `sediment` layers."
                .to_owned(),
            inputs: Vec::new(),
            outputs: vec![NodePortSpec {
                name: "out".to_owned(),
                description: "Heightfield terrain geometry stream.".to_owned(),
            }],
            params: vec![
                NodeParamSpec {
                    name: "rows".to_owned(),
                    param_type: "u32".to_owned(),
                    description: "Number of grid rows along +Z (>= 2).".to_owned(),
                },
                NodeParamSpec {
                    name: "cols".to_owned(),
                    param_type: "u32".to_owned(),
                    description: "Number of grid columns along +X (>= 2).".to_owned(),
                },
                NodeParamSpec {
                    name: "size_x".to_owned(),
                    param_type: "f32".to_owned(),
                    description: "Total terrain extent along +X in metres (> 0).".to_owned(),
                },
                NodeParamSpec {
                    name: "size_z".to_owned(),
                    param_type: "f32".to_owned(),
                    description: "Total terrain extent along +Z in metres (> 0).".to_owned(),
                },
            ],
        },
        NodeTypeSpec {
            type_name: "hf_noise".to_owned(),
            category: "terrain".to_owned(),
            summary: "Applies deterministic derivative-warped multi-fractal fBm noise (`Philox4x32`) to a heightfield terrain."
                .to_owned(),
            inputs: vec![NodePortSpec {
                name: "in".to_owned(),
                description: "Upstream heightfield terrain or grid geometry.".to_owned(),
            }],
            outputs: vec![NodePortSpec {
                name: "out".to_owned(),
                description: "Heightfield terrain with displaced `height` layer and `@P.y`.".to_owned(),
            }],
            params: vec![
                NodeParamSpec {
                    name: "amplitude".to_owned(),
                    param_type: "f32".to_owned(),
                    description: "Vertical displacement amplitude in metres.".to_owned(),
                },
                NodeParamSpec {
                    name: "frequency".to_owned(),
                    param_type: "f32".to_owned(),
                    description: "Base spatial frequency (> 0).".to_owned(),
                },
                NodeParamSpec {
                    name: "octaves".to_owned(),
                    param_type: "u32".to_owned(),
                    description: "Number of fBm octaves (1..=12).".to_owned(),
                },
                NodeParamSpec {
                    name: "seed".to_owned(),
                    param_type: "u64".to_owned(),
                    description: "Deterministic Philox4x32 PRNG seed.".to_owned(),
                },
            ],
        },
        NodeTypeSpec {
            type_name: "hf_erode".to_owned(),
            category: "terrain".to_owned(),
            summary: "Simulates shallow-water virtual-pipe hydraulic erosion and thermal talus slumping across a heightfield terrain."
                .to_owned(),
            inputs: vec![NodePortSpec {
                name: "in".to_owned(),
                description: "Upstream heightfield terrain geometry.".to_owned(),
            }],
            outputs: vec![NodePortSpec {
                name: "out".to_owned(),
                description: "Eroded heightfield terrain with updated `height`, `water`, and `sediment` layers.".to_owned(),
            }],
            params: vec![
                NodeParamSpec {
                    name: "iterations".to_owned(),
                    param_type: "u32".to_owned(),
                    description: "Number of hydraulic/thermal solver steps (1..=500).".to_owned(),
                },
                NodeParamSpec {
                    name: "rain_rate".to_owned(),
                    param_type: "f32".to_owned(),
                    description: "Rainfall rate per unit step (>= 0).".to_owned(),
                },
                NodeParamSpec {
                    name: "talus_angle".to_owned(),
                    param_type: "f32".to_owned(),
                    description: "Critical thermal talus angle in degrees (1..=89).".to_owned(),
                },
                NodeParamSpec {
                    name: "seed".to_owned(),
                    param_type: "u64".to_owned(),
                    description: "Deterministic Philox4x32 rainfall seed.".to_owned(),
                },
            ],
        },
        NodeTypeSpec {
            type_name: "vdb_from_polygons".to_owned(),
            category: "vdb".to_owned(),
            summary: "Voxelizes a polygon mesh into a narrow-band sparse 8x8x8 brick signed distance field (`VdbGrid`) using generalized solid-angle winding numbers."
                .to_owned(),
            inputs: vec![NodePortSpec {
                name: "in".to_owned(),
                description: "Upstream polygon mesh.".to_owned(),
            }],
            outputs: vec![NodePortSpec {
                name: "out".to_owned(),
                description: "Sparse narrow-band SDF volume representation (`f@phi`, `i@vx`, `i@vy`, `i@vz`).".to_owned(),
            }],
            params: vec![NodeParamSpec {
                name: "voxel_size".to_owned(),
                param_type: "f32".to_owned(),
                description: "World-space voxel edge length in metres (>= 0.05).".to_owned(),
            }],
        },
        NodeTypeSpec {
            type_name: "vdb_reshape".to_owned(),
            category: "vdb".to_owned(),
            summary: "Applies morphological level-set dilation/erosion (`offset`) and Laplacian smoothing to a narrow-band SDF."
                .to_owned(),
            inputs: vec![NodePortSpec {
                name: "in".to_owned(),
                description: "Upstream VDB signed distance field.".to_owned(),
            }],
            outputs: vec![NodePortSpec {
                name: "out".to_owned(),
                description: "Reshaped VDB signed distance field.".to_owned(),
            }],
            params: vec![
                NodeParamSpec {
                    name: "offset".to_owned(),
                    param_type: "f32".to_owned(),
                    description: "Signed world-space level-set offset (> 0 dilates, < 0 erodes).".to_owned(),
                },
                NodeParamSpec {
                    name: "smooth_iterations".to_owned(),
                    param_type: "u32".to_owned(),
                    description: "Number of 6-neighbor Laplacian smoothing passes (0..=50).".to_owned(),
                },
            ],
        },
        NodeTypeSpec {
            type_name: "convert_vdb".to_owned(),
            category: "vdb".to_owned(),
            summary: "Extracts the zero-isosurface of a VDB signed distance field as a guaranteed closed manifold all-quad mesh via Dual Surface Nets."
                .to_owned(),
            inputs: vec![NodePortSpec {
                name: "in".to_owned(),
                description: "Upstream VDB signed distance field.".to_owned(),
            }],
            outputs: vec![NodePortSpec {
                name: "out".to_owned(),
                description: "Closed manifold quad polygon mesh.".to_owned(),
            }],
            params: Vec::new(),
        },
        NodeTypeSpec {
            type_name: "boolean".to_owned(),
            category: "modeling".to_owned(),
            summary: "Performs a guaranteed-manifold volumetric CSG boolean (`union`, `intersect`, `subtract`) between two polygon meshes."
                .to_owned(),
            inputs: vec![
                NodePortSpec {
                    name: "left".to_owned(),
                    description: "Primary A polygon mesh.".to_owned(),
                },
                NodePortSpec {
                    name: "right".to_owned(),
                    description: "Secondary B polygon mesh.".to_owned(),
                },
            ],
            outputs: vec![NodePortSpec {
                name: "out".to_owned(),
                description: "Closed manifold quad boolean result.".to_owned(),
            }],
            params: vec![
                NodeParamSpec {
                    name: "mode".to_owned(),
                    param_type: "string".to_owned(),
                    description: "Boolean operation (`union`, `intersect`, `subtract`).".to_owned(),
                },
                NodeParamSpec {
                    name: "voxel_size".to_owned(),
                    param_type: "f32".to_owned(),
                    description: "World-space SDF evaluation voxel size (>= 0.05).".to_owned(),
                },
            ],
        },
    ]
}

#[must_use]
pub fn search_node_catalog(query: &str) -> Vec<NodeTypeSpec> {
    let needle = query.trim().to_ascii_lowercase();
    node_catalog()
        .into_iter()
        .filter(|spec| {
            needle.is_empty()
                || spec.type_name.to_ascii_lowercase().contains(&needle)
                || spec.category.to_ascii_lowercase().contains(&needle)
                || spec.summary.to_ascii_lowercase().contains(&needle)
        })
        .collect()
}

#[must_use]
pub fn inspect_node_type(type_name: &str) -> Option<NodeTypeSpec> {
    let normalized = type_name.trim().to_ascii_lowercase();
    node_catalog()
        .into_iter()
        .find(|spec| spec.type_name == normalized)
}

#[derive(Clone, Debug, PartialEq)]
pub struct GraphSnapshot {
    pub revision: RevisionId,
    pub nodes: Vec<Node>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SceneError {
    Header,
    Field,
    NodeOrder,
    NonFinite,
}

impl GraphSnapshot {
    #[must_use]
    pub fn demo(count: u32, offset: Vec3) -> Self {
        Self {
            revision: RevisionId(1),
            nodes: vec![
                Node::CreatePoints {
                    id: NodeId(1),
                    count,
                },
                Node::Transform {
                    id: NodeId(2),
                    input: NodeId(1),
                    offset,
                },
            ],
        }
    }

    /// Validates topology, node IDs, and finite parameter invariants of the snapshot.
    pub fn validate(&self) -> Result<(), SceneError> {
        if self.revision.0 == 0 {
            return Err(SceneError::Field);
        }
        if self.nodes.len() < 2 {
            return Err(SceneError::NodeOrder);
        }
        if !self.nodes.first().is_some_and(Node::is_generator) {
            return Err(SceneError::NodeOrder);
        }
        if self.nodes.last().is_some_and(Node::is_generator) {
            return Err(SceneError::NodeOrder);
        }
        let mut defined = BTreeSet::new();
        for node in &self.nodes {
            let id = node.id();
            if id.0 == 0 {
                return Err(SceneError::Field);
            }
            if defined.contains(&id) {
                return Err(SceneError::NodeOrder);
            }
            match node {
                Node::CreatePoints { .. } => {}
                Node::CreateGrid {
                    rows,
                    cols,
                    size_x,
                    size_z,
                    ..
                } => {
                    if !size_x.is_finite() || !size_z.is_finite() {
                        return Err(SceneError::NonFinite);
                    }
                    if *rows < 2 || *cols < 2 || *size_x <= 0.0 || *size_z <= 0.0 {
                        return Err(SceneError::Field);
                    }
                }
                Node::CreateBox { size, .. } => {
                    Vec3::checked(size.x, size.y, size.z).map_err(|_| SceneError::NonFinite)?;
                    if size.x <= 0.0 || size.y <= 0.0 || size.z <= 0.0 {
                        return Err(SceneError::Field);
                    }
                }
                Node::Transform { input, offset, .. } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    Vec3::checked(offset.x, offset.y, offset.z)
                        .map_err(|_| SceneError::NonFinite)?;
                }
                Node::Normal { input, .. } | Node::Scatter { input, .. } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                }
                Node::PointWrangle { input, snippet, .. } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    if snippet.trim().is_empty() || !snippet.is_ascii() {
                        return Err(SceneError::Field);
                    }
                }
                Node::Subdivide {
                    input, iterations, ..
                } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    if *iterations == 0 || *iterations > 6 {
                        return Err(SceneError::Field);
                    }
                }
                Node::PolyExtrude {
                    input,
                    distance,
                    inset,
                    ..
                } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    if !distance.is_finite() || !inset.is_finite() {
                        return Err(SceneError::NonFinite);
                    }
                    if *inset < 0.0 {
                        return Err(SceneError::Field);
                    }
                }
                Node::AttribPromote {
                    input,
                    name,
                    from_domain,
                    to_domain,
                    ..
                } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    validate_identifier_strict(name)?;
                    if from_domain == to_domain {
                        return Err(SceneError::Field);
                    }
                }
                Node::Blast {
                    input, group_name, ..
                } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    validate_identifier_strict(group_name)?;
                }
                Node::UvProject { input, .. } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                }
                Node::HeightField {
                    rows,
                    cols,
                    size_x,
                    size_z,
                    ..
                } => {
                    if !size_x.is_finite() || !size_z.is_finite() {
                        return Err(SceneError::NonFinite);
                    }
                    if *rows < 2 || *cols < 2 || *size_x <= 0.0 || *size_z <= 0.0 {
                        return Err(SceneError::Field);
                    }
                }
                Node::HeightFieldNoise {
                    input,
                    amplitude,
                    frequency,
                    octaves,
                    ..
                } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    if !amplitude.is_finite() || !frequency.is_finite() {
                        return Err(SceneError::NonFinite);
                    }
                    if *octaves == 0 || *octaves > 12 || *frequency <= 0.0 {
                        return Err(SceneError::Field);
                    }
                }
                Node::HeightFieldErode {
                    input,
                    iterations,
                    rain_rate,
                    talus_angle,
                    ..
                } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    if !rain_rate.is_finite() || !talus_angle.is_finite() {
                        return Err(SceneError::NonFinite);
                    }
                    if *iterations == 0
                        || *iterations > 500
                        || *rain_rate < 0.0
                        || !(1.0..=89.0).contains(talus_angle)
                    {
                        return Err(SceneError::Field);
                    }
                }
                Node::VdbFromPolygons {
                    input, voxel_size, ..
                } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    if !voxel_size.is_finite() {
                        return Err(SceneError::NonFinite);
                    }
                    if *voxel_size < 0.05 {
                        return Err(SceneError::Field);
                    }
                }
                Node::VdbReshape {
                    input,
                    offset,
                    smooth_iterations,
                    ..
                } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    if !offset.is_finite() {
                        return Err(SceneError::NonFinite);
                    }
                    if *smooth_iterations > 50 {
                        return Err(SceneError::Field);
                    }
                }
                Node::ConvertVdb { input, .. } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                }
                Node::Boolean {
                    left,
                    right,
                    voxel_size,
                    ..
                } => {
                    if !defined.contains(left) || !defined.contains(right) {
                        return Err(SceneError::NodeOrder);
                    }
                    if !voxel_size.is_finite() {
                        return Err(SceneError::NonFinite);
                    }
                    if *voxel_size < 0.05 {
                        return Err(SceneError::Field);
                    }
                }
                Node::CopyToPoints { source, target, .. } => {
                    if !defined.contains(source) || !defined.contains(target) {
                        return Err(SceneError::NodeOrder);
                    }
                }
                Node::Merge { left, right, .. } => {
                    if !defined.contains(left) || !defined.contains(right) {
                        return Err(SceneError::NodeOrder);
                    }
                }
            }
            defined.insert(id);
        }
        Ok(())
    }

    /// Applies a batch of fine-grained edits, topologically sorts the resulting node graph,
    /// and validates all DAG and numeric invariants before returning the new snapshot.
    pub fn apply_edits(
        &self,
        next_revision: RevisionId,
        edits: &[GraphEdit],
    ) -> Result<Self, SceneError> {
        if edits.is_empty() {
            return Err(SceneError::Field);
        }
        let mut nodes = self.nodes.clone();
        for edit in edits {
            match edit {
                GraphEdit::AddNode { node } => {
                    let id = node.id();
                    if id.0 == 0 {
                        return Err(SceneError::Field);
                    }
                    if nodes.iter().any(|existing| existing.id() == id) {
                        return Err(SceneError::NodeOrder);
                    }
                    match node {
                        Node::Transform { offset, .. } => {
                            Vec3::checked(offset.x, offset.y, offset.z)
                                .map_err(|_| SceneError::NonFinite)?;
                        }
                        Node::CreateGrid {
                            rows,
                            cols,
                            size_x,
                            size_z,
                            ..
                        } => {
                            if !size_x.is_finite() || !size_z.is_finite() {
                                return Err(SceneError::NonFinite);
                            }
                            if *rows < 2 || *cols < 2 || *size_x <= 0.0 || *size_z <= 0.0 {
                                return Err(SceneError::Field);
                            }
                        }
                        Node::CreateBox { size, .. } => {
                            Vec3::checked(size.x, size.y, size.z)
                                .map_err(|_| SceneError::NonFinite)?;
                            if size.x <= 0.0 || size.y <= 0.0 || size.z <= 0.0 {
                                return Err(SceneError::Field);
                            }
                        }
                        Node::PointWrangle { snippet, .. } => {
                            if snippet.trim().is_empty() || !snippet.is_ascii() {
                                return Err(SceneError::Field);
                            }
                        }
                        Node::Subdivide { iterations, .. } => {
                            if *iterations == 0 || *iterations > 6 {
                                return Err(SceneError::Field);
                            }
                        }
                        Node::PolyExtrude {
                            distance, inset, ..
                        } => {
                            if !distance.is_finite() || !inset.is_finite() {
                                return Err(SceneError::NonFinite);
                            }
                            if *inset < 0.0 {
                                return Err(SceneError::Field);
                            }
                        }
                        Node::AttribPromote {
                            name,
                            from_domain,
                            to_domain,
                            ..
                        } => {
                            validate_identifier_strict(name)?;
                            if from_domain == to_domain {
                                return Err(SceneError::Field);
                            }
                        }
                        Node::Blast { group_name, .. } => {
                            validate_identifier_strict(group_name)?;
                        }
                        Node::HeightField {
                            rows,
                            cols,
                            size_x,
                            size_z,
                            ..
                        } => {
                            if !size_x.is_finite() || !size_z.is_finite() {
                                return Err(SceneError::NonFinite);
                            }
                            if *rows < 2 || *cols < 2 || *size_x <= 0.0 || *size_z <= 0.0 {
                                return Err(SceneError::Field);
                            }
                        }
                        Node::HeightFieldNoise {
                            amplitude,
                            frequency,
                            octaves,
                            ..
                        } => {
                            if !amplitude.is_finite() || !frequency.is_finite() {
                                return Err(SceneError::NonFinite);
                            }
                            if *octaves == 0 || *octaves > 12 || *frequency <= 0.0 {
                                return Err(SceneError::Field);
                            }
                        }
                        Node::HeightFieldErode {
                            iterations,
                            rain_rate,
                            talus_angle,
                            ..
                        } => {
                            if !rain_rate.is_finite() || !talus_angle.is_finite() {
                                return Err(SceneError::NonFinite);
                            }
                            if *iterations == 0
                                || *iterations > 500
                                || *rain_rate < 0.0
                                || !(1.0..=89.0).contains(talus_angle)
                            {
                                return Err(SceneError::Field);
                            }
                        }
                        Node::VdbFromPolygons { voxel_size, .. }
                        | Node::Boolean { voxel_size, .. } => {
                            if !voxel_size.is_finite() {
                                return Err(SceneError::NonFinite);
                            }
                            if *voxel_size < 0.05 {
                                return Err(SceneError::Field);
                            }
                        }
                        Node::VdbReshape {
                            offset,
                            smooth_iterations,
                            ..
                        } => {
                            if !offset.is_finite() {
                                return Err(SceneError::NonFinite);
                            }
                            if *smooth_iterations > 50 {
                                return Err(SceneError::Field);
                            }
                        }
                        _ => {}
                    }
                    nodes.push(node.clone());
                }
                GraphEdit::RemoveNode { id } => {
                    let before = nodes.len();
                    nodes.retain(|node| node.id() != *id);
                    if nodes.len() == before {
                        return Err(SceneError::Field);
                    }
                }
                GraphEdit::SetCreateCount { id, count } => {
                    let mut matched = false;
                    for node in &mut nodes {
                        if node.id() == *id {
                            if let Node::CreatePoints {
                                count: target_count,
                                ..
                            } = node
                            {
                                *target_count = *count;
                                matched = true;
                            } else {
                                return Err(SceneError::Field);
                            }
                        }
                    }
                    if !matched {
                        return Err(SceneError::Field);
                    }
                }
                GraphEdit::SetTransformOffset { id, offset } => {
                    let checked = Vec3::checked(offset.x, offset.y, offset.z)
                        .map_err(|_| SceneError::NonFinite)?;
                    let mut matched = false;
                    for node in &mut nodes {
                        if node.id() == *id {
                            if let Node::Transform {
                                offset: target_offset,
                                ..
                            } = node
                            {
                                *target_offset = checked;
                                matched = true;
                            } else {
                                return Err(SceneError::Field);
                            }
                        }
                    }
                    if !matched {
                        return Err(SceneError::Field);
                    }
                }
                GraphEdit::ConnectTransformInput { id, input } => {
                    let mut matched = false;
                    for node in &mut nodes {
                        if node.id() == *id {
                            if let Node::Transform {
                                input: target_input,
                                ..
                            } = node
                            {
                                *target_input = *input;
                                matched = true;
                            } else {
                                return Err(SceneError::Field);
                            }
                        }
                    }
                    if !matched {
                        return Err(SceneError::Field);
                    }
                }
                GraphEdit::ConnectMergeInputs { id, left, right } => {
                    let mut matched = false;
                    for node in &mut nodes {
                        if node.id() == *id {
                            if let Node::Merge {
                                left: target_left,
                                right: target_right,
                                ..
                            } = node
                            {
                                *target_left = *left;
                                *target_right = *right;
                                matched = true;
                            } else {
                                return Err(SceneError::Field);
                            }
                        }
                    }
                    if !matched {
                        return Err(SceneError::Field);
                    }
                }
            }
        }
        let sorted = topological_sort(&nodes)?;
        let candidate = Self {
            revision: next_revision,
            nodes: sorted,
        };
        candidate.validate()?;
        Ok(candidate)
    }

    /// Canonical `.ofxs` serialization for the DAG snapshot.
    #[must_use]
    pub fn to_ofxs(&self) -> String {
        let canonical = |v: f32| if v == 0.0 { 0.0 } else { v };
        let mut text = format!("ofxs 1\nrevision {}\n", self.revision.0);
        for node in &self.nodes {
            match node {
                Node::CreatePoints { id, count } => {
                    text.push_str(&format!("create {} {}\n", id.0, count));
                }
                Node::CreateGrid {
                    id,
                    rows,
                    cols,
                    size_x,
                    size_z,
                } => {
                    text.push_str(&format!(
                        "grid {} {} {} {:e} {:e}\n",
                        id.0,
                        rows,
                        cols,
                        canonical(*size_x),
                        canonical(*size_z)
                    ));
                }
                Node::CreateBox { id, size } => {
                    text.push_str(&format!(
                        "box {} {:e} {:e} {:e}\n",
                        id.0,
                        canonical(size.x),
                        canonical(size.y),
                        canonical(size.z)
                    ));
                }
                Node::Transform { id, input, offset } => {
                    text.push_str(&format!(
                        "transform {} {} {:e} {:e} {:e}\n",
                        id.0,
                        input.0,
                        canonical(offset.x),
                        canonical(offset.y),
                        canonical(offset.z)
                    ));
                }
                Node::Normal { id, input } => {
                    text.push_str(&format!("normal {} {}\n", id.0, input.0));
                }
                Node::PointWrangle { id, input, snippet } => {
                    text.push_str(&format!(
                        "wrangle {} {} {}\n",
                        id.0,
                        input.0,
                        encode_hex(snippet.as_bytes())
                    ));
                }
                Node::Subdivide {
                    id,
                    input,
                    iterations,
                } => {
                    text.push_str(&format!("subdivide {} {} {}\n", id.0, input.0, iterations));
                }
                Node::PolyExtrude {
                    id,
                    input,
                    distance,
                    inset,
                } => {
                    text.push_str(&format!(
                        "extrude {} {} {:e} {:e}\n",
                        id.0,
                        input.0,
                        canonical(*distance),
                        canonical(*inset)
                    ));
                }
                Node::AttribPromote {
                    id,
                    input,
                    name,
                    from_domain,
                    to_domain,
                    mode,
                } => {
                    text.push_str(&format!(
                        "attribpromote {} {} {} {} {} {}\n",
                        id.0,
                        input.0,
                        name,
                        from_domain.as_str(),
                        to_domain.as_str(),
                        mode.as_str()
                    ));
                }
                Node::Blast {
                    id,
                    input,
                    group_name,
                    negate,
                } => {
                    text.push_str(&format!(
                        "blast {} {} {} {}\n",
                        id.0,
                        input.0,
                        group_name,
                        if *negate { "true" } else { "false" }
                    ));
                }
                Node::UvProject { id, input, mode } => {
                    text.push_str(&format!(
                        "uvproject {} {} {}\n",
                        id.0,
                        input.0,
                        mode.as_str()
                    ));
                }
                Node::HeightField {
                    id,
                    rows,
                    cols,
                    size_x,
                    size_z,
                } => {
                    text.push_str(&format!(
                        "heightfield {} {} {} {:e} {:e}\n",
                        id.0,
                        rows,
                        cols,
                        canonical(*size_x),
                        canonical(*size_z)
                    ));
                }
                Node::HeightFieldNoise {
                    id,
                    input,
                    amplitude,
                    frequency,
                    octaves,
                    seed,
                } => {
                    text.push_str(&format!(
                        "hf_noise {} {} {:e} {:e} {} {}\n",
                        id.0,
                        input.0,
                        canonical(*amplitude),
                        canonical(*frequency),
                        octaves,
                        seed
                    ));
                }
                Node::HeightFieldErode {
                    id,
                    input,
                    iterations,
                    rain_rate,
                    talus_angle,
                    seed,
                } => {
                    text.push_str(&format!(
                        "hf_erode {} {} {} {:e} {:e} {}\n",
                        id.0,
                        input.0,
                        iterations,
                        canonical(*rain_rate),
                        canonical(*talus_angle),
                        seed
                    ));
                }
                Node::VdbFromPolygons {
                    id,
                    input,
                    voxel_size,
                } => {
                    text.push_str(&format!(
                        "vdb_from_polygons {} {} {:e}\n",
                        id.0,
                        input.0,
                        canonical(*voxel_size)
                    ));
                }
                Node::VdbReshape {
                    id,
                    input,
                    offset,
                    smooth_iterations,
                } => {
                    text.push_str(&format!(
                        "vdb_reshape {} {} {:e} {}\n",
                        id.0,
                        input.0,
                        canonical(*offset),
                        smooth_iterations
                    ));
                }
                Node::ConvertVdb { id, input } => {
                    text.push_str(&format!("convert_vdb {} {}\n", id.0, input.0));
                }
                Node::Boolean {
                    id,
                    left,
                    right,
                    mode,
                    voxel_size,
                } => {
                    text.push_str(&format!(
                        "boolean {} {} {} {} {:e}\n",
                        id.0,
                        left.0,
                        right.0,
                        mode.as_str(),
                        canonical(*voxel_size)
                    ));
                }
                Node::Scatter {
                    id,
                    input,
                    count,
                    seed,
                } => {
                    text.push_str(&format!(
                        "scatter {} {} {} {}\n",
                        id.0, input.0, count, seed
                    ));
                }
                Node::CopyToPoints { id, source, target } => {
                    text.push_str(&format!(
                        "copytopoints {} {} {}\n",
                        id.0, source.0, target.0
                    ));
                }
                Node::Merge { id, left, right } => {
                    text.push_str(&format!("merge {} {} {}\n", id.0, left.0, right.0));
                }
            }
        }
        text
    }

    pub fn from_ofxs(text: &str) -> Result<Self, SceneError> {
        if !text.is_ascii() || text.contains('\r') {
            return Err(SceneError::Field);
        }
        let Some(body) = text.strip_suffix('\n') else {
            return Err(SceneError::Field);
        };
        let mut lines = body.split('\n');
        if lines.next() != Some("ofxs 1") {
            return Err(SceneError::Header);
        }
        let revision = parse_u64_strict(parse_pair(lines.next(), "revision")?)?;
        let mut nodes = Vec::new();
        for (line_idx, line) in lines.enumerate() {
            if line.is_empty() {
                return Err(SceneError::Field);
            }
            let mut parts = line.split(' ');
            let op = parts.next().ok_or(SceneError::Field)?;
            match op {
                "create" => {
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let count = parse_u32_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::CreatePoints {
                        id: NodeId(id),
                        count,
                    });
                }
                "grid" => {
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let rows = parse_u32_strict(parse_next(&mut parts)?)?;
                    let cols = parse_u32_strict(parse_next(&mut parts)?)?;
                    let size_x = parse_f32_checked(parse_next(&mut parts)?)?;
                    let size_z = parse_f32_checked(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    if rows < 2 || cols < 2 || size_x <= 0.0 || size_z <= 0.0 {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::CreateGrid {
                        id: NodeId(id),
                        rows,
                        cols,
                        size_x: if size_x == 0.0 { 0.0 } else { size_x },
                        size_z: if size_z == 0.0 { 0.0 } else { size_z },
                    });
                }
                "box" => {
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let sx = parse_f32_checked(parse_next(&mut parts)?)?;
                    let sy = parse_f32_checked(parse_next(&mut parts)?)?;
                    let sz = parse_f32_checked(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    let size = Vec3::checked(sx, sy, sz).map_err(|_| SceneError::NonFinite)?;
                    if size.x <= 0.0 || size.y <= 0.0 || size.z <= 0.0 {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::CreateBox {
                        id: NodeId(id),
                        size,
                    });
                }
                "transform" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let x = parse_f32_checked(parse_next(&mut parts)?)?;
                    let y = parse_f32_checked(parse_next(&mut parts)?)?;
                    let z = parse_f32_checked(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    let offset = Vec3::checked(x, y, z).map_err(|_| SceneError::NonFinite)?;
                    nodes.push(Node::Transform {
                        id: NodeId(id),
                        input: NodeId(input),
                        offset,
                    });
                }
                "normal" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::Normal {
                        id: NodeId(id),
                        input: NodeId(input),
                    });
                }
                "wrangle" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let hex_snippet = parse_next(&mut parts)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    let snippet = decode_hex_ascii(hex_snippet)?;
                    if snippet.trim().is_empty() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::PointWrangle {
                        id: NodeId(id),
                        input: NodeId(input),
                        snippet,
                    });
                }
                "subdivide" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let iterations = parse_u32_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some() || iterations == 0 || iterations > 6 {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::Subdivide {
                        id: NodeId(id),
                        input: NodeId(input),
                        iterations,
                    });
                }
                "extrude" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let distance = parse_f32_checked(parse_next(&mut parts)?)?;
                    let inset = parse_f32_checked(parse_next(&mut parts)?)?;
                    if parts.next().is_some() || inset < 0.0 {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::PolyExtrude {
                        id: NodeId(id),
                        input: NodeId(input),
                        distance: if distance == 0.0 { 0.0 } else { distance },
                        inset: if inset == 0.0 { 0.0 } else { inset },
                    });
                }
                "attribpromote" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let name = parse_next(&mut parts)?;
                    let from_domain = AttrDomain::from_token(parse_next(&mut parts)?)?;
                    let to_domain = AttrDomain::from_token(parse_next(&mut parts)?)?;
                    let mode = PromoteMode::from_token(parse_next(&mut parts)?)?;
                    if parts.next().is_some() || from_domain == to_domain {
                        return Err(SceneError::Field);
                    }
                    validate_identifier_strict(name)?;
                    nodes.push(Node::AttribPromote {
                        id: NodeId(id),
                        input: NodeId(input),
                        name: name.to_owned(),
                        from_domain,
                        to_domain,
                        mode,
                    });
                }
                "blast" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let group_name = parse_next(&mut parts)?;
                    let negate = parse_bool_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    validate_identifier_strict(group_name)?;
                    nodes.push(Node::Blast {
                        id: NodeId(id),
                        input: NodeId(input),
                        group_name: group_name.to_owned(),
                        negate,
                    });
                }
                "uvproject" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let mode = UvProjectMode::from_token(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::UvProject {
                        id: NodeId(id),
                        input: NodeId(input),
                        mode,
                    });
                }
                "heightfield" => {
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let rows = parse_u32_strict(parse_next(&mut parts)?)?;
                    let cols = parse_u32_strict(parse_next(&mut parts)?)?;
                    let size_x = parse_f32_checked(parse_next(&mut parts)?)?;
                    let size_z = parse_f32_checked(parse_next(&mut parts)?)?;
                    if parts.next().is_some()
                        || rows < 2
                        || cols < 2
                        || size_x <= 0.0
                        || size_z <= 0.0
                    {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::HeightField {
                        id: NodeId(id),
                        rows,
                        cols,
                        size_x: if size_x == 0.0 { 0.0 } else { size_x },
                        size_z: if size_z == 0.0 { 0.0 } else { size_z },
                    });
                }
                "hf_noise" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let amplitude = parse_f32_checked(parse_next(&mut parts)?)?;
                    let frequency = parse_f32_checked(parse_next(&mut parts)?)?;
                    let octaves = parse_u32_strict(parse_next(&mut parts)?)?;
                    let seed = parse_u64_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some() || octaves == 0 || octaves > 12 || frequency <= 0.0 {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::HeightFieldNoise {
                        id: NodeId(id),
                        input: NodeId(input),
                        amplitude: if amplitude == 0.0 { 0.0 } else { amplitude },
                        frequency: if frequency == 0.0 { 0.0 } else { frequency },
                        octaves,
                        seed,
                    });
                }
                "hf_erode" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let iterations = parse_u32_strict(parse_next(&mut parts)?)?;
                    let rain_rate = parse_f32_checked(parse_next(&mut parts)?)?;
                    let talus_angle = parse_f32_checked(parse_next(&mut parts)?)?;
                    let seed = parse_u64_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some()
                        || iterations == 0
                        || iterations > 500
                        || rain_rate < 0.0
                        || !(1.0..=89.0).contains(&talus_angle)
                    {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::HeightFieldErode {
                        id: NodeId(id),
                        input: NodeId(input),
                        iterations,
                        rain_rate: if rain_rate == 0.0 { 0.0 } else { rain_rate },
                        talus_angle: if talus_angle == 0.0 { 0.0 } else { talus_angle },
                        seed,
                    });
                }
                "vdb_from_polygons" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let voxel_size = parse_f32_checked(parse_next(&mut parts)?)?;
                    if parts.next().is_some() || voxel_size < 0.05 {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::VdbFromPolygons {
                        id: NodeId(id),
                        input: NodeId(input),
                        voxel_size: if voxel_size == 0.0 { 0.0 } else { voxel_size },
                    });
                }
                "vdb_reshape" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let offset = parse_f32_checked(parse_next(&mut parts)?)?;
                    let smooth_iterations = parse_u32_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some() || smooth_iterations > 50 {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::VdbReshape {
                        id: NodeId(id),
                        input: NodeId(input),
                        offset: if offset == 0.0 { 0.0 } else { offset },
                        smooth_iterations,
                    });
                }
                "convert_vdb" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::ConvertVdb {
                        id: NodeId(id),
                        input: NodeId(input),
                    });
                }
                "boolean" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let left = parse_u64_strict(parse_next(&mut parts)?)?;
                    let right = parse_u64_strict(parse_next(&mut parts)?)?;
                    let mode = BooleanMode::from_token(parse_next(&mut parts)?)?;
                    let voxel_size = parse_f32_checked(parse_next(&mut parts)?)?;
                    if parts.next().is_some() || voxel_size < 0.05 {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::Boolean {
                        id: NodeId(id),
                        left: NodeId(left),
                        right: NodeId(right),
                        mode,
                        voxel_size: if voxel_size == 0.0 { 0.0 } else { voxel_size },
                    });
                }
                "scatter" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let count = parse_u32_strict(parse_next(&mut parts)?)?;
                    let seed = parse_u64_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::Scatter {
                        id: NodeId(id),
                        input: NodeId(input),
                        count,
                        seed,
                    });
                }
                "copytopoints" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let source = parse_u64_strict(parse_next(&mut parts)?)?;
                    let target = parse_u64_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::CopyToPoints {
                        id: NodeId(id),
                        source: NodeId(source),
                        target: NodeId(target),
                    });
                }
                "merge" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let left = parse_u64_strict(parse_next(&mut parts)?)?;
                    let right = parse_u64_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::Merge {
                        id: NodeId(id),
                        left: NodeId(left),
                        right: NodeId(right),
                    });
                }
                _ => return Err(SceneError::Field),
            }
        }
        if nodes.len() < 2 {
            return Err(SceneError::Field);
        }
        let snapshot = Self {
            revision: RevisionId(revision),
            nodes,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }
}

fn topological_sort(nodes: &[Node]) -> Result<Vec<Node>, SceneError> {
    if nodes.len() < 2 {
        return Err(SceneError::NodeOrder);
    }
    let mut index_by_id = BTreeMap::new();
    for (idx, node) in nodes.iter().enumerate() {
        let id = node.id();
        if id.0 == 0 {
            return Err(SceneError::Field);
        }
        if index_by_id.insert(id, idx).is_some() {
            return Err(SceneError::NodeOrder);
        }
    }
    let mut in_degree = vec![0usize; nodes.len()];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (idx, node) in nodes.iter().enumerate() {
        for input_id in node.inputs() {
            let Some(&upstream_idx) = index_by_id.get(&input_id) else {
                return Err(SceneError::NodeOrder);
            };
            if upstream_idx == idx {
                return Err(SceneError::NodeOrder);
            }
            dependents[upstream_idx].push(idx);
            in_degree[idx] += 1;
        }
    }
    let mut ready: Vec<usize> = in_degree
        .iter()
        .enumerate()
        .filter_map(|(idx, &deg)| (deg == 0).then_some(idx))
        .collect();
    let mut result = Vec::with_capacity(nodes.len());
    while !ready.is_empty() {
        let current = ready.remove(0);
        result.push(nodes[current].clone());
        for &dep in &dependents[current] {
            in_degree[dep] -= 1;
            if in_degree[dep] == 0 {
                let pos = ready.partition_point(|&existing| existing < dep);
                ready.insert(pos, dep);
            }
        }
    }
    if result.len() != nodes.len() {
        return Err(SceneError::NodeOrder);
    }
    Ok(result)
}

fn parse_pair<'a>(line: Option<&'a str>, key: &str) -> Result<&'a str, SceneError> {
    let line = line.ok_or(SceneError::Field)?;
    line.strip_prefix(&format!("{key} "))
        .filter(|s| !s.is_empty())
        .ok_or(SceneError::Field)
}

fn parse_next<'a>(parts: &mut impl Iterator<Item = &'a str>) -> Result<&'a str, SceneError> {
    parts
        .next()
        .filter(|s| !s.is_empty())
        .ok_or(SceneError::Field)
}

fn parse_u64_strict(token: &str) -> Result<u64, SceneError> {
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_digit()) {
        return Err(SceneError::Field);
    }
    token.parse::<u64>().map_err(|_| SceneError::Field)
}

fn parse_u32_strict(token: &str) -> Result<u32, SceneError> {
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_digit()) {
        return Err(SceneError::Field);
    }
    token.parse::<u32>().map_err(|_| SceneError::Field)
}

fn parse_f32_checked(token: &str) -> Result<f32, SceneError> {
    let val = token.parse::<f32>().map_err(|_| SceneError::Field)?;
    if val.is_finite() {
        Ok(val)
    } else {
        Err(SceneError::NonFinite)
    }
}

fn parse_bool_strict(token: &str) -> Result<bool, SceneError> {
    match token {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(SceneError::Field),
    }
}

fn validate_identifier_strict(name: &str) -> Result<(), SceneError> {
    if name.is_empty() || name == "P" {
        return Err(SceneError::Field);
    }
    let mut chars = name.chars();
    if !chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    {
        return Err(SceneError::Field);
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(SceneError::Field);
    }
    Ok(())
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn decode_hex_nibble(b: u8) -> Result<u8, SceneError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        _ => Err(SceneError::Field),
    }
}

fn decode_hex_ascii(hex: &str) -> Result<String, SceneError> {
    let bytes = hex.as_bytes();
    if bytes.is_empty() || !bytes.len().is_multiple_of(2) {
        return Err(SceneError::Field);
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut idx = 0usize;
    while idx < bytes.len() {
        let hi = decode_hex_nibble(bytes[idx])?;
        let lo = decode_hex_nibble(bytes[idx + 1])?;
        let val = (hi << 4) | lo;
        if !val.is_ascii() {
            return Err(SceneError::Field);
        }
        out.push(val);
        idx += 2;
    }
    String::from_utf8(out).map_err(|_| SceneError::Field)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SplitMix64 {
        state: u64,
    }

    impl SplitMix64 {
        const fn new(seed: u64) -> Self {
            Self { state: seed }
        }

        fn next_u64(&mut self) -> u64 {
            self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        fn next_u32(&mut self) -> u32 {
            (self.next_u64() >> 32) as u32
        }

        fn next_finite_f32(&mut self) -> f32 {
            loop {
                let bits = self.next_u32();
                let candidate = f32::from_bits(bits);
                if candidate.is_finite() {
                    return if candidate == 0.0 { 0.0 } else { candidate };
                }
            }
        }
    }

    #[test]
    fn ofxs_is_canonical_and_round_trips() {
        let graph = GraphSnapshot::demo(100, Vec3::checked(1., 2., 3.).unwrap_or(Vec3::ZERO));
        let saved = graph.to_ofxs();
        assert_eq!(
            GraphSnapshot::from_ofxs(&saved).map(|value| value.to_ofxs()),
            Ok(saved)
        );
    }

    #[test]
    fn ofxs_rejects_non_finite_values() {
        assert!(matches!(
            GraphSnapshot::from_ofxs("ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 1 NaN 0e0 0e0\n"),
            Err(SceneError::NonFinite)
        ));
        assert!(matches!(
            GraphSnapshot::from_ofxs("ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 1 inf 0e0 0e0\n"),
            Err(SceneError::NonFinite)
        ));
        assert!(matches!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 1 0e0 -inf 0e0\n"
            ),
            Err(SceneError::NonFinite)
        ));
        assert!(matches!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 1 1e50 0e0 0e0\n"
            ),
            Err(SceneError::NonFinite)
        ));
    }

    #[test]
    fn property_valid_graphs_round_trip_canonically() {
        let mut rng = SplitMix64::new(0x0FF5_EC0D_E123_4567);
        for _ in 0..1_000 {
            let revision = (rng.next_u64() % 1_000_000).saturating_add(1);
            let create_id = (rng.next_u64() % 10_000).saturating_add(1);
            let mut transform_id = (rng.next_u64() % 10_000).saturating_add(1);
            if transform_id == create_id {
                transform_id = transform_id.saturating_add(1);
            }
            let count = rng.next_u32() % 500_000;
            let x = rng.next_finite_f32();
            let y = rng.next_finite_f32();
            let z = rng.next_finite_f32();
            let offset = Vec3::checked(x, y, z).unwrap_or(Vec3::ZERO);

            let snapshot = GraphSnapshot {
                revision: RevisionId(revision),
                nodes: vec![
                    Node::CreatePoints {
                        id: NodeId(create_id),
                        count,
                    },
                    Node::Transform {
                        id: NodeId(transform_id),
                        input: NodeId(create_id),
                        offset,
                    },
                ],
            };
            assert_eq!(snapshot.validate(), Ok(()));

            let serialized = snapshot.to_ofxs();
            let parsed = GraphSnapshot::from_ofxs(&serialized);
            assert_eq!(parsed.as_ref(), Ok(&snapshot));
            assert_eq!(parsed.map(|g| g.to_ofxs()), Ok(serialized));
        }
    }

    #[test]
    fn graph_validation_rejects_malformed_topologies_and_non_finite_offsets() {
        let empty = GraphSnapshot {
            revision: RevisionId(1),
            nodes: Vec::new(),
        };
        assert_eq!(empty.validate(), Err(SceneError::NodeOrder));

        let zero_rev = GraphSnapshot::demo(4, Vec3::ZERO);
        let zero_rev = GraphSnapshot {
            revision: RevisionId(0),
            nodes: zero_rev.nodes,
        };
        assert_eq!(zero_rev.validate(), Err(SceneError::Field));

        let duplicate_id = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::CreatePoints {
                    id: NodeId(1),
                    count: 4,
                },
                Node::Transform {
                    id: NodeId(1),
                    input: NodeId(1),
                    offset: Vec3::ZERO,
                },
            ],
        };
        assert_eq!(duplicate_id.validate(), Err(SceneError::NodeOrder));

        let dangling_input = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::CreatePoints {
                    id: NodeId(1),
                    count: 4,
                },
                Node::Transform {
                    id: NodeId(2),
                    input: NodeId(99),
                    offset: Vec3::ZERO,
                },
            ],
        };
        assert_eq!(dangling_input.validate(), Err(SceneError::NodeOrder));

        let reversed_order = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::Transform {
                    id: NodeId(2),
                    input: NodeId(1),
                    offset: Vec3::ZERO,
                },
                Node::CreatePoints {
                    id: NodeId(1),
                    count: 4,
                },
            ],
        };
        assert_eq!(reversed_order.validate(), Err(SceneError::NodeOrder));

        let raw_nan_offset = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::CreatePoints {
                    id: NodeId(1),
                    count: 4,
                },
                Node::Transform {
                    id: NodeId(2),
                    input: NodeId(1),
                    offset: Vec3 {
                        x: f32::NAN,
                        y: 0.0,
                        z: 0.0,
                    },
                },
            ],
        };
        assert_eq!(raw_nan_offset.validate(), Err(SceneError::NonFinite));
    }

    #[test]
    fn fuzz_malformed_ofxs_corpus_and_mutations_never_panic_and_reject_invalid() {
        let malformed_corpus: &[(&str, SceneError)] = &[
            ("", SceneError::Field),
            ("\n", SceneError::Header),
            (
                "ofxs 2\nrevision 1\ncreate 1 1\ntransform 2 1 0e0 0e0 0e0\n",
                SceneError::Header,
            ),
            (
                "ofxs 1\r\nrevision 1\r\ncreate 1 1\r\ntransform 2 1 0e0 0e0 0e0\r\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 1 0e0 0e0 0e0",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 0\ncreate 1 1\ntransform 2 1 0e0 0e0 0e0\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision +1\ncreate 1 1\ntransform 2 1 0e0 0e0 0e0\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision -1\ncreate 1 1\ntransform 2 1 0e0 0e0 0e0\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 18446744073709551616\ncreate 1 1\ntransform 2 1 0e0 0e0 0e0\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 0 1\ntransform 2 0 0e0 0e0 0e0\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 1 4294967296\ntransform 2 1 0e0 0e0 0e0\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 1\ncreate  1 1\ntransform 2 1 0e0 0e0 0e0\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 1 1 extra\ntransform 2 1 0e0 0e0 0e0\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 1\ntransform 2 1 0e0 0e0 0e0\ncreate 1 1\n",
                SceneError::NodeOrder,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 1 1\ntransform 1 1 0e0 0e0 0e0\n",
                SceneError::NodeOrder,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 99 0e0 0e0 0e0\n",
                SceneError::NodeOrder,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 1 0e0 0e0\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 1 0e0 0e0 0e0 0e0\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 1 0e0 0e0 0e0\n\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 1 0e0 0e0 0e0\ntrailing\n",
                SceneError::Field,
            ),
            (
                "ofxs 1\nrevision 1\ncreate 1 1\ntransform 2 1 \u{00e9} 0e0 0e0\n",
                SceneError::Field,
            ),
        ];

        for (input, expected_err) in malformed_corpus {
            assert_eq!(
                GraphSnapshot::from_ofxs(input),
                Err(*expected_err),
                "unexpected result for corpus input: {input:?}"
            );
        }

        // Deterministic byte-level fuzzing around a valid canonical `.ofxs` payload.
        let base = GraphSnapshot::demo(256, Vec3::checked(1.25, -3.5, 42.0).unwrap_or(Vec3::ZERO))
            .to_ofxs();
        let base_bytes = base.as_bytes();
        let mut rng = SplitMix64::new(0xF022_CAFE_BAAD_BEEF);

        for _ in 0..1_000 {
            let mut mutated = base_bytes.to_vec();
            let op = rng.next_u32() % 4;
            match op {
                0 => {
                    let idx = (rng.next_u64() as usize) % mutated.len();
                    mutated[idx] ^= ((rng.next_u32() % 255) as u8).saturating_add(1);
                }
                1 => {
                    let idx = (rng.next_u64() as usize) % mutated.len();
                    mutated.remove(idx);
                }
                2 => {
                    let idx = (rng.next_u64() as usize) % (mutated.len() + 1);
                    let byte = (rng.next_u32() & 0x7F) as u8;
                    mutated.insert(idx, byte);
                }
                _ => {
                    let cut = (rng.next_u64() as usize) % mutated.len();
                    mutated.truncate(cut);
                }
            }
            if let Ok(as_str) = std::str::from_utf8(&mutated)
                && let Ok(parsed) = GraphSnapshot::from_ofxs(as_str)
            {
                assert_eq!(parsed.validate(), Ok(()));
                let round_tripped = parsed.to_ofxs();
                assert_eq!(GraphSnapshot::from_ofxs(&round_tripped), Ok(parsed));
            }
        }
    }

    #[test]
    fn diamond_dag_apply_edits_and_catalog_introspection() {
        let base = GraphSnapshot::demo(8, Vec3::checked(1.0, 0.0, 0.0).unwrap_or(Vec3::ZERO));
        let edits = vec![
            GraphEdit::AddNode {
                node: Node::Transform {
                    id: NodeId(3),
                    input: NodeId(1),
                    offset: Vec3::checked(0.0, 2.0, 0.0).unwrap_or(Vec3::ZERO),
                },
            },
            GraphEdit::AddNode {
                node: Node::Merge {
                    id: NodeId(4),
                    left: NodeId(2),
                    right: NodeId(3),
                },
            },
        ];
        let edited = base.apply_edits(RevisionId(2), &edits);
        assert!(edited.is_ok());
        if let Ok(diamond) = edited {
            let ofxs = diamond.to_ofxs();
            assert_eq!(GraphSnapshot::from_ofxs(&ofxs), Ok(diamond.clone()));

            // Introducing a cycle (rewiring node 2 to depend on downstream merge node 4) must be rejected.
            let cycle = diamond.apply_edits(
                RevisionId(3),
                &[GraphEdit::ConnectTransformInput {
                    id: NodeId(2),
                    input: NodeId(4),
                }],
            );
            assert_eq!(cycle, Err(SceneError::NodeOrder));
        }

        assert_eq!(search_node_catalog("").len(), 21);
        assert_eq!(search_node_catalog("merge").len(), 1);
        assert!(inspect_node_type("transform").is_some());
        assert!(inspect_node_type("grid").is_some());
        assert!(inspect_node_type("box").is_some());
        assert!(inspect_node_type("normal").is_some());
        assert!(inspect_node_type("wrangle").is_some());
        assert!(inspect_node_type("subdivide").is_some());
        assert!(inspect_node_type("extrude").is_some());
        assert!(inspect_node_type("attribpromote").is_some());
        assert!(inspect_node_type("blast").is_some());
        assert!(inspect_node_type("uvproject").is_some());
        assert!(inspect_node_type("heightfield").is_some());
        assert!(inspect_node_type("hf_noise").is_some());
        assert!(inspect_node_type("hf_erode").is_some());
        assert!(inspect_node_type("vdb_from_polygons").is_some());
        assert!(inspect_node_type("vdb_reshape").is_some());
        assert!(inspect_node_type("convert_vdb").is_some());
        assert!(inspect_node_type("boolean").is_some());
        assert!(inspect_node_type("scatter").is_some());
        assert!(inspect_node_type("copytopoints").is_some());
        assert!(inspect_node_type("nonexistent").is_none());

        let grid_scatter = GraphSnapshot {
            revision: RevisionId(1),
            nodes: vec![
                Node::HeightField {
                    id: NodeId(1),
                    rows: 8,
                    cols: 8,
                    size_x: 10.0,
                    size_z: 20.0,
                },
                Node::CreateBox {
                    id: NodeId(2),
                    size: Vec3::checked(1.0, 2.0, 3.0).unwrap_or(Vec3::ZERO),
                },
                Node::Subdivide {
                    id: NodeId(3),
                    input: NodeId(2),
                    iterations: 2,
                },
                Node::PolyExtrude {
                    id: NodeId(4),
                    input: NodeId(3),
                    distance: 0.5,
                    inset: 0.1,
                },
                Node::VdbFromPolygons {
                    id: NodeId(15),
                    input: NodeId(4),
                    voxel_size: 0.5,
                },
                Node::VdbReshape {
                    id: NodeId(16),
                    input: NodeId(15),
                    offset: 0.1,
                    smooth_iterations: 1,
                },
                Node::ConvertVdb {
                    id: NodeId(17),
                    input: NodeId(16),
                },
                Node::Boolean {
                    id: NodeId(18),
                    left: NodeId(2),
                    right: NodeId(17),
                    mode: BooleanMode::Union,
                    voxel_size: 0.5,
                },
                Node::Normal {
                    id: NodeId(5),
                    input: NodeId(18),
                },
                Node::UvProject {
                    id: NodeId(12),
                    input: NodeId(5),
                    mode: UvProjectMode::Box,
                },
                Node::HeightFieldNoise {
                    id: NodeId(13),
                    input: NodeId(1),
                    amplitude: 4.0,
                    frequency: 0.1,
                    octaves: 3,
                    seed: 9,
                },
                Node::HeightFieldErode {
                    id: NodeId(14),
                    input: NodeId(13),
                    iterations: 5,
                    rain_rate: 0.05,
                    talus_angle: 35.0,
                    seed: 11,
                },
                Node::Scatter {
                    id: NodeId(6),
                    input: NodeId(14),
                    count: 50,
                    seed: 42,
                },
                Node::PointWrangle {
                    id: NodeId(7),
                    input: NodeId(6),
                    snippet: "f@pscale = 0.5;\nf@keep = 1.0;\n@P.y = sin(@P.x);".to_owned(),
                },
                Node::AttribPromote {
                    id: NodeId(8),
                    input: NodeId(7),
                    name: "pscale".to_owned(),
                    from_domain: AttrDomain::Point,
                    to_domain: AttrDomain::Detail,
                    mode: PromoteMode::Average,
                },
                Node::AttribPromote {
                    id: NodeId(9),
                    input: NodeId(8),
                    name: "pscale".to_owned(),
                    from_domain: AttrDomain::Detail,
                    to_domain: AttrDomain::Point,
                    mode: PromoteMode::Max,
                },
                Node::Blast {
                    id: NodeId(10),
                    input: NodeId(9),
                    group_name: "keep".to_owned(),
                    negate: true,
                },
                Node::CopyToPoints {
                    id: NodeId(11),
                    source: NodeId(12),
                    target: NodeId(10),
                },
            ],
        };
        let gs_ofxs = grid_scatter.to_ofxs();
        assert_eq!(GraphSnapshot::from_ofxs(&gs_ofxs), Ok(grid_scatter));

        // Malformed input rejection for new node types
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nvdb_from_polygons 2 1 1e-3\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nvdb_from_polygons 2 1 5e-1\nvdb_reshape 3 2 1e-1 999\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nbox 2 1e0 1e0 1e0\nboolean 3 1 2 bad_op 5e-1\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nuvproject 2 1 invalid_mode\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nheightfield 1 1 8 10e0 10e0\nnormal 2 1\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nheightfield 1 8 8 10e0 10e0\nhf_noise 2 1 5e0 1e-1 0 1\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nheightfield 1 8 8 10e0 10e0\nhf_erode 2 1 10 5e-2 95e0 1\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs("ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nsubdivide 2 1 0\n"),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs("ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nsubdivide 2 1 99\n"),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nextrude 2 1 1e0 -1e-1\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nextrude 2 1 NaN 0e0\n"
            ),
            Err(SceneError::NonFinite)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nattribpromote 2 1 P point prim average\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nattribpromote 2 1 heat point point average\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nblast 2 1 1bad false\n"
            ),
            Err(SceneError::Field)
        );
        assert_eq!(
            GraphSnapshot::from_ofxs(
                "ofxs 1\nrevision 1\nbox 1 1e0 1e0 1e0\nblast 2 1 grp maybe\n"
            ),
            Err(SceneError::Field)
        );
    }
}
