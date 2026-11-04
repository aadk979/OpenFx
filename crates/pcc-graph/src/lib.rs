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
    CreateSphere {
        id: NodeId,
        radius: f32,
        rows: u32,
        cols: u32,
    },
    CreateTube {
        id: NodeId,
        radius: f32,
        height: f32,
        rows: u32,
        cols: u32,
    },
    CreateTorus {
        id: NodeId,
        major_radius: f32,
        minor_radius: f32,
        rows: u32,
        cols: u32,
    },
    CreateLine {
        id: NodeId,
        start: Vec3,
        end: Vec3,
        points: u32,
    },
    Xform {
        id: NodeId,
        input: NodeId,
        translate: Vec3,
        rotate_degrees: Vec3,
        scale: Vec3,
    },
    Reverse {
        id: NodeId,
        input: NodeId,
    },
    Triangulate {
        id: NodeId,
        input: NodeId,
    },
    Fuse {
        id: NodeId,
        input: NodeId,
        distance: f32,
    },
    GroupBounds {
        id: NodeId,
        input: NodeId,
        group_name: String,
        domain: AttrDomain,
        min: Vec3,
        max: Vec3,
    },
    Resample {
        id: NodeId,
        input: NodeId,
        segment_length: f32,
    },
    Sweep {
        id: NodeId,
        input: NodeId,
        radius: f32,
        sides: u32,
        capped: bool,
    },
    AttribTransfer {
        id: NodeId,
        input: NodeId,
        source: NodeId,
        name: String,
        radius: f32,
    },
    Measure {
        id: NodeId,
        input: NodeId,
    },
    PolyBevel {
        id: NodeId,
        input: NodeId,
        distance: f32,
        min_angle_degrees: f32,
    },
    PolyReduce {
        id: NodeId,
        input: NodeId,
        target_ratio: f32,
    },
    Remesh {
        id: NodeId,
        input: NodeId,
        target_length: f32,
        iterations: u32,
        feature_angle_degrees: f32,
    },
    Clip {
        id: NodeId,
        input: NodeId,
        origin: Vec3,
        normal: Vec3,
        cap: bool,
    },
    VoronoiFracture {
        id: NodeId,
        input: NodeId,
        seeds: NodeId,
    },
    ScatterSurface {
        id: NodeId,
        input: NodeId,
        count: u32,
        seed: u64,
        min_distance: f32,
    },
    UvFlatten {
        id: NodeId,
        input: NodeId,
        seam_angle_degrees: f32,
    },
    UvLayout {
        id: NodeId,
        input: NodeId,
        padding: f32,
        tiles: u32,
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
            | Self::Merge { id, .. }
            | Self::CreateSphere { id, .. }
            | Self::CreateTube { id, .. }
            | Self::CreateTorus { id, .. }
            | Self::CreateLine { id, .. }
            | Self::Xform { id, .. }
            | Self::Reverse { id, .. }
            | Self::Triangulate { id, .. }
            | Self::Fuse { id, .. }
            | Self::GroupBounds { id, .. }
            | Self::Resample { id, .. }
            | Self::Sweep { id, .. }
            | Self::AttribTransfer { id, .. }
            | Self::Measure { id, .. }
            | Self::PolyBevel { id, .. }
            | Self::PolyReduce { id, .. }
            | Self::Remesh { id, .. }
            | Self::Clip { id, .. }
            | Self::VoronoiFracture { id, .. }
            | Self::ScatterSurface { id, .. }
            | Self::UvFlatten { id, .. }
            | Self::UvLayout { id, .. } => *id,
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
                | Self::CreateSphere { .. }
                | Self::CreateTube { .. }
                | Self::CreateTorus { .. }
                | Self::CreateLine { .. }
        )
    }

    #[must_use]
    pub fn inputs(&self) -> Vec<NodeId> {
        match self {
            Self::CreatePoints { .. }
            | Self::CreateGrid { .. }
            | Self::CreateBox { .. }
            | Self::HeightField { .. }
            | Self::CreateSphere { .. }
            | Self::CreateTube { .. }
            | Self::CreateTorus { .. }
            | Self::CreateLine { .. } => Vec::new(),
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
            Self::Xform { input, .. } => vec![*input],
            Self::Reverse { input, .. } => vec![*input],
            Self::Triangulate { input, .. } => vec![*input],
            Self::Fuse { input, .. } => vec![*input],
            Self::GroupBounds { input, .. } => vec![*input],
            Self::Resample { input, .. } => vec![*input],
            Self::Sweep { input, .. } => vec![*input],
            Self::AttribTransfer { input, source, .. } => vec![*input, *source],
            Self::Measure { input, .. } => vec![*input],
            Self::PolyBevel { input, .. } => vec![*input],
            Self::PolyReduce { input, .. } => vec![*input],
            Self::Remesh { input, .. } => vec![*input],
            Self::Clip { input, .. } => vec![*input],
            Self::VoronoiFracture { input, seeds, .. } => vec![*input, *seeds],
            Self::ScatterSurface { input, .. } => vec![*input],
            Self::UvFlatten { input, .. } => vec![*input],
            Self::UvLayout { input, .. } => vec![*input],
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
        sop_spec("sphere", "generator", "Generates a closed, outward-facing latitude-longitude sphere centred on the origin.", &[], &[("radius", "f32", "Sphere radius in metres (> 0)."), ("rows", "u32", "Latitude bands (>= 2)."), ("cols", "u32", "Longitude segments (>= 3).")]),
        sop_spec("tube", "generator", "Generates a closed, capped cylinder centred on the origin along +Y.", &[], &[("radius", "f32", "Tube radius in metres (> 0)."), ("height", "f32", "Tube height in metres (> 0)."), ("rows", "u32", "Height bands (>= 1)."), ("cols", "u32", "Sides (>= 3).")]),
        sop_spec("torus", "generator", "Generates a closed quad torus around +Y.", &[], &[("major_radius", "f32", "Distance from the axis to the tube centre (> minor_radius)."), ("minor_radius", "f32", "Tube radius (> 0)."), ("rows", "u32", "Segments around the axis (>= 3)."), ("cols", "u32", "Segments around the tube (>= 3).")]),
        sop_spec("line", "generator", "Generates one straight polyline of evenly spaced points.", &[], &[("start", "vec3", "First point."), ("end", "vec3", "Last point."), ("points", "u32", "Point count (>= 2).")]),
        sop_spec("xform", "transform", "Applies scale, Euler XYZ rotation (degrees), then translation to positions and `N`; a mirroring scale also reverses winding so solids keep facing outward.", &[("input", "Upstream geometry.")], &[("translate", "vec3", "Translation in metres."), ("rotate_degrees", "vec3", "Euler rotation, applied as Rz * Ry * Rx."), ("scale", "vec3", "Per-axis scale (all non-zero).")]),
        sop_spec("reverse", "modeling", "Flips the orientation of every polygon, keeping attributes attached.", &[("input", "Upstream geometry.")], &[]),
        sop_spec("triangulate", "modeling", "Splits polygons into triangles, handling concave outlines. Point data survives; vertex and primitive attributes do not.", &[("input", "Upstream geometry.")], &[]),
        sop_spec("fuse", "modeling", "Welds points closer than a distance, removing collapsed primitives. Point attributes of the lowest-indexed point survive; primitive attributes do not.", &[("input", "Upstream geometry.")], &[("distance", "f32", "Weld distance in metres (>= 0).")]),
        sop_spec("group_bounds", "attributes", "Creates a named point or primitive group from an axis-aligned box, for use by `blast`.", &[("input", "Upstream geometry.")], &[("group_name", "string", "Group name."), ("domain", "string", "`point` or `prim`."), ("min", "vec3", "Box minimum corner."), ("max", "vec3", "Box maximum corner.")]),
        sop_spec("resample", "curves", "Rebuilds polylines with uniform arc-length spacing and exact endpoints. Polygons pass through; attributes do not survive.", &[("input", "Upstream geometry.")], &[("segment_length", "f32", "Target spacing in metres (> 0).")]),
        sop_spec("sweep", "curves", "Sweeps a regular polygon along polylines using rotation-minimizing frames, consuming polygons.", &[("input", "Upstream curves.")], &[("radius", "f32", "Tube radius in metres (> 0)."), ("sides", "u32", "Cross-section sides (>= 3)."), ("capped", "bool", "Close tube ends with caps.")]),
        sop_spec("attribtransfer", "attributes", "Transfers an f32 or vec3 point attribute from a source by normalized kernel average within a radius; points out of range keep their value.", &[("input", "Geometry receiving the attribute."), ("source", "Geometry providing the attribute.")], &[("name", "string", "Point attribute name."), ("radius", "f32", "Kernel radius in metres (> 0).")]),
        sop_spec("measure", "attributes", "Writes primitive `f@area` and `f@perimeter` and detail `f@total_area` and `f@total_volume`.", &[("input", "Upstream geometry.")], &[]),
        sop_spec("polybevel", "modeling", "Bevels manifold edges at or above a dihedral angle by offsetting each adjacent face inward. Attributes do not survive.", &[("input", "Upstream manifold polygons.")], &[("distance", "f32", "Inward face offset in metres (> 0)."), ("min_angle_degrees", "f32", "Minimum dihedral angle to bevel, in [0, 180].")]),
        sop_spec("polyreduce", "modeling", "Decimates to a target triangle ratio with quadric error metrics, preserving manifoldness, genus, and boundaries. Output carries positions only.", &[("input", "Upstream oriented manifold polygons.")], &[("target_ratio", "f32", "Fraction of triangles to keep, in (0, 1].")]),
        sop_spec("remesh", "modeling", "Isotropically remeshes to a target edge length, keeping boundaries and sharp features in place. Output carries positions only.", &[("input", "Upstream oriented manifold polygons.")], &[("target_length", "f32", "Target edge length in metres (> 0)."), ("iterations", "u32", "Refinement rounds, in [1, 100]."), ("feature_angle_degrees", "f32", "Dihedral angle at or above which edges stay sharp, in [0, 180].")]),
        sop_spec("clip", "modeling", "Removes geometry on the +normal side of a plane. With caps, closed solids stay closed. Output carries positions only.", &[("input", "Upstream polygons.")], &[("origin", "vec3", "Point on the plane."), ("normal", "vec3", "Plane normal pointing at the removed side (non-zero)."), ("cap", "bool", "Close openings on the plane.")]),
        sop_spec("voronoifracture", "fracture", "Cuts a closed manifold into the Voronoi cells of seed points; each piece is a closed shell tagged `i@piece` on points and primitives, conserving total volume.", &[("input", "Closed manifold solid."), ("seeds", "Points used as Voronoi seeds.")], &[]),
        sop_spec("scatter_surface", "generator_modifier", "Scatters points on polygon surfaces with area-uniform density, writing `v@N`; a positive minimum distance keeps only samples at least that far apart.", &[("input", "Upstream polygons.")], &[("count", "u32", "Maximum number of points."), ("seed", "u64", "Deterministic 64-bit Philox seed."), ("min_distance", "f32", "Minimum pairwise spacing in metres (>= 0; 0 disables).")]),
        sop_spec("uvflatten", "uv", "Flattens polygons with least-squares conformal maps into non-overlapping islands in the unit square, cutting closed shapes into disks. Writes vertex `v@uv` and primitive `i@uv_island` with no inverted triangles.", &[("input", "Upstream polygons.")], &[("seam_angle_degrees", "f32", "Dihedral angle above which faces start in separate islands, in [0, 180].")]),
        sop_spec("uvlayout", "uv", "Packs existing `v@uv` islands into UDIM tiles (ten per row) at one uniform scale.", &[("input", "Geometry with vertex or point `v@uv`.")], &[("padding", "f32", "Space around each island, in [0, 0.25)."), ("tiles", "u32", "Tile count, in [1, 100].")]),
    ]
}

/// Compact constructor for catalog entries with one `out` port.
fn sop_spec(
    type_name: &str,
    category: &str,
    summary: &str,
    inputs: &[(&str, &str)],
    params: &[(&str, &str, &str)],
) -> NodeTypeSpec {
    NodeTypeSpec {
        type_name: type_name.to_owned(),
        category: category.to_owned(),
        summary: summary.to_owned(),
        inputs: inputs
            .iter()
            .map(|(name, description)| NodePortSpec {
                name: (*name).to_owned(),
                description: (*description).to_owned(),
            })
            .collect(),
        outputs: vec![NodePortSpec {
            name: "out".to_owned(),
            description: "Output geometry stream.".to_owned(),
        }],
        params: params
            .iter()
            .map(|(name, param_type, description)| NodeParamSpec {
                name: (*name).to_owned(),
                param_type: (*param_type).to_owned(),
                description: (*description).to_owned(),
            })
            .collect(),
    }
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
                Node::CreateSphere {
                    radius, rows, cols, ..
                } => {
                    check_positive(&[*radius])?;
                    if *rows < 2 || *cols < 3 {
                        return Err(SceneError::Field);
                    }
                }
                Node::CreateTube {
                    radius,
                    height,
                    rows,
                    cols,
                    ..
                } => {
                    check_positive(&[*radius, *height])?;
                    if *rows < 1 || *cols < 3 {
                        return Err(SceneError::Field);
                    }
                }
                Node::CreateTorus {
                    major_radius,
                    minor_radius,
                    rows,
                    cols,
                    ..
                } => {
                    check_positive(&[*major_radius, *minor_radius])?;
                    if *major_radius <= *minor_radius || *rows < 3 || *cols < 3 {
                        return Err(SceneError::Field);
                    }
                }
                Node::CreateLine {
                    start, end, points, ..
                } => {
                    check_finite_vec3(&[*start, *end])?;
                    if *points < 2 {
                        return Err(SceneError::Field);
                    }
                }
                Node::Xform {
                    input,
                    translate,
                    rotate_degrees,
                    scale,
                    ..
                } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    check_finite_vec3(&[*translate, *rotate_degrees, *scale])?;
                    if scale.x == 0.0 || scale.y == 0.0 || scale.z == 0.0 {
                        return Err(SceneError::Field);
                    }
                }
                Node::Reverse { input, .. } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                }
                Node::Triangulate { input, .. } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                }
                Node::Fuse {
                    input, distance, ..
                } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    check_range(*distance, 0.0, f32::MAX)?;
                }
                Node::GroupBounds {
                    input,
                    group_name,
                    domain,
                    min,
                    max,
                    ..
                } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    validate_identifier_strict(group_name)?;
                    check_finite_vec3(&[*min, *max])?;
                    if *domain == AttrDomain::Detail
                        || min.x > max.x
                        || min.y > max.y
                        || min.z > max.z
                    {
                        return Err(SceneError::Field);
                    }
                }
                Node::Resample {
                    input,
                    segment_length,
                    ..
                } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    check_positive(&[*segment_length])?;
                }
                Node::Sweep {
                    input,
                    radius,
                    sides,
                    ..
                } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    check_positive(&[*radius])?;
                    if *sides < 3 {
                        return Err(SceneError::Field);
                    }
                }
                Node::AttribTransfer {
                    input,
                    source,
                    name,
                    radius,
                    ..
                } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    if !defined.contains(source) {
                        return Err(SceneError::NodeOrder);
                    }
                    validate_identifier_strict(name)?;
                    check_positive(&[*radius])?;
                }
                Node::Measure { input, .. } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                }
                Node::PolyBevel {
                    input,
                    distance,
                    min_angle_degrees,
                    ..
                } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    check_positive(&[*distance])?;
                    check_range(*min_angle_degrees, 0.0, 180.0)?;
                }
                Node::PolyReduce {
                    input,
                    target_ratio,
                    ..
                } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    check_positive(&[*target_ratio])?;
                    check_range(*target_ratio, 0.0, 1.0)?;
                }
                Node::Remesh {
                    input,
                    target_length,
                    iterations,
                    feature_angle_degrees,
                    ..
                } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    check_positive(&[*target_length])?;
                    check_range(*feature_angle_degrees, 0.0, 180.0)?;
                    if *iterations < 1 || *iterations > 100 {
                        return Err(SceneError::Field);
                    }
                }
                Node::Clip {
                    input,
                    origin,
                    normal,
                    ..
                } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    check_finite_vec3(&[*origin, *normal])?;
                    if normal.dot(*normal) <= 0.0 {
                        return Err(SceneError::Field);
                    }
                }
                Node::VoronoiFracture { input, seeds, .. } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    if !defined.contains(seeds) {
                        return Err(SceneError::NodeOrder);
                    }
                }
                Node::ScatterSurface {
                    input,
                    min_distance,
                    ..
                } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    check_range(*min_distance, 0.0, f32::MAX)?;
                }
                Node::UvFlatten {
                    input,
                    seam_angle_degrees,
                    ..
                } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    check_range(*seam_angle_degrees, 0.0, 180.0)?;
                }
                Node::UvLayout {
                    input,
                    padding,
                    tiles,
                    ..
                } => {
                    if !defined.contains(input) {
                        return Err(SceneError::NodeOrder);
                    }
                    check_range(*padding, 0.0, 0.25)?;
                    if *padding >= 0.25 || *tiles < 1 || *tiles > 100 {
                        return Err(SceneError::Field);
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
                Node::CreateSphere {
                    id,
                    radius,
                    rows,
                    cols,
                } => {
                    text.push_str(&format!(
                        "sphere {} {:e} {} {}\n",
                        id.0,
                        canonical(*radius),
                        rows,
                        cols
                    ));
                }
                Node::CreateTube {
                    id,
                    radius,
                    height,
                    rows,
                    cols,
                } => {
                    text.push_str(&format!(
                        "tube {} {:e} {:e} {} {}\n",
                        id.0,
                        canonical(*radius),
                        canonical(*height),
                        rows,
                        cols
                    ));
                }
                Node::CreateTorus {
                    id,
                    major_radius,
                    minor_radius,
                    rows,
                    cols,
                } => {
                    text.push_str(&format!(
                        "torus {} {:e} {:e} {} {}\n",
                        id.0,
                        canonical(*major_radius),
                        canonical(*minor_radius),
                        rows,
                        cols
                    ));
                }
                Node::CreateLine {
                    id,
                    start,
                    end,
                    points,
                } => {
                    text.push_str(&format!(
                        "line {} {:e} {:e} {:e} {:e} {:e} {:e} {}\n",
                        id.0,
                        canonical(start.x),
                        canonical(start.y),
                        canonical(start.z),
                        canonical(end.x),
                        canonical(end.y),
                        canonical(end.z),
                        points
                    ));
                }
                Node::Xform {
                    id,
                    input,
                    translate,
                    rotate_degrees,
                    scale,
                } => {
                    text.push_str(&format!(
                        "xform {} {} {:e} {:e} {:e} {:e} {:e} {:e} {:e} {:e} {:e}\n",
                        id.0,
                        input.0,
                        canonical(translate.x),
                        canonical(translate.y),
                        canonical(translate.z),
                        canonical(rotate_degrees.x),
                        canonical(rotate_degrees.y),
                        canonical(rotate_degrees.z),
                        canonical(scale.x),
                        canonical(scale.y),
                        canonical(scale.z)
                    ));
                }
                Node::Reverse { id, input } => {
                    text.push_str(&format!("reverse {} {}\n", id.0, input.0));
                }
                Node::Triangulate { id, input } => {
                    text.push_str(&format!("triangulate {} {}\n", id.0, input.0));
                }
                Node::Fuse {
                    id,
                    input,
                    distance,
                } => {
                    text.push_str(&format!(
                        "fuse {} {} {:e}\n",
                        id.0,
                        input.0,
                        canonical(*distance)
                    ));
                }
                Node::GroupBounds {
                    id,
                    input,
                    group_name,
                    domain,
                    min,
                    max,
                } => {
                    text.push_str(&format!(
                        "group_bounds {} {} {} {} {:e} {:e} {:e} {:e} {:e} {:e}\n",
                        id.0,
                        input.0,
                        group_name,
                        domain.as_str(),
                        canonical(min.x),
                        canonical(min.y),
                        canonical(min.z),
                        canonical(max.x),
                        canonical(max.y),
                        canonical(max.z)
                    ));
                }
                Node::Resample {
                    id,
                    input,
                    segment_length,
                } => {
                    text.push_str(&format!(
                        "resample {} {} {:e}\n",
                        id.0,
                        input.0,
                        canonical(*segment_length)
                    ));
                }
                Node::Sweep {
                    id,
                    input,
                    radius,
                    sides,
                    capped,
                } => {
                    text.push_str(&format!(
                        "sweep {} {} {:e} {} {}\n",
                        id.0,
                        input.0,
                        canonical(*radius),
                        sides,
                        capped
                    ));
                }
                Node::AttribTransfer {
                    id,
                    input,
                    source,
                    name,
                    radius,
                } => {
                    text.push_str(&format!(
                        "attribtransfer {} {} {} {} {:e}\n",
                        id.0,
                        input.0,
                        source.0,
                        name,
                        canonical(*radius)
                    ));
                }
                Node::Measure { id, input } => {
                    text.push_str(&format!("measure {} {}\n", id.0, input.0));
                }
                Node::PolyBevel {
                    id,
                    input,
                    distance,
                    min_angle_degrees,
                } => {
                    text.push_str(&format!(
                        "polybevel {} {} {:e} {:e}\n",
                        id.0,
                        input.0,
                        canonical(*distance),
                        canonical(*min_angle_degrees)
                    ));
                }
                Node::PolyReduce {
                    id,
                    input,
                    target_ratio,
                } => {
                    text.push_str(&format!(
                        "polyreduce {} {} {:e}\n",
                        id.0,
                        input.0,
                        canonical(*target_ratio)
                    ));
                }
                Node::Remesh {
                    id,
                    input,
                    target_length,
                    iterations,
                    feature_angle_degrees,
                } => {
                    text.push_str(&format!(
                        "remesh {} {} {:e} {} {:e}\n",
                        id.0,
                        input.0,
                        canonical(*target_length),
                        iterations,
                        canonical(*feature_angle_degrees)
                    ));
                }
                Node::Clip {
                    id,
                    input,
                    origin,
                    normal,
                    cap,
                } => {
                    text.push_str(&format!(
                        "clip {} {} {:e} {:e} {:e} {:e} {:e} {:e} {}\n",
                        id.0,
                        input.0,
                        canonical(origin.x),
                        canonical(origin.y),
                        canonical(origin.z),
                        canonical(normal.x),
                        canonical(normal.y),
                        canonical(normal.z),
                        cap
                    ));
                }
                Node::VoronoiFracture { id, input, seeds } => {
                    text.push_str(&format!(
                        "voronoifracture {} {} {}\n",
                        id.0, input.0, seeds.0
                    ));
                }
                Node::ScatterSurface {
                    id,
                    input,
                    count,
                    seed,
                    min_distance,
                } => {
                    text.push_str(&format!(
                        "scatter_surface {} {} {} {} {:e}\n",
                        id.0,
                        input.0,
                        count,
                        seed,
                        canonical(*min_distance)
                    ));
                }
                Node::UvFlatten {
                    id,
                    input,
                    seam_angle_degrees,
                } => {
                    text.push_str(&format!(
                        "uvflatten {} {} {:e}\n",
                        id.0,
                        input.0,
                        canonical(*seam_angle_degrees)
                    ));
                }
                Node::UvLayout {
                    id,
                    input,
                    padding,
                    tiles,
                } => {
                    text.push_str(&format!(
                        "uvlayout {} {} {:e} {}\n",
                        id.0,
                        input.0,
                        canonical(*padding),
                        tiles
                    ));
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
                "sphere" => {
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let radius = parse_f32_checked(parse_next(&mut parts)?)?;
                    let rows = parse_u32_strict(parse_next(&mut parts)?)?;
                    let cols = parse_u32_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::CreateSphere {
                        id: NodeId(id),
                        radius,
                        rows,
                        cols,
                    });
                }
                "tube" => {
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let radius = parse_f32_checked(parse_next(&mut parts)?)?;
                    let height = parse_f32_checked(parse_next(&mut parts)?)?;
                    let rows = parse_u32_strict(parse_next(&mut parts)?)?;
                    let cols = parse_u32_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::CreateTube {
                        id: NodeId(id),
                        radius,
                        height,
                        rows,
                        cols,
                    });
                }
                "torus" => {
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let major_radius = parse_f32_checked(parse_next(&mut parts)?)?;
                    let minor_radius = parse_f32_checked(parse_next(&mut parts)?)?;
                    let rows = parse_u32_strict(parse_next(&mut parts)?)?;
                    let cols = parse_u32_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::CreateTorus {
                        id: NodeId(id),
                        major_radius,
                        minor_radius,
                        rows,
                        cols,
                    });
                }
                "line" => {
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let start = parse_vec3(&mut parts)?;
                    let end = parse_vec3(&mut parts)?;
                    let points = parse_u32_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::CreateLine {
                        id: NodeId(id),
                        start,
                        end,
                        points,
                    });
                }
                "xform" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let translate = parse_vec3(&mut parts)?;
                    let rotate_degrees = parse_vec3(&mut parts)?;
                    let scale = parse_vec3(&mut parts)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::Xform {
                        id: NodeId(id),
                        input: NodeId(input),
                        translate,
                        rotate_degrees,
                        scale,
                    });
                }
                "reverse" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::Reverse {
                        id: NodeId(id),
                        input: NodeId(input),
                    });
                }
                "triangulate" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::Triangulate {
                        id: NodeId(id),
                        input: NodeId(input),
                    });
                }
                "fuse" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let distance = parse_f32_checked(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::Fuse {
                        id: NodeId(id),
                        input: NodeId(input),
                        distance,
                    });
                }
                "group_bounds" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let group_name = parse_next(&mut parts)?.to_owned();
                    let domain = AttrDomain::from_token(parse_next(&mut parts)?)?;
                    let min = parse_vec3(&mut parts)?;
                    let max = parse_vec3(&mut parts)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::GroupBounds {
                        id: NodeId(id),
                        input: NodeId(input),
                        group_name,
                        domain,
                        min,
                        max,
                    });
                }
                "resample" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let segment_length = parse_f32_checked(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::Resample {
                        id: NodeId(id),
                        input: NodeId(input),
                        segment_length,
                    });
                }
                "sweep" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let radius = parse_f32_checked(parse_next(&mut parts)?)?;
                    let sides = parse_u32_strict(parse_next(&mut parts)?)?;
                    let capped = parse_bool_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::Sweep {
                        id: NodeId(id),
                        input: NodeId(input),
                        radius,
                        sides,
                        capped,
                    });
                }
                "attribtransfer" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let source = parse_u64_strict(parse_next(&mut parts)?)?;
                    let name = parse_next(&mut parts)?.to_owned();
                    let radius = parse_f32_checked(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::AttribTransfer {
                        id: NodeId(id),
                        input: NodeId(input),
                        source: NodeId(source),
                        name,
                        radius,
                    });
                }
                "measure" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::Measure {
                        id: NodeId(id),
                        input: NodeId(input),
                    });
                }
                "polybevel" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let distance = parse_f32_checked(parse_next(&mut parts)?)?;
                    let min_angle_degrees = parse_f32_checked(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::PolyBevel {
                        id: NodeId(id),
                        input: NodeId(input),
                        distance,
                        min_angle_degrees,
                    });
                }
                "polyreduce" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let target_ratio = parse_f32_checked(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::PolyReduce {
                        id: NodeId(id),
                        input: NodeId(input),
                        target_ratio,
                    });
                }
                "remesh" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let target_length = parse_f32_checked(parse_next(&mut parts)?)?;
                    let iterations = parse_u32_strict(parse_next(&mut parts)?)?;
                    let feature_angle_degrees = parse_f32_checked(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::Remesh {
                        id: NodeId(id),
                        input: NodeId(input),
                        target_length,
                        iterations,
                        feature_angle_degrees,
                    });
                }
                "clip" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let origin = parse_vec3(&mut parts)?;
                    let normal = parse_vec3(&mut parts)?;
                    let cap = parse_bool_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::Clip {
                        id: NodeId(id),
                        input: NodeId(input),
                        origin,
                        normal,
                        cap,
                    });
                }
                "voronoifracture" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let seeds = parse_u64_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::VoronoiFracture {
                        id: NodeId(id),
                        input: NodeId(input),
                        seeds: NodeId(seeds),
                    });
                }
                "scatter_surface" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let count = parse_u32_strict(parse_next(&mut parts)?)?;
                    let seed = parse_u64_strict(parse_next(&mut parts)?)?;
                    let min_distance = parse_f32_checked(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::ScatterSurface {
                        id: NodeId(id),
                        input: NodeId(input),
                        count,
                        seed,
                        min_distance,
                    });
                }
                "uvflatten" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let seam_angle_degrees = parse_f32_checked(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::UvFlatten {
                        id: NodeId(id),
                        input: NodeId(input),
                        seam_angle_degrees,
                    });
                }
                "uvlayout" => {
                    if line_idx == 0 {
                        return Err(SceneError::NodeOrder);
                    }
                    let id = parse_u64_strict(parse_next(&mut parts)?)?;
                    let input = parse_u64_strict(parse_next(&mut parts)?)?;
                    let padding = parse_f32_checked(parse_next(&mut parts)?)?;
                    let tiles = parse_u32_strict(parse_next(&mut parts)?)?;
                    if parts.next().is_some() {
                        return Err(SceneError::Field);
                    }
                    nodes.push(Node::UvLayout {
                        id: NodeId(id),
                        input: NodeId(input),
                        padding,
                        tiles,
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

fn parse_vec3<'a>(parts: &mut impl Iterator<Item = &'a str>) -> Result<Vec3, SceneError> {
    let x = parse_f32_checked(parse_next(parts)?)?;
    let y = parse_f32_checked(parse_next(parts)?)?;
    let z = parse_f32_checked(parse_next(parts)?)?;
    Ok(Vec3 { x, y, z })
}

/// Every value must be finite and strictly positive.
fn check_positive(values: &[f32]) -> Result<(), SceneError> {
    for &value in values {
        if !value.is_finite() {
            return Err(SceneError::NonFinite);
        }
        if value <= 0.0 {
            return Err(SceneError::Field);
        }
    }
    Ok(())
}

/// The value must be finite and within the inclusive range.
fn check_range(value: f32, low: f32, high: f32) -> Result<(), SceneError> {
    if !value.is_finite() {
        return Err(SceneError::NonFinite);
    }
    if value < low || value > high {
        return Err(SceneError::Field);
    }
    Ok(())
}

fn check_finite_vec3(values: &[Vec3]) -> Result<(), SceneError> {
    for value in values {
        Vec3::checked(value.x, value.y, value.z).map_err(|_| SceneError::NonFinite)?;
    }
    Ok(())
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

    fn v3(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3 { x, y, z }
    }

    /// One valid instance of every SOP operator, wired over two generators.
    fn sop_operator_graph() -> GraphSnapshot {
        let sphere = NodeId(1);
        let seeds = NodeId(2);
        let curve = NodeId(3);
        let nodes = vec![
            Node::CreateSphere {
                id: sphere,
                radius: 1.5,
                rows: 8,
                cols: 12,
            },
            Node::CreateTube {
                id: seeds,
                radius: 0.5,
                height: 2.0,
                rows: 2,
                cols: 8,
            },
            Node::CreateLine {
                id: curve,
                start: v3(0.0, 0.0, 0.0),
                end: v3(1.0, 2.0, -3.5),
                points: 9,
            },
            Node::CreateTorus {
                id: NodeId(4),
                major_radius: 2.0,
                minor_radius: 0.5,
                rows: 8,
                cols: 6,
            },
            Node::Xform {
                id: NodeId(5),
                input: sphere,
                translate: v3(1.0, 0.0, 0.0),
                rotate_degrees: v3(0.0, 45.0, 0.0),
                scale: v3(1.0, -2.0, 1.0),
            },
            Node::Reverse {
                id: NodeId(6),
                input: sphere,
            },
            Node::Triangulate {
                id: NodeId(7),
                input: sphere,
            },
            Node::Fuse {
                id: NodeId(8),
                input: sphere,
                distance: 0.001,
            },
            Node::GroupBounds {
                id: NodeId(9),
                input: sphere,
                group_name: "top".to_owned(),
                domain: AttrDomain::Prim,
                min: v3(-2.0, 0.0, -2.0),
                max: v3(2.0, 2.0, 2.0),
            },
            Node::Resample {
                id: NodeId(10),
                input: curve,
                segment_length: 0.25,
            },
            Node::Sweep {
                id: NodeId(11),
                input: curve,
                radius: 0.1,
                sides: 6,
                capped: true,
            },
            Node::AttribTransfer {
                id: NodeId(12),
                input: sphere,
                source: seeds,
                name: "heat".to_owned(),
                radius: 0.5,
            },
            Node::Measure {
                id: NodeId(13),
                input: sphere,
            },
            Node::PolyBevel {
                id: NodeId(14),
                input: sphere,
                distance: 0.01,
                min_angle_degrees: 30.0,
            },
            Node::PolyReduce {
                id: NodeId(15),
                input: sphere,
                target_ratio: 0.5,
            },
            Node::Remesh {
                id: NodeId(16),
                input: sphere,
                target_length: 0.3,
                iterations: 3,
                feature_angle_degrees: 60.0,
            },
            Node::Clip {
                id: NodeId(17),
                input: sphere,
                origin: v3(0.0, 0.25, 0.0),
                normal: v3(0.0, 1.0, 0.0),
                cap: false,
            },
            Node::VoronoiFracture {
                id: NodeId(18),
                input: sphere,
                seeds,
            },
            Node::ScatterSurface {
                id: NodeId(19),
                input: sphere,
                count: 100,
                seed: u64::MAX,
                min_distance: 0.05,
            },
            Node::UvFlatten {
                id: NodeId(20),
                input: sphere,
                seam_angle_degrees: 66.0,
            },
            Node::UvLayout {
                id: NodeId(21),
                input: NodeId(20),
                padding: 0.01,
                tiles: 4,
            },
        ];
        GraphSnapshot {
            revision: RevisionId(7),
            nodes,
        }
    }

    #[test]
    fn every_sop_operator_round_trips_through_ofxs_and_is_in_the_catalog() {
        let graph = sop_operator_graph();
        assert!(graph.validate().is_ok());
        let text = graph.to_ofxs();
        let parsed = GraphSnapshot::from_ofxs(&text);
        assert_eq!(parsed.as_ref(), Ok(&graph));
        assert_eq!(parsed.map(|g| g.to_ofxs()), Ok(text.clone()));
        for line in text.lines().skip(2) {
            let token = line.split(' ').next().unwrap_or("");
            assert!(
                inspect_node_type(token).is_some(),
                "{token} is not in the catalog"
            );
        }
        // Every catalogued parameter and port appears as a serialized field of the node's line.
        for node in &graph.nodes {
            let line = text
                .lines()
                .skip(2)
                .find(|l| l.split(' ').nth(1) == Some(node.id().0.to_string().as_str()))
                .unwrap_or("");
            let token = line.split(' ').next().unwrap_or("");
            let spec = inspect_node_type(token);
            assert!(spec.is_some(), "{line}");
            if let Some(spec) = spec {
                let expected: usize = 2
                    + spec.inputs.len()
                    + spec
                        .params
                        .iter()
                        .map(|p| if p.param_type == "vec3" { 3 } else { 1 })
                        .sum::<usize>();
                assert_eq!(line.split(' ').count(), expected, "{line}");
            }
        }
    }

    #[test]
    fn sop_operator_parameters_are_validated() {
        let bad = |node: Node| {
            let mut nodes = vec![Node::CreateSphere {
                id: NodeId(1),
                radius: 1.0,
                rows: 8,
                cols: 8,
            }];
            nodes.push(node);
            // A scene may not end on a generator, so a consumer always follows the node under test.
            nodes.push(Node::Reverse {
                id: NodeId(3),
                input: NodeId(1),
            });
            GraphSnapshot {
                revision: RevisionId(1),
                nodes,
            }
            .validate()
        };
        let sphere = NodeId(1);
        let id = NodeId(2);
        let field = Err(SceneError::Field);
        let non_finite = Err(SceneError::NonFinite);
        let ones = v3(1.0, 1.0, 1.0);
        assert_eq!(
            bad(Node::CreateSphere {
                id,
                radius: 0.0,
                rows: 8,
                cols: 8
            }),
            field
        );
        assert_eq!(
            bad(Node::CreateSphere {
                id,
                radius: f32::NAN,
                rows: 8,
                cols: 8
            }),
            non_finite
        );
        assert_eq!(
            bad(Node::CreateSphere {
                id,
                radius: 1.0,
                rows: 1,
                cols: 8
            }),
            field
        );
        assert_eq!(
            bad(Node::CreateTube {
                id,
                radius: 1.0,
                height: -1.0,
                rows: 1,
                cols: 8
            }),
            field
        );
        assert_eq!(
            bad(Node::CreateTorus {
                id,
                major_radius: 0.5,
                minor_radius: 1.0,
                rows: 8,
                cols: 8
            }),
            field
        );
        assert_eq!(
            bad(Node::CreateLine {
                id,
                start: ones,
                end: ones,
                points: 1
            }),
            field
        );
        assert_eq!(
            bad(Node::Xform {
                id,
                input: sphere,
                translate: ones,
                rotate_degrees: ones,
                scale: v3(0.0, 1.0, 1.0)
            }),
            field
        );
        assert_eq!(
            bad(Node::Xform {
                id,
                input: sphere,
                translate: v3(f32::INFINITY, 0.0, 0.0),
                rotate_degrees: ones,
                scale: ones
            }),
            non_finite
        );
        assert_eq!(
            bad(Node::Fuse {
                id,
                input: sphere,
                distance: -1.0
            }),
            field
        );
        assert_eq!(
            bad(Node::GroupBounds {
                id,
                input: sphere,
                group_name: "g".to_owned(),
                domain: AttrDomain::Detail,
                min: ones,
                max: ones
            }),
            field
        );
        assert_eq!(
            bad(Node::GroupBounds {
                id,
                input: sphere,
                group_name: "g".to_owned(),
                domain: AttrDomain::Point,
                min: v3(2.0, 2.0, 2.0),
                max: ones
            }),
            field
        );
        assert_eq!(
            bad(Node::GroupBounds {
                id,
                input: sphere,
                group_name: "not valid".to_owned(),
                domain: AttrDomain::Point,
                min: ones,
                max: ones
            }),
            field
        );
        assert_eq!(
            bad(Node::Resample {
                id,
                input: sphere,
                segment_length: 0.0
            }),
            field
        );
        assert_eq!(
            bad(Node::Sweep {
                id,
                input: sphere,
                radius: 1.0,
                sides: 2,
                capped: false
            }),
            field
        );
        assert_eq!(
            bad(Node::AttribTransfer {
                id,
                input: sphere,
                source: sphere,
                name: "P".to_owned(),
                radius: 1.0
            }),
            field
        );
        assert_eq!(
            bad(Node::PolyBevel {
                id,
                input: sphere,
                distance: 0.1,
                min_angle_degrees: 181.0
            }),
            field
        );
        assert_eq!(
            bad(Node::PolyReduce {
                id,
                input: sphere,
                target_ratio: 0.0
            }),
            field
        );
        assert_eq!(
            bad(Node::PolyReduce {
                id,
                input: sphere,
                target_ratio: 1.5
            }),
            field
        );
        assert_eq!(
            bad(Node::Remesh {
                id,
                input: sphere,
                target_length: 1.0,
                iterations: 0,
                feature_angle_degrees: 30.0
            }),
            field
        );
        assert_eq!(
            bad(Node::Remesh {
                id,
                input: sphere,
                target_length: 1.0,
                iterations: 101,
                feature_angle_degrees: 30.0
            }),
            field
        );
        assert_eq!(
            bad(Node::Clip {
                id,
                input: sphere,
                origin: ones,
                normal: v3(0.0, 0.0, 0.0),
                cap: true
            }),
            field
        );
        assert_eq!(
            bad(Node::ScatterSurface {
                id,
                input: sphere,
                count: 1,
                seed: 1,
                min_distance: -0.5
            }),
            field
        );
        assert_eq!(
            bad(Node::UvFlatten {
                id,
                input: sphere,
                seam_angle_degrees: -1.0
            }),
            field
        );
        assert_eq!(
            bad(Node::UvLayout {
                id,
                input: sphere,
                padding: 0.25,
                tiles: 1
            }),
            field
        );
        assert_eq!(
            bad(Node::UvLayout {
                id,
                input: sphere,
                padding: 0.0,
                tiles: 0
            }),
            field
        );
        // Inputs must already be defined: forward references are ordering errors.
        assert_eq!(
            bad(Node::VoronoiFracture {
                id,
                input: sphere,
                seeds: NodeId(9)
            }),
            Err(SceneError::NodeOrder)
        );
        assert_eq!(
            bad(Node::AttribTransfer {
                id,
                input: NodeId(9),
                source: sphere,
                name: "a".to_owned(),
                radius: 1.0
            }),
            Err(SceneError::NodeOrder)
        );
    }

    #[test]
    fn malformed_sop_lines_are_rejected_by_the_parser() {
        for body in [
            "sphere 1 1e0 8\n",
            "sphere 1 1e0 8 8 extra\n",
            "line 1 0e0 0e0 0e0 1e0 1e0 5\n",
            "sweep 2 1 1e-1 6 maybe\n",
            "group_bounds 2 1 g vertex 0e0 0e0 0e0 1e0 1e0 1e0\n",
            "attribtransfer 2 1 1 bad-name 1e0\n",
            "polyreduce 2 1 nan\n",
            "voronoifracture 2 1\n",
        ] {
            let text = format!("ofxs 1\nrevision 1\nsphere 1 1e0 8 8\n{body}");
            assert!(GraphSnapshot::from_ofxs(&text).is_err(), "{body}");
        }
        // A consumer cannot be the first line of a scene.
        assert_eq!(
            GraphSnapshot::from_ofxs("ofxs 1\nrevision 1\nreverse 2 1\nsphere 1 1e0 8 8\n"),
            Err(SceneError::NodeOrder)
        );
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

        assert_eq!(search_node_catalog("").len(), 42);
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
        for spec in node_catalog() {
            assert!(
                !spec.summary.is_empty() && !spec.outputs.is_empty(),
                "{}",
                spec.type_name
            );
        }

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
